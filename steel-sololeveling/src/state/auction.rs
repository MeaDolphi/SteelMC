//! Auction house orchestration: queued lot operations, one ledger, deliveries.
//!
//! Every mutation runs on the serialized persistence lane: the lot ledger and
//! the accounts it charges are published in one core storage transaction, so a
//! bid can never be withdrawn from a balance that the ledger did not record.
//! Stacks are escrowed through an inventory reservation, which keeps the
//! listing and its published lot on the same side of a crash.

use std::io;
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use rustc_hash::FxHashSet;
use steel_core::inventory::container::Container as _;
use steel_core::player::Player;
use steel_core::player::player_data_storage::transaction::{
    ExtensionStorageWrite, TransactionCommit,
};
use steel_core::player::player_inventory::{PlayerInventory, Prediction};
use steel_core::server::Server;
use steel_registry::data_components::vanilla_components::CUSTOM_NAME;
use steel_registry::item_stack::ItemStack;
use steel_utils::Identifier;
use steel_utils::types::InteractionHand;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::SoloLevelingState;
use super::persistence::{AccountUpdate, CommitReport, PreparedCommit};
use crate::auction::{AuctionError, Ledger, Lot, LotKind};
use crate::item::{binding, nbt};
use crate::player_data::SoloLevelingPlayerData;

/// How often lot expiry and pending deliveries are checked, in ticks.
pub const MAINTENANCE_INTERVAL_TICKS: u64 = 20;

/// Returns the current Unix time in seconds.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Returns the gameplay document holding every lot and delivery.
const fn document() -> Identifier {
    Identifier::new_static("sololeveling", "auction/ledger")
}

/// Owns the ledger and the exact bytes the stored document had last time.
#[derive(Debug, Default)]
pub struct AuctionState {
    ledger: Ledger,
    expected: Option<Vec<u8>>,
}

impl AuctionState {
    /// Installs a ledger whose document was just published.
    fn committed(&mut self, ledger: Ledger, bytes: Vec<u8>) {
        self.ledger = ledger;
        self.expected = Some(bytes);
    }
}

/// One queued auction operation.
#[derive(Debug)]
pub enum AuctionRequest {
    /// Escrows the stack in the hunter's main hand.
    List {
        /// Hunter listing the stack.
        player: Weak<Player>,
        /// How the lot accepts buyers.
        kind: LotKind,
        /// Fixed price, or the lowest first bid.
        price: u64,
        /// Duration in whole hours.
        hours: u64,
    },
    /// Pays a fixed price immediately.
    Buy {
        /// Hunter paying the price.
        player: Weak<Player>,
        /// Lot to buy.
        lot: u64,
    },
    /// Places a bidding-lot bid and escrows its money.
    Bid {
        /// Hunter placing the bid.
        player: Weak<Player>,
        /// Lot to bid on.
        lot: u64,
        /// Escrowed amount.
        amount: u64,
    },
    /// Withdraws a lot nobody has bid on.
    Cancel {
        /// Seller withdrawing the lot.
        player: Weak<Player>,
        /// Lot to withdraw.
        lot: u64,
    },
    /// Resolves every lot whose deadline has passed.
    Expire,
}

/// One lot as the board displays it.
#[derive(Debug, Clone)]
pub struct LotView {
    /// Identifier used by `/auction buy`, `bid`, and `cancel`.
    pub id: u64,
    /// Seller, so a menu can tell a hunter's own lots apart.
    pub seller: Uuid,
    /// Seller name captured at listing time.
    pub seller_name: String,
    /// Display name of the escrowed stack.
    pub item_name: String,
    /// Decoded escrowed item for the auction menu.
    pub item: Option<ItemStack>,
    /// Stack size.
    pub count: i32,
    /// How the lot accepts buyers.
    pub kind: LotKind,
    /// Fixed price, or the lowest first bid.
    pub price: u64,
    /// Standing bid, when there is one.
    pub bid: Option<u64>,
    /// Seconds until the lot resolves.
    pub seconds_left: u64,
}

/// Returns the display name of an escrowed stack.
fn item_name(stack: &ItemStack) -> String {
    stack
        .get(CUSTOM_NAME)
        .map_or_else(|| stack.item.key.path.to_string(), ToString::to_string)
}

