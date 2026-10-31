//! Atomic city-merchant purchases: the Won debit, the personal mark and the
//! item commit together, with overflow going to the System storage.

use std::sync::Arc;

use steel_core::player::Player;
use steel_core::player::connection::NetworkConnection as _;
use steel_core::player::player_data_storage::transaction::{
    ExtensionStorageWrite, TransactionCommit,
};
use steel_core::server::Server;
use text_components::TextComponent;
use uuid::Uuid;

use super::{Request, account, stage_inventory_item};
use crate::item::nbt;
use crate::merchant::{self, MerchantOffer, Showcase};
use crate::merchant_menu::MerchantMenuKind;
use crate::rank::HunterRank;
use crate::sound::SystemSound;
use crate::state::SoloLevelingState;

/// Why a merchant purchase was refused before anything changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MerchantError {
    /// The showcase changed since the offer was shown.
    Expired,
    /// The personal offer was already bought this period.
    AlreadyBought,
    /// The hunter's rank is too low for the item.
    RankTooLow,
    /// The hunter has too little Won.
    InsufficientWon,
    /// Another economic operation of this hunter is still running.
    Busy,
    /// The hunter's data cannot be written right now.
    Storage,
}

impl MerchantError {
    /// Returns the Russian message shown to the hunter.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Expired => "Витрина обновилась; выберите товар заново.",
            Self::AlreadyBought => "Это предложение уже куплено.",
            Self::RankTooLow => "Этот товар продают охотникам более высокого ранга.",
            Self::InsufficientWon => "Недостаточно Won.",
            Self::Busy => "Предыдущая операция ещё обрабатывается.",
            Self::Storage => "Данные охотника сейчас недоступны для записи.",
        }
    }
}

impl SoloLevelingState {
    /// Returns the offers a hunter sees right now: shared, then personal.
    #[must_use]
    pub fn merchant_offers(&self, uuid: Uuid) -> (Vec<MerchantOffer>, Vec<MerchantOffer>) {
        let now = merchant::unix_now();
        let class = self.class_of(uuid);
        (
            merchant::shared_showcase(merchant::shared_period(now)),
            merchant::personal_showcase(uuid, class, merchant::personal_period(now)),
        )
    }

    /// Re-derives `offer` for now and checks it is still on sale to `uuid`.
    fn current_offer(&self, uuid: Uuid, offer: MerchantOffer) -> Result<(), MerchantError> {
        let (shared, personal) = self.merchant_offers(uuid);
        let showcase = match offer.showcase {
            Showcase::Shared => shared,
            Showcase::Personal => personal,
        };
        if showcase.get(offer.index) == Some(&offer) {
            Ok(())
        } else {
            Err(MerchantError::Expired)
        }
    }

