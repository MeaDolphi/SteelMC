//! Serialized gameplay persistence, using the core transaction coordinator.
//!
//! Ordinary saves capture live state when dequeued, not when requested, so a
//! queued snapshot cannot undo a payment that completed in the meantime.

mod account;
use account::load_account;
pub(crate) use account::{AccountUpdate, CommitReport, PreparedCommit};

use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Weak},
};

use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::{
    inventory::container::Container as _, player::Player,
    player::connection::NetworkConnection as _,
    player::inventory_transaction::StoredInventoryReservation,
    player::player_data_storage::transaction::ExtensionStorageWrite, server::Server,
};
use steel_registry::item_stack::ItemStack;
use steel_utils::types::InteractionHand;
use text_components::TextComponent;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::SoloLevelingState;
use super::auction::AuctionRequest;
use super::organization::GroupPayout;
use crate::{
    combat_reward::HunterReward,
    daily,
    economy::PaymentError,
    item::crystal::{self, CrystalSale, CrystalSaleError},
    organization::{Contract, GroupContract, OrganizationError},
    player_data::SoloLevelingPlayerData,
    rank::HunterRank,
    shop::{Offer, ShopError},
};

#[derive(Debug)]
enum Request {
    Load {
        uuid: Uuid,
        reply: oneshot::Sender<io::Result<SoloLevelingPlayerData>>,
    },
    Save {
        uuid: Uuid,
        retire: bool,
    },
    Pay {
        sender: Weak<Player>,
        recipient: Weak<Player>,
        amount: u64,
    },
    Credit {
        uuid: Uuid,
        amount: u64,
    },
    Award {
        rewards: Vec<(Uuid, HunterReward)>,
    },
    ShopPurchase {
        uuid: Uuid,
        slot: usize,
        offer: Offer,
    },
    TurnInPersonal {
        uuid: Uuid,
        index: usize,
        contract: Contract,
    },
    TurnInGroup {
        group: GroupContract,
    },
    CrystalSale {
        uuid: Uuid,
    },
    Auction(AuctionRequest),
    AuctionDeliver {
        hunter: Uuid,
    },
}

#[derive(Debug, Default)]
pub(super) struct Persistence {
    requests: VecDeque<Request>,
    saves: FxHashSet<Uuid>,
    expected: FxHashMap<Uuid, Option<Vec<u8>>>,
    prepared: FxHashMap<Uuid, SoloLevelingPlayerData>,
    failed: FxHashSet<Uuid>,
    busy: bool,
    quiescing: bool,
}

impl Persistence {
    /// Returns whether a new write for `uuid` may still be accepted.
    ///
    /// A refused account stays refused for the rest of the process, and a
    /// quiescing store is draining the shutdown fence.
    pub fn accepts_writes(&self, uuid: Uuid) -> bool {
        !self.quiescing && !self.failed.contains(&uuid)
    }

    /// Returns whether auction maintenance is already waiting in this lane.
    pub fn has_auction_work(&self) -> bool {
        self.requests.iter().any(|request| {
            matches!(
                request,
                Request::Auction(_) | Request::AuctionDeliver { .. }
            )
        })
    }
}