/// Returns one lot as the board displays it.
fn view(lot: &Lot, now: u64) -> LotView {
    let stack = nbt::decode(&lot.item).ok();
    LotView {
        id: lot.id,
        seller: lot.seller,
        seller_name: lot.seller_name.clone(),
        item_name: stack
            .as_ref()
            .map_or_else(|| "неизвестный предмет".to_owned(), item_name),
        count: stack.as_ref().map_or(1, |stack| stack.count),
        item: stack,
        kind: lot.kind,
        price: lot.price,
        bid: lot.bid.map(|bid| bid.amount),
        seconds_left: lot.expires_at.saturating_sub(now),
    }
}

/// Returns how a refused lot operation reads in chat.
const fn refusal(error: AuctionError) -> &'static str {
    match error {
        AuctionError::UnknownLot => "Такого лота больше нет.",
        AuctionError::OwnLot => "Нельзя торговать со своим лотом.",
        AuctionError::NotSeller => "Снять лот может только его продавец.",
        AuctionError::Expired => "Срок лота истёк.",
        AuctionError::NotForBidding => "Этот лот продаётся с торгов: используйте /auction bid.",
        AuctionError::NotForSale => "Лот с фиксированной ценой: используйте /auction buy.",
        AuctionError::BidsExist => "Лот со ставкой снять нельзя.",
        AuctionError::LotLimit => "У вас уже 10 активных лотов.",
        AuctionError::InvalidPrice => "Цена и ставка — целое число Won не меньше 1.",
        AuctionError::InvalidDuration => "Срок лота — целое число часов от 1 до 24.",
        AuctionError::BidTooLow => "Ставка ниже минимальной.",
    }
}

/// Tells a hunter that an auction operation was refused.
fn refuse(player: &Player, message: &str) {
    player.send_message(&TextComponent::plain(message.to_owned()).color(Color::Red));
}

/// Announces a committed auction operation.
fn announce(player: &Player, message: String) {
    player.send_message(&TextComponent::plain(message).color(Color::Gold));
}

impl SoloLevelingState {
    /// Loads the auction ledger once, before the server accepts connections.
    pub async fn load_auction(&self, server: &Server) -> Result<(), String> {
        let bytes = server
            .player_data_storage
            .load_extension(&document())
            .await
            .map_err(|error| format!("failed to load the auction ledger: {error}"))?;
        let ledger = match &bytes {
            Some(bytes) => serde_json::from_slice(bytes)
                .map_err(|error| format!("failed to parse the auction ledger: {error}"))?,
            None => Ledger::default(),
        };
        let mut auction = self.auction.lock();
        auction.ledger = ledger;
        auction.expected = bytes;
        Ok(())
    }

    /// Returns whether the auction house still owes this hunter something.
    #[must_use]
    pub fn has_pending_delivery(&self, hunter: Uuid) -> bool {
        self.auction.lock().ledger.delivery(hunter).is_some()
    }

    /// Returns every active lot, soonest deadline first.
    #[must_use]
    pub fn auction_lots(&self) -> Vec<LotView> {
        let now = unix_now();
        let auction = self.auction.lock();
        let mut views = auction
            .ledger
            .lots()
            .iter()
            .map(|lot| view(lot, now))
            .collect::<Vec<_>>();
        views.sort_by_key(|view| view.seconds_left);
        views
    }

    /// Returns the caller's active lots and everything owed to them.
    #[must_use]
    pub fn auction_personal(&self, hunter: Uuid) -> (Vec<LotView>, u64, usize) {
        let now = unix_now();
        let auction = self.auction.lock();
        let mut lots = auction
            .ledger
            .lots()
            .iter()
            .filter(|lot| lot.seller == hunter)
            .map(|lot| view(lot, now))
            .collect::<Vec<_>>();
        lots.sort_by_key(|view| view.seconds_left);
        let delivery = auction.ledger.delivery(hunter);
        (
            lots,
            delivery.map_or(0, |delivery| delivery.won),
            delivery.map_or(0, |delivery| delivery.items.len()),
        )
    }

    /// Queues lot expiry and pending deliveries once per second.
    pub fn tick_auction(&self, server: &Arc<Server>, tick: u64) {
        if !tick.is_multiple_of(MAINTENANCE_INTERVAL_TICKS) {
            return;
        }
        if self.auction_maintenance_queued() {
            return;
        }
        let now = unix_now();
        let online = server
            .get_players()
            .iter()
            .map(|player| player.gameprofile.id())
            .collect::<FxHashSet<_>>();
        let (expiring, owed) = {
            let auction = self.auction.lock();
            (
                auction
                    .ledger
                    .lots()
                    .iter()
                    .any(|lot| lot.expires_at <= now),
                auction
                    .ledger
                    .delivery_owners()
                    .filter(|hunter| online.contains(hunter))
                    .collect::<Vec<_>>(),
            )
        };
        if expiring {
            self.request_auction(AuctionRequest::Expire);
        }
        for hunter in owed {
            self.request_auction_delivery(hunter);
        }
    }