    /// Queues the purchase of one merchant offer.
    pub fn request_merchant_purchase(
        &self,
        player: &Player,
        offer: MerchantOffer,
    ) -> Result<(), MerchantError> {
        let uuid = player.gameprofile.id();
        self.current_offer(uuid, offer)?;
        let accepted = self
            .sessions
            .with_mut(uuid, |session| {
                if session.economic_busy {
                    return Err(MerchantError::Busy);
                }
                check(
                    session.stats.rank(),
                    session.won.amount(),
                    session.merchant,
                    offer,
                )?;
                session.economic_busy = true;
                Ok(())
            })
            .ok_or(MerchantError::Storage)?;
        accepted?;
        let mut persistence = self.persistence.lock();
        if persistence.quiescing || persistence.failed.contains(&uuid) {
            drop(persistence);
            self.sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            return Err(MerchantError::Storage);
        }
        persistence
            .requests
            .push_back(Request::Merchant { uuid, offer });
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the Won debit, the personal mark and the item must commit as one transaction"
    )]
    pub(super) fn begin_merchant_purchase(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: Arc<Player>,
        uuid: Uuid,
        offer: MerchantOffer,
    ) -> bool {
        let fail = |message: &'static str| {
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain(message));
            false
        };
        if player.connection.closed() {
            self.release_shop_purchase(uuid);
            return false;
        }
        if let Err(error) = self.current_offer(uuid, offer) {
            return fail(error.message());
        }
        let Ok(mut reservation) = player.reserve_stored_inventory_with_menu::<MerchantMenuKind>()
        else {
            return fail(
                "Покупку не удалось начать: инвентарь занят или предмет находится на курсоре или в сетке крафта.",
            );
        };
        let Some(mut snapshot) = self
            .sessions
            .with_mut(uuid, |session| session.saved_data(uuid))
        else {
            return fail("Сессия охотника не найдена.");
        };
        if let Err(error) = check(
            snapshot.stats.rank(),
            snapshot.won.amount(),
            snapshot.merchant,
            offer,
        ) {
            return fail(error.message());
        }
        if !snapshot.won.debit(offer.price) {
            return fail(MerchantError::InsufficientWon.message());
        }
        if offer.showcase == Showcase::Personal {
            snapshot.merchant.mark(offer.period, offer.index);
        }
        let item = offer.item.stack();
        let mut stored = false;
        if stage_inventory_item(&mut reservation, item.clone()).is_err() {
            match nbt::encode(&item) {
                Ok(bytes) => {
                    snapshot.deferred_inventory.push(bytes);
                    stored = true;
                }
                Err(error) => {
                    log::error!(
                        "sololeveling: could not encode a merchant item for {uuid}: {error}"
                    );
                    return fail("Не удалось подготовить покупку.");
                }
            }
        }
        let expected = self.persistence.lock().expected.get(&uuid).cloned();
        let Some(expected) = expected else {
            return fail("Данные охотника не загружены.");
        };
        let replacement = match serde_json::to_vec(&snapshot) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::error!("sololeveling: could not encode merchant purchase for {uuid}: {error}");
                return fail("Не удалось сохранить покупку.");
            }
        };
        let won = snapshot.won;
        let record = snapshot.merchant;
        let deferred_inventory = snapshot.deferred_inventory;
        let state = Arc::clone(self);
        let completion_player = Arc::clone(&player);
        let committed_bytes = replacement.clone();
        let transaction = server.submit_stored_inventory_transaction(
            vec![reservation],
            vec![ExtensionStorageWrite {
                key: account::key(uuid),
                expected,
                replacement,
            }],
            move |result| {
                match result {
                    Ok(commit) => {
                        state
                            .persistence
                            .lock()
                            .expected
                            .insert(uuid, Some(committed_bytes));
                        state.sessions.with_mut(uuid, |session| {
                            session.won = won;
                            session.merchant = record;
                            session.deferred_inventory = deferred_inventory;
                            session.economic_busy = false;
                        });
                        SystemSound::Reward.play(&completion_player);
                        let name = offer.item.display_name();
                        completion_player.send_message(&TextComponent::plain(if stored {
                            format!("Куплено: {name}. Инвентарь полон — товар в Хранилище.")
                        } else {
                            format!("Куплено: {name}.")
                        }));
                        if commit == TransactionCommit::DurabilityUncertain {
                            completion_player.send_message(&TextComponent::plain(
                                "Покупка выполнена, но подтверждение диска не получено.",
                            ));
                        }
                    }
                    Err(error) => {
                        log::error!("sololeveling: merchant purchase failed: {error:?}");
                        state
                            .sessions
                            .with_mut(uuid, |session| session.economic_busy = false);
                        completion_player.send_message(&TextComponent::plain(
                            "Покупка не сохранена; Won и предметы не изменены.",
                        ));
                    }
                }
                state.persistence.lock().busy = false;
            },
        );
        if let Err(error) = transaction {
            log::error!("sololeveling: could not start merchant purchase: {error}");
            return fail("Покупку не удалось сохранить.");
        }
        true
    }
}

/// Checks the rank, balance and personal mark rules of one purchase.
fn check(
    rank: HunterRank,
    won: u64,
    record: merchant::MerchantRecord,
    offer: MerchantOffer,
) -> Result<(), MerchantError> {
    if !merchant::may_buy(offer.item, rank) {
        return Err(MerchantError::RankTooLow);
    }
    if offer.showcase == Showcase::Personal && record.bought(offer.period, offer.index) {
        return Err(MerchantError::AlreadyBought);
    }
    if won < offer.price {
        return Err(MerchantError::InsufficientWon);
    }
    Ok(())
}