impl SoloLevelingState {
    fn release_shop_purchase(&self, uuid: Uuid) {
        self.persistence.lock().busy = false;
        self.sessions
            .with_mut(uuid, |session| session.economic_busy = false);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "preparing and submitting this one inventory/account transaction stays auditable"
    )]
    fn begin_shop_purchase(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: Arc<Player>,
        uuid: Uuid,
        slot: usize,
        offer: Offer,
    ) -> bool {
        if player.connection.closed() {
            self.release_shop_purchase(uuid);
            return false;
        }
        let Ok(mut reservation) = player.reserve_stored_inventory() else {
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain(
                "Покупку не удалось начать: инвентарь сейчас занят.",
            ));
            return false;
        };
        let snapshot = self.sessions.with_mut(uuid, |session| {
            let mut snapshot = session.saved_data(uuid);
            snapshot.shop.roll_over(daily::today());
            if snapshot.shop.offer(slot) != Some(offer) || snapshot.shop.bought(slot) {
                return Err("Этот слот магазина уже недоступен.");
            }
            if !snapshot.system_points.debit(offer.price) {
                return Err("Недостаточно очков Системы.");
            }
            if !snapshot.shop.mark(slot) {
                return Err("Этот слот магазина уже куплен.");
            }
            Ok(snapshot)
        });
        let snapshot = match snapshot {
            Some(Ok(snapshot)) => snapshot,
            Some(Err(message)) => {
                self.release_shop_purchase(uuid);
                player.send_message(&TextComponent::plain(message));
                return false;
            }
            None => {
                self.release_shop_purchase(uuid);
                player.send_message(&TextComponent::plain("Сессия охотника не найдена."));
                return false;
            }
        };
        if let Err(error) = stage_inventory_item(&mut reservation, offer.stack(uuid)) {
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain(error));
            return false;
        }
        let expected = self.persistence.lock().expected.get(&uuid).cloned();
        let Some(expected) = expected else {
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain("Данные охотника не загружены."));
            return false;
        };
        let replacement = match serde_json::to_vec(&snapshot) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::error!("sololeveling: could not encode shop purchase for {uuid}: {error}");
                self.release_shop_purchase(uuid);
                player.send_message(&TextComponent::plain("Не удалось сохранить покупку."));
                return false;
            }
        };
        let points = snapshot.system_points;
        let shop = snapshot.shop;
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
                let committed = result.is_ok();
                if committed {
                    state
                        .persistence
                        .lock()
                        .expected
                        .insert(uuid, Some(committed_bytes));
                    state.sessions.with_mut(uuid, |session| {
                        session.system_points = points;
                        session.shop = shop;
                        session.economic_busy = false;
                    });
                    if let Err(error) = state.save_session(uuid) {
                        log::warn!(
                            "sololeveling: could not queue follow-up shop account save for {uuid}: {error}"
                        );
                    }
                    completion_player.send_message(&TextComponent::plain(format!(
                        "Куплено: {}.",
                        offer.display_name()
                    )));
                } else {
                    log::error!("sololeveling: shop purchase transaction failed: {result:?}");
                    state
                        .sessions
                        .with_mut(uuid, |session| session.economic_busy = false);
                    completion_player.send_message(&TextComponent::plain(
                        "Покупка не сохранена; очки и предмет не списаны.",
                    ));
                }
                state.persistence.lock().busy = false;
            },
        );
        if let Err(error) = transaction {
            log::error!("sololeveling: could not start shop purchase transaction: {error}");
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain("Покупку не удалось сохранить."));
            return false;
        }
        true
    }

    #[expect(
        clippy::too_many_lines,
        reason = "crystal removal and account credit must be staged and committed as one transaction"
    )]
    fn begin_crystal_sale(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: Arc<Player>,
        uuid: Uuid,
    ) -> bool {
        for hand in [InteractionHand::MainHand, InteractionHand::OffHand] {
            let held = player.inventory.lock().get_item_in_hand(hand).clone();
            if crystal::rank(&held).is_some()
                && super::auction::park_hand_stack(&player, hand).is_none()
            {
                self.release_shop_purchase(uuid);
                player.send_message(&TextComponent::plain(
                    "Освободите место, чтобы убрать кристалл из руки перед продажей.",
                ));
                return false;
            }
        }
        let Ok(mut reservation) = player.reserve_stored_inventory() else {
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain(
                "Продажу не удалось начать: инвентарь сейчас занят.",
            ));
            return false;
        };
        let mut sale = CrystalSale::default();
        for slot in 0..36 {
            let Ok(stack) = reservation.item(slot) else {
                continue;
            };
            let Some(rank) = crystal::rank(&stack) else {
                continue;
            };
            let count = u32::try_from(stack.count).unwrap_or(0);
            sale.count = sale.count.saturating_add(count);
            sale.total = sale
                .total
                .saturating_add(crystal::sale_price(rank) * u64::from(count));
            if reservation.set_item(slot, ItemStack::empty()).is_err() {
                self.release_shop_purchase(uuid);
                player.send_message(&TextComponent::plain("Не удалось подготовить кристаллы."));
                return false;
            }
        }
        if sale.count == 0 {
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain(
                "В инвентаре нет магических кристаллов.",
            ));
            return false;
        }
        let Some(mut snapshot) = self
            .sessions
            .with_mut(uuid, |session| session.saved_data(uuid))
        else {
            self.release_shop_purchase(uuid);
            return false;
        };
        if !snapshot.won.try_credit(sale.total) {
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain(
                "Баланс Won переполнен; продажа отменена.",
            ));
            return false;
        }
        let balance = snapshot.won;
        let Some(expected) = self.persistence.lock().expected.get(&uuid).cloned() else {
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain("Данные охотника не загружены."));
            return false;
        };
        let replacement = match serde_json::to_vec(&snapshot) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::error!("sololeveling: could not encode crystal sale for {uuid}: {error}");
                self.release_shop_purchase(uuid);
                return false;
            }
        };
        let committed_bytes = replacement.clone();
        let state = Arc::clone(self);
        let completion_player = Arc::clone(&player);
        let transaction = server.submit_stored_inventory_transaction(
            vec![reservation],
            vec![ExtensionStorageWrite {
                key: account::key(uuid),
                expected,
                replacement,
            }],
            move |result| {
                if result.is_ok() {
                    state
                        .persistence
                        .lock()
                        .expected
                        .insert(uuid, Some(committed_bytes));
                    state.sessions.with_mut(uuid, |session| {
                        session.won = balance;
                        session.economic_busy = false;
                    });
                    if let Err(error) = state.save_session(uuid) {
                        log::warn!("sololeveling: could not queue sale follow-up save: {error}");
                    }
                    completion_player.send_message(&TextComponent::plain(format!(
                        "Организация авантюристов купила {} кристалл(ов) за {} Won.",
                        sale.count, sale.total
                    )));
                } else {
                    log::error!("sololeveling: crystal sale transaction failed: {result:?}");
                    state
                        .sessions
                        .with_mut(uuid, |session| session.economic_busy = false);
                    completion_player.send_message(&TextComponent::plain(
                        "Продажа не сохранена; кристаллы и баланс не изменены.",
                    ));
                }
                state.persistence.lock().busy = false;
            },
        );
        if let Err(error) = transaction {
            log::error!("sololeveling: could not start crystal sale transaction: {error}");
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain("Продажу не удалось сохранить."));
            return false;
        }
        true
    }

    fn begin_personal_contract_turn_in(
        self: &Arc<Self>,
        server: &Arc<Server>,
        uuid: Uuid,
        index: usize,
        expected_contract: Contract,
    ) {
        let Some(mut snapshot) = self
            .sessions
            .with_mut(uuid, |session| session.saved_data(uuid))
        else {
            self.release_shop_purchase(uuid);
            return;
        };
        let current = snapshot.organization.contracts.get(index).copied();
        if current != Some(expected_contract) || !expected_contract.is_complete() {
            self.release_shop_purchase(uuid);
            return;
        }
        let Ok(contract) = snapshot.organization.turn_in(index) else {
            self.release_shop_purchase(uuid);
            return;
        };
        if !snapshot.won.try_credit(contract.payment()) {
            self.release_shop_purchase(uuid);
            if let Some(player) = server
                .get_players()
                .into_iter()
                .find(|player| player.gameprofile.id() == uuid)
            {
                player.send_message(&TextComponent::plain(
                    "Баланс Won переполнен; сдача отменена.",
                ));
            }
            return;
        }
        let balance = snapshot.won;
        let book = snapshot.organization.clone();
        let state_server = Arc::clone(server);
        self.submit_commit(
            server,
            PreparedCommit::prepare(
                self,
                vec![snapshot],
                vec![
                    AccountUpdate::Won { uuid, balance },
                    AccountUpdate::Organization { uuid, book },
                ],
            ),
            move |state, report| {
                if !report.committed {
                    state
                        .sessions
                        .with_mut(uuid, |session| session.economic_busy = false);
                }
                if let Some(player) = state_server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                {
                    let message = if report.committed {
                        format!(
                            "Задание «{}» сдано. Начислено {} Won.",
                            contract.template.title(),
                            contract.payment()
                        )
                    } else {
                        "Не удалось сохранить сдачу задания; награда не начислена.".to_owned()
                    };
                    player.send_message(&TextComponent::plain(message));
                }
            },
        );
    }

    fn begin_group_contract_turn_in(self: &Arc<Self>, server: &Arc<Server>, group: GroupContract) {
        let payment = group.contract.payment();
        let mut snapshots = Vec::with_capacity(group.participants.len());
        let mut updates = Vec::with_capacity(group.participants.len());
        for uuid in &group.participants {
            let Some(mut snapshot) = self
                .sessions
                .with_mut(*uuid, |session| session.saved_data(*uuid))
            else {
                self.reset_group_turn_in(&group);
                for participant in &group.participants {
                    self.sessions
                        .with_mut(*participant, |session| session.economic_busy = false);
                }
                self.persistence.lock().busy = false;
                return;
            };
            if !snapshot.won.try_credit(payment) {
                self.reset_group_turn_in(&group);
                for participant in &group.participants {
                    self.sessions
                        .with_mut(*participant, |session| session.economic_busy = false);
                }
                self.persistence.lock().busy = false;
                return;
            }
            updates.push(AccountUpdate::Won {
                uuid: *uuid,
                balance: snapshot.won,
            });
            snapshots.push(snapshot);
        }
        let state_server = Arc::clone(server);
        let completion_group = group;
        self.submit_commit(
            server,
            PreparedCommit::prepare(self, snapshots, updates),
            move |state, report| {
                if report.committed {
                    for party in state.party_runtime.lock().parties_mut() {
                        if let Some(index) = party.contracts.iter().position(|active| {
                            active.pending_turn_in
                                && active.contract == completion_group.contract
                                && active
                                    .participants
                                    .iter()
                                    .any(|uuid| completion_group.participants.contains(uuid))
                        }) {
                            party.contracts.remove(index);
                            break;
                        }
                    }
                } else {
                    state.reset_group_turn_in(&completion_group);
                    for participant in &completion_group.participants {
                        state
                            .sessions
                            .with_mut(*participant, |session| session.economic_busy = false);
                    }
                }
                for player in state_server.get_players() {
                    if completion_group
                        .participants
                        .contains(&player.gameprofile.id())
                    {
                        let message = if report.committed {
                            format!(
                                "Групповое задание «{}» сдано. Получено {payment} Won.",
                                completion_group.contract.template.title()
                            )
                        } else {
                            "Не удалось сохранить выплату за групповое задание.".to_owned()
                        };
                        player.send_message(&TextComponent::plain(message));
                    }
                }
            },
        );
    }

    /// Waits off-tick for the account load ordered after earlier saves/payments.
    pub async fn prepare_join(self: Arc<Self>, uuid: Uuid) -> Result<(), String> {
        let (reply, receiver) = oneshot::channel();
        {
            let mut persistence = self.persistence.lock();
            if persistence.quiescing || persistence.failed.contains(&uuid) {
                return Err("Gameplay storage is unavailable".to_owned());
            }
            persistence
                .requests
                .push_back(Request::Load { uuid, reply });
        }
        let data = receiver
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        self.persistence.lock().prepared.insert(uuid, data);
        Ok(())
    }

    /// Transfers prepared data to the player's tick-owned admission hook.
    pub fn take_prepared_player(&self, uuid: Uuid) -> Option<SoloLevelingPlayerData> {
        self.persistence.lock().prepared.remove(&uuid)
    }

    /// Queues a current-state save; this is not a durable acknowledgement.
    pub fn save_session(&self, uuid: Uuid) -> io::Result<()> {
        let mut persistence = self.persistence.lock();
        if persistence.failed.contains(&uuid) {
            return Err(io::Error::other("gameplay account persistence failed"));
        }
        if persistence.saves.insert(uuid) {
            persistence.requests.push_back(Request::Save {
                uuid,
                retire: false,
            });
        }
        Ok(())
    }

    /// Queues a shop purchase for the serialized inventory/account transaction lane.
    ///
    /// Inventory reservation and disk work happen later, after the menu callback
    /// has released its locks.
    pub fn request_shop_purchase(&self, player: &Player, slot: usize) -> Result<Offer, ShopError> {
        let uuid = player.gameprofile.id();
        let today = daily::today();
        let offer = self
            .sessions
            .with_mut(uuid, |session| {
                if session.economic_busy {
                    return Err(ShopError::Busy);
                }
                session.shop.roll_over(today);
                let offer = session.shop.offer(slot).ok_or(ShopError::UnknownSlot)?;
                if session.shop.bought(slot) {
                    return Err(ShopError::AlreadyBought);
                }
                if session.system_points.amount() < offer.price {
                    return Err(ShopError::InsufficientPoints);
                }
                session.economic_busy = true;
                Ok(offer)
            })
            .ok_or(ShopError::NoSession)??;

        let mut persistence = self.persistence.lock();
        if persistence.quiescing || persistence.failed.contains(&uuid) {
            drop(persistence);
            self.sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            return Err(ShopError::Storage);
        }
        persistence
            .requests
            .push_back(Request::ShopPurchase { uuid, slot, offer });
        Ok(offer)
    }

    /// Queues a crystal sale as one inventory/account transaction.
    pub fn request_crystal_sale(&self, player: &Player) -> Result<(), CrystalSaleError> {
        let uuid = player.gameprofile.id();
        if !self.persistence.lock().accepts_writes(uuid) {
            return Err(CrystalSaleError::Storage);
        }
        let has_crystals = {
            let inventory = player.inventory.lock();
            (0..inventory.get_container_size())
                .any(|slot| crystal::rank(inventory.get_item(slot)).is_some())
        };
        if !has_crystals {
            return Err(CrystalSaleError::NoCrystals);
        }
        let Some(accepted) = self.sessions.with_mut(uuid, |session| {
            if session.economic_busy {
                return false;
            }
            session.economic_busy = true;
            true
        }) else {
            return Err(CrystalSaleError::Storage);
        };
        if !accepted {
            return Err(CrystalSaleError::Storage);
        }
        let mut persistence = self.persistence.lock();
        if persistence.quiescing || persistence.failed.contains(&uuid) {
            drop(persistence);
            self.sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            return Err(CrystalSaleError::Storage);
        }
        persistence
            .requests
            .push_back(Request::CrystalSale { uuid });
        Ok(())
    }

    /// Queues a finished personal contract for an account-and-reward transaction.
    pub fn turn_in_personal_contract(
        &self,
        uuid: Uuid,
        index: usize,
    ) -> Result<Contract, OrganizationError> {
        if !self.persistence.lock().accepts_writes(uuid) {
            return Err(OrganizationError::Storage);
        }
        let contract = self
            .sessions
            .with_mut(uuid, |session| {
                if session.economic_busy {
                    return Err(OrganizationError::Storage);
                }
                let contract = session
                    .organization
                    .contracts
                    .get(index)
                    .copied()
                    .ok_or(OrganizationError::Missing)?;
                if !contract.is_complete() {
                    return Err(OrganizationError::NotComplete);
                }
                session.economic_busy = true;
                Ok(contract)
            })
            .ok_or(OrganizationError::NoSession)??;
        let mut persistence = self.persistence.lock();
        if persistence.quiescing || persistence.failed.contains(&uuid) {
            drop(persistence);
            self.sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            return Err(OrganizationError::Storage);
        }
        persistence.requests.push_back(Request::TurnInPersonal {
            uuid,
            index,
            contract,
        });
        Ok(contract)
    }

    /// Queues the fixed shares for every member who remains in a finished party contract.
    pub fn turn_in_group_contract(
        &self,
        leader: Uuid,
        index: usize,
    ) -> Result<GroupPayout, OrganizationError> {
        let group = {
            let mut parties = self.party_runtime.lock();
            let party = parties
                .led_party_mut(leader)
                .map_err(super::organization::party_error)?;
            let group = party
                .contracts
                .get_mut(index)
                .ok_or(OrganizationError::Missing)?;
            if !group.contract.is_complete() {
                return Err(OrganizationError::NotComplete);
            }
            if group.pending_turn_in {
                return Err(OrganizationError::Storage);
            }
            group.pending_turn_in = true;
            group.clone()
        };
        if group
            .participants
            .iter()
            .any(|uuid| !self.persistence.lock().accepts_writes(*uuid))
        {
            self.reset_group_turn_in(&group);
            return Err(OrganizationError::Storage);
        }
        let mut busy = Vec::new();
        for uuid in &group.participants {
            if self.sessions.with_mut(*uuid, |session| {
                if session.economic_busy {
                    return false;
                }
                session.economic_busy = true;
                true
            }) != Some(true)
            {
                for participant in busy {
                    self.sessions
                        .with_mut(participant, |session| session.economic_busy = false);
                }
                self.reset_group_turn_in(&group);
                return Err(OrganizationError::Storage);
            }
            busy.push(*uuid);
        }
        self.persistence
            .lock()
            .requests
            .push_back(Request::TurnInGroup {
                group: group.clone(),
            });
        Ok(GroupPayout {
            contract: group.contract,
            paid: group.participants,
        })
    }

    fn reset_group_turn_in(&self, expected: &GroupContract) {
        for party in self.party_runtime.lock().parties_mut() {
            if let Some(group) = party.contracts.iter_mut().find(|group| {
                group.pending_turn_in
                    && group.contract == expected.contract
                    && group
                        .participants
                        .iter()
                        .any(|uuid| expected.participants.contains(uuid))
            }) {
                group.pending_turn_in = false;
            }
        }
    }

    /// Saves and removes a disconnected session after earlier financial work.
    pub fn retire_session(&self, uuid: Uuid) {
        self.persistence
            .lock()
            .requests
            .push_back(Request::Save { uuid, retire: true });
    }

    /// Queues a payment; only its completion sends a durable success message.
    pub fn request_payment(
        &self,
        sender: &Arc<Player>,
        recipient: &Arc<Player>,
        amount: u64,
    ) -> Result<(), PaymentError> {
        let from = sender.gameprofile.id();
        let to = recipient.gameprofile.id();
        if from == to {
            return Err(PaymentError::SelfPayment);
        }
        let from_balance = self
            .sessions
            .with_mut(from, |session| session.won)
            .ok_or(PaymentError::Unavailable)?;
        let to_balance = self
            .sessions
            .with_mut(to, |session| session.won)
            .ok_or(PaymentError::Unavailable)?;
        from_balance.payment(to_balance, amount)?;
        let mut persistence = self.persistence.lock();
        if persistence.failed.contains(&from)
            || persistence.failed.contains(&to)
            || persistence.quiescing
        {
            return Err(PaymentError::Storage);
        }
        persistence.requests.push_back(Request::Pay {
            sender: Arc::downgrade(sender),
            recipient: Arc::downgrade(recipient),
            amount,
        });
        Ok(())
    }

    /// Reward sources enqueue Won mutations in the same lane as payments.
    pub fn credit_won(&self, uuid: Uuid, amount: u64) {
        self.persistence
            .lock()
            .requests
            .push_back(Request::Credit { uuid, amount });
    }

    /// Awards only the qualifying killer/owner, never the rest of the party.
    pub fn credit_dungeon_mob(&self, uuid: Uuid, rank: HunterRank) {
        self.persistence.lock().requests.push_back(Request::Award {
            rewards: vec![(uuid, HunterReward::for_mob(rank))],
        });
    }

    /// Queues one auction operation on the serialized persistence lane.
    ///
    /// Auction commits write the lot ledger together with the accounts they
    /// charge, so they must not interleave with any other account write.
    pub fn request_auction(&self, request: AuctionRequest) {
        self.persistence
            .lock()
            .requests
            .push_back(Request::Auction(request));
    }

    /// Queues the delivery of everything the auction house owes one hunter.
    pub fn request_auction_delivery(&self, hunter: Uuid) {
        self.persistence
            .lock()
            .requests
            .push_back(Request::AuctionDeliver { hunter });
    }

    /// Returns whether auction maintenance is already queued.
    pub fn auction_maintenance_queued(&self) -> bool {
        self.persistence.lock().has_auction_work()
    }

    /// Releases the lane after a commit that another module started.
    ///
    /// The lane is taken when a request is dequeued; a commit submitted outside
    /// [`SoloLevelingState::submit_commit`] must return it once it is resolved.
    pub(super) fn release_persistence_lane(&self) {
        self.persistence.lock().busy = false;
    }

    /// Settles the shared boss pool only for the captured living finishers.
    pub fn credit_dungeon_boss(&self, survivors: Vec<Uuid>, rank: HunterRank) {
        let rewards = HunterReward::boss_shares(rank, survivors.len());
        if rewards.is_empty() {
            return;
        }
        self.persistence.lock().requests.push_back(Request::Award {
            rewards: survivors.into_iter().zip(rewards).collect(),
        });
    }

    /// Starts at most one I/O operation without awaiting it on the game tick.
    ///
    /// # Panics
    ///
    /// Panics when the account load job fails, which means the storage layer is
    /// unusable and the server cannot keep serving gameplay state.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per persistence request keeps the single I/O lane legible"
    )]
    pub fn tick_persistence(self: &Arc<Self>, server: &Arc<Server>) {
        let request = {
            let mut persistence = self.persistence.lock();
            if persistence.busy {
                return;
            }
            let Some(request) = persistence.requests.pop_front() else {
                return;
            };
            persistence.busy = true;
            if let Request::Save { uuid, .. } = &request {
                persistence.saves.remove(uuid);
            }
            request
        };
        match request {
            Request::Load { uuid, reply } => {
                let state = Arc::clone(self);
                let server_task = Arc::clone(server);
                let legacy = self.storage.clone();
                server.jobs.spawn_io(
                    async move { load_account(&server_task.player_data_storage, legacy, uuid).await },
                    move |result| {
                        let loaded = match result {
                            Ok(result) => result,
                            Err(error) => panic!("gameplay account load task failed: {error}"),
                        };
                        let mut persistence = state.persistence.lock();
                        persistence.busy = false;
                        match loaded {
                            Ok(loaded) => {
                                persistence.expected.insert(uuid, loaded.bytes);
                                if reply.send(Ok(loaded.data)).is_err() {
                                    persistence.expected.remove(&uuid);
                                }
                            }
                            Err(error) => {
                                let _ = reply.send(Err(error));
                            }
                        }
                    },
                );
            }
            Request::Save { uuid, retire } => {
                let snapshot = self
                    .sessions
                    .with_mut(uuid, |session| session.saved_data(uuid));
                let Some(snapshot) = snapshot else {
                    self.persistence.lock().busy = false;
                    return;
                };
                self.submit_commit(
                    server,
                    PreparedCommit::prepare(self, vec![snapshot], Vec::new()),
                    move |state, report| {
                        if report.confirmed && retire {
                            state.sessions.remove(uuid);
                            state.persistence.lock().expected.remove(&uuid);
                        }
                    },
                );
            }
            Request::Pay {
                sender,
                recipient,
                amount,
            } => {
                let Some(sender) = sender.upgrade() else {
                    self.persistence.lock().busy = false;
                    return;
                };
                let Some(recipient) = recipient.upgrade() else {
                    self.persistence.lock().busy = false;
                    return;
                };
                if sender.connection.closed() || recipient.connection.closed() {
                    self.persistence.lock().busy = false;
                    sender.send_message(&TextComponent::plain(
                        "Перевод отменён: получатель или отправитель вышел.",
                    ));
                    return;
                }
                let from = sender.gameprofile.id();
                let to = recipient.gameprofile.id();
                let snapshots = self
                    .sessions
                    .with_mut(from, |session| session.saved_data(from))
                    .zip(self.sessions.with_mut(to, |session| session.saved_data(to)));
                let Some((mut from_data, mut to_data)) = snapshots else {
                    self.persistence.lock().busy = false;
                    return;
                };
                let balances = from_data.won.payment(to_data.won, amount);
                let Ok((from_balance, to_balance)) = balances else {
                    self.persistence.lock().busy = false;
                    sender.send_message(&TextComponent::plain(
                        "Перевод отменён: недостаточно средств или переполнение баланса.",
                    ));
                    return;
                };
                from_data.won = from_balance;
                to_data.won = to_balance;
                self.submit_commit(server, PreparedCommit::prepare(self, vec![from_data, to_data], vec![
                    AccountUpdate::Won { uuid: from, balance: from_balance },
                    AccountUpdate::Won { uuid: to, balance: to_balance },
                ]), move |_, report| {
                    if report.confirmed {
                        sender.send_message(&TextComponent::plain(format!("Передано {amount} Won игроку {}.", recipient.gameprofile.name())));
                        recipient.send_message(&TextComponent::plain(format!("Получено {amount} Won от {}.", sender.gameprofile.name())));
                    } else {
                        sender.send_message(&TextComponent::plain("Перевод не подтверждён. Не повторяйте его до проверки администратором."));
                    }
                });
            }
            Request::Credit { uuid, amount } => {
                let Some(mut snapshot) = self
                    .sessions
                    .with_mut(uuid, |session| session.saved_data(uuid))
                else {
                    self.persistence.lock().busy = false;
                    return;
                };
                if !snapshot.won.try_credit(amount) {
                    self.persistence.lock().busy = false;
                    log::error!("sololeveling: Won reward overflows account {uuid}");
                    return;
                }
                let balance = snapshot.won;
                self.submit_commit(
                    server,
                    PreparedCommit::prepare(
                        self,
                        vec![snapshot],
                        vec![AccountUpdate::Won { uuid, balance }],
                    ),
                    |_, _| {},
                );
            }
            Request::Award { rewards } => {
                self.submit_commit(server, PreparedCommit::rewards(self, rewards), |_, _| {});
            }
            Request::ShopPurchase { uuid, slot, offer } => {
                let Some(player) = server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                else {
                    self.release_shop_purchase(uuid);
                    return;
                };
                if !self.begin_shop_purchase(server, player, uuid, slot, offer) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::CrystalSale { uuid } => {
                let Some(player) = server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                else {
                    self.release_shop_purchase(uuid);
                    return;
                };
                if !self.begin_crystal_sale(server, player, uuid) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::TurnInPersonal {
                uuid,
                index,
                contract,
            } => self.begin_personal_contract_turn_in(server, uuid, index, contract),
            Request::TurnInGroup { group } => {
                self.begin_group_contract_turn_in(server, group);
            }
            Request::Auction(request) => {
                if !self.begin_auction(server, request) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::AuctionDeliver { hunter } => {
                if !self.begin_auction_delivery(server, hunter) {
                    self.persistence.lock().busy = false;
                }
            }
        }
    }

    /// Starts one transaction without awaiting it on the game tick.
    pub(super) fn submit_commit(
        self: &Arc<Self>,
        server: &Arc<Server>,
        prepared: io::Result<PreparedCommit>,
        completion: impl FnOnce(&Self, CommitReport) + Send + 'static,
    ) {
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.persistence.lock().busy = false;
                log::error!("sololeveling: could not prepare account write: {error}");
                completion(
                    self,
                    CommitReport {
                        committed: false,
                        confirmed: false,
                    },
                );
                return;
            }
        };
        let state = Arc::clone(self);
        let server_task = Arc::clone(server);
        server.jobs.spawn_io(
            async move {
                let result = server_task
                    .player_data_storage
                    .commit_transaction(&prepared.transaction)
                    .await;
                (prepared, result)
            },
            move |result| {
                let (prepared, result) = match result {
                    Ok(result) => result,
                    Err(error) => panic!("gameplay account commit task failed: {error}"),
                };
                let report = prepared.complete(&state, &result);
                completion(&state, report);
            },
        );
    }

    /// Drains queued writes off-tick, rejecting new admissions during shutdown.
    pub async fn flush_persistence(self: Arc<Self>, server: Arc<Server>) -> Result<(), String> {
        self.persistence.lock().quiescing = true;
        for uuid in self.sessions.player_ids() {
            self.save_session(uuid).map_err(|error| error.to_string())?;
        }
        loop {
            server.jobs.finish_all().await;
            let empty = {
                let persistence = self.persistence.lock();
                if !persistence.failed.is_empty() {
                    return Err(format!(
                        "Gameplay accounts with failed writes: {:?}",
                        persistence.failed
                    ));
                }
                persistence.requests.is_empty() && !persistence.busy
            };
            if empty {
                return Ok(());
            }
            self.tick_persistence(&server);
        }
    }
}