    /// Runs one queued auction operation, returning whether it started a commit.
    pub(super) fn begin_auction(
        self: &Arc<Self>,
        server: &Arc<Server>,
        request: AuctionRequest,
    ) -> bool {
        match request {
            AuctionRequest::List {
                player,
                kind,
                price,
                hours,
            } => {
                let Some(player) = player.upgrade() else {
                    return false;
                };
                self.list_lot(server, &player, kind, price, hours)
            }
            AuctionRequest::Buy { player, lot } => {
                let Some(player) = player.upgrade() else {
                    return false;
                };
                self.buy_lot(server, &player, lot)
            }
            AuctionRequest::Bid {
                player,
                lot,
                amount,
            } => {
                let Some(player) = player.upgrade() else {
                    return false;
                };
                self.bid_lot(server, &player, lot, amount)
            }
            AuctionRequest::Cancel { player, lot } => {
                let Some(player) = player.upgrade() else {
                    return false;
                };
                self.cancel_lot(server, &player, lot)
            }
            AuctionRequest::Expire => self.expire_lots(server),
        }
    }

    /// Hands a hunter everything the ledger owes them.
    pub(super) fn begin_auction_delivery(
        self: &Arc<Self>,
        server: &Arc<Server>,
        hunter: Uuid,
    ) -> bool {
        let Some(player) = server
            .get_players()
            .into_iter()
            .find(|player| player.gameprofile.id() == hunter)
        else {
            // An offline hunter keeps the delivery in the ledger.
            return false;
        };
        let plan = {
            let auction = self.auction.lock();
            let mut ledger = auction.ledger.clone();
            ledger
                .take_delivery(hunter)
                .map(|delivery| (ledger, delivery, auction.expected.clone()))
        };
        let Some((ledger, delivery, expected)) = plan else {
            return false;
        };

        let (snapshots, updates) = if delivery.won == 0 {
            (Vec::new(), Vec::new())
        } else {
            let Some(mut snapshot) = self
                .sessions
                .with_mut(hunter, |session| session.saved_data(hunter))
            else {
                return false;
            };
            if !snapshot.won.try_credit(delivery.won) {
                log::error!("sololeveling: auction delivery overflows account {hunter}");
                return false;
            }
            let balance = snapshot.won;
            (
                vec![snapshot],
                vec![AccountUpdate::Won {
                    uuid: hunter,
                    balance,
                }],
            )
        };

        let mut items = Vec::with_capacity(delivery.items.len());
        for bytes in &delivery.items {
            match nbt::decode(bytes) {
                Ok(stack) => items.push(stack),
                Err(error) => log::error!(
                    "sololeveling: dropping an unreadable auction stack of {hunter}: {error}"
                ),
            }
        }
        let won = delivery.won;
        self.commit_auction(
            server,
            ledger,
            expected,
            snapshots,
            updates,
            move |_, report| {
                if !report.committed {
                    return;
                }
                for stack in items {
                    player.add_item_or_drop(stack, Prediction::ServerOnly);
                }
                if won > 0 {
                    announce(&player, format!("Аукцион: получено {won} Won."));
                }
            },
        )
    }

