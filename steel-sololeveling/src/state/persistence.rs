//! Serialized gameplay persistence, using the core transaction coordinator.
//!
//! Ordinary saves capture live state when dequeued, not when requested, so a
//! queued snapshot cannot undo a payment that completed in the meantime.

mod account;
mod buyback;
mod crafting;
mod deferred;
mod enchant;
mod gear;
mod merchant;
mod starter;
use account::load_account;
pub(crate) use account::{AccountUpdate, CommitReport, PreparedCommit};
use deferred::DeferredTransfer;
pub use starter::StarterError;
use starter::StarterTransfer;

use crate::item::serial::SerialId;
use crate::merchant::MerchantOffer;

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
use steel_utils::{Identifier, types::InteractionHand};
use text_components::TextComponent;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::auction::AuctionRequest;
use super::organization::GroupPayout;
use super::{EnchantTarget, SoloLevelingState};
use crate::{
    combat_reward::HunterReward,
    dungeon::{GateRegistrySnapshot, GateRegistryWrite},
    economy::PaymentError,
    gear::GearTransfer,
    guild::{GuildError, GuildRegistry},
    item::crystal::{self, CrystalSale, CrystalSaleError},
    item::enchantment::Enchantment,
    organization::{Contract, GroupContract, OrganizationError},
    player_data::SoloLevelingPlayerData,
    quest_book::Goal,
    rank::HunterRank,
    shop::{Offer, ShopError},
    shop_menu::ShopMenuKind,
    sound::SystemSound,
};
use steel_utils::locks::SyncMutex;

const fn guild_document_key() -> Identifier {
    Identifier::new_static("sololeveling", "guilds/registry")
}

const fn gate_registry_document_key() -> Identifier {
    Identifier::new_static("sololeveling", "gates/registry")
}

const fn island_document_key() -> Identifier {
    Identifier::new_static("sololeveling", "island")
}

const fn party_document_key() -> Identifier {
    Identifier::new_static("sololeveling", "parties")
}

/// A prepared account that is not claimed by the admission hook within this
/// many lane ticks is dropped, so abandoned joins do not leak account data.
const PREPARED_JOIN_TTL_TICKS: u64 = 20 * 60 * 5;

/// Decodes one gameplay document, restoring the previous committed backup
/// once when the current bytes no longer decode.
///
/// Returns the decoded value and, when a restore happened, the backup bytes,
/// so callers can store them as the new document concurrency token.
async fn decode_document_with_backup<T>(
    server: &Server,
    key: &Identifier,
    label: &str,
    bytes: &[u8],
    decode: impl Fn(&[u8]) -> Result<T, String>,
) -> Result<(T, Option<Vec<u8>>), String> {
    match decode(bytes) {
        Ok(value) => Ok((value, None)),
        Err(error) => {
            log::error!(
                "sololeveling: {label} document is corrupt, trying the last committed backup: {error}"
            );
            match server.player_data_storage.restore_extension_backup(key).await {
                Ok(Some(backup)) => decode(&backup)
                    .map(|value| (value, Some(backup)))
                    .map_err(|backup_error| {
                        format!(
                            "the document and its committed backup are both corrupt: {error}; backup error: {backup_error}"
                        )
                    }),
                Ok(None) => Err(format!(
                    "{error}; there is no committed backup to restore"
                )),
                Err(restore_error) => {
                    Err(format!("{error}; restoring the committed backup failed: {restore_error}"))
                }
            }
        }
    }
}

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
        shop: crate::shop::ShopDay,
        from_menu: bool,
    },
    ShopRefresh {
        uuid: Uuid,
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
    Buyback {
        uuid: Uuid,
        slot: usize,
        seen: ItemStack,
    },
    Enchant {
        uuid: Uuid,
        enchantment: Enchantment,
        target: EnchantTarget,
        completion: Arc<SyncMutex<Option<EnchantTarget>>>,
    },
    Gear {
        uuid: Uuid,
        transfer: GearTransfer,
    },
    Deferred {
        uuid: Uuid,
        transfer: DeferredTransfer,
    },
    Starter {
        uuid: Uuid,
        transfer: StarterTransfer,
    },
    Merchant {
        uuid: Uuid,
        offer: MerchantOffer,
    },
    Crafting {
        uuid: Uuid,
        output: SerialId,
    },
    GateRegistry(GateRegistryWrite),
    Auction(AuctionRequest),
    AuctionDeliver {
        hunter: Uuid,
    },
    GuildSnapshot {
        bytes: Vec<u8>,
        notify: Vec<Uuid>,
    },
    GuildTurnIn {
        actor: Uuid,
        index: usize,
    },
    GuildDeposit {
        player: Uuid,
        amount: u64,
    },
    GuildCollect {
        player: Uuid,
    },
    IslandSnapshot {
        bytes: Vec<u8>,
    },
    PartySnapshot {
        bytes: Vec<u8>,
    },
}

#[derive(Debug, Default)]
pub(super) struct Persistence {
    requests: VecDeque<Request>,
    saves: FxHashSet<Uuid>,
    expected: FxHashMap<Uuid, Option<Vec<u8>>>,
    prepared: FxHashMap<Uuid, (SoloLevelingPlayerData, u64)>,
    /// Lane tick counter driving the prepared-account expiry.
    tick: u64,
    failed: FxHashSet<Uuid>,
    document_expected: FxHashMap<Identifier, Option<Vec<u8>>>,
    guild_document_failed: bool,
    guild_transaction_pending: bool,
    gate_document_failed: bool,
    island_document_failed: bool,
    party_document_failed: bool,
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