fn stage_inventory_item(
    reservation: &mut StoredInventoryReservation,
    item: ItemStack,
) -> Result<(), &'static str> {
    const INVENTORY_SLOTS: usize = 36;
    let mut slots = (0..INVENTORY_SLOTS)
        .map(|slot| reservation.item(slot).ok())
        .collect::<Vec<_>>();
    insert_stack(&mut slots, item)?;
    for (slot, item) in slots.into_iter().enumerate() {
        let Some(item) = item else {
            continue;
        };
        reservation
            .set_item(slot, item)
            .map_err(|_| "Не удалось подготовить инвентарь к покупке.")?;
    }
    Ok(())
}

fn insert_stack(slots: &mut [Option<ItemStack>], item: ItemStack) -> Result<(), &'static str> {
    let mut staged = slots.to_vec();
    let mut item = item;
    let mut empty_slot = None;
    for (slot, current) in staged.iter_mut().enumerate() {
        let Some(current) = current else {
            continue;
        };
        if current.is_empty() {
            empty_slot.get_or_insert(slot);
            continue;
        }
        if !ItemStack::is_same_item_same_components(current, &item) {
            continue;
        }
        let space = current.max_stack_size().saturating_sub(current.count);
        if space <= 0 {
            continue;
        }
        let moved = space.min(item.count);
        current.set_count(current.count + moved);
        item.set_count(item.count - moved);
        if item.is_empty() {
            slots.clone_from_slice(&staged);
            return Ok(());
        }
    }
    let Some(slot) = empty_slot else {
        return Err("Инвентарь заполнен. Освободите место для покупки.");
    };
    staged[slot] = Some(item);
    slots.clone_from_slice(&staged);
    Ok(())
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, item_stack::ItemStack, vanilla_items};

    use super::insert_stack;

    #[test]
    fn shop_delivery_merges_matching_stacks_before_using_an_empty_slot() {
        init_vanilla_registry();
        let mut slots = vec![
            Some(ItemStack::with_count(&vanilla_items::BAKED_POTATO, 60)),
            Some(ItemStack::empty()),
            None,
        ];
        insert_stack(
            &mut slots,
            ItemStack::with_count(&vanilla_items::BAKED_POTATO, 8),
        )
        .expect("the complete offer fits");
        assert_eq!(slots[0].as_ref().map(|stack| stack.count), Some(64));
        assert_eq!(slots[1].as_ref().map(|stack| stack.count), Some(4));
        assert!(slots[2].is_none());
    }

    #[test]
    fn a_full_inventory_refuses_the_whole_offer_without_partial_changes() {
        init_vanilla_registry();
        let mut slots = vec![Some(ItemStack::with_count(&vanilla_items::STONE, 64)); 36];
        let before = slots.clone();
        assert!(
            insert_stack(
                &mut slots,
                ItemStack::with_count(&vanilla_items::BAKED_POTATO, 8),
            )
            .is_err()
        );
        assert_eq!(slots, before);
    }
}
