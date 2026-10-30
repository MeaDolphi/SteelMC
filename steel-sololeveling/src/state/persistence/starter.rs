//! Atomic delivery and weapon swap of the Association starter kit.
//!
//! The right is spent in the same account write that hands out the items, so
//! a double click, a disconnect, or a failed save can never yield a second
//! kit or spend the right without the items.

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
use crate::item::artifact::ArtifactId;
use crate::item::{artifact, binding, nbt};
use crate::quest_book::Goal;
use crate::sound::SystemSound;
use crate::starter::{self, StarterKit};
use crate::starter_menu::StarterMenuKind;
use crate::state::SoloLevelingState;

/// Slots of the main inventory and hotbar a returned weapon may sit in.
const INVENTORY_SLOTS: usize = 36;

#[derive(Debug, Clone, Copy)]
pub(super) enum StarterTransfer {
    /// Claim the kit with this weapon.
    Claim(ArtifactId),
    /// Hand the claimed weapon back for this one.
    Swap(ArtifactId),
    /// Hand out the pickaxe a kit claimed before it existed still owes.
    Pickaxe,
}

/// Why a starter-kit request was refused before anything changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarterError {
    /// The free kit was already claimed.
    AlreadyClaimed,
    /// Nothing to swap, or the swap ended with the first instance entry.
    SwapClosed,
    /// The request named something that is not a starter weapon.
    NotAWeapon,
    /// The pickaxe was handed out already, or the kit is not claimed yet.
    PickaxeNotOwed,
    /// Another economic operation of this hunter is still running.
    Busy,
    /// The hunter's data cannot be written right now.
    Storage,
}

impl StarterError {
    /// Returns the Russian message shown to the hunter.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::AlreadyClaimed => "Стартовый набор уже получен.",
            Self::SwapClosed => {
                "Бесплатный обмен оружия доступен только до первого входа во врата."
            }
            Self::NotAWeapon => "Это не стартовое оружие.",
            Self::PickaxeNotOwed => "Кирка выдаётся вместе с набором и только один раз.",
            Self::Busy => "Предыдущая операция ещё обрабатывается.",
            Self::Storage => "Данные охотника сейчас недоступны для записи.",
        }
    }
}

impl SoloLevelingState {
    /// Returns the hunter's starter-kit record.
    #[must_use]
    pub fn starter_kit(&self, uuid: Uuid) -> Option<StarterKit> {
        self.sessions.with_mut(uuid, |session| session.starter)
    }

    /// Queues the one-time starter kit with the chosen `weapon`.
    pub fn request_starter_kit(
        &self,
        player: &Player,
        weapon: ArtifactId,
    ) -> Result<(), StarterError> {
        self.queue_starter(player, StarterTransfer::Claim(weapon))
    }

    /// Queues a free swap of the claimed starter weapon for `weapon`.
    pub fn request_starter_swap(
        &self,
        player: &Player,
        weapon: ArtifactId,
    ) -> Result<(), StarterError> {
        self.queue_starter(player, StarterTransfer::Swap(weapon))
    }

    /// Queues the pickaxe owed to a kit claimed before it existed.
    pub fn request_starter_pickaxe(&self, player: &Player) -> Result<(), StarterError> {
        self.queue_starter(player, StarterTransfer::Pickaxe)
    }

    /// Ends the free weapon swap once the hunter stands inside an instance.
    pub(crate) fn close_starter_swap(&self, uuid: Uuid) {
        let closed = self.sessions.with_mut(uuid, |session| {
            if !session.starter.swappable() {
                return false;
            }
            session.starter.swap_closed = true;
            true
        });
        if closed == Some(true)
            && let Err(error) = self.save_session(uuid)
        {
            log::warn!("sololeveling: could not queue starter swap closure for {uuid}: {error}");
        }
    }