    /// Escrows the main-hand stack and lists it.
    fn list_lot(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: &Arc<Player>,
        kind: LotKind,
        price: u64,
        hours: u64,
    ) -> bool {
        let Some(held) = main_hand_stack(player) else {
            refuse(player, "Возьмите предмет в руку.");
            return false;
        };
        if binding::bound_owner(&held).is_some() {
            refuse(player, "Личный предмет нельзя выставить на аукцион.");
            return false;
        }
        let Ok(bytes) = nbt::encode(&held) else {
            refuse(player, "Этот предмет нельзя выставить.");
            return false;
        };
        let hunter = player.gameprofile.id();
        let plan = {
            let auction = self.auction.lock();
            let mut ledger = auction.ledger.clone();
            ledger
                .list(
                    hunter,
                    player.gameprofile.name(),
                    bytes,
                    kind,
                    price,
                    hours,
                    unix_now(),
                )
                .map(|id| (ledger, id, auction.expected.clone()))
        };
        let (ledger, lot, expected) = match plan {
            Ok(plan) => plan,
            Err(error) => {
                refuse(player, refusal(error));
                return false;
            }
        };
        // A durable transaction cannot own the equipped hand, so the stack is
        // parked in storage first; the move alone never changes what is owned.
        let Ok(bytes) = serde_json::to_vec(&ledger) else {
            refuse(player, "Не удалось выставить лот.");
            return false;
        };
        let Some(slot) = park_main_hand_stack(player) else {
            refuse(player, "Освободите место в инвентаре.");
            return false;
        };
        let Ok(mut reservation) = player.reserve_stored_inventory() else {
            refuse(player, "Инвентарь занят: повторите через мгновение.");
            return false;
        };
        if reservation.set_item(slot, ItemStack::empty()).is_err() {
            refuse(player, "Не удалось изъять предмет из инвентаря.");
            return false;
        }

        let state = Arc::clone(self);
        let listed = Arc::clone(player);
        let published = bytes.clone();
        let completion = move |result: io::Result<TransactionCommit>| {
            let committed = result.is_ok();
            if committed {
                state.auction.lock().committed(ledger, published);
            } else {
                log::error!("sololeveling: auction listing was not persisted: {result:?}");
            }
            state.release_persistence_lane();
            if committed {
                announce(&listed, format!("Лот #{lot} выставлен на {hours} ч."));
            } else {
                refuse(&listed, "Лот не сохранён: предмет остался в инвентаре.");
            }
        };
        match server.submit_stored_inventory_transaction(
            vec![reservation],
            vec![ExtensionStorageWrite {
                key: document(),
                expected,
                replacement: bytes,
            }],
            completion,
        ) {
            Ok(()) => true,
            Err(error) => {
                log::error!("sololeveling: could not start auction listing: {error}");
                refuse(player, "Лот не сохранён: предмет остался в инвентаре.");
                false
            }
        }
    }

    /// Charges a buyer and sells a fixed-price lot.
    fn buy_lot(self: &Arc<Self>, server: &Arc<Server>, player: &Arc<Player>, lot: u64) -> bool {
        let hunter = player.gameprofile.id();
        let plan = {
            let auction = self.auction.lock();
            let mut ledger = auction.ledger.clone();
            ledger
                .buy(lot, hunter, unix_now())
                .map(|price| (ledger, price, auction.expected.clone()))
        };
        let (ledger, price, expected) = match plan {
            Ok(plan) => plan,
            Err(error) => {
                refuse(player, refusal(error));
                return false;
            }
        };
        let Some(plan) = self.charge(hunter, price) else {
            refuse(player, "Недостаточно Won.");
            return false;
        };
        let buyer = Arc::clone(player);
        self.commit_auction(
            server,
            ledger,
            expected,
            vec![plan.snapshot],
            vec![plan.update],
            move |_, report| {
                if report.committed {
                    announce(&buyer, format!("Лот #{lot} куплен за {price} Won."));
                } else {
                    refuse(&buyer, "Покупка не сохранилась: деньги не списаны.");
                }
            },
        )
    }

    /// Escrows a bid on a bidding lot.
    fn bid_lot(
        self: &Arc<Self>,
        server: &Arc<Server>,
        player: &Arc<Player>,
        lot: u64,
        amount: u64,
    ) -> bool {
        let hunter = player.gameprofile.id();
        let plan = {
            let auction = self.auction.lock();
            let mut ledger = auction.ledger.clone();
            ledger
                .bid(lot, hunter, amount, unix_now())
                .map(|amount| (ledger, amount, auction.expected.clone()))
        };
        let (ledger, amount, expected) = match plan {
            Ok(plan) => plan,
            Err(error) => {
                refuse(player, refusal(error));
                return false;
            }
        };
        let Some(plan) = self.charge(hunter, amount) else {
            refuse(player, "Недостаточно Won для этой ставки.");
            return false;
        };
        let bidder = Arc::clone(player);
        self.commit_auction(
            server,
            ledger,
            expected,
            vec![plan.snapshot],
            vec![plan.update],
            move |_, report| {
                if report.committed {
                    announce(
                        &bidder,
                        format!("Ставка {amount} Won на лот #{lot} принята."),
                    );
                } else {
                    refuse(&bidder, "Ставка не сохранилась: деньги не списаны.");
                }
            },
        )
    }

