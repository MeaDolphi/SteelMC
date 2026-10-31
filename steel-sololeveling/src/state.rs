//! Shared mutable state for Solo Leveling gameplay systems.

mod abilities;
mod auction;
mod class;
pub use auction::{AuctionRequest, LotView};
mod organization;
pub use organization::{BoardView, GroupPayout};
pub use persistence::StarterError;
pub use rewards::GateClear;
mod enchants;
mod persistence;
mod rewards;
mod spells;

use std::io;
use std::mem;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use glam::DVec3;
use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::entity::LivingEntity;
use steel_core::player::{Player, PlayerInput};
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::item_stack::ItemStack;
use steel_utils::Identifier;
use steel_utils::locks::SyncMutex;
use uuid::Uuid;

use crate::castle;
use crate::city;
use crate::combat_reward::HunterReward;
use crate::daily;
use crate::dungeon::Gate;
use crate::dungeon::mobs::MobClass;
use crate::dungeon::mobs::MobLabel;
use crate::dungeon::runtime::{self, DungeonRuntime, RunDeath};
use crate::guild::{Guild, GuildError, GuildRegistry, GuildRole};
use crate::island::{self, IslandRuntime};
use crate::item::artifact::{self, ArtifactId};
use crate::item::crystal::CrystalSaleError;
use crate::item::enchantment::Enchantment;
use crate::item::{material, pouch};
use crate::merchant;
use crate::notice::Notice;
use crate::organization::Event;
use crate::organization::rank_multiplier;
use crate::party::{Party, PartyError, PartyExit, PartyRuntime};
use crate::player_data::{PlayerDataStorage, SoloLevelingPlayerData};
use crate::quest_book::Goal;
use crate::rank::HunterRank;
use crate::resource_pack::{self, ServedPack};
use crate::session::{HunterSession, QuestUpdate, SessionRegistry, SidebarChange};
use crate::shadow::{RECOVERY_HEALTH_PER_MINUTE, Shadow, ShadowStat, army_limit};
use crate::shadow_runtime::ShadowRuntime;
use crate::shop::ShopDay;
use crate::sidebar::{ContractScope, SidebarView};
use crate::stats::{HunterStat, HunterStats, LevelUp};
use steel_core::entity::{AlliedMode, SharedEntity};

/// Percent chance that each fighter of the Ant King gets its ring.
const ANT_KING_RING_PERCENT: u32 = 5;
/// Won for an island nest of an E-rank zone.
const ISLAND_NEST_WON: u64 = 200;
/// Won every fighter of the Ant King receives.
const ISLAND_KING_WON: u64 = 5_000;
/// Won per Demon Castle floor, times the floor number.
const CASTLE_FLOOR_WON: u64 = 20;

/// Where the artifact being enchanted lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnchantTarget {
    /// A slot of the player inventory.
    Inventory(usize),
    /// A slot of the System gear.
    Gear(usize),
}

/// An item maker action requested from inside a menu callback.
#[derive(Debug)]
pub enum EnchantAction {
    /// Open the artifact picker of the item maker.
    OpenMenu,
    /// Lay one enchantment on the chosen artifact.
    Apply(
        Enchantment,
        EnchantTarget,
        Arc<SyncMutex<Option<EnchantTarget>>>,
    ),
}

/// Deferred gate-menu actions that run after the inventory callback releases its lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMenuAction {
    /// Open the requested page.
    OpenPage(usize),
    /// Teleport near the committed gate, without entering it.
    Teleport(u64),
}

/// Storage and live sessions shared by Solo Leveling hooks and commands.
#[derive(Debug)]
pub struct SoloLevelingState {
    storage: PlayerDataStorage,
    sessions: SessionRegistry,
    dungeon_runtime: SyncMutex<DungeonRuntime>,
    shadow_runtime: SyncMutex<ShadowRuntime>,
    island: SyncMutex<IslandRuntime>,
    island_travel: SyncMutex<Vec<(Uuid, bool)>>,
    castle_ranking: SyncMutex<castle::RankingFile>,
    party_runtime: SyncMutex<PartyRuntime>,
    guilds: SyncMutex<GuildRegistry>,
    active_guild_banks: SyncMutex<FxHashSet<String>>,
    auction: SyncMutex<auction::AuctionState>,
    persistence: SyncMutex<persistence::Persistence>,
    organization_boards: SyncMutex<organization::Boards>,
    organization_events: SyncMutex<Vec<(Vec<Uuid>, Event)>>,
    storms: SyncMutex<Vec<abilities::Storm>>,
    spell_runtime: SyncMutex<spells::SpellRuntime>,
    enchant_runtime: SyncMutex<enchants::EnchantRuntime>,
    crystal_sales: SyncMutex<FxHashSet<Uuid>>,
    enchant_actions: SyncMutex<FxHashMap<Uuid, EnchantAction>>,
    gear_menus: SyncMutex<FxHashSet<Uuid>>,
    auction_sell_menus: SyncMutex<FxHashSet<Uuid>>,
    buyback_menus: SyncMutex<FxHashSet<Uuid>>,
    deferred_menus: SyncMutex<FxHashMap<Uuid, usize>>,
    gate_menu_actions: SyncMutex<FxHashMap<Uuid, GateMenuAction>>,
    /// The running server, for menus that list online hunters.
    server: SyncMutex<Weak<Server>>,
    /// Latest server tick, for menus that act on tick-stamped state.
    current_tick: AtomicU64,
    /// The served resource pack, once startup found one.
    resource_pack: SyncMutex<Option<ServedPack>>,
    notices: SyncMutex<FxHashMap<Uuid, Notice>>,
}

