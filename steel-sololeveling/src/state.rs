//! Shared mutable state for Solo Leveling gameplay systems.

mod abilities;
mod auction;
pub use auction::{AuctionRequest, LotView};
mod organization;
pub use organization::{BoardView, GroupPayout};
mod persistence;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use glam::DVec3;
use rustc_hash::FxHashSet;
use steel_core::entity::LivingEntity;
use steel_core::inventory::container::Container as _;
use steel_core::player::player_inventory::Prediction;
use steel_core::player::{Player, PlayerInput};
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::item_stack::ItemStack;
use steel_utils::Identifier;
use steel_utils::locks::SyncMutex;
use uuid::Uuid;

use crate::daily;
use crate::dungeon::runtime::{self, DungeonRuntime, RunDeath};
use crate::guild::{Guild, GuildError, GuildRegistry, GuildRole};
use crate::item::crystal::{self, CrystalSale, CrystalSaleError};
use crate::organization::Event;
use crate::party::{Party, PartyError, PartyExit, PartyRuntime};
use crate::player_data::{PlayerDataStorage, SoloLevelingPlayerData};
use crate::rank::HunterRank;
use crate::session::{HunterSession, QuestUpdate, SessionRegistry};
use crate::shadow::{RECOVERY_HEALTH_PER_MINUTE, Shadow, ShadowStat, army_limit};
use crate::shadow_runtime::ShadowRuntime;
use crate::shop::{Offer, ShopDay, ShopError};
use crate::stats::{HunterStat, HunterStats, LevelUp};

/// Storage and live sessions shared by Solo Leveling hooks and commands.
#[derive(Debug)]
pub struct SoloLevelingState {
    storage: PlayerDataStorage,
    sessions: SessionRegistry,
    dungeon_runtime: SyncMutex<DungeonRuntime>,
    shadow_runtime: SyncMutex<ShadowRuntime>,
    party_runtime: SyncMutex<PartyRuntime>,
    guilds: SyncMutex<GuildRegistry>,
    active_guild_banks: SyncMutex<FxHashSet<String>>,
    auction: SyncMutex<auction::AuctionState>,
    persistence: SyncMutex<persistence::Persistence>,
    organization_boards: SyncMutex<organization::Boards>,
    organization_events: SyncMutex<Vec<(Vec<Uuid>, Event)>>,
    storms: SyncMutex<Vec<abilities::Storm>>,
}

impl SoloLevelingState {
    /// Creates state rooted at `base_dir`, creating the directory if needed.
    pub fn new(base_dir: PathBuf) -> io::Result<Self> {
        let storage = PlayerDataStorage::new(base_dir);
        storage.init()?;
        let guilds = GuildRegistry::load(storage.base_dir())?;
        let boards =
            organization::Boards::load(storage.base_dir().join("organization_boards.json"))?;
        Ok(Self {
            storage,
            sessions: SessionRegistry::new(),
            dungeon_runtime: SyncMutex::new(DungeonRuntime::default()),
            shadow_runtime: SyncMutex::new(ShadowRuntime::default()),
            party_runtime: SyncMutex::new(PartyRuntime::default()),
            guilds: SyncMutex::new(guilds),
            active_guild_banks: SyncMutex::new(FxHashSet::default()),
            auction: SyncMutex::new(auction::AuctionState::default()),
            persistence: SyncMutex::new(persistence::Persistence::default()),
            organization_boards: SyncMutex::new(boards),
            organization_events: SyncMutex::new(Vec::new()),
            storms: SyncMutex::new(Vec::new()),
        })
    }