    /// Returns an escrowed lot to its seller.
    fn cancel_lot(self: &Arc<Self>, server: &Arc<Server>, player: &Arc<Player>, lot: u64) -> bool {
        let hunter = player.gameprofile.id();
        let plan = {
            let auction = self.auction.lock();
            let mut ledger = auction.ledger.clone();
            ledger
                .cancel(lot, hunter)
                .map(|()| (ledger, auction.expected.clone()))
        };
        let (ledger, expected) = match plan {
            Ok(plan) => plan,
            Err(error) => {
                refuse(player, refusal(error));
                return false;
            }
        };
        let seller = Arc::clone(player);
        self.commit_auction(
            server,
            ledger,
            expected,
            Vec::new(),
            Vec::new(),
            move |_, report| {
                if report.committed {
                    announce(&seller, format!("Лот #{lot} снят."));
                } else {
                    refuse(&seller, "Не удалось снять лот.");
                }
            },
        )
    }

    /// Resolves every lot whose deadline has passed.
    fn expire_lots(self: &Arc<Self>, server: &Arc<Server>) -> bool {
        let plan = {
            let auction = self.auction.lock();
            let mut ledger = auction.ledger.clone();
            let resolved = ledger.expire(unix_now());
            (resolved > 0).then(|| (ledger, auction.expected.clone()))
        };
        let Some((ledger, expected)) = plan else {
            return false;
        };
        self.commit_auction(server, ledger, expected, Vec::new(), Vec::new(), |_, _| {})
    }

    /// Captures one hunter's account with `amount` so small it is refused.
    fn charge(&self, hunter: Uuid, amount: u64) -> Option<Charge> {
        let mut snapshot = self
            .sessions
            .with_mut(hunter, |session| session.saved_data(hunter))?;
        if !snapshot.won.debit(amount) {
            return None;
        }
        let balance = snapshot.won;
        Some(Charge {
            snapshot,
            update: AccountUpdate::Won {
                uuid: hunter,
                balance,
            },
        })
    }

    /// Publishes a ledger change with the accounts it moves.
    fn commit_auction(
        self: &Arc<Self>,
        server: &Arc<Server>,
        ledger: Ledger,
        expected: Option<Vec<u8>>,
        snapshots: Vec<SoloLevelingPlayerData>,
        updates: Vec<AccountUpdate>,
        completion: impl FnOnce(&Self, CommitReport) + Send + 'static,
    ) -> bool {
        let Ok(bytes) = serde_json::to_vec(&ledger) else {
            log::error!("sololeveling: could not encode the auction ledger");
            self.release_persistence_lane();
            return false;
        };
        let prepared = if snapshots.is_empty() {
            Ok(PreparedCommit::document(
                document(),
                expected,
                bytes.clone(),
            ))
        } else {
            PreparedCommit::prepare(self, snapshots, updates)
                .map(|commit| commit.with_document(document(), expected, bytes.clone()))
        };
        self.submit_commit(server, prepared, move |state, report| {
            if report.committed {
                state.auction.lock().committed(ledger, bytes);
            }
            completion(state, report);
        });
        true
    }
}

/// An account after-image with the balance change that produced it.
struct Charge {
    snapshot: SoloLevelingPlayerData,
    update: AccountUpdate,
}

/// Returns a copy of the stack in the hunter's main hand.
pub(super) fn main_hand_stack(player: &Player) -> Option<ItemStack> {
    let inventory = player.inventory.lock();
    let held = inventory
        .get_item_in_hand(InteractionHand::MainHand)
        .clone();
    (!held.is_empty()).then_some(held)
}

/// Moves the main-hand stack into a free storage slot and returns that slot.
pub(super) fn park_main_hand_stack(player: &Player) -> Option<usize> {
    park_hand_stack(player, InteractionHand::MainHand)
}

/// Moves one equipped stack into a free storage slot.
pub(super) fn park_hand_stack(player: &Player, hand: InteractionHand) -> Option<usize> {
    let mut inventory = player.inventory.lock();
    let held = inventory.get_item_in_hand(hand).clone();
    if held.is_empty() {
        return None;
    }
    let selected = usize::from(inventory.get_selected_slot());
    let slot = (0..PlayerInventory::INVENTORY_SIZE)
        .find(|slot| *slot != selected && inventory.get_item(*slot).is_empty())?;
    inventory.set_item_in_hand(hand, ItemStack::empty());
    inventory.set_item(slot, held);
    drop(inventory);
    player.send_inventory_to_remote();
    Some(slot)
}