    fn queue_starter(
        &self,
        player: &Player,
        transfer: StarterTransfer,
    ) -> Result<(), StarterError> {
        let uuid = player.gameprofile.id();
        if let StarterTransfer::Claim(weapon) | StarterTransfer::Swap(weapon) = transfer
            && !starter::is_weapon(weapon)
        {
            return Err(StarterError::NotAWeapon);
        }
        let accepted = self
            .sessions
            .with_mut(uuid, |session| {
                if session.economic_busy {
                    return Err(StarterError::Busy);
                }
                match transfer {
                    StarterTransfer::Claim(_) if !session.starter.claimable() => {
                        return Err(StarterError::AlreadyClaimed);
                    }
                    StarterTransfer::Swap(_) if !session.starter.swappable() => {
                        return Err(StarterError::SwapClosed);
                    }
                    StarterTransfer::Pickaxe if !session.starter.pickaxe_owed() => {
                        return Err(StarterError::PickaxeNotOwed);
                    }
                    _ => {}
                }
                session.economic_busy = true;
                Ok(())
            })
            .ok_or(StarterError::Storage)?;
        accepted?;
        let mut persistence = self.persistence.lock();
        if persistence.quiescing || persistence.failed.contains(&uuid) {
            drop(persistence);
            self.sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            return Err(StarterError::Storage);
        }
        persistence
            .requests
            .push_back(Request::Starter { uuid, transfer });
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the right, the items and the overflow storage must commit as one transaction"
    )]
    pub(super) fn begin_starter_transfer(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: Arc<Player>,
        uuid: Uuid,
        transfer: StarterTransfer,
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
        let Ok(mut reservation) = player.reserve_stored_inventory_with_menu::<StarterMenuKind>()
        else {
            return fail(
                "Выдачу не удалось начать: инвентарь занят или предмет находится на курсоре или в сетке крафта.",
            );
        };
        let Some(mut snapshot) = self
            .sessions
            .with_mut(uuid, |session| session.saved_data(uuid))
        else {
            return fail("Сессия охотника не найдена.");
        };
        let delivered = match transfer {
            StarterTransfer::Claim(weapon) => {
                if !snapshot.starter.claimable() {
                    return fail(StarterError::AlreadyClaimed.message());
                }
                snapshot.starter.weapon = Some(weapon);
                snapshot.starter.pickaxe = true;
                starter::kit(uuid, weapon)
            }
            StarterTransfer::Pickaxe => {
                if !snapshot.starter.pickaxe_owed() {
                    return fail(StarterError::PickaxeNotOwed.message());
                }
                snapshot.starter.pickaxe = true;
                vec![artifact::stack(ArtifactId::HunterPickaxe, Some(uuid))]
            }
            StarterTransfer::Swap(weapon) => {
                let Some(old) = snapshot
                    .starter
                    .weapon
                    .filter(|_| snapshot.starter.swappable())
                else {
                    return fail(StarterError::SwapClosed.message());
                };
                if old == weapon {
                    return fail("Это оружие у вас уже есть.");
                }
                // The old weapon is handed back in the same write, so a swap
                // can never leave the hunter with two starter weapons.
                let returned = (0..INVENTORY_SLOTS).find(|slot| {
                    reservation.item(*slot).is_ok_and(|stack| {
                        artifact::of(&stack) == Some(old)
                            && binding::bound_owner(&stack) == Some(uuid)
                    })
                });
                let Some(slot) = returned else {
                    return fail(
                        "Положите старое стартовое оружие в инвентарь, чтобы обменять его.",
                    );
                };
                if reservation
                    .set_item(slot, artifact::stack(weapon, Some(uuid)))
                    .is_err()
                {
                    return fail("Не удалось подготовить инвентарь к обмену.");
                }
                snapshot.starter.weapon = Some(weapon);
                Vec::new()
            }
        };
        let mut stored = 0_usize;
        for item in delivered {
            if stage_inventory_item(&mut reservation, item.clone()).is_ok() {
                continue;
            }
            match nbt::encode(&item) {
                Ok(bytes) => {
                    snapshot.deferred_inventory.push(bytes);
                    stored += 1;
                }
                Err(error) => {
                    log::error!(
                        "sololeveling: could not encode a starter item for {uuid}: {error}"
                    );
                    return fail("Не удалось подготовить стартовый набор.");
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
                log::error!("sololeveling: could not encode starter kit for {uuid}: {error}");
                return fail("Не удалось сохранить выдачу.");
            }
        };
        let record = snapshot.starter;
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
                        if matches!(transfer, StarterTransfer::Claim(_)) {
                            state.record_goal(uuid, Goal::StarterKit, 1);
                        }
                        state
                            .persistence
                            .lock()
                            .expected
                            .insert(uuid, Some(committed_bytes));
                        state.sessions.with_mut(uuid, |session| {
                            session.starter = record;
                            session.deferred_inventory = deferred_inventory;
                            session.economic_busy = false;
                        });
                        SystemSound::Reward.play(&completion_player);
                        let message = match transfer {
                            StarterTransfer::Claim(_) if stored > 0 => format!(
                                "Стартовый набор получен. Не поместилось предметов: {stored} — они в Хранилище."
                            ),
                            StarterTransfer::Claim(_) => "Стартовый набор получен.".to_owned(),
                            StarterTransfer::Swap(weapon) => {
                                format!("Оружие обменено: {}.", weapon.artifact().name)
                            }
                            StarterTransfer::Pickaxe if stored > 0 => {
                                "Кирка охотника получена — она в Хранилище.".to_owned()
                            }
                            StarterTransfer::Pickaxe => "Кирка охотника получена.".to_owned(),
                        };
                        completion_player.send_message(&TextComponent::plain(message));
                        if commit == TransactionCommit::DurabilityUncertain {
                            completion_player.send_message(&TextComponent::plain(
                                "Выдача выполнена, но подтверждение диска не получено.",
                            ));
                        }
                    }
                    Err(error) => {
                        log::error!("sololeveling: starter kit transaction failed: {error:?}");
                        state
                            .sessions
                            .with_mut(uuid, |session| session.economic_busy = false);
                        completion_player.send_message(&TextComponent::plain(
                            "Выдача не сохранена; набор и право на него не изменены.",
                        ));
                    }
                }
                state.persistence.lock().busy = false;
            },
        );
        if let Err(error) = transaction {
            log::error!("sololeveling: could not start starter kit transaction: {error}");
            return fail("Выдачу не удалось сохранить.");
        }
        true
    }
}