impl SoloLevelingState {
    /// Creates state rooted at `base_dir`, creating the directory if needed.
    pub fn new(base_dir: PathBuf) -> io::Result<Self> {
        let storage = PlayerDataStorage::new(base_dir);
        storage.init()?;
        let guilds = GuildRegistry::empty();
        let boards =
            organization::Boards::load(storage.base_dir().join("organization_boards.json"))?;
        let castle_ranking =
            castle::RankingFile::load(&storage.base_dir().join("castle_ranking.json"))?;
        let mut dungeon_runtime = DungeonRuntime::default();
        dungeon_runtime.load_region_records(storage.base_dir().join("instance_regions.json"))?;
        dungeon_runtime
            .load_legacy_gate_cleanups(storage.base_dir().join("legacy_gate_cleanups.json"))?;
        Ok(Self {
            storage,
            sessions: SessionRegistry::new(),
            dungeon_runtime: SyncMutex::new(dungeon_runtime),
            shadow_runtime: SyncMutex::new(ShadowRuntime::default()),
            island: SyncMutex::new(IslandRuntime::new()),
            island_travel: SyncMutex::new(Vec::new()),
            castle_ranking: SyncMutex::new(castle_ranking),
            party_runtime: SyncMutex::new(PartyRuntime::default()),
            guilds: SyncMutex::new(guilds),
            active_guild_banks: SyncMutex::new(FxHashSet::default()),
            auction: SyncMutex::new(auction::AuctionState::default()),
            persistence: SyncMutex::new(persistence::Persistence::default()),
            organization_boards: SyncMutex::new(boards),
            organization_events: SyncMutex::new(Vec::new()),
            storms: SyncMutex::new(Vec::new()),
            spell_runtime: SyncMutex::new(spells::SpellRuntime::default()),
            enchant_runtime: SyncMutex::new(enchants::EnchantRuntime::default()),
            crystal_sales: SyncMutex::new(FxHashSet::default()),
            enchant_actions: SyncMutex::new(FxHashMap::default()),
            gear_menus: SyncMutex::new(FxHashSet::default()),
            auction_sell_menus: SyncMutex::new(FxHashSet::default()),
            buyback_menus: SyncMutex::new(FxHashSet::default()),
            deferred_menus: SyncMutex::new(FxHashMap::default()),
            gate_menu_actions: SyncMutex::new(FxHashMap::default()),
            server: SyncMutex::new(Weak::new()),
            current_tick: AtomicU64::new(0),
            resource_pack: SyncMutex::new(None),
            notices: SyncMutex::new(FxHashMap::default()),
        })
    }

    /// Starts or replaces a player's live session.
    pub fn begin_session(&self, player_uuid: Uuid, data: SoloLevelingPlayerData) {
        let mut session = HunterSession::new(data.stats, data.won, data.quest, data.shadows);
        session.system_points = data.system_points;
        session.shop = data.shop;
        session.organization = data.organization;
        session.cooldowns = data.cooldowns;
        session.class = data.class;
        session.saved_gear = data.gear.clone();
        session.gear = data.gear.items();
        session.deferred_inventory = data.deferred_inventory;
        session.starter = data.starter;
        session.merchant = data.merchant;
        session.castle = data.castle;
        session.spells = data.spells;
        session.quests = data.quests;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        session.start_day(daily::day_at(now), now);
        self.sessions.insert(player_uuid, session);
    }

    /// Mutates a live session if it exists.
    pub fn with_session_mut<R>(
        &self,
        player_uuid: Uuid,
        f: impl FnOnce(&mut HunterSession) -> R,
    ) -> Option<R> {
        self.sessions.with_mut(player_uuid, f)
    }

    /// Allocates one unspent point to `stat`, returning the resulting stats.
    ///
    /// Returns `None` when the player has no active Solo Leveling session or
    /// has no points available to allocate.
    pub fn allocate_point(&self, player_uuid: Uuid, stat: HunterStat) -> Option<HunterStats> {
        self.sessions.with_mut(player_uuid, |session| {
            let stats = &mut session.stats;
            if stats.unspent_points == 0 {
                return None;
            }

            stats.unspent_points -= 1;
            stats.increment(stat);
            Some(*stats)
        })?
    }

    /// Grants experience to an online hunter.
    ///
    /// Future dungeon, quest, and boss systems should award progression only
    /// through this method so level and stat-point rules stay centralized.
    pub fn grant_experience(
        &self,
        player_uuid: Uuid,
        amount: u64,
    ) -> Option<(HunterStats, LevelUp)> {
        self.sessions.with_mut(player_uuid, |session| {
            let level_up = session.stats.grant_experience(amount);
            (session.stats, level_up)
        })
    }

    /// Records one player's quest activity for the current two-hour window.
    pub fn observe_quest(
        &self,
        player_uuid: Uuid,
        window: u64,
        position: DVec3,
        input: PlayerInput,
    ) -> Option<QuestUpdate> {
        self.sessions.with_mut(player_uuid, |session| {
            session.observe_quest(window, position, input)
        })
    }

    /// Returns the online hunter's currently derived rank.
    pub fn hunter_rank(&self, player_uuid: Uuid) -> Option<HunterRank> {
        self.sessions
            .with_mut(player_uuid, |session| session.stats.rank())
    }

    /// Claims a pending penalty zone exactly once.
    pub fn claim_penalty(&self, player_uuid: Uuid) -> Option<HunterRank> {
        self.sessions.with_mut(player_uuid, |session| {
            session.claim_penalty().then(|| session.stats.rank())
        })?
    }

    /// Returns whether this hunter still owes the system a penalty zone.
    pub fn has_pending_penalty(&self, player_uuid: Uuid) -> bool {
        self.sessions
            .with_mut(player_uuid, |session| session.quest.penalty_pending)
            .unwrap_or(false)
    }