    /// Drops prepared accounts whose owner never finished joining.
    fn evict_stale_prepared(&mut self) {
        self.prepared
            .retain(|_, (_, prepared_at)| self.tick - *prepared_at <= PREPARED_JOIN_TTL_TICKS);
    }
}

impl SoloLevelingState {
    /// Refuses guild mutations while a multi-account guild transaction is active.
    pub(super) fn ensure_guild_mutation_available(&self) -> Result<(), GuildError> {
        let persistence = self.persistence.lock();
        if persistence.guild_transaction_pending {
            return Err(GuildError::TransactionPending);
        }
        if persistence.guild_document_failed || persistence.quiescing {
            return Err(GuildError::Storage);
        }
        Ok(())
    }

    /// Loads the canonical guild registry from core gameplay-document storage.
    pub async fn load_guilds(&self, server: &Server) -> Result<(), String> {
        let key = guild_document_key();
        let bytes = server
            .player_data_storage
            .load_extension(&key)
            .await
            .map_err(|error| format!("failed to load guild registry: {error}"))?;
        let import_legacy = bytes.is_none()
            && server
                .player_data_storage
                .allows_legacy_player_data_import();
        let (registry, restored) = match bytes.as_deref() {
            Some(document) => {
                decode_document_with_backup(server, &key, "guild registry", document, |bytes| {
                    GuildRegistry::from_document(bytes).map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| format!("failed to decode guild registry: {error}"))?
            }
            None if import_legacy => (
                GuildRegistry::load_legacy(self.storage.base_dir())
                    .map_err(|error| format!("failed to decode guild registry: {error}"))?,
                None,
            ),
            None => (GuildRegistry::empty(), None),
        };
        *self.guilds.lock() = registry;
        self.persistence
            .lock()
            .document_expected
            .insert(key, restored.or(bytes));
        if import_legacy {
            self.queue_guild_save(None)
                .map_err(|error| format!("failed to queue guild import: {error}"))?;
        }
        Ok(())
    }

    /// Loads the durable active-gate registry before the first gameplay tick.
    pub async fn load_gate_registry(&self, server: &Server) -> Result<(), String> {
        let key = gate_registry_document_key();
        let bytes = server
            .player_data_storage
            .load_extension(&key)
            .await
            .map_err(|error| format!("failed to load gate registry: {error}"))?;
        let (snapshot, restored) = match bytes.as_deref() {
            Some(document) => {
                decode_document_with_backup(server, &key, "gate registry", document, |bytes| {
                    serde_json::from_slice::<GateRegistrySnapshot>(bytes)
                        .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| format!("failed to decode gate registry: {error}"))?
            }
            None => (GateRegistrySnapshot::default(), None),
        };
        self.dungeon_runtime
            .lock()
            .restore_gate_registry(snapshot)
            .map_err(|error| format!("failed to restore gate registry: {error}"))?;
        self.persistence
            .lock()
            .document_expected
            .insert(key, restored.or(bytes));
        Ok(())
    }

    /// Loads the shared island infestation before the first gameplay tick.
    pub async fn load_island(&self, server: &Server) -> Result<(), String> {
        let key = island_document_key();
        let bytes = server
            .player_data_storage
            .load_extension(&key)
            .await
            .map_err(|error| format!("failed to load island state: {error}"))?;
        let restored = match bytes.as_deref() {
            Some(document) => {
                let (snapshot, restored) =
                    decode_document_with_backup(server, &key, "island state", document, |bytes| {
                        serde_json::from_slice::<crate::island::IslandSnapshot>(bytes)
                            .map_err(|error| error.to_string())
                    })
                    .await
                    .map_err(|error| format!("failed to decode island state: {error}"))?;
                self.island.lock().restore(snapshot);
                restored
            }
            None => None,
        };
        self.persistence
            .lock()
            .document_expected
            .insert(key, restored.or(bytes));
        Ok(())
    }

    /// Loads active parties and their group contracts before the first gameplay tick.
    pub async fn load_parties(&self, server: &Server) -> Result<(), String> {
        let key = party_document_key();
        let bytes = server
            .player_data_storage
            .load_extension(&key)
            .await
            .map_err(|error| format!("failed to load party state: {error}"))?;
        let restored = match bytes.as_deref() {
            Some(document) => {
                let (snapshot, restored) =
                    decode_document_with_backup(server, &key, "party state", document, |bytes| {
                        serde_json::from_slice::<crate::party::PartyRegistrySnapshot>(bytes)
                            .map_err(|error| error.to_string())
                    })
                    .await
                    .map_err(|error| format!("failed to decode party state: {error}"))?;
                self.party_runtime.lock().restore(snapshot);
                restored
            }
            None => None,
        };
        self.persistence
            .lock()
            .document_expected
            .insert(key, restored.or(bytes));
        Ok(())
    }

    /// Queues a changed gate registry without waiting on disk during a game tick.
    pub(crate) fn queue_gate_registry_snapshot(
        &self,
        snapshot: GateRegistryWrite,
    ) -> io::Result<()> {
        let mut persistence = self.persistence.lock();
        if persistence.quiescing || persistence.gate_document_failed {
            return Err(io::Error::other("gate registry storage is unavailable"));
        }
        persistence
            .requests
            .push_back(Request::GateRegistry(snapshot));
        Ok(())
    }

    fn restore_guild_registry(&self, bytes: Option<&[u8]>) {
        let registry =
            bytes.map_or_else(|| Ok(GuildRegistry::empty()), GuildRegistry::from_document);
        match registry {
            Ok(registry) => *self.guilds.lock() = registry,
            Err(error) => {
                log::error!(
                    "sololeveling: failed to restore last committed guild registry: {error}"
                );
            }
        }
    }

    /// Queues the current guild snapshot for durable publication.
    pub(super) fn queue_guild_save(&self, notify: Option<Uuid>) -> io::Result<()> {
        let bytes = self.guilds.lock().to_document()?;
        let mut persistence = self.persistence.lock();
        if persistence.guild_transaction_pending
            || persistence.guild_document_failed
            || persistence.quiescing
        {
            return Err(io::Error::other("guild document storage is unavailable"));
        }
        if let Some(Request::GuildSnapshot {
            bytes: pending_bytes,
            notify: pending_players,
        }) = persistence
            .requests
            .iter_mut()
            .find(|request| matches!(request, Request::GuildSnapshot { .. }))
        {
            *pending_bytes = bytes;
            if let Some(player) = notify
                && !pending_players.contains(&player)
            {
                pending_players.push(player);
            }
        } else {
            persistence.requests.push_back(Request::GuildSnapshot {
                bytes,
                notify: notify.into_iter().collect(),
            });
        }
        Ok(())
    }

    /// Queues the durable settlement of a finished guild contract.
    pub(super) fn queue_guild_turn_in(&self, actor: Uuid, index: usize) -> io::Result<()> {
        if !self.active_guild_banks.lock().is_empty() {
            return Err(io::Error::other("guild bank is open"));
        }
        let mut persistence = self.persistence.lock();
        if persistence.guild_transaction_pending
            || persistence.guild_document_failed
            || persistence.quiescing
        {
            return Err(io::Error::other("guild storage is busy or unavailable"));
        }
        persistence.guild_transaction_pending = true;
        persistence
            .requests
            .push_back(Request::GuildTurnIn { actor, index });
        Ok(())
    }

    /// Queues a guild treasury deposit as a balance-and-document transaction.
    pub(super) fn queue_guild_deposit(&self, player: Uuid, amount: u64) -> io::Result<()> {
        if !self.active_guild_banks.lock().is_empty() {
            return Err(io::Error::other("guild bank is open"));
        }
        let mut persistence = self.persistence.lock();
        if persistence.guild_transaction_pending
            || persistence.guild_document_failed
            || persistence.quiescing
        {
            return Err(io::Error::other("guild storage is busy or unavailable"));
        }
        persistence.guild_transaction_pending = true;
        persistence
            .requests
            .push_back(Request::GuildDeposit { player, amount });
        Ok(())
    }

    /// Queues collection of offline guild earnings with their account credit.
    pub(super) fn queue_guild_collection(&self, player: Uuid) -> io::Result<()> {
        let mut persistence = self.persistence.lock();
        if persistence.guild_transaction_pending
            || persistence.guild_document_failed
            || persistence.quiescing
        {
            return Err(io::Error::other("guild storage is busy or unavailable"));
        }
        persistence.guild_transaction_pending = true;
        persistence
            .requests
            .push_back(Request::GuildCollect { player });
        Ok(())
    }

    /// Queues the current island infestation snapshot for durable publication.
    pub(crate) fn queue_island_save(&self) -> io::Result<()> {
        let bytes = serde_json::to_vec(&self.island.lock().snapshot())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut persistence = self.persistence.lock();
        if persistence.island_document_failed || persistence.quiescing {
            return Err(io::Error::other("island document storage is unavailable"));
        }
        if let Some(Request::IslandSnapshot { bytes: pending }) = persistence
            .requests
            .iter_mut()
            .find(|request| matches!(request, Request::IslandSnapshot { .. }))
        {
            *pending = bytes;
        } else {
            persistence
                .requests
                .push_back(Request::IslandSnapshot { bytes });
        }
        Ok(())
    }

    /// Queues the current party registry snapshot for durable publication.
    pub(crate) fn queue_party_save(&self) -> io::Result<()> {
        let bytes = serde_json::to_vec(&self.party_runtime.lock().snapshot())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut persistence = self.persistence.lock();
        if persistence.party_document_failed || persistence.quiescing {
            return Err(io::Error::other("party document storage is unavailable"));
        }
        if let Some(Request::PartySnapshot { bytes: pending }) = persistence
            .requests
            .iter_mut()
            .find(|request| matches!(request, Request::PartySnapshot { .. }))
        {
            *pending = bytes;
        } else {
            persistence
                .requests
                .push_back(Request::PartySnapshot { bytes });
        }
        Ok(())
    }

    fn release_shop_purchase(&self, uuid: Uuid) {
        self.persistence.lock().busy = false;
        self.sessions
            .with_mut(uuid, |session| session.economic_busy = false);
    }

    fn begin_shop_refresh(self: &Arc<Self>, server: &Arc<Server>, uuid: Uuid) {
        let Some(mut snapshot) = self
            .sessions
            .with_mut(uuid, |session| session.saved_data(uuid))
        else {
            self.release_shop_purchase(uuid);
            return;
        };
        snapshot.shop.refresh(crate::shop::current_window());
        let shop = snapshot.shop;
        let prepared = PreparedCommit::prepare(
            self,
            vec![snapshot],
            vec![AccountUpdate::Shop { uuid, shop }],
        );
        let server_task = Arc::clone(server);
        self.submit_commit(server, prepared, move |state, report| {
            state
                .sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            if report.committed {
                state.queue_account_followup_save(uuid);
            }
            let Some(player) = server_task
                .get_players()
                .into_iter()
                .find(|player| player.gameprofile.id() == uuid)
            else {
                return;
            };
            let message = if report.confirmed {
                "Ассортимент Магазина Системы обновлён оператором."
            } else if report.committed {
                "Ассортимент обновлён, но подтверждение диска не получено."
            } else {
                "Не удалось сохранить новый ассортимент магазина."
            };
            player.send_message(&TextComponent::plain(message));
        });
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
        shop: crate::shop::ShopDay,
        from_menu: bool,
    ) -> bool {
        if player.connection.closed() {
            self.release_shop_purchase(uuid);
            return false;
        }
        let reservation = if from_menu {
            player.reserve_stored_inventory_with_menu::<ShopMenuKind>()
        } else {
            player.reserve_stored_inventory()
        };
        let Ok(mut reservation) = reservation else {
            self.release_shop_purchase(uuid);
            player.send_message(&TextComponent::plain(
                "Покупку не удалось начать: инвентарь занят или предмет находится на курсоре или в сетке крафта.",
            ));
            return false;
        };
        let snapshot = self.sessions.with_mut(uuid, |session| {
            let mut snapshot = session.saved_data(uuid);
            if !snapshot
                .shop
                .permits_purchase(shop, crate::shop::current_window(), slot, offer)
            {
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
                    SystemSound::Reward.play(&completion_player);
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
                    state.record_goal(uuid, Goal::CrystalSale, 1);
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
        let state_server = Arc::clone(server);
        self.submit_commit(
            server,
            PreparedCommit::prepare(
                self,
                vec![snapshot],
                vec![
                    AccountUpdate::Won { uuid, balance },
                    AccountUpdate::Organization {
                        uuid,
                        index,
                        contract,
                    },
                ],
            ),
            move |state, report| {
                if !report.committed {
                    state
                        .sessions
                        .with_mut(uuid, |session| session.economic_busy = false);
                } else if let Err(error) = state.save_session(uuid) {
                    log::warn!("sololeveling: failed to persist concurrent contract changes for {uuid}: {error}");
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
                    let mut removed = false;
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
                            removed = true;
                            break;
                        }
                    }
                    if removed {
                        if let Err(error) = state.queue_party_save() {
                            log::warn!(
                                "sololeveling: could not queue party save after group turn-in: {error}"
                            );
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
        let mut persistence = self.persistence.lock();
        let tick = persistence.tick;
        persistence.prepared.insert(uuid, (data, tick));
        Ok(())
    }

    /// Transfers prepared data to the player's tick-owned admission hook.
    pub fn take_prepared_player(&self, uuid: Uuid) -> Option<SoloLevelingPlayerData> {
        self.persistence
            .lock()
            .prepared
            .remove(&uuid)
            .map(|(data, _)| data)
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
    pub fn request_shop_purchase(
        &self,
        player: &Player,
        slot: usize,
        from_menu: bool,
        displayed: Option<crate::shop::ShopDay>,
    ) -> Result<Offer, ShopError> {
        let uuid = player.gameprofile.id();
        self.shop_day(uuid).ok_or(ShopError::NoSession)?;
        let (offer, shop) = self
            .sessions
            .with_mut(uuid, |session| {
                if session.economic_busy {
                    return Err(ShopError::Busy);
                }
                if session.shop.day != crate::shop::current_window()
                    || displayed.is_some_and(|view| view != session.shop)
                {
                    return Err(ShopError::Stale);
                }
                let offer = session.shop.offer(slot).ok_or(ShopError::UnknownSlot)?;
                if session.shop.bought(slot) {
                    return Err(ShopError::AlreadyBought);
                }
                if session.system_points.amount() < offer.price {
                    return Err(ShopError::InsufficientPoints);
                }
                session.economic_busy = true;
                Ok((offer, session.shop))
            })
            .ok_or(ShopError::NoSession)??;

        let mut persistence = self.persistence.lock();
        if persistence.quiescing || persistence.failed.contains(&uuid) {
            drop(persistence);
            self.sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            return Err(ShopError::Storage);
        }
        persistence.requests.push_back(Request::ShopPurchase {
            uuid,
            slot,
            offer,
            shop,
            from_menu,
        });
        Ok(offer)
    }

    /// Queues an operator's per-hunter shop assortment refresh.
    pub fn request_shop_refresh(&self, uuid: Uuid) -> Result<(), ShopError> {
        if !self.persistence.lock().accepts_writes(uuid) {
            return Err(ShopError::Storage);
        }
        let Some(accepted) = self.sessions.with_mut(uuid, |session| {
            if session.economic_busy {
                return false;
            }
            session.economic_busy = true;
            true
        }) else {
            return Err(ShopError::NoSession);
        };
        if !accepted {
            return Err(ShopError::Busy);
        }
        let mut persistence = self.persistence.lock();
        if persistence.quiescing || persistence.failed.contains(&uuid) {
            drop(persistence);
            self.sessions
                .with_mut(uuid, |session| session.economic_busy = false);
            return Err(ShopError::Storage);
        }
        persistence
            .requests
            .push_back(Request::ShopRefresh { uuid });
        Ok(())
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

    /// Queues experience and system points for one hunter.
    pub(super) fn award(&self, uuid: Uuid, reward: HunterReward) {
        self.persistence.lock().requests.push_back(Request::Award {
            rewards: vec![(uuid, reward)],
        });
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
            rewards: vec![(
                uuid,
                self.enchanted_reward(uuid, HunterReward::for_mob(rank)),
            )],
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
        let rewards = survivors
            .into_iter()
            .zip(rewards)
            .map(|(uuid, reward)| (uuid, self.enchanted_reward(uuid, reward)))
            .collect();
        self.persistence
            .lock()
            .requests
            .push_back(Request::Award { rewards });
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
            persistence.tick = persistence.tick.wrapping_add(1);
            persistence.evict_stale_prepared();
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
            Request::ShopPurchase {
                uuid,
                slot,
                offer,
                shop,
                from_menu,
            } => {
                let Some(player) = server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                else {
                    self.release_shop_purchase(uuid);
                    return;
                };
                if !self.begin_shop_purchase(server, player, uuid, slot, offer, shop, from_menu) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::ShopRefresh { uuid } => {
                self.begin_shop_refresh(server, uuid);
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
            Request::Buyback { uuid, slot, seen } => {
                let Some(player) = server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                else {
                    self.release_shop_purchase(uuid);
                    return;
                };
                if !self.begin_buyback(server, player, uuid, slot, &seen) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::Enchant {
                uuid,
                enchantment,
                target,
                completion,
            } => {
                let Some(player) = server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                else {
                    self.release_shop_purchase(uuid);
                    return;
                };
                if !self.begin_enchantment(server, player, uuid, enchantment, target, completion) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::Gear { uuid, transfer } => {
                let Some(player) = server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                else {
                    self.release_shop_purchase(uuid);
                    return;
                };
                if !self.begin_gear_transfer(server, player, uuid, transfer) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::Deferred { uuid, transfer } => {
                let Some(player) = server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                else {
                    let DeferredTransfer::Deliver(stack) = transfer else {
                        self.release_shop_purchase(uuid);
                        return;
                    };
                    if !self.begin_offline_deferred_delivery(server, uuid, stack) {
                        self.persistence.lock().busy = false;
                    }
                    return;
                };
                if !self.begin_deferred_transfer(server, player, uuid, transfer) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::Starter { uuid, transfer } => {
                let Some(player) = server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                else {
                    self.release_shop_purchase(uuid);
                    return;
                };
                if !self.begin_starter_transfer(server, player, uuid, transfer) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::Merchant { uuid, offer } => {
                let Some(player) = server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                else {
                    self.release_shop_purchase(uuid);
                    return;
                };
                if !self.begin_merchant_purchase(server, player, uuid, offer) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::Crafting { uuid, output } => {
                let Some(player) = server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == uuid)
                else {
                    self.release_shop_purchase(uuid);
                    return;
                };
                if !self.begin_crafting(server, player, uuid, output) {
                    self.persistence.lock().busy = false;
                }
            }
            Request::GateRegistry(write) => {
                let replacement = match serde_json::to_vec(&write.snapshot) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        log::error!("sololeveling: could not encode gate registry: {error}");
                        self.persistence.lock().gate_document_failed = true;
                        self.persistence.lock().busy = false;
                        self.dungeon_runtime
                            .lock()
                            .fail_gate_persistence(write.revision);
                        return;
                    }
                };
                let expected = self
                    .persistence
                    .lock()
                    .document_expected
                    .get(&gate_registry_document_key())
                    .cloned()
                    .flatten();
                let prepared =
                    PreparedCommit::document(gate_registry_document_key(), expected, replacement);
                let completion_server = Arc::clone(server);
                self.submit_commit(server, Ok(prepared), move |state, report| {
                    if report.committed {
                        state
                            .dungeon_runtime
                            .lock()
                            .commit_gate_persistence(&completion_server, write.revision);
                    } else {
                        log::error!(
                            "sololeveling: gate registry persistence failed; gate changes are disabled"
                        );
                        state.persistence.lock().gate_document_failed = true;
                        state
                            .dungeon_runtime
                            .lock()
                            .fail_gate_persistence(write.revision);
                    }
                });
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
            Request::GuildSnapshot { bytes, notify } => {
                let key = guild_document_key();
                let expected = self
                    .persistence
                    .lock()
                    .document_expected
                    .get(&key)
                    .cloned()
                    .flatten();
                let prepared = PreparedCommit::document(key, expected, bytes.clone());
                let server_task = Arc::clone(server);
                self.submit_commit(server, Ok(prepared), move |state, report| {
                    let mut persistence = state.persistence.lock();
                    if report.committed {
                        persistence
                            .document_expected
                            .insert(guild_document_key(), Some(bytes));
                    } else {
                        persistence.guild_document_failed = true;
                        persistence
                            .requests
                            .retain(|request| !matches!(request, Request::GuildSnapshot { .. }));
                        let previous = persistence
                            .document_expected
                            .get(&guild_document_key())
                            .cloned()
                            .flatten();
                        drop(persistence);
                        state.restore_guild_registry(previous.as_deref());
                        log::error!(
                            "sololeveling: guild registry write failed; further guild writes are disabled"
                        );
                    }
                    for player in server_task.get_players() {
                        if notify.contains(&player.gameprofile.id()) {
                            let message = if report.confirmed {
                                "Изменение гильдии сохранено."
                            } else if report.committed {
                                "Изменение гильдии записано, но подтверждение диска не получено."
                            } else {
                                "Не удалось сохранить изменение гильдии; восстановлены последние сохранённые данные."
                            };
                            player.send_message(&TextComponent::plain(message));
                        }
                    }
                });
            }
            Request::GuildTurnIn { actor, index } => {
                if self.persistence.lock().guild_document_failed {
                    self.finish_guild_turn_in_failure(
                        server,
                        actor,
                        index,
                        "Хранилище гильдий недоступно; сдача отменена.",
                    );
                } else {
                    self.begin_guild_contract_turn_in(server, actor, index);
                }
            }
            Request::GuildDeposit { player, amount } => {
                if self.persistence.lock().guild_document_failed {
                    self.finish_guild_deposit_failure(
                        server,
                        player,
                        "Хранилище гильдий недоступно; взнос не проведён.",
                    );
                } else {
                    self.begin_guild_deposit(server, player, amount);
                }
            }
            Request::GuildCollect { player } => {
                if self.persistence.lock().guild_document_failed {
                    self.finish_guild_deposit_failure(
                        server,
                        player,
                        "Хранилище гильдий недоступно; выплата осталась в гильдии.",
                    );
                } else {
                    self.begin_guild_collection(server, player);
                }
            }
            Request::IslandSnapshot { bytes } => {
                let key = island_document_key();
                let expected = self
                    .persistence
                    .lock()
                    .document_expected
                    .get(&key)
                    .cloned()
                    .flatten();
                let prepared = PreparedCommit::document(key, expected, bytes.clone());
                self.submit_commit(server, Ok(prepared), move |state, report| {
                    let mut persistence = state.persistence.lock();
                    if report.committed {
                        persistence
                            .document_expected
                            .insert(island_document_key(), Some(bytes));
                    } else {
                        persistence.island_document_failed = true;
                        persistence.requests.retain(|request| {
                            !matches!(request, Request::IslandSnapshot { .. })
                        });
                        log::error!(
                            "sololeveling: island state write failed; further island writes are disabled"
                        );
                    }
                });
            }
            Request::PartySnapshot { bytes } => {
                let key = party_document_key();
                let expected = self
                    .persistence
                    .lock()
                    .document_expected
                    .get(&key)
                    .cloned()
                    .flatten();
                let prepared = PreparedCommit::document(key, expected, bytes.clone());
                self.submit_commit(server, Ok(prepared), move |state, report| {
                    let mut persistence = state.persistence.lock();
                    if report.committed {
                        persistence
                            .document_expected
                            .insert(party_document_key(), Some(bytes));
                    } else {
                        persistence.party_document_failed = true;
                        persistence.requests.retain(|request| {
                            !matches!(request, Request::PartySnapshot { .. })
                        });
                        log::error!(
                            "sololeveling: party state write failed; further party writes are disabled"
                        );
                    }
                });
            }
        }
    }

    fn begin_guild_deposit(self: &Arc<Self>, server: &Arc<Server>, player: Uuid, amount: u64) {
        let mut candidate = self.guilds.lock().clone();
        if let Err(error) = candidate.deposit_treasury(player, amount) {
            self.finish_guild_deposit_failure(server, player, error.message());
            return;
        }
        let Some(mut snapshot) = self
            .sessions
            .with_mut(player, |session| session.saved_data(player))
        else {
            self.finish_guild_deposit_failure(server, player, "Данные охотника не загружены.");
            return;
        };
        if !snapshot.won.debit(amount) {
            self.finish_guild_deposit_failure(server, player, "Недостаточно Won.");
            return;
        }
        let balance = snapshot.won;
        let replacement = match candidate.to_document() {
            Ok(bytes) => bytes,
            Err(error) => {
                self.finish_guild_deposit_failure(server, player, "Не удалось подготовить данные.");
                log::error!("sololeveling: could not encode guild deposit: {error}");
                return;
            }
        };
        let key = guild_document_key();
        let expected = self
            .persistence
            .lock()
            .document_expected
            .get(&key)
            .cloned()
            .flatten();
        let prepared = PreparedCommit::prepare(
            self,
            vec![snapshot],
            vec![AccountUpdate::Won {
                uuid: player,
                balance,
            }],
        )
        .map(|prepared| {
            prepared
                .with_document(key, expected, replacement)
                .with_guild_registry(candidate)
        });
        let state_server = Arc::clone(server);
        self.submit_commit(server, prepared, move |state, report| {
            let mut persistence = state.persistence.lock();
            persistence.guild_transaction_pending = false;
            if !report.committed {
                persistence.guild_document_failed = true;
            }
            drop(persistence);
            state
                .sessions
                .with_mut(player, |session| session.economic_busy = false);
            if report.committed {
                state.queue_account_followup_save(player);
            }
            if let Some(player) = state_server
                .get_players()
                .into_iter()
                .find(|online| online.gameprofile.id() == player)
            {
                let message = if report.confirmed {
                    format!("В казну гильдии внесено {amount} Won.")
                } else if report.committed {
                    "Взнос записан, но подтверждение диска не получено; повторять операцию не нужно."
                        .to_owned()
                } else {
                    "Взнос не сохранён; баланс не списан.".to_owned()
                };
                player.send_message(&TextComponent::plain(message));
            }
        });
    }

    fn finish_guild_deposit_failure(&self, server: &Server, player_uuid: Uuid, reason: &str) {
        self.sessions
            .with_mut(player_uuid, |session| session.economic_busy = false);
        let mut persistence = self.persistence.lock();
        persistence.guild_transaction_pending = false;
        persistence.busy = false;
        drop(persistence);
        if let Some(player) = server
            .get_players()
            .into_iter()
            .find(|player| player.gameprofile.id() == player_uuid)
        {
            player.send_message(&TextComponent::plain(reason.to_owned()));
        }
    }

    fn queue_account_followup_save(&self, player: Uuid) {
        if let Err(error) = self.save_session(player) {
            log::error!(
                "sololeveling: could not queue post-transaction account save for {player}: {error}"
            );
        }
    }

    fn begin_guild_collection(self: &Arc<Self>, server: &Arc<Server>, player_uuid: Uuid) {
        let mut candidate = self.guilds.lock().clone();
        let amount = candidate
            .guild_of_mut(player_uuid)
            .map_or(0, |guild| guild.organization.take_owed(player_uuid));
        if amount == 0 {
            self.finish_guild_deposit_failure(server, player_uuid, "Невостребованных выплат нет.");
            return;
        }
        let Some(mut snapshot) = self
            .sessions
            .with_mut(player_uuid, |session| session.saved_data(player_uuid))
        else {
            self.finish_guild_deposit_failure(server, player_uuid, "Данные охотника не загружены.");
            return;
        };
        if !snapshot.won.try_credit(amount) {
            self.finish_guild_deposit_failure(
                server,
                player_uuid,
                "Баланс Won переполнен; выплата оставлена в гильдии.",
            );
            return;
        }
        let balance = snapshot.won;
        let replacement = match candidate.to_document() {
            Ok(bytes) => bytes,
            Err(error) => {
                self.finish_guild_deposit_failure(
                    server,
                    player_uuid,
                    "Не удалось подготовить данные выплаты.",
                );
                log::error!("sololeveling: could not encode guild collection: {error}");
                return;
            }
        };
        let key = guild_document_key();
        let expected = self
            .persistence
            .lock()
            .document_expected
            .get(&key)
            .cloned()
            .flatten();
        let prepared = PreparedCommit::prepare(
            self,
            vec![snapshot],
            vec![AccountUpdate::Won {
                uuid: player_uuid,
                balance,
            }],
        )
        .map(|prepared| {
            prepared
                .with_document(key, expected, replacement)
                .with_guild_registry(candidate)
        });
        let server_task = Arc::clone(server);
        self.submit_commit(server, prepared, move |state, report| {
            let mut persistence = state.persistence.lock();
            persistence.guild_transaction_pending = false;
            if !report.committed {
                persistence.guild_document_failed = true;
            }
            drop(persistence);
            state
                .sessions
                .with_mut(player_uuid, |session| session.economic_busy = false);
            if report.committed {
                state.queue_account_followup_save(player_uuid);
            }
            if let Some(player) = server_task
                .get_players()
                .into_iter()
                .find(|player| player.gameprofile.id() == player_uuid)
            {
                let message = if report.confirmed {
                    format!("Гильдия выплатила вам {amount} Won.")
                } else if report.committed {
                    "Выплата записана, но подтверждение диска не получено; не повторяйте операцию."
                        .to_owned()
                } else {
                    "Не удалось сохранить выплату; она остаётся в гильдии.".to_owned()
                };
                player.send_message(&TextComponent::plain(message));
            }
        });
    }

    fn begin_guild_contract_turn_in(
        self: &Arc<Self>,
        server: &Arc<Server>,
        actor: Uuid,
        index: usize,
    ) {
        let (prepared, contract, payment, credited) =
            match self.prepare_guild_contract_turn_in(actor, index) {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.finish_guild_turn_in_failure(
                        server,
                        actor,
                        index,
                        "Не удалось подготовить выплату гильдии.",
                    );
                    log::error!("sololeveling: could not prepare guild settlement: {error}");
                    return;
                }
            };
        let state_server = Arc::clone(server);
        self.submit_commit(server, Ok(prepared), move |state, report| {
            let mut persistence = state.persistence.lock();
            persistence.guild_transaction_pending = false;
            if !report.committed {
                persistence.guild_document_failed = true;
            }
            drop(persistence);
            if !report.committed {
                state.clear_guild_turn_in_pending(actor, index);
                let message =
                    "Не удалось сохранить сдачу задания. Выплаты не произведены.".to_owned();
                if let Some(player) = state_server
                    .get_players()
                    .into_iter()
                    .find(|player| player.gameprofile.id() == actor)
                {
                    player.send_message(&TextComponent::plain(message));
                }
                return;
            }
            for hunter in &credited {
                state.queue_account_followup_save(*hunter);
            }
            for player in state_server.get_players() {
                let uuid = player.gameprofile.id();
                if uuid == actor && report.confirmed {
                    player.send_message(&TextComponent::plain(format!(
                        "Задание гильдии «{}» сдано; участникам начислено по {payment} Won.",
                        contract.template.title()
                    )));
                } else if uuid == actor {
                    player.send_message(&TextComponent::plain(
                        "Сдача зафиксирована, но подтверждение диска не получено; не повторяйте её.",
                    ));
                } else if credited.contains(&uuid) {
                    let message = if report.confirmed {
                        format!("Гильдия выплатила вам {payment} Won.")
                    } else {
                        "Выплата зафиксирована, но подтверждение диска не получено; не повторяйте операцию."
                            .to_owned()
                    };
                    player.send_message(&TextComponent::plain(message));
                }
            }
        });
    }

    fn prepare_guild_contract_turn_in(
        &self,
        actor: Uuid,
        index: usize,
    ) -> io::Result<(PreparedCommit, Contract, u64, Vec<Uuid>)> {
        let mut candidate = self.guilds.lock().clone();
        let (contract, paid) = candidate
            .settle_contract(actor, index)
            .map_err(|error| io::Error::other(error.message()))?;
        let payment = contract.payment();
        let mut snapshots = Vec::new();
        let mut updates = Vec::new();
        let mut credited = Vec::new();
        for hunter in paid {
            let Some(guild) = candidate.guild_of_mut(hunter) else {
                continue;
            };
            let amount = guild.organization.take_owed(hunter);
            if amount == 0 {
                continue;
            }
            let snapshot = self.sessions.with_mut(hunter, |session| {
                if session.economic_busy {
                    return None;
                }
                let mut snapshot = session.saved_data(hunter);
                snapshot.won.try_credit(amount).then_some(snapshot)
            });
            match snapshot.flatten() {
                Some(snapshot) => {
                    credited.push(hunter);
                    updates.push(AccountUpdate::Won {
                        uuid: hunter,
                        balance: snapshot.won,
                    });
                    snapshots.push(snapshot);
                }
                None => {
                    if let Some(guild) = candidate.guild_of_mut(hunter) {
                        guild.organization.owed.insert(hunter, amount);
                    }
                }
            }
        }
        let replacement = candidate.to_document()?;
        let key = guild_document_key();
        let expected = self
            .persistence
            .lock()
            .document_expected
            .get(&key)
            .cloned()
            .flatten();
        let prepared = if snapshots.is_empty() {
            PreparedCommit::document(key, expected, replacement)
        } else {
            PreparedCommit::prepare(self, snapshots, updates)?.with_document(
                key,
                expected,
                replacement,
            )
        }
        .with_guild_registry(candidate);
        Ok((prepared, contract, payment, credited))
    }

    fn finish_guild_turn_in_failure(
        &self,
        server: &Server,
        actor: Uuid,
        index: usize,
        reason: &str,
    ) {
        self.clear_guild_turn_in_pending(actor, index);
        let mut persistence = self.persistence.lock();
        persistence.guild_transaction_pending = false;
        persistence.busy = false;
        log::warn!("sololeveling: guild contract settlement refused: {reason}");
        drop(persistence);
        if let Some(player) = server
            .get_players()
            .into_iter()
            .find(|player| player.gameprofile.id() == actor)
        {
            player.send_message(&TextComponent::plain(reason.to_owned()));
        }
    }

    pub(super) fn clear_guild_turn_in_pending(&self, actor: Uuid, index: usize) {
        if let Some(group) = self
            .guilds
            .lock()
            .guild_of_mut(actor)
            .and_then(|guild| guild.organization.contracts.get_mut(index))
        {
            group.pending_turn_in = false;
        }
    }

    /// Starts one transaction without awaiting it on the game tick.
    pub(super) fn submit_commit(
        self: &Arc<Self>,
        server: &Arc<Server>,
        prepared: io::Result<PreparedCommit>,
        completion: impl FnOnce(&Self, CommitReport) + Send + 'static,
    ) {
        let mut prepared = match prepared {
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
                let result = account::commit_with_conflict_retry(
                    &server_task.player_data_storage,
                    &mut prepared.transaction,
                )
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
        for uuid in self.sessions.player_ids() {
            self.save_session(uuid).map_err(|error| error.to_string())?;
        }
        // Queued before quiescing: those queues refuse new work once shutdown
        // starts, but a graceful shutdown must still write them.
        self.queue_island_save()
            .map_err(|error| error.to_string())?;
        self.queue_party_save().map_err(|error| error.to_string())?;
        self.persistence.lock().quiescing = true;
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
                if persistence.guild_document_failed {
                    return Err("Guild registry storage failed".to_owned());
                }
                if persistence.gate_document_failed {
                    return Err("Gate registry storage failed".to_owned());
                }
                if persistence.island_document_failed {
                    return Err("Island state storage failed".to_owned());
                }
                if persistence.party_document_failed {
                    return Err("Party state storage failed".to_owned());
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
    use uuid::Uuid;

    use super::{PREPARED_JOIN_TTL_TICKS, Persistence, insert_stack};
    use crate::player_data::SoloLevelingPlayerData;

    #[test]
    fn stale_prepared_accounts_are_dropped_without_a_join() {
        let mut persistence = Persistence::default();
        let stale = Uuid::new_v4();
        let fresh = Uuid::new_v4();
        persistence.tick = PREPARED_JOIN_TTL_TICKS + 10;
        persistence
            .prepared
            .insert(stale, (SoloLevelingPlayerData::new(stale), 5));
        persistence.prepared.insert(
            fresh,
            (
                SoloLevelingPlayerData::new(fresh),
                persistence.tick - PREPARED_JOIN_TTL_TICKS,
            ),
        );

        persistence.evict_stale_prepared();

        assert!(
            !persistence.prepared.contains_key(&stale),
            "an unclaimed account older than the TTL is dropped"
        );
        assert!(
            persistence.prepared.contains_key(&fresh),
            "a recently prepared account survives"
        );
        assert_eq!(
            persistence.prepared.get(&fresh).map(|(data, _)| data.uuid),
            Some(fresh)
        );
    }

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