    /// Starts or replaces a player's live session.
    pub fn begin_session(&self, player_uuid: Uuid, data: SoloLevelingPlayerData) {
        let mut session = HunterSession::new(data.stats, data.won, data.quest, data.shadows);
        session.system_points = data.system_points;
        session.shop = data.shop;
        session.organization = data.organization;
        session.cooldowns = data.cooldowns;
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

    /// Advances live gate and dungeon orchestration at the server tick boundary.
    ///
    /// The runtime lock is released before any deferred run action runs, so
    /// instance bookkeeping and session mutation never nest in both directions.
    pub fn tick_dungeons(&self, server: &Arc<Server>, tick: u64) {
        let (actions, loot) = {
            let mut runtime = self.dungeon_runtime.lock();
            let actions = runtime.tick(self, server, tick);
            (actions, runtime.take_pending_loot())
        };
        for action in &actions {
            runtime::apply_action(self, server, action);
        }
        for (recipient, stack) in loot {
            if let Some(player) = server
                .get_players()
                .into_iter()
                .find(|player| player.gameprofile.id() == recipient)
            {
                player.add_item_or_drop(stack, Prediction::ServerOnly);
            }
        }
        self.shadow_runtime.lock().tick();
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
        self.party_runtime.lock().invite(inviter, target, tick)
    }

    /// Accepts the caller's current party invitation.
    pub fn accept_party_invitation(&self, player: Uuid, tick: u64) -> Result<Uuid, PartyError> {
        self.party_runtime.lock().accept(player, tick)
    }

    /// Leaves the caller's current party.
    pub fn leave_party(&self, player: Uuid) -> Option<PartyExit> {
        self.party_runtime.lock().leave(player)
    }

    /// Removes a party member at its leader's request.
    pub fn kick_from_party(&self, leader: Uuid, target: Uuid) -> Result<(), PartyError> {
        self.party_runtime.lock().kick(leader, target)
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
        let mut guilds = self.guilds.lock();
        guilds.create(name, guildmaster)?;
        guilds.save().map_err(|_| GuildError::Storage)
    }

    /// Adds a recruit to the guild of `inviter`.
    pub fn add_guild_recruit(&self, inviter: Uuid, recruit: Uuid) -> Result<(), GuildError> {
        let mut guilds = self.guilds.lock();
        guilds.add_recruit(inviter, recruit)?;
        guilds.save().map_err(|_| GuildError::Storage)
    }

    /// Removes a lower-ranked guild member.
    pub fn remove_guild_member(&self, actor: Uuid, target: Uuid) -> Result<(), GuildError> {
        let mut guilds = self.guilds.lock();
        guilds.remove(actor, target)?;
        guilds.save().map_err(|_| GuildError::Storage)
    }

    /// Changes a non-guildmaster member's rank.
    pub fn set_guild_role(
        &self,
        actor: Uuid,
        target: Uuid,
        role: GuildRole,
    ) -> Result<(), GuildError> {
        let mut guilds = self.guilds.lock();
        guilds.set_role(actor, target, role)?;
        guilds.save().map_err(|_| GuildError::Storage)
    }

    /// Leaves the caller's guild unless they are its guildmaster.
    pub fn leave_guild(&self, player: Uuid) -> Result<(), GuildError> {
        let mut guilds = self.guilds.lock();
        guilds.leave(player)?;
        guilds.save().map_err(|_| GuildError::Storage)
    }

    /// Reserves an authorized guild bank and returns its saved inventory.
    pub fn open_guild_bank(
        &self,
        player: Uuid,
        slots: usize,
    ) -> Result<(String, Vec<ItemStack>), GuildError> {
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
            let mut guilds = self.guilds.lock();
            guilds.set_bank_items(player, items)?;
            guilds.save().map_err(|_| GuildError::Storage)
        })();
        self.active_guild_banks.lock().remove(key);
        if let Err(error) = result {
            log::warn!("sololeveling: failed to persist guild bank {key}: {error:?}");
        }
    }

    /// Moves a hunter's Won into an authorized guild treasury.
    pub fn deposit_guild_treasury(&self, player: Uuid, amount: u64) -> Result<(), GuildError> {
        if amount == 0 {
            return Ok(());
        }
        let debited = self
            .sessions
            .with_mut(player, |session| {
                if session.economic_busy {
                    return Err(GuildError::TransactionPending);
                }
                Ok(session.won.debit(amount))
            })
            .transpose()?;
        if debited != Some(true) {
            return Err(GuildError::InsufficientFunds);
        }
        let result = (|| {
            let mut guilds = self.guilds.lock();
            guilds.deposit_treasury(player, amount)?;
            guilds.save().map_err(|_| GuildError::Storage)
        })();
        if result.is_err() {
            self.credit_won(player, amount);
        }
        result
    }

    /// Returns the caller's guild snapshot.
    #[must_use]
    pub fn guild_of(&self, player: Uuid) -> Option<Guild> {
        self.guilds.lock().guild_of(player).cloned()
    }

    /// Asks for a prototype gate next to a hunter who just joined.
    pub fn request_test_gate(&self, player_uuid: Uuid) {
        self.dungeon_runtime.lock().request_test_gate(player_uuid);
    }

    /// Buys one daily shop offer for the calling hunter.
    ///
    /// The debit and the daily counter change together under the session lock;
    /// a refused purchase never charges points, and the granted item is bound to
    /// the buyer so it cannot be resold.
    pub fn buy_shop_offer(&self, player: &Arc<Player>, slot: usize) -> Result<Offer, ShopError> {
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
                if !session.system_points.debit(offer.price) {
                    return Err(ShopError::InsufficientPoints);
                }
                if !session.shop.mark(slot) {
                    return Err(ShopError::UnknownSlot);
                }
                Ok(offer)
            })
            .ok_or(ShopError::NoSession)??;

        player.add_item_or_drop(offer.stack(uuid), Prediction::ServerOnly);
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: failed to persist a shop purchase of {uuid}: {error}");
        }
        Ok(offer)
    }

    /// Returns today's shop of a hunter, rolling and saving it on first look.
    pub fn shop_day(&self, uuid: Uuid) -> Option<ShopDay> {
        let today = daily::today();
        let (rolled, day) = self.sessions.with_mut(uuid, |session| {
            (session.shop.roll_over(today), session.shop)
        })?;
        if rolled && let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: failed to persist the shop slots of {uuid}: {error}");
        }
        Some(day)
    }

    /// Sells every magic crystal in a hunter's inventory to the Organization.
    ///
    /// Crystals leave the inventory before the Won credit is queued, so a failed
    /// account write cannot pay twice; ordinary diamonds carry no rank and are
    /// therefore never bought.
    pub fn sell_crystals(&self, player: &Arc<Player>) -> Result<CrystalSale, CrystalSaleError> {
        let uuid = player.gameprofile.id();
        if !self.persistence.lock().accepts_writes(uuid) {
            return Err(CrystalSaleError::Storage);
        }

        let mut sale = CrystalSale::default();
        let mut inventory = player.inventory.lock();
        for slot in 0..inventory.get_container_size() {
            let stack = inventory.get_item(slot).clone();
            let Some(rank) = crystal::rank(&stack) else {
                continue;
            };
            let count = u32::try_from(stack.count).unwrap_or(0);
            sale.count = sale.count.saturating_add(count);
            sale.total = sale
                .total
                .saturating_add(crystal::sale_price(rank).saturating_mul(u64::from(count)));
            inventory.set_item(slot, ItemStack::empty());
        }
        drop(inventory);

        if sale.count == 0 {
            return Err(CrystalSaleError::NoCrystals);
        }
        self.credit_won(uuid, sale.total);
        Ok(sale)
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

    /// Captures tracked dungeon drops for deferred contribution-based delivery.
    pub fn capture_raid_loot(&self, world: &World, entity_id: i32, drops: &mut Vec<ItemStack>) {
        self.dungeon_runtime
            .lock()
            .capture_raid_loot(world, entity_id, drops);
    }

    /// Retains a nearby dungeon mob killed directly by a hunter.
    pub fn arise_from_player_kill(
        &self,
        hunter: &Player,
        world: &World,
        entity: &dyn LivingEntity,
    ) -> Option<Shadow> {
        let shadow = self
            .sessions
            .with_mut(hunter.gameprofile.id(), |session| {
                ShadowRuntime::arise_from_player_kill(hunter, world, entity, session)
            })??;
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
    use crate::economy::{SystemPoints, Won};
    use crate::organization::OrganizationBook;
    use crate::quest::SystemQuest;
    use crate::shadow::ShadowRoster;
    use crate::shop::ShopDay;
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