    /// Returns how many seconds remain until an owed penalty zone starts, or
    /// `None` when it is already due or nothing is owed.
    pub fn penalty_wait_seconds(&self, player_uuid: Uuid) -> Option<u64> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        self.sessions
            .with_mut(player_uuid, |session| session.penalty_ready_at())
            .flatten()
            .map(|ready_at| ready_at.saturating_sub(now))
            .filter(|wait| *wait > 0)
    }

    /// Returns whether an owed penalty zone should start now.
    ///
    /// A penalty from an earlier day waits a few minutes after login; one from
    /// a day that ended while the hunter was online is due at once.
    pub fn penalty_due(&self, player_uuid: Uuid) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        self.sessions
            .with_mut(player_uuid, |session| session.penalty_due(now))
            .unwrap_or(false)
    }

    /// Applies the penalty-zone loss and returns changed attributes.
    pub fn apply_penalty_failure(&self, player_uuid: Uuid, roll: usize) -> Option<HunterStats> {
        self.sessions.with_mut(player_uuid, |session| {
            session.stats.decrement_nonzero_for_roll(roll)?;
            Some(session.stats)
        })?
    }

    /// Advances the shared island infestation and serves travel requests.
    pub fn tick_island(&self, server: &Arc<Server>, tick: u64) {
        let before = self.island.lock().snapshot();
        let travels = mem::take(&mut *self.island_travel.lock());
        for (uuid, to_island) in travels {
            let Some(player) = server
                .get_players()
                .into_iter()
                .find(|player| player.gameprofile.id() == uuid)
            else {
                continue;
            };
            if to_island {
                let rank = self.hunter_rank(uuid).unwrap_or(HunterRank::E);
                island::travel_to(server, player, rank);
            } else if let Some(city) = server
                .worlds
                .values()
                .find(|world| city::is_city_world(world))
                && let Err(error) = server.queue_player_world_selection(player, city)
            {
                log::warn!("sololeveling: could not evacuate hunter {uuid}: {error}");
            }
        }
        self.island.lock().tick(server, tick);
        let after = self.island.lock().snapshot();
        if before != after {
            if let Err(error) = self.queue_island_save() {
                log::warn!("sololeveling: could not queue island save after tick: {error}");
            }
        }
    }

    /// Asks to move a hunter to the island, or back to the city.
    pub fn request_island_travel(&self, uuid: Uuid, to_island: bool) {
        self.island_travel.lock().push((uuid, to_island));
    }

    /// Remembers that `hunter` hit something on the island, for the King's reward.
    pub fn record_island_fighter(&self, hunter: Uuid) {
        self.island.lock().record_fighter(hunter);
    }

    /// Settles a death in the island world and pays the hunters; returns
    /// whether the entity was an island mob.
    pub fn observe_island_death(
        &self,
        world: &World,
        entity_id: i32,
        hunter: Option<Uuid>,
    ) -> bool {
        if !island::is_island_world(world) {
            return false;
        }
        if let Some(hunter) = hunter {
            self.island.lock().record_fighter(hunter);
        }
        let Some(kill) = self.island.lock().on_death(world, entity_id) else {
            return false;
        };
        let mut roll = || rand::random_range(0..100);
        match &kill {
            island::IslandKill::Ant(rank) => {
                if let Some(hunter) = hunter {
                    self.credit_dungeon_mob(hunter, *rank);
                    self.record_goal(hunter, Goal::Ant, 1);
                }
            }
            island::IslandKill::Nest { zone, .. } => {
                if let Some(hunter) = hunter {
                    self.credit_dungeon_boss(vec![hunter], zone.rank());
                    let bounty = ISLAND_NEST_WON * rank_multiplier(zone.rank());
                    self.credit_won(hunter, bounty);
                    self.notify(hunter, &format!("Награда за гнездо: +{bounty} Won."));
                    self.record_goal(hunter, Goal::Nest, 1);
                    self.record_organization_event(vec![hunter], Event::Nest);
                }
            }
            island::IslandKill::King { hunters } => {
                self.credit_dungeon_boss(hunters.clone(), HunterRank::S);
                for fighter in hunters {
                    let rank = self.hunter_rank(*fighter).unwrap_or(HunterRank::C);
                    for stack in island::king_loot(rank) {
                        self.queue_system_loot(*fighter, stack);
                    }
                    self.credit_won(*fighter, ISLAND_KING_WON);
                    self.notify(
                        *fighter,
                        &format!("Награда за Короля муравьёв: +{ISLAND_KING_WON} Won."),
                    );
                    self.record_goal(*fighter, Goal::AntKing, 1);
                    if rand::random_range(0..100) < ANT_KING_RING_PERCENT {
                        self.queue_system_loot(
                            *fighter,
                            artifact::stack(ArtifactId::AntKingRing, None),
                        );
                    }
                }
            }
        }
        if let Some(hunter) = hunter {
            for stack in island::kill_loot(&kill, &mut roll) {
                self.queue_system_loot(hunter, stack);
            }
        }
        if let Err(error) = self.queue_island_save() {
            log::warn!("sololeveling: could not queue island save after island death: {error}");
        }
        true
    }

    /// Advances live gate and dungeon orchestration at the server tick boundary.
    ///
    /// The runtime lock is released before any deferred run action runs, so
    /// instance bookkeeping and session mutation never nest in both directions.
    pub fn tick_dungeons(&self, server: &Arc<Server>, tick: u64) {
        let (actions, loot, gate_write) = {
            let mut runtime = self.dungeon_runtime.lock();
            let actions = runtime.tick(self, server, tick);
            let gate_write = runtime.begin_gate_persistence();
            (actions, runtime.take_pending_loot(), gate_write)
        };
        if let Some(gate_write) = gate_write {
            let revision = gate_write.revision;
            if let Err(error) = self.queue_gate_registry_snapshot(gate_write) {
                log::error!("sololeveling: could not queue gate registry write: {error}");
                self.dungeon_runtime.lock().fail_gate_persistence(revision);
            }
        }
        for action in &actions {
            runtime::apply_action(self, server, action);
        }
        for (recipient, stack) in loot {
            self.queue_system_loot(recipient, stack);
        }
        self.shadow_runtime.lock().tick(tick);
        if tick.is_multiple_of(20) {
            self.follow_owners_across_worlds(server);
        }
        if tick.is_multiple_of(20 * 6) {
            for owner in self
                .sessions
                .recover_shadows(RECOVERY_HEALTH_PER_MINUTE / 10)
            {
                if let Err(error) = self.save_session(owner) {
                    log::warn!(
                        "sololeveling: failed to persist shadow recovery for {owner}: {error}"
                    );
                }
            }
        }
        self.party_runtime.lock().tick(tick);
    }

    /// Creates or refreshes a one-minute party invitation.
    pub fn invite_to_party(
        &self,
        inviter: Uuid,
        target: Uuid,
        tick: u64,
    ) -> Result<(), PartyError> {
        let result = self.party_runtime.lock().invite(inviter, target, tick);
        if result.is_ok() {
            if let Err(error) = self.queue_party_save() {
                log::warn!("sololeveling: could not queue party save after invite: {error}");
            }
        }
        result
    }

    /// Returns the leader whose party currently invites `player`.
    pub fn party_invitation(&self, player: Uuid) -> Option<Uuid> {
        self.party_runtime
            .lock()
            .invitation_for(player, self.current_tick())
    }

    /// Accepts the caller's current party invitation.
    pub fn accept_party_invitation(&self, player: Uuid, tick: u64) -> Result<Uuid, PartyError> {
        let result = self.party_runtime.lock().accept(player, tick);
        if result.is_ok() {
            if let Err(error) = self.queue_party_save() {
                log::warn!("sololeveling: could not queue party save after accept: {error}");
            }
        }
        result
    }

    /// Leaves the caller's current party.
    pub fn leave_party(&self, player: Uuid) -> Option<PartyExit> {
        let result = self.party_runtime.lock().leave(player);
        if result.is_some() {
            if let Err(error) = self.queue_party_save() {
                log::warn!("sololeveling: could not queue party save after leave: {error}");
            }
        }
        result
    }

    /// Removes a party member at its leader's request.
    pub fn kick_from_party(&self, leader: Uuid, target: Uuid) -> Result<(), PartyError> {
        let result = self.party_runtime.lock().kick(leader, target);
        if result.is_ok() {
            if let Err(error) = self.queue_party_save() {
                log::warn!("sololeveling: could not queue party save after kick: {error}");
            }
        }
        result
    }

    /// Returns the caller's current party.
    #[must_use]
    pub fn party_of(&self, player: Uuid) -> Option<Party> {
        self.party_runtime.lock().party_of(player).cloned()
    }

    /// Transfers party leadership or cleans up membership on disconnect.
    pub fn on_party_disconnect(&self, player: Uuid) {
        let _ = self.leave_party(player);
    }

    /// Creates a persistent guild with `guildmaster` as its first member.
    pub fn create_guild(&self, name: String, guildmaster: Uuid) -> Result<(), GuildError> {
        self.ensure_guild_mutation_available()?;
        let mut guilds = self.guilds.lock();
        guilds.create(name, guildmaster)?;
        drop(guilds);
        self.record_goal(guildmaster, Goal::GuildJoined, 1);
        self.queue_guild_save(Some(guildmaster))
            .map_err(|_| GuildError::Storage)
    }

    /// Adds a recruit to the guild of `inviter`.
    pub fn add_guild_recruit(&self, inviter: Uuid, recruit: Uuid) -> Result<(), GuildError> {
        self.ensure_guild_mutation_available()?;
        let mut guilds = self.guilds.lock();
        guilds.add_recruit(inviter, recruit)?;
        drop(guilds);
        self.record_goal(recruit, Goal::GuildJoined, 1);
        self.queue_guild_save(Some(inviter))
            .map_err(|_| GuildError::Storage)
    }

    /// Removes a lower-ranked guild member.
    pub fn remove_guild_member(&self, actor: Uuid, target: Uuid) -> Result<(), GuildError> {
        self.ensure_guild_mutation_available()?;
        let mut guilds = self.guilds.lock();
        guilds.remove(actor, target)?;
        drop(guilds);
        self.queue_guild_save(Some(actor))
            .map_err(|_| GuildError::Storage)
    }

    /// Changes a non-guildmaster member's rank.
    pub fn set_guild_role(
        &self,
        actor: Uuid,
        target: Uuid,
        role: GuildRole,
    ) -> Result<(), GuildError> {
        self.ensure_guild_mutation_available()?;
        let mut guilds = self.guilds.lock();
        guilds.set_role(actor, target, role)?;
        drop(guilds);
        self.queue_guild_save(Some(actor))
            .map_err(|_| GuildError::Storage)
    }

    /// Leaves the caller's guild unless they are its guildmaster.
    pub fn leave_guild(&self, player: Uuid) -> Result<(), GuildError> {
        self.ensure_guild_mutation_available()?;
        let mut guilds = self.guilds.lock();
        guilds.leave(player)?;
        drop(guilds);
        self.queue_guild_save(Some(player))
            .map_err(|_| GuildError::Storage)
    }

    /// Reserves an authorized guild bank and returns its saved inventory.
    pub fn open_guild_bank(
        &self,
        player: Uuid,
        slots: usize,
    ) -> Result<(String, Vec<ItemStack>), GuildError> {
        self.ensure_guild_mutation_available()?;
        let (key, items) = self.guilds.lock().bank_contents_for(player, slots)?;
        let mut active = self.active_guild_banks.lock();
        if !active.insert(key.clone()) {
            return Err(GuildError::BankInUse);
        }
        Ok((key, items))
    }

    /// Persists and releases a bank reservation after its menu closes.
    pub fn close_guild_bank(&self, player: Uuid, key: &str, items: &[ItemStack]) {
        let result = (|| {
            self.ensure_guild_mutation_available()?;
            let mut guilds = self.guilds.lock();
            guilds.set_bank_items(player, items)?;
            drop(guilds);
            self.queue_guild_save(Some(player))
                .map_err(|_| GuildError::Storage)
        })();
        self.active_guild_banks.lock().remove(key);
        if let Err(error) = result {
            log::warn!("sololeveling: failed to persist guild bank {key}: {error:?}");
        }
    }

    /// Moves a hunter's Won into an authorized guild treasury.
    pub fn deposit_guild_treasury(&self, player: Uuid, amount: u64) -> Result<(), GuildError> {
        self.ensure_guild_mutation_available()?;
        if amount == 0 {
            return Ok(());
        }
        self.guilds
            .lock()
            .clone()
            .deposit_treasury(player, amount)?;
        self.sessions
            .with_mut(player, |session| {
                if session.economic_busy {
                    return Err(GuildError::TransactionPending);
                }
                if session.won.amount() < amount {
                    return Err(GuildError::InsufficientFunds);
                }
                session.economic_busy = true;
                Ok(())
            })
            .ok_or(GuildError::Storage)??;
        if self.queue_guild_deposit(player, amount).is_err() {
            self.sessions
                .with_mut(player, |session| session.economic_busy = false);
            return Err(GuildError::Storage);
        }
        Ok(())
    }

    /// Returns the caller's guild snapshot.
    #[must_use]
    pub fn guild_of(&self, player: Uuid) -> Option<Guild> {
        self.guilds.lock().guild_of(player).cloned()
    }

    /// Opens the hunter's next Demon Castle floor next to them, paid by a
    /// castle key or by today's free entry; returns the floor.
    pub fn request_castle_floor(&self, uuid: Uuid, keyed: bool) -> Result<u32, &'static str> {
        let now = merchant::unix_now();
        let progress = self
            .sessions
            .with_mut(uuid, |session| session.castle)
            .ok_or("Сессия охотника не найдена.")?;
        if !keyed && progress.free_entry_in(now) > 0 {
            return Err("Бесплатный вход ещё не восстановился: нужен ключ Замка демонов.");
        }
        let floor = progress.next_floor.max(1);
        // The runtime lock is never taken inside a session closure: the
        // runtime itself reads sessions while it holds its lock.
        if !self.dungeon_runtime.lock().request_key_gate(
            uuid,
            castle::rank_for_floor(floor),
            Some(floor),
        ) {
            return Err("Врата по предыдущему ключу ещё открываются.");
        }
        self.sessions.with_mut(uuid, |session| {
            session.castle.enter();
            if !keyed {
                session.castle.free_at = now;
            }
        });
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: could not save castle entry of {uuid}: {error}");
        }
        Ok(floor)
    }

    /// Records a cleared castle floor for every survivor and the weekly ranking.
    pub fn record_castle_clear(&self, survivors: &[(Uuid, String)], floor: u32) {
        let week = castle::week_of(daily::today());
        for (uuid, name) in survivors {
            let won = CASTLE_FLOOR_WON * u64::from(floor);
            self.credit_won(*uuid, won);
            self.notify(*uuid, &format!("Замок демонов, этаж {floor}: +{won} Won."));
            self.sessions
                .with_mut(*uuid, |session| session.castle.clear(floor));
            if let Err(error) = self.save_session(*uuid) {
                log::warn!("sololeveling: could not save castle progress of {uuid}: {error}");
            }
            let mut ranking = self.castle_ranking.lock();
            ranking.ranking.record(week, *uuid, name, floor);
            if let Err(error) = ranking.save() {
                log::warn!("sololeveling: could not save the castle ranking: {error}");
            }
        }
    }

    /// Returns a hunter's climb and this week's ranking.
    pub fn castle_view(
        &self,
        uuid: Uuid,
    ) -> Option<(castle::CastleProgress, Vec<castle::RankingEntry>)> {
        let progress = self.sessions.with_mut(uuid, |session| session.castle)?;
        let week = castle::week_of(daily::today());
        Some((
            progress,
            self.castle_ranking.lock().ranking.top(week).to_vec(),
        ))
    }

    /// Asks for a gate of `rank` next to a hunter who used a dungeon key;
    /// `false` while the hunter's previous key gate is still being placed.
    pub fn request_key_gate(&self, player_uuid: Uuid, rank: HunterRank) -> bool {
        self.dungeon_runtime
            .lock()
            .request_key_gate(player_uuid, rank, None)
    }

    /// Asks for a prototype gate next to a hunter who just joined.
    pub fn request_test_gate(&self, player_uuid: Uuid) {
        self.dungeon_runtime.lock().request_test_gate(player_uuid);
    }

    /// Returns the current shop, rolling and saving it on first look.
    pub fn shop_day(&self, uuid: Uuid) -> Option<ShopDay> {
        let window = crate::shop::current_window();
        let (rolled, day) = self.sessions.with_mut(uuid, |session| {
            let rolled = !session.economic_busy && session.shop.roll_over(window);
            (rolled, session.shop)
        })?;
        if rolled && let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: failed to persist the shop slots of {uuid}: {error}");
        }
        Some(day)
    }

    /// Queues a crystal sale for an atomic inventory/account transaction.
    pub fn sell_crystals(&self, player: &Player) -> Result<(), CrystalSaleError> {
        self.request_crystal_sale(player)
    }

    /// Records a death observed by the world-owned gameplay hook.
    pub fn observe_dungeon_mob_death(
        &self,
        world: &World,
        entity_id: i32,
        hunter: Option<Uuid>,
    ) -> bool {
        let mut runtime = self.dungeon_runtime.lock();
        let was_dungeon_mob = runtime.is_active_dungeon_mob(world, entity_id);
        let materials = hunter.and_then(|hunter| {
            runtime
                .material_source(world, entity_id, hunter)
                .map(|(class, rank)| (hunter, class, rank))
        });
        let reward = hunter.and_then(|hunter| {
            runtime
                .ordinary_mob_rank(world, entity_id, hunter)
                .map(|rank| (hunter, rank))
        });
        if let Some(hunter) = hunter {
            runtime.record_mob_kill(world, entity_id, hunter);
        }
        runtime.on_living_death(world, entity_id);
        drop(runtime);
        if let Some((hunter, rank)) = reward {
            self.credit_dungeon_mob(hunter, rank);
        }
        if let Some((hunter, rank)) = reward {
            self.record_goal(hunter, Goal::MobKill, 1);
            if rand::random_range(0..100) < pouch::MOB_POUCH_PERCENT {
                self.queue_system_loot(hunter, pouch::stack(pouch::mob_amount(rank)));
            }
        }
        // Materials go to the credited hunter only, like the mob reward.
        if let Some((hunter, class, rank)) = materials {
            if !matches!(class, MobClass::Warrior | MobClass::Guard) {
                self.record_goal(hunter, Goal::EliteKill, 1);
            }
            let stacks = material::roll_mob(class, rank, || rand::random_range(0..100));
            let count: u32 = stacks
                .iter()
                .map(|stack| u32::try_from(stack.count).unwrap_or(0))
                .sum();
            for stack in stacks {
                self.queue_system_loot(hunter, stack);
            }
            self.record_goal(hunter, Goal::Material, count);
            for _ in 0..count {
                self.record_organization_event(vec![hunter], Event::Material(rank));
            }
        }
        was_dungeon_mob
    }

    /// Returns the effective rank of the dungeon run inside `world`.
    pub fn instance_rank(&self, world: &World) -> Option<HunterRank> {
        self.dungeon_runtime.lock().instance_rank(world)
    }

    /// Adds the shadow of a summon item to a hunter's roster.
    ///
    /// Returns the new shadow's name, or `None` when the army is already at its
    /// level-dependent limit, so a refused activation can keep its item.
    pub fn arise_shadow_from_item(&self, player_uuid: Uuid, entity: &Identifier) -> Option<String> {
        let name = self.sessions.with_mut(player_uuid, |session| {
            if session.shadows.shadows.len() >= army_limit(session.stats.level) {
                return None;
            }
            Some(session.shadows.arise(entity.clone()).name.clone())
        })??;
        self.record_goal(player_uuid, Goal::ShadowRaise, 1);
        if let Err(error) = self.save_session(player_uuid) {
            log::warn!(
                "sololeveling: failed to persist summoned shadow for {player_uuid}: {error}"
            );
        }
        Some(name)
    }

    /// Records final boss damage by a hunter participating in that raid.
    pub fn observe_raid_boss_damage(
        &self,
        world: &World,
        entity_id: i32,
        hunter: Uuid,
        amount: f32,
    ) {
        self.dungeon_runtime
            .lock()
            .record_boss_damage(world, entity_id, hunter, amount);
    }

    /// Builds what the sidebar shows `uuid`: session values plus every taken
    /// contract, personal, party and guild.
    pub fn sidebar_view(&self, uuid: Uuid) -> Option<SidebarView> {
        let mut view = self.sessions.with_mut(uuid, |session| SidebarView {
            stats: session.stats,
            won: session.won,
            system_points: session.system_points,
            quest: session.quest,
            contracts: session
                .organization
                .contracts
                .iter()
                .map(|contract| (ContractScope::Personal, *contract))
                .collect(),
        })?;
        if let Some(party) = self.party_of(uuid) {
            view.contracts.extend(
                party
                    .contracts()
                    .iter()
                    .filter(|group| group.participants.contains(&uuid))
                    .map(|group| (ContractScope::Group, group.contract)),
            );
        }
        if let Some(guild) = self.guilds.lock().guild_of(uuid) {
            view.contracts.extend(
                guild
                    .organization
                    .contracts
                    .iter()
                    .map(|group| (ContractScope::Guild, group.contract)),
            );
        }
        Some(view)
    }

    /// Returns the current sidebar view with the one shown before, or `None`
    /// when the sidebar is already up to date.
    pub fn sidebar_update(&self, uuid: Uuid) -> Option<(Option<SidebarView>, SidebarView)> {
        let view = self.sidebar_view(uuid)?;
        match self
            .sessions
            .with_mut(uuid, |session| session.sidebar_changed(&view))?
        {
            SidebarChange::Unchanged => None,
            SidebarChange::Changed { previous } => Some((previous, view)),
        }
    }

    /// Records committed experience and system points to show the hunter.
    pub fn notify_progress(&self, uuid: Uuid, reward: HunterReward, level_up: LevelUp, level: u32) {
        let mut notices = self.notices.lock();
        let notice = notices.entry(uuid).or_default();
        notice.experience = notice.experience.saturating_add(reward.experience);
        notice.system_points = notice.system_points.saturating_add(reward.system_points);
        if level_up.levels_gained > 0 {
            notice.levels = notice.levels.saturating_add(level_up.levels_gained);
            notice.level = level;
        }
    }

    /// Records items handed to the hunter to show them.
    pub fn notify_loot(&self, uuid: Uuid, stack: &ItemStack) {
        self.notices.lock().entry(uuid).or_default().add_loot(stack);
    }

    /// Takes everything collected for the hunter since it was last shown.
    pub fn take_notice(&self, uuid: Uuid) -> Option<Notice> {
        self.notices.lock().remove(&uuid)
    }

    /// Defers a crystal sale requested from a menu until its inventory locks are released.
    pub fn defer_crystal_sale(&self, uuid: Uuid) {
        self.crystal_sales.lock().insert(uuid);
    }

    /// Starts serving the resource pack from the Solo Leveling data directory.
    pub async fn start_resource_pack(&self) {
        let pack = resource_pack::start(self.storage.base_dir()).await;
        *self.resource_pack.lock() = pack;
    }

    /// Offers the resource pack to a hunter who just joined.
    pub fn offer_resource_pack(&self, player: &Player) {
        if let Some(pack) = self.resource_pack.lock().as_ref() {
            pack.offer(player);
        }
    }

    /// Records the running server and its tick for menus.
    pub fn observe_server(&self, server: &Arc<Server>, tick: u64) {
        self.current_tick.store(tick, Ordering::Relaxed);
        let mut stored = self.server.lock();
        if stored.strong_count() == 0 {
            *stored = Arc::downgrade(server);
        }
    }

    /// Returns the latest server tick seen by the tick hook.
    pub fn current_tick(&self) -> u64 {
        self.current_tick.load(Ordering::Relaxed)
    }

    /// Returns every online player, across all worlds.
    pub fn online_players(&self) -> Vec<Arc<Player>> {
        self.server
            .lock()
            .upgrade()
            .map_or_else(Vec::new, |server| server.get_players())
    }

    /// Defers opening the item maker until the menu callback's locks are released.
    pub fn defer_enchant_menu(&self, uuid: Uuid) {
        self.enchant_actions
            .lock()
            .insert(uuid, EnchantAction::OpenMenu);
    }

    /// Defers one enchantment chosen in the item maker menu.
    pub fn defer_enchantment(
        &self,
        uuid: Uuid,
        enchantment: Enchantment,
        target: EnchantTarget,
        completion: Arc<SyncMutex<Option<EnchantTarget>>>,
    ) {
        self.enchant_actions
            .lock()
            .insert(uuid, EnchantAction::Apply(enchantment, target, completion));
    }

    /// Takes a pending item maker action.
    pub fn take_enchant_action(&self, uuid: Uuid) -> Option<EnchantAction> {
        self.enchant_actions.lock().remove(&uuid)
    }

    /// Opens equipment after the current menu callback releases inventory locks.
    pub fn defer_gear_menu(&self, uuid: Uuid) {
        self.gear_menus.lock().insert(uuid);
    }

    /// Takes a pending equipment menu request.
    pub fn take_gear_menu(&self, uuid: Uuid) -> bool {
        self.gear_menus.lock().remove(&uuid)
    }

    /// Defers the auction item picker until its callback releases the inventory.
    pub fn defer_auction_sell_menu(&self, uuid: Uuid) {
        self.auction_sell_menus.lock().insert(uuid);
    }

    /// Defers the merchant buyback until its callback releases the inventory.
    pub fn defer_buyback_menu(&self, uuid: Uuid) {
        self.buyback_menus.lock().insert(uuid);
    }

    /// Takes a pending merchant buyback request.
    pub fn take_buyback_menu(&self, uuid: Uuid) -> bool {
        self.buyback_menus.lock().remove(&uuid)
    }

    /// Takes a pending auction item picker request.
    pub fn take_auction_sell_menu(&self, uuid: Uuid) -> bool {
        self.auction_sell_menus.lock().remove(&uuid)
    }

    /// Defers reopening the System deferred-inventory menu until its callback releases locks.
    pub fn defer_deferred_menu_open(&self, uuid: Uuid, page: usize) {
        self.deferred_menus.lock().insert(uuid, page);
    }

    /// Takes a pending deferred-inventory menu request.
    pub fn take_deferred_menu(&self, uuid: Uuid) -> Option<usize> {
        self.deferred_menus.lock().remove(&uuid)
    }

    /// Returns committed gates for a menu, or `None` while persistence blocks access.
    pub fn active_gates(&self) -> Option<Vec<Gate>> {
        let runtime = self.dungeon_runtime.lock();
        (!runtime.gate_persistence_blocks_actions()).then(|| runtime.committed_gates())
    }

    /// Returns one committed, still active gate.
    pub fn committed_gate(&self, id: u64) -> Option<Gate> {
        let runtime = self.dungeon_runtime.lock();
        (!runtime.gate_persistence_blocks_actions())
            .then(|| runtime.committed_gate(id).cloned())
            .flatten()
    }

    /// Defers a gate menu operation until after the current menu callback returns.
    pub fn defer_gate_menu_action(&self, uuid: Uuid, action: GateMenuAction) {
        self.gate_menu_actions.lock().insert(uuid, action);
    }

    /// Takes one pending gate menu action.
    pub fn take_gate_menu_action(&self, uuid: Uuid) -> Option<GateMenuAction> {
        self.gate_menu_actions.lock().remove(&uuid)
    }

    /// Takes a pending menu-triggered crystal sale request.
    pub fn take_crystal_sale(&self, uuid: Uuid) -> bool {
        self.crystal_sales.lock().remove(&uuid)
    }

    /// Returns the nameplate of a dungeon mob, while its run still knows it.
    pub fn dungeon_mob_label(&self, world: &World, entity_id: i32) -> Option<MobLabel> {
        self.dungeon_runtime.lock().mob_label(world, entity_id)
    }

    /// Returns whether two tracked mobs belong to the same dungeon or penalty run.
    pub fn are_dungeon_mobs_allied(&self, world: &World, first: i32, second: i32) -> bool {
        self.dungeon_runtime
            .lock()
            .are_in_same_run(world, first, second)
    }

    /// Rewrites a dungeon mob's nameplate after it lost health.
    pub fn refresh_mob_label(&self, world: &World, entity: &dyn LivingEntity) {
        let label = self.dungeon_runtime.lock().mob_label(world, entity.id());
        if let Some(label) = label {
            label.show(world, entity);
        }
    }

    /// Retains a nearby dungeon mob killed directly by a hunter.
    pub fn arise_from_player_kill(
        &self,
        hunter: &Player,
        world: &World,
        entity: &dyn LivingEntity,
        label: Option<&MobLabel>,
    ) -> Option<Shadow> {
        let shadow = self
            .sessions
            .with_mut(hunter.gameprofile.id(), |session| {
                ShadowRuntime::arise_from_player_kill(hunter, world, entity, label, session)
            })??;
        self.record_goal(hunter.gameprofile.id(), Goal::ShadowRaise, 1);
        if let Err(error) = self.save_session(hunter.gameprofile.id()) {
            log::warn!(
                "sololeveling: failed to persist arisen shadow for {}: {error}",
                hunter.gameprofile.id()
            );
        }
        Some(shadow)
    }

    /// Summons one named shadow for the player.
    pub fn summon_shadow(&self, hunter: &Player, name: &str) -> Result<(), &'static str> {
        let mut runtime = self.shadow_runtime.lock();
        self.sessions
            .with_mut(hunter.gameprofile.id(), |session| {
                runtime.summon(hunter, session, name)
            })
            .unwrap_or(Err("Сессия охотника не загружена."))
    }

    /// Summons every available roster shadow up to the hunter's army limit.
    pub fn summon_all_shadows(&self, hunter: &Player) -> Option<usize> {
        let mut runtime = self.shadow_runtime.lock();
        self.sessions.with_mut(hunter.gameprofile.id(), |session| {
            runtime.summon_all(hunter, session)
        })
    }

    /// Recalls every shadow currently summoned by this player.
    pub fn recall_shadows(&self, player_uuid: Uuid) -> usize {
        self.shadow_runtime.lock().recall(player_uuid)
    }

    /// Returns the roster ids of the player's currently summoned shadows.
    pub fn active_shadows(&self, player_uuid: Uuid) -> Vec<Uuid> {
        self.shadow_runtime.lock().active_shadows(player_uuid)
    }

    /// Recalls one summoned shadow, returning whether it was out.
    pub fn dismiss_shadow(&self, player_uuid: Uuid, shadow_id: Uuid) -> bool {
        self.shadow_runtime.lock().dismiss(player_uuid, shadow_id)
    }

    /// Returns the army-wide mode of a hunter's shadows.
    pub fn shadow_mode(&self, player_uuid: Uuid) -> AlliedMode {
        self.shadow_runtime.lock().mode(player_uuid)
    }

    /// Orders every summoned shadow of a hunter into `mode`.
    pub fn set_shadow_mode(&self, player_uuid: Uuid, mode: AlliedMode) {
        self.shadow_runtime.lock().set_mode(player_uuid, mode);
    }

    /// Sends a hunter's summoned shadows after `target`.
    pub fn focus_shadows(&self, owner: Uuid, target: &SharedEntity) {
        self.shadow_runtime.lock().focus(owner, target);
    }

    /// Summons again, next to their owner, shadows left behind in another world.
    pub fn follow_owners_across_worlds(&self, server: &Server) {
        let players = server.get_players();
        let world_of = |owner: Uuid| {
            players
                .iter()
                .find(|player| player.gameprofile.id() == owner)
                .map(|player| player.get_world().key.clone())
        };
        let displaced = self.shadow_runtime.lock().displaced(world_of);
        for (owner, shadow_id) in displaced {
            let Some(hunter) = players
                .iter()
                .find(|player| player.gameprofile.id() == owner)
            else {
                continue;
            };
            let Some(name) = self
                .shadows(owner)
                .and_then(|roster| roster.into_iter().find(|shadow| shadow.id == shadow_id))
                .map(|shadow| shadow.name)
            else {
                continue;
            };
            self.dismiss_shadow(owner, shadow_id);
            if let Err(error) = self.summon_shadow(hunter, &name) {
                log::debug!("sololeveling: shadow {name} could not follow {owner}: {error}");
            }
        }
    }

    /// Returns how many shadows the player may have summoned at once.
    pub fn shadow_army_limit(&self, player_uuid: Uuid) -> usize {
        self.sessions
            .with_mut(player_uuid, |session| army_limit(session.stats.level))
            .unwrap_or(0)
    }

    /// Returns a snapshot of the player's persistent roster.
    pub fn shadows(&self, player_uuid: Uuid) -> Option<Vec<Shadow>> {
        self.sessions
            .with_mut(player_uuid, |session| session.shadows.shadows.clone())
    }

    /// Spends a shadow point and applies it to a currently active body.
    pub fn upgrade_shadow(
        &self,
        player_uuid: Uuid,
        name: &str,
        stat: ShadowStat,
    ) -> Result<Shadow, &'static str> {
        let mut runtime = self.shadow_runtime.lock();
        self.sessions
            .with_mut(player_uuid, |session| {
                runtime.upgrade(player_uuid, session, name, stat)
            })
            .unwrap_or(Err("Сессия охотника не загружена."))
    }

    /// Starts persisted recovery for a shadow whose active body died.
    pub fn observe_shadow_death(&self, entity_uuid: Uuid) {
        let Some((owner, shadow_id, maximum_health)) =
            self.shadow_runtime.lock().observe_death(entity_uuid)
        else {
            return;
        };
        let changed = self.sessions.with_mut(owner, |session| {
            let Some(shadow) = session
                .shadows
                .shadows
                .iter_mut()
                .find(|shadow| shadow.id == shadow_id)
            else {
                return false;
            };
            shadow.begin_recovery(maximum_health);
            true
        });
        if changed == Some(true)
            && let Err(error) = self.save_session(owner)
        {
            log::warn!("sololeveling: failed to persist shadow recovery for {owner}: {error}");
        }
    }

    /// Gives the shadow its own vanilla reward. Hunter rewards are routed separately
    /// through the dungeon's rank table, without awarding party members a mob kill.
    pub fn credit_shadow_kill(
        &self,
        killer_uuid: Uuid,
        victim: &dyn LivingEntity,
        world: &World,
    ) -> Option<(String, u16)> {
        let (owner, shadow_id, experience) =
            self.shadow_runtime
                .lock()
                .kill_reward(killer_uuid, victim, world)?;
        let reward = self.sessions.with_mut(owner, |session| {
            let shadow = session
                .shadows
                .shadows
                .iter_mut()
                .find(|shadow| shadow.id == shadow_id)?;
            let levels = shadow.grant_experience(experience);
            Some((shadow.name.clone(), levels))
        })?;
        if experience > 0
            && let Err(error) = self.save_session(owner)
        {
            log::warn!("sololeveling: failed to persist shadow progression for {owner}: {error}");
        }
        reward
    }

    /// Resolves a death while a hunter is inside a Solo Leveling instance.
    pub fn observe_dungeon_player_death(&self, player: &Player) {
        let outcome = self.dungeon_runtime.lock().on_player_death(player);
        match outcome {
            RunDeath::PenaltyFailed { roll } => {
                self.apply_penalty_failure(player.gameprofile.id(), roll);
            }
            RunDeath::None | RunDeath::DungeonFailed => {}
        }
    }

    /// Drops the instance state owned by a disconnecting hunter.
    pub fn end_player_run(&self, server: &Server, player_uuid: Uuid) {
        self.dungeon_runtime
            .lock()
            .on_player_disconnect(server, player_uuid);
    }

    /// Releases the instance region of a run that was already acted on.
    pub fn complete_run(&self, player: &Player) {
        self.dungeon_runtime.lock().complete_run(player);
    }

    /// Releases the run of a hunter that went offline before its end action ran.
    pub fn complete_offline_run(&self, player_uuid: Uuid) {
        self.dungeon_runtime
            .lock()
            .complete_offline_run(player_uuid);
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use uuid::Uuid;

    use super::{SoloLevelingPlayerData, SoloLevelingState};
    use crate::abilities::AbilityCooldowns;
    use crate::castle::CastleProgress;
    use crate::class::ClassChoice;
    use crate::economy::{SystemPoints, Won};
    use crate::gear::SavedGear;
    use crate::merchant::MerchantRecord;
    use crate::organization::OrganizationBook;
    use crate::quest::SystemQuest;
    use crate::quest_book::QuestBook;
    use crate::shadow::ShadowRoster;
    use crate::shop::ShopDay;
    use crate::spell::SpellBook;
    use crate::starter::StarterKit;
    use crate::stats::{HunterStat, HunterStats};

    #[test]
    fn allocation_updates_the_active_session_and_refuses_empty_point_pools() {
        let directory = env::temp_dir().join(format!("sololeveling-state-test-{}", Uuid::new_v4()));
        let service = SoloLevelingState::new(directory.clone()).expect("state should initialize");
        let player_uuid = Uuid::new_v4();
        service.begin_session(
            player_uuid,
            SoloLevelingPlayerData {
                uuid: player_uuid,
                stats: HunterStats {
                    unspent_points: 1,
                    ..HunterStats::new()
                },
                won: Won::starting_balance(),
                system_points: SystemPoints::zero(),
                quest: SystemQuest::default(),
                shadows: ShadowRoster::default(),
                shop: ShopDay::unset(),
                organization: OrganizationBook::new(),
                cooldowns: AbilityCooldowns::default(),
                class: ClassChoice::default(),
                gear: SavedGear::default(),
                deferred_inventory: Vec::new(),
                starter: StarterKit::default(),
                merchant: MerchantRecord::default(),
                castle: CastleProgress::default(),
                spells: SpellBook::default(),
                quests: QuestBook::default(),
            },
        );

        let allocated_stats = service
            .allocate_point(player_uuid, HunterStat::Agility)
            .expect("one available point should allocate");
        assert_eq!(allocated_stats.agility, 1);
        assert_eq!(allocated_stats.unspent_points, 0);
        assert!(
            service
                .allocate_point(player_uuid, HunterStat::Agility)
                .is_none()
        );

        fs::remove_dir_all(directory).expect("cleanup should succeed");
    }
}
