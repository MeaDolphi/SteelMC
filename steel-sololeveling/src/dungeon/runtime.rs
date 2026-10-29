//! Live gate and instance orchestration.

use std::f64::consts::TAU;
use std::fmt::{self, Debug, Formatter};
use std::io;
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use glam::DVec3;
use rand::RngExt as _;
use rustc_hash::FxHashMap;
use steel_core::chunk::chunk_request::{
    ChunkRequest, ChunkRequestHandle, ChunkRequestState, ChunkTicketKind,
};
use steel_core::chunk::status::ChunkStatus;
use steel_core::entity::damage::DamageSource;
use steel_core::entity::{Entity as _, LivingEntity as _, RemovalReason};
use steel_core::player::Player;
use steel_core::player::connection::NetworkConnection as _;
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_damage_types;
use steel_utils::{BlockPos, Identifier};
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use super::boss::{BossAbilityAction, BossFight};
use super::instance::{
    arena_chunks, build_dungeon, build_penalty_arena, dungeon_chunks, erase_footprint,
    gate_frame_chunks, gate_ground_is_dry, gate_space_is_clear, open_dungeon_exit,
    opening_contains, place_gate_frame, remove_gate_frame, spawn_dungeon_boss, spawn_dungeon_wave,
    spawn_penalty_wave,
};
use super::mobs::MobLabel;
use super::{
    DUNGEON_TIME_LIMIT_TICKS, DungeonLayout, DungeonWave, Footprint, GATE_SPAWN_INTERVAL_TICKS,
    GATES_PER_SPAWN_CYCLE, Gate, GateRegistry, INSTANCE_WORLD_PATH, InstanceRegion,
    InstanceRegionAllocator, PENALTY_ZONE_DURATION_TICKS, PENALTY_ZONE_WAVE_INTERVAL_TICKS,
    penalty_wave_size, wave_for_gate,
};
use crate::item::chest::{self, ChestKind};
use crate::organization::Event;
use crate::party::RaidContribution;
use crate::rank::HunterRank;
use crate::state::SoloLevelingState;

const GATE_MIN_DISTANCE: f64 = 128.0;
const GATE_MIN_RADIUS: f64 = 256.0;
const GATE_MAX_RADIUS: f64 = 1_024.0;

/// Prototype switch: place one gate next to a hunter who joins the overworld,
/// so a dungeon can be reached without waiting for the automatic cycle.
///
/// Set to `false` to restore vanilla gate scheduling only.
pub const TEST_GATE_ON_JOIN: bool = true;
/// Rank of the prototype gate placed next to a joining hunter.
const TEST_GATE_RANK: HunterRank = HunterRank::D;
/// Whether the prototype gate is a red gate.
const TEST_GATE_RED: bool = false;
/// Distance from the hunter the prototype gate is placed at.
const TEST_GATE_DISTANCE: f64 = 10.0;
/// Minimum gap between the prototype gate and another gate, so repeated joins
/// cannot stack frames in the same spot.
const TEST_GATE_MIN_DISTANCE: f64 = 4.0;
/// Radius around a hunter that already holds a gate, which makes the prototype
/// gate redundant.
const TEST_GATE_REUSE_RADIUS: f64 = 64.0;
/// Sites the prototype gate placement tries before giving up.
const TEST_GATE_MAX_ATTEMPTS: u8 = 8;
/// Sites one spawn cycle may test before settling for fewer gates, so a crowded
/// world cannot keep the scheduler busy forever.
const GATE_MAX_SITE_ATTEMPTS: u8 = 12;

/// A transition requested by the runtime when a run ends.
///
/// Instance bookkeeping lives under one lock, so actions carry the work that
/// re-enters gameplay state (lethal damage, world moves, stat loss) out of it
/// and are applied by [`apply_action`] once the lock is released. That keeps
/// the lock order one-way: the runtime may read sessions, never the reverse.
#[derive(Debug, Clone)]
pub enum RunAction {
    /// Executes boss damage outside the runtime lock so death hooks can re-enter it.
    BossAbility(BossAbilityAction),
    /// Deal lethal instance damage so the hunter dies inside the dungeon.
    KillPlayer(Uuid),
    /// Send a hunter back to the ordinary world.
    ReturnPlayer {
        /// Hunter to move.
        uuid: Uuid,
        /// Registered world to move the hunter into.
        world: Identifier,
    },
    /// Deduct one randomly chosen nonzero stat after a penalty-zone death.
    PenaltyLoss {
        /// Hunter that failed the penalty zone.
        uuid: Uuid,
        /// Rolls which nonzero stat is deducted.
        roll: usize,
    },
}

/// Outcome of a hunter death observed inside an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunDeath {
    /// The hunter was not inside an instance.
    None,
    /// The dungeon run ended without clearing its rooms.
    DungeonFailed,
    /// The penalty-zone challenge was failed.
    PenaltyFailed {
        /// Rolls which nonzero stat is deducted.
        roll: usize,
    },
}

/// Runs the non-persistent lifecycle for gates, dungeon instances, and
/// penalty-zone instances.
#[derive(Default)]
pub struct DungeonRuntime {
    gates: GateRegistry,
    regions: InstanceRegionAllocator,
    gate_cycle: Option<GateCycle>,
    pending_gates: Vec<PendingGate>,
    pending_entries: FxHashMap<Uuid, PendingEntry>,
    runs: FxHashMap<Uuid, InstanceRun>,
    /// Gate frames to tear down once a [`Server`] handle is available again.
    spent_gates: Vec<u64>,
    /// Instance mobs to discard once a [`Server`] handle is available again.
    spent_mobs: Vec<(Identifier, i32)>,
    /// Loot removed from instance entities and delivered after the runtime lock.
    pending_loot: Vec<(Uuid, ItemStack)>,
    /// Hunters waiting for a prototype gate, with the attempts they spent.
    test_gates: FxHashMap<Uuid, u8>,
}

impl Debug for DungeonRuntime {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DungeonRuntime")
            .field("gates", &self.gates)
            .field("regions", &self.regions)
            .field("gate_cycle", &self.gate_cycle.is_some())
            .field("pending_gates", &self.pending_gates.len())
            .field("pending_entries", &self.pending_entries.len())
            .field("runs", &self.runs.len())
            .field("spent_gates", &self.spent_gates.len())
            .field("spent_mobs", &self.spent_mobs.len())
            .field("pending_loot", &self.pending_loot.len())
            .field("test_gates", &self.test_gates.len())
            .finish()
    }
}

struct PendingGate {
    world: Arc<World>,
    candidate: BlockPos,
    /// Requests for every chunk the frame occupies.
    requests: Vec<ChunkRequestHandle>,
    tick: u64,
    /// Set for a prototype gate placed next to a hunter instead of rolled.
    test: Option<PendingTestGate>,
}

/// State of one prototype gate placement.
struct PendingTestGate {
    /// Hunter the gate is placed next to.
    player: Uuid,
    /// Placement attempts already spent, including this one.
    attempts: u8,
}

/// Progress of the ten-minute gate-spawn cycle of the main world.
struct GateCycle {
    /// Main world the cycle places gates in.
    world: Identifier,
    last_tick: u64,
    /// Candidate sites requested so far, including rejected ones.
    sites_requested: u8,
    /// Gates that were successfully placed this cycle.
    gates_created: u8,
}

struct PendingEntry {
    players: Vec<Arc<Player>>,
    source_world: Identifier,
    instance_world: Arc<World>,
    region: InstanceRegion,
    start: RunStart,
    request: ChunkRequestHandle,
}

/// Which kind of instance has to be prepared for a pending entry.
enum RunStart {
    /// A dungeon reached through a discovered gate.
    Gate { gate: Gate, layout: DungeonLayout },
    /// A penalty-zone challenge for an unfinished system quest.
    Penalty,
}

struct InstanceRun {
    /// Members who entered this particular instance, in party join order.
    members: Vec<Uuid>,
    source_world: Identifier,
    instance_world: Identifier,
    region: InstanceRegion,
    /// Keeps the region's chunks loaded for as long as the run is live: waves
    /// spawn several rooms ahead of the hunter, past any ticket the hunter's own
    /// loading distance keeps ready. Held for its tickets only.
    #[expect(
        dead_code,
        reason = "keeps the region's chunk tickets alive until the run ends"
    )]
    chunks: ChunkRequestHandle,
    started_tick: u64,
    duration_ticks: u64,
    progress: RunProgress,
    /// Gate consumed by this run, removed from the world when the run ends.
    gate: Option<u64>,
    /// Whether the hunter was already teleported to the region spawn.
    placed: bool,
    /// Set once an action was queued so the run stops ticking before it ends.
    finishing: bool,
}

enum RunProgress {
    Dungeon(Box<DungeonProgress>),
    Penalty(PenaltyProgress),
}

struct DungeonProgress {
    rank: HunterRank,
    /// Whether the gate was a red gate; scales waves, boss, and reward.
    red: bool,
    /// Wave parameters derived from `rank` and `red` once, at entry.
    wave: DungeonWave,
    layout: DungeonLayout,
    /// Combat room currently in progress; zero until the hunter is placed.
    combat_room: u8,
    remaining_mobs: Vec<i32>,
    /// Lifecycle of the boss guarding the final room.
    boss: BossState,
    /// Ability cooldowns and boss-bar state of a boss that already appeared.
    fight: Option<BossFight>,
    /// Exit opened in the boss room once the boss is defeated.
    exit: Option<BlockPos>,
    /// Damage and mob-clear credit used to divide the cleared-gate Won reward.
    contributions: FxHashMap<Uuid, RaidContribution>,
    /// Chest type rolled once for the squad when the boss died, handed to every
    /// survivor of the completion.
    boss_chest: Option<ChestKind>,
    /// Survivors captured at completion, in dungeon-entry order.
    survivors: Vec<Uuid>,
    /// Nameplates of the run's mobs and boss, by entity id.
    labels: FxHashMap<i32, MobLabel>,
}

impl DungeonProgress {
    /// Spawns the wave of the current combat room and remembers its plates.
    fn spawn_wave(&mut self, world: &Arc<World>, region: InstanceRegion) {
        let spawned = spawn_dungeon_wave(
            world,
            region,
            &self.layout,
            self.wave,
            self.rank,
            self.combat_room,
        );
        self.remaining_mobs = spawned.iter().map(|(id, _)| *id).collect();
        self.labels.extend(spawned);
    }
}

/// Lifecycle of the boss of one dungeon run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BossState {
    /// Every combat room is not cleared yet, so the boss room is empty.
    Waiting,
    /// The boss is alive with this entity id.
    Alive(i32),
    /// The boss was defeated, which opens the exit.
    Defeated,
}

struct PenaltyProgress {
    next_wave_tick: u64,
    remaining_mobs: Vec<i32>,
}

impl InstanceRun {
    /// Removes the run's live mob ids, so their entities can be discarded.
    fn take_mobs(&mut self) -> Vec<i32> {
        match &mut self.progress {
            RunProgress::Dungeon(progress) => {
                let mut mobs = mem::take(&mut progress.remaining_mobs);
                if let BossState::Alive(id) = progress.boss {
                    mobs.push(id);
                }
                mobs
            }
            RunProgress::Penalty(progress) => mem::take(&mut progress.remaining_mobs),
        }
    }

    /// Removes the boss bar the run showed, if its boss ever appeared.
    ///
    /// The client keeps a boss bar until it is removed, even across the death
    /// screen, so it outlives the run otherwise.
    fn hide_boss_bar(&self, player: &Player) {
        if let RunProgress::Dungeon(progress) = &self.progress
            && let Some(fight) = &progress.fight
        {
            fight.abandon(player);
        }
    }
}

/// The hunter and the instance one dungeon run advances.
///
/// Bundled so the run step does not take the same six values as separate
/// parameters.
struct RunContext<'a> {
    state: &'a SoloLevelingState,
    world: &'a Arc<World>,
    player: &'a Player,
    uuid: Uuid,
    members: &'a [Uuid],
    region: InstanceRegion,
    source_world: &'a Identifier,
}

impl DungeonRuntime {
    /// Loads what each instance region holds, saving later builds to `path`.
    pub fn load_region_records(&mut self, path: PathBuf) -> io::Result<()> {
        self.regions.load_footprints(path)
    }

    /// Advances gate scheduling, chunk preparation, entry, and active runs.
    ///
    /// Returns the run transitions that have to be applied outside the runtime
    /// lock.
    pub fn tick(
        &mut self,
        state: &SoloLevelingState,
        server: &Arc<Server>,
        tick: u64,
    ) -> Vec<RunAction> {
        self.flush_spent(server);
        self.expire_gates(server, tick);
        self.schedule_gates(server, tick);
        self.schedule_test_gates(server, tick);
        self.finish_gate_requests(server);
        self.find_gate_entries(state, server);
        self.find_penalty_entries(state, server);
        self.finish_entries(state, server, tick);

        let mut actions = self.tick_runs(state, server, tick);
        actions.extend(self.return_strays(server));
        actions
    }

    /// Returns the effective rank of the dungeon run inside `world`.
    ///
    /// Red gates store their already promoted rank, so this is the rank the run
    /// is fought and rewarded at.
    #[must_use]
    pub fn instance_rank(&self, world: &World) -> Option<HunterRank> {
        self.runs.values().find_map(|run| {
            if run.instance_world != world.key {
                return None;
            }
            match &run.progress {
                RunProgress::Dungeon(progress) => Some(progress.rank),
                RunProgress::Penalty(_) => None,
            }
        })
    }

    /// Marks an instance mob dead and opens the next combat room when needed.
    pub fn on_living_death(&mut self, world: &World, entity_id: i32) {
        for run in self.runs.values_mut() {
            if run.instance_world != world.key {
                continue;
            }
            if let RunProgress::Dungeon(progress) = &mut run.progress
                && progress.boss == BossState::Alive(entity_id)
            {
                progress.boss = BossState::Defeated;
                return;
            }
            let remaining = match &mut run.progress {
                RunProgress::Dungeon(progress) => &mut progress.remaining_mobs,
                RunProgress::Penalty(progress) => &mut progress.remaining_mobs,
            };
            if let Some(index) = remaining.iter().position(|id| *id == entity_id) {
                remaining.swap_remove(index);
                return;
            }
        }
    }

    /// Returns the nameplate of a dungeon mob or boss in `world`.
    #[must_use]
    pub fn mob_label(&self, world: &World, entity_id: i32) -> Option<MobLabel> {
        self.runs.values().find_map(|run| {
            if run.instance_world != world.key {
                return None;
            }
            let RunProgress::Dungeon(progress) = &run.progress else {
                return None;
            };
            progress.labels.get(&entity_id).cloned()
        })
    }

    /// Returns whether an entity belongs to an active dungeon, rather than a
    /// penalty-zone run sharing the same instance world.
    #[must_use]
    pub fn is_active_dungeon_mob(&self, world: &World, entity_id: i32) -> bool {
        self.runs.values().any(|run| {
            run.instance_world == world.key
                && matches!(
                    &run.progress,
                    RunProgress::Dungeon(progress)
                        if progress.boss == BossState::Alive(entity_id)
                            || progress.remaining_mobs.contains(&entity_id)
                )
        })
    }

    /// Resolves an ordinary kill's effective rank only for a participant of that run.
    /// Boss kills are settled separately when the surviving party clears the gate.
    #[must_use]
    pub fn ordinary_mob_rank(
        &self,
        world: &World,
        entity_id: i32,
        hunter: Uuid,
    ) -> Option<HunterRank> {
        self.runs.values().find_map(|run| {
            if run.instance_world != world.key || !run.members.contains(&hunter) {
                return None;
            }
            let RunProgress::Dungeon(progress) = &run.progress else {
                return None;
            };
            progress
                .remaining_mobs
                .contains(&entity_id)
                .then_some(progress.rank)
        })
    }

    /// Credits a participating hunter for damage that reduced a dungeon boss's health.
    pub fn record_boss_damage(&mut self, world: &World, entity_id: i32, hunter: Uuid, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        for run in self.runs.values_mut() {
            if run.instance_world != world.key || !run.members.contains(&hunter) {
                continue;
            }
            let RunProgress::Dungeon(progress) = &mut run.progress else {
                continue;
            };
            if progress.boss == BossState::Alive(entity_id) {
                let contribution = progress.contributions.entry(hunter).or_default();
                contribution.boss_damage = contribution.boss_damage.saturating_add(amount as u64);
                return;
            }
        }
    }

    /// Credits a participating hunter for clearing an ordinary dungeon mob.
    pub fn record_mob_kill(&mut self, world: &World, entity_id: i32, hunter: Uuid) {
        for run in self.runs.values_mut() {
            if run.instance_world != world.key || !run.members.contains(&hunter) {
                continue;
            }
            let RunProgress::Dungeon(progress) = &mut run.progress else {
                continue;
            };
            if progress.remaining_mobs.contains(&entity_id) {
                let contribution = progress.contributions.entry(hunter).or_default();
                contribution.mob_kills = contribution.mob_kills.saturating_add(1);
                return;
            }
        }
    }

    /// Removes tracked dungeon drops and queues their contribution-based delivery.
    pub fn capture_raid_loot(&mut self, world: &World, entity_id: i32, drops: &mut Vec<ItemStack>) {
        for run in self.runs.values_mut() {
            if run.instance_world != world.key {
                continue;
            }
            let RunProgress::Dungeon(progress) = &run.progress else {
                continue;
            };
            if progress.boss != BossState::Alive(entity_id)
                && !progress.remaining_mobs.contains(&entity_id)
            {
                continue;
            }
            let total = run
                .members
                .iter()
                .map(|member| {
                    progress
                        .contributions
                        .get(member)
                        .copied()
                        .unwrap_or_default()
                        .score()
                })
                .sum::<u64>();
            for stack in mem::take(drops) {
                let count = stack.count().max(0) as u64;
                let mut allocated = 0_u64;
                for (index, member) in run.members.iter().enumerate() {
                    let score = progress
                        .contributions
                        .get(member)
                        .copied()
                        .unwrap_or_default()
                        .score();
                    let share = count
                        .saturating_mul(score)
                        .checked_div(total)
                        .unwrap_or_else(|| even_share(count, run.members.len(), index));
                    allocated = allocated.saturating_add(share);
                    let Ok(share) = i32::try_from(share) else {
                        continue;
                    };
                    if share == 0 {
                        continue;
                    }
                    let mut stack = stack.clone();
                    stack.set_count(share);
                    self.pending_loot.push((*member, stack));
                }
                if total > 0
                    && let Some(member) = run.members.iter().max_by_key(|member| {
                        progress
                            .contributions
                            .get(member)
                            .copied()
                            .unwrap_or_default()
                            .score()
                    })
                    && let Ok(remainder) = i32::try_from(count.saturating_sub(allocated))
                    && remainder > 0
                {
                    let mut remainder_stack = stack;
                    remainder_stack.set_count(remainder);
                    self.pending_loot.push((*member, remainder_stack));
                }
            }
            return;
        }
    }

    /// Drains loot that must be delivered outside the runtime lock.
    pub fn take_pending_loot(&mut self) -> Vec<(Uuid, ItemStack)> {
        mem::take(&mut self.pending_loot)
    }

    /// Removes a dead hunter from a run, releasing its region and gate only
    /// after the last participant has died.
    pub fn on_player_death(&mut self, player: &Player) -> RunDeath {
        let uuid = player.gameprofile.id();
        self.discard_pending_entry(uuid);
        let Some(run_key) = self
            .runs
            .iter()
            .find_map(|(key, run)| run.members.contains(&uuid).then_some(*key))
        else {
            return RunDeath::None;
        };
        let Some(run) = self.runs.get_mut(&run_key) else {
            return RunDeath::None;
        };
        run.hide_boss_bar(player);
        run.members.retain(|member| *member != uuid);
        let death = match &run.progress {
            RunProgress::Dungeon(_) => RunDeath::DungeonFailed,
            RunProgress::Penalty(_) => RunDeath::PenaltyFailed {
                roll: rand::rng().random_range(0..5),
            },
        };
        if !run.members.is_empty() {
            log::debug!(
                "sololeveling: hunter {uuid} died inside an instance; {} participant(s) remain",
                run.members.len()
            );
            return death;
        }
        let Some(run) = self.runs.remove(&run_key) else {
            return RunDeath::None;
        };
        log::debug!("sololeveling: last hunter {uuid} died inside an instance; {death:?}");
        self.release_run(run);
        death
    }

    /// Drops any run or pending entry owned by a disconnecting hunter.
    ///
    /// The connection is already gone, so the run's boss bar needs no removal.
    pub fn on_player_disconnect(&mut self, server: &Server, player_uuid: Uuid) {
        self.discard_pending_entry(player_uuid);
        self.complete_offline_run(player_uuid);
        self.flush_spent(server);
    }

    /// Releases a run whose end action was already applied.
    pub fn complete_run(&mut self, player: &Player) {
        let player_uuid = player.gameprofile.id();
        let Some(run_key) = self
            .runs
            .iter()
            .find_map(|(key, run)| run.members.contains(&player_uuid).then_some(*key))
        else {
            return;
        };
        let Some(run) = self.runs.remove(&run_key) else {
            return;
        };
        run.hide_boss_bar(player);
        self.release_run(run);
    }

    /// Releases the run of a hunter that is no longer online, whose client can
    /// no longer receive a boss bar removal.
    pub fn complete_offline_run(&mut self, player_uuid: Uuid) {
        let Some(run_key) = self
            .runs
            .iter()
            .find_map(|(key, run)| run.members.contains(&player_uuid).then_some(*key))
        else {
            return;
        };
        let Some(run) = self.runs.remove(&run_key) else {
            return;
        };
        self.release_run(run);
    }

    /// Frees the region, gate, and mobs a finished run held.
    fn release_run(&mut self, mut run: InstanceRun) {
        self.regions.release(run.region);
        self.spend_gate(run.gate);
        self.spend_mobs(&mut run);
    }

    fn discard_pending_entry(&mut self, player_uuid: Uuid) -> bool {
        let Some(entry) = self.pending_entries.remove(&player_uuid) else {
            return false;
        };
        self.regions.release(entry.region);
        true
    }

    fn spend_gate(&mut self, gate: Option<u64>) {
        if let Some(gate) = gate {
            self.spent_gates.push(gate);
        }
    }

    fn spend_mobs(&mut self, run: &mut InstanceRun) {
        for mob in run.take_mobs() {
            self.spent_mobs.push((run.instance_world.clone(), mob));
        }
    }

    /// Tears down what finished runs leave behind. Runs also end where no
    /// [`Server`] handle is reachable (death hooks, disconnects), so their
    /// cleanup is queued and drained here.
    fn flush_spent(&mut self, server: &Server) {
        for id in mem::take(&mut self.spent_gates) {
            let Some(gate) = self.gates.remove(id) else {
                continue;
            };
            if let Some(world) = server.worlds.get(&gate.world) {
                remove_gate_frame(world, &gate);
            }
        }
        for (world_key, entity_id) in mem::take(&mut self.spent_mobs) {
            let Some(world) = server.worlds.get(&world_key) else {
                continue;
            };
            let Some(entity) = world.get_entity_by_id(entity_id) else {
                continue;
            };
            entity.set_removed(RemovalReason::Discarded);
        }
    }

    fn expire_gates(&mut self, server: &Server, tick: u64) {
        for gate in self.gates.expire_unused(tick) {
            if let Some(world) = server.worlds.get(&gate.world) {
                remove_gate_frame(world, &gate);
            }
        }
    }

    /// Restarts the gate cycle of the main world when its interval elapsed.
    ///
    /// Gates are limited to the main world on purpose: a dungeon gate in another
    /// dimension or in the shared instance world would be unreachable or
    /// confusing, and the automatic cycle only exists to seed the overworld.
    fn schedule_gates(&mut self, server: &Arc<Server>, tick: u64) {
        if tick == 0 {
            return;
        }
        let Some(world) = main_world(server) else {
            return;
        };
        let started = self.gate_cycle.as_ref().is_none_or(|cycle| {
            cycle.world != world.key
                || tick.saturating_sub(cycle.last_tick) >= GATE_SPAWN_INTERVAL_TICKS
        });
        if started {
            self.gate_cycle = Some(GateCycle {
                world: world.key.clone(),
                last_tick: tick,
                sites_requested: 0,
                gates_created: 0,
            });
        }
        self.request_gate_site(server);
    }

    /// Keeps one candidate site in flight until the cycle places all of its
    /// gates or runs out of attempts.
    ///
    /// Rejected sites are routine: a hunter or an existing gate is often closer
    /// than [`GATE_MIN_DISTANCE`], or the surface is not buildable. Retrying
    /// keeps a cycle from ending with fewer gates than the world should have.
    fn request_gate_site(&mut self, server: &Server) {
        let Some(cycle) = &self.gate_cycle else {
            return;
        };
        let world_key = cycle.world.clone();
        let cycle_tick = cycle.last_tick;
        if cycle.gates_created >= GATES_PER_SPAWN_CYCLE
            || cycle.sites_requested >= GATE_MAX_SITE_ATTEMPTS
        {
            return;
        }
        if self
            .pending_gates
            .iter()
            .any(|pending| pending.world.key == world_key)
        {
            return;
        }
        let Some(world) = server.worlds.get(&world_key) else {
            return;
        };
        let spawn = world.level_data.read().data().spawn_pos();
        let candidate = random_gate_candidate(spawn);
        let requests = gate_frame_chunks(candidate)
            .into_iter()
            .map(|chunk| {
                world
                    .chunk_map
                    .request_chunk(chunk, ChunkStatus::Full, ChunkTicketKind::Command)
            })
            .collect();
        if let Some(cycle) = &mut self.gate_cycle {
            cycle.sites_requested += 1;
        }
        self.pending_gates.push(PendingGate {
            world: Arc::clone(world),
            candidate,
            requests,
            tick: cycle_tick,
            test: None,
        });
    }

    /// Asks for a prototype gate next to one hunter.
    ///
    /// The request is served on a later tick, once the chunks around the
    /// hunter's position are loaded, so it works from the join hook where no
    /// [`Server`] tick is available yet.
    pub fn request_test_gate(&mut self, player: Uuid) {
        self.test_gates.insert(player, 0);
    }

    /// Starts the prototype gate placements the join hook asked for.
    fn schedule_test_gates(&mut self, server: &Arc<Server>, tick: u64) {
        if self.test_gates.is_empty() {
            return;
        }
        let requests = mem::take(&mut self.test_gates);
        for (uuid, attempts) in requests {
            let Some(player) = find_player(server, uuid) else {
                continue;
            };
            let world = player.get_world();
            if main_world(server).is_none_or(|main| main.key != world.key) {
                log::debug!("sololeveling: hunter {uuid} is not in the main world; no test gate");
                continue;
            }
            if self.gates_near(&world.key, player.position(), TEST_GATE_REUSE_RADIUS) {
                log::debug!("sololeveling: hunter {uuid} already stands next to a gate");
                continue;
            }
            let candidate = test_gate_candidate(player.position());
            let chunk_requests = gate_frame_chunks(candidate)
                .into_iter()
                .map(|chunk| {
                    world.chunk_map.request_chunk(
                        chunk,
                        ChunkStatus::Full,
                        ChunkTicketKind::Command,
                    )
                })
                .collect();
            self.pending_gates.push(PendingGate {
                world: Arc::clone(&world),
                candidate,
                requests: chunk_requests,
                tick,
                test: Some(PendingTestGate {
                    player: uuid,
                    attempts: attempts.saturating_add(1),
                }),
            });
        }
    }

    /// Returns whether a live gate stands within `radius` of `position`, measured
    /// horizontally.
    fn gates_near(&self, world: &Identifier, position: DVec3, radius: f64) -> bool {
        let column = BlockPos::new(position.x.floor() as i32, 0, position.z.floor() as i32);
        self.gates_too_close_within(world, column, radius)
    }

    fn finish_gate_requests(&mut self, server: &Server) {
        let mut index = 0;
        while index < self.pending_gates.len() {
            if self.pending_gates[index]
                .requests
                .iter()
                .any(|request| request.poll() != ChunkRequestState::Ready)
            {
                index += 1;
                continue;
            }
            let pending = self.pending_gates.swap_remove(index);
            let Some(surface) = pending
                .world
                .safe_surface_position(pending.candidate.x(), pending.candidate.z())
            else {
                self.retry_test_gate(&pending);
                log::debug!(
                    "sololeveling: gate candidate {:?} in {} has no dry surface",
                    pending.candidate,
                    pending.world.key
                );
                continue;
            };
            // A prototype gate stands next to its hunter on purpose, so only the
            // automatic cycle keeps its distance from players.
            let min_distance = if pending.test.is_some() {
                TEST_GATE_MIN_DISTANCE
            } else {
                GATE_MIN_DISTANCE
            };
            let crowded =
                pending.test.is_none() && players_too_close(&pending.world, surface, server);
            if self.gates_too_close_within(&pending.world.key, surface, min_distance)
                || crowded
                || !gate_space_is_clear(&pending.world, surface)
                || !gate_ground_is_dry(&pending.world, surface)
            {
                self.retry_test_gate(&pending);
                log::debug!(
                    "sololeveling: rejected gate site {surface:?} in {} (occupied, crowded, or too wet)",
                    pending.world.key
                );
                continue;
            }
            let test_gate = pending.test.is_some();
            let gate = if test_gate {
                self.gates.create_ranked(
                    pending.world.key.clone(),
                    surface,
                    pending.tick,
                    TEST_GATE_RANK,
                    TEST_GATE_RED,
                )
            } else {
                let mut rng = rand::rng();
                self.gates.create(
                    pending.world.key.clone(),
                    surface,
                    pending.tick,
                    rng.random_range(0..100),
                    rng.random_range(0..100),
                )
            };
            place_gate_frame(&pending.world, surface, gate.red);
            if !test_gate
                && let Some(cycle) = &mut self.gate_cycle
                && cycle.world == pending.world.key
            {
                cycle.gates_created += 1;
            }
            log::info!(
                "sololeveling: created {}-rank{} gate {} in {} at {:?}{}",
                gate.rank.name(),
                if gate.red { " red" } else { "" },
                gate.id,
                gate.world,
                surface,
                if test_gate { " (test gate)" } else { "" }
            );
            if test_gate
                && let Some(test) = &pending.test
                && let Some(player) = find_player(server, test.player)
            {
                player.send_message(
                    &TextComponent::plain(format!(
                        "Test gate: {}-rank at {}, {}, {}",
                        gate.rank.name(),
                        surface.x(),
                        surface.y(),
                        surface.z()
                    ))
                    .color(Color::Aqua),
                );
            }
        }
    }

    /// Re-queues a prototype gate whose site was rejected, so the next attempt
    /// rolls a different column around its hunter.
    fn retry_test_gate(&mut self, pending: &PendingGate) {
        let Some(test) = &pending.test else {
            return;
        };
        if test.attempts >= TEST_GATE_MAX_ATTEMPTS {
            log::warn!(
                "sololeveling: no free site for the test gate next to hunter {} after {} attempts",
                test.player,
                test.attempts
            );
            return;
        }
        self.test_gates.insert(test.player, test.attempts);
    }

    fn gates_too_close_within(&self, world: &Identifier, position: BlockPos, radius: f64) -> bool {
        self.gates.values().any(|gate| {
            gate.world == *world && squared_distance(gate.entrance, position) < radius.powi(2)
        })
    }

    fn find_gate_entries(&mut self, state: &SoloLevelingState, server: &Arc<Server>) {
        for player in server.get_players() {
            let uuid = player.gameprofile.id();
            if self.is_busy(uuid) || state.has_pending_penalty(uuid) {
                continue;
            }
            let world = player.get_world();
            let party = state.party_of(uuid);
            if party.as_ref().is_some_and(|party| party.leader() != uuid) {
                continue;
            }
            let Some(rank) = state.hunter_rank(uuid) else {
                continue;
            };
            let Some(gate) = self.gates.enter_at(&world.key, rank, player.position()) else {
                continue;
            };
            let Some(instance_world) = instance_world_of(server, &world) else {
                log::error!(
                    "sololeveling: instance world is not loaded for {}",
                    world.key
                );
                continue;
            };
            let start = RunStart::Gate {
                gate: gate.clone(),
                layout: DungeonLayout::from_seed(rand::rng().random()),
            };
            log::info!(
                "sololeveling: hunter {} entered {}-rank gate {}",
                uuid,
                gate.rank.name(),
                gate.id
            );
            let participants = party.map_or_else(
                || vec![Arc::clone(&player)],
                |party| {
                    party
                        .members()
                        .iter()
                        .filter_map(|member| find_player(server, *member))
                        .filter(|member| {
                            member.get_world().key == world.key
                                && member.position().distance_squared(player.position())
                                    <= 100.0_f64.powi(2)
                                && !self.is_busy(member.gameprofile.id())
                        })
                        .collect()
                },
            );
            self.begin_entry(participants, world.key.clone(), instance_world, start);
        }
    }

    fn find_penalty_entries(&mut self, state: &SoloLevelingState, server: &Arc<Server>) {
        for player in server.get_players() {
            let uuid = player.gameprofile.id();
            if self.is_busy(uuid) || !state.penalty_due(uuid) {
                continue;
            }
            let world = player.get_world();
            if world.key.path.as_ref() == INSTANCE_WORLD_PATH {
                continue;
            }
            let Some(instance_world) = instance_world_of(server, &world) else {
                log::error!(
                    "sololeveling: instance world is not loaded for {}",
                    world.key
                );
                continue;
            };
            log::info!("sololeveling: hunter {uuid} was sent to the penalty zone");
            self.begin_entry(
                vec![player],
                world.key.clone(),
                instance_world,
                RunStart::Penalty,
            );
        }
    }

    fn is_busy(&self, player_uuid: Uuid) -> bool {
        self.pending_entries.values().any(|entry| {
            entry
                .players
                .iter()
                .any(|player| player.gameprofile.id() == player_uuid)
        }) || self
            .runs
            .values()
            .any(|run| run.members.contains(&player_uuid))
    }

    fn begin_entry(
        &mut self,
        players: Vec<Arc<Player>>,
        source_world: Identifier,
        instance_world: Arc<World>,
        start: RunStart,
    ) {
        let region = self.regions.allocate();
        // The whole build, and the erasure of what the region held before, has
        // to be loaded before the first block is written.
        let previous = self.regions.footprint(region);
        let positions = match &start {
            RunStart::Gate { layout, .. } => dungeon_chunks(region, layout, previous),
            RunStart::Penalty => arena_chunks(region, previous),
        };
        let request = instance_world.chunk_map.request_chunks(ChunkRequest {
            status: ChunkStatus::Full,
            positions,
            ticket_kind: ChunkTicketKind::Portal,
        });
        let Some(leader) = players.first().map(|player| player.gameprofile.id()) else {
            return;
        };
        self.pending_entries.insert(
            leader,
            PendingEntry {
                players,
                source_world,
                instance_world,
                region,
                start,
                request,
            },
        );
    }

    #[expect(
        clippy::too_many_lines,
        reason = "entering a run stays one reviewable sequence: cancel, wait, then admit"
    )]
    fn finish_entries(&mut self, state: &SoloLevelingState, server: &Arc<Server>, tick: u64) {
        let mut cancelled = Vec::new();
        let ready = self
            .pending_entries
            .iter()
            .filter_map(|(uuid, entry)| match entry.request.poll() {
                ChunkRequestState::Ready => Some(*uuid),
                ChunkRequestState::Cancelled => {
                    cancelled.push(*uuid);
                    None
                }
                ChunkRequestState::Pending { .. } => None,
            })
            .collect::<Vec<_>>();
        for uuid in cancelled {
            self.discard_pending_entry(uuid);
        }

        for uuid in ready {
            let Some(entry) = self.pending_entries.remove(&uuid) else {
                continue;
            };
            let PendingEntry {
                players,
                source_world,
                instance_world,
                region,
                start,
                request,
            } = entry;
            let duration_ticks = match &start {
                RunStart::Gate { .. } => DUNGEON_TIME_LIMIT_TICKS,
                RunStart::Penalty => {
                    if state.claim_penalty(uuid).is_none() {
                        self.regions.release(region);
                        continue;
                    }
                    PENALTY_ZONE_DURATION_TICKS
                }
            };
            erase_footprint(&instance_world, region, self.regions.footprint(region));
            let footprint = match &start {
                RunStart::Gate { layout, .. } => {
                    build_dungeon(&instance_world, region, layout);
                    log::info!(
                        "sololeveling: built a {} dungeon (seed {}) in instance region {}",
                        layout.blueprint.theme.display_name(),
                        layout.seed,
                        region.slot
                    );
                    Footprint::Dungeon { seed: layout.seed }
                }
                RunStart::Penalty => {
                    build_penalty_arena(&instance_world, region);
                    Footprint::Arena
                }
            };
            self.regions.record(region, footprint);
            let mut members = Vec::with_capacity(players.len());
            for player in players {
                let player_uuid = player.gameprofile.id();
                if let Err(error) =
                    server.queue_player_world_selection(Arc::clone(&player), &instance_world)
                {
                    log::warn!(
                        "sololeveling: unable to move hunter {player_uuid} into an instance: {error}"
                    );
                    continue;
                }
                members.push(player_uuid);
            }
            if members.is_empty() {
                self.regions.release(region);
                continue;
            }
            let (progress, gate) = match start {
                RunStart::Gate { gate, layout } => (
                    RunProgress::Dungeon(Box::new(DungeonProgress {
                        rank: gate.rank,
                        red: gate.red,
                        wave: wave_for_gate(gate.rank, gate.red),
                        layout,
                        combat_room: 0,
                        remaining_mobs: Vec::new(),
                        boss: BossState::Waiting,
                        fight: None,
                        exit: None,
                        contributions: members
                            .iter()
                            .copied()
                            .map(|member| (member, RaidContribution::default()))
                            .collect(),
                        boss_chest: None,
                        survivors: Vec::new(),
                        labels: FxHashMap::default(),
                    })),
                    Some(gate.id),
                ),
                RunStart::Penalty => (
                    RunProgress::Penalty(PenaltyProgress {
                        next_wave_tick: tick + PENALTY_ZONE_WAVE_INTERVAL_TICKS,
                        remaining_mobs: Vec::new(),
                    }),
                    None,
                ),
            };
            self.runs.insert(
                uuid,
                InstanceRun {
                    members,
                    source_world,
                    instance_world: instance_world.key.clone(),
                    region,
                    chunks: request,
                    started_tick: tick,
                    duration_ticks,
                    progress,
                    gate,
                    placed: false,
                    finishing: false,
                },
            );
        }
    }

    /// Advances one dungeon run: waves, then the boss, then the exit.
    ///
    /// Returns the action that ends the run, or `None` while the fight goes on.
    ///
    /// A room counts as cleared when its mobs are gone, not when they die: a mob
    /// can be removed without a death event, so the live entities are what the
    /// room is waiting for.
    fn tick_dungeon_run(
        progress: &mut DungeonProgress,
        context: &RunContext<'_>,
        tick: u64,
    ) -> Option<RunAction> {
        let RunContext {
            state,
            world,
            player,
            uuid,
            members,
            region,
            source_world,
        } = *context;
        if progress.combat_room == 0 {
            progress.combat_room = 1;
            progress.spawn_wave(world, region);
            return None;
        }
        progress
            .remaining_mobs
            .retain(|id| world.get_entity_by_id(*id).is_some());
        if !progress.remaining_mobs.is_empty() {
            return None;
        }
        if progress.combat_room < progress.layout.combat_room_count {
            progress.combat_room += 1;
            progress.spawn_wave(world, region);
            return None;
        }
        match progress.boss {
            BossState::Waiting => {
                let Some(boss) = spawn_dungeon_boss(
                    world,
                    region,
                    &progress.layout,
                    progress.rank,
                    progress.red,
                ) else {
                    // Without a boss the run can never be cleared, so the hunter is
                    // sent home instead of waiting for the timer.
                    log::error!(
                        "sololeveling: the boss of the {}-rank dungeon of hunter {uuid} could not be spawned",
                        progress.rank.name()
                    );
                    return Some(RunAction::ReturnPlayer {
                        uuid,
                        world: source_world.clone(),
                    });
                };
                progress.boss = BossState::Alive(boss.id);
                progress.labels.insert(boss.id, boss.label.clone());
                let mut fight = BossFight::new(&boss.boss, boss.id, tick);
                if let Some(entity) = world.get_entity_by_id(boss.id) {
                    fight.announce(&entity, player, &boss.name);
                }
                progress.fight = Some(fight);
                log::info!(
                    "sololeveling: the boss of the {}-rank dungeon of hunter {uuid} appeared",
                    progress.rank.name()
                );
                return None;
            }
            BossState::Alive(_) => {
                return progress
                    .fight
                    .as_mut()
                    .and_then(|fight| fight.tick(world, player, members, tick))
                    .map(RunAction::BossAbility);
            }
            BossState::Defeated => {
                if let Some(fight) = progress.fight.take() {
                    fight.finish(player);
                }
            }
        }
        if progress.exit.is_none() {
            progress.exit = Some(open_dungeon_exit(world, region, &progress.layout));
            log::info!(
                "sololeveling: hunter {uuid} defeated the boss of a {}-rank dungeon; the exit is open",
                progress.rank.name()
            );
            return None;
        }
        let exit = progress.exit?;
        if !opening_contains(exit, player.position()) {
            return None;
        }
        let survivors: Vec<Uuid> = members
            .iter()
            .filter_map(|member| {
                let player = world.players.get_by_uuid(member)?;
                (!player.is_dead_or_dying() && !player.connection.closed()).then_some(*member)
            })
            .collect();
        state.credit_dungeon_boss(survivors.clone(), progress.rank);
        state.record_organization_event(survivors.clone(), Event::Boss);
        state.record_organization_event(survivors.clone(), Event::Dungeon(progress.rank));
        // One roll decides whether the squad gets a chest at all; every survivor
        // then receives the same kind, as the boss reward table specifies.
        progress.boss_chest = chest::boss_chest_for(progress.rank, rand::random_range(0..100));
        progress.survivors = survivors;
        Some(RunAction::ReturnPlayer {
            uuid,
            world: source_world.clone(),
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "every per-tick run transition is easier to compare side by side"
    )]
    fn tick_runs(
        &mut self,
        state: &SoloLevelingState,
        server: &Arc<Server>,
        tick: u64,
    ) -> Vec<RunAction> {
        let players = server
            .get_players()
            .into_iter()
            .map(|player| (player.gameprofile.id(), player))
            .collect::<FxHashMap<_, _>>();
        let mut actions = Vec::new();
        for run in self.runs.values_mut() {
            if run.finishing {
                continue;
            }
            let Some((active_uuid, player)) = run.members.iter().find_map(|member| {
                let player = players.get(member)?;
                (player.get_world().key == run.instance_world).then_some((*member, player))
            }) else {
                continue;
            };
            let uuid = active_uuid;
            let region = run.region;
            let source_world = run.source_world.clone();
            let members = run.members.clone();
            if !run.placed {
                run.placed = true;
                let theme = match &run.progress {
                    RunProgress::Dungeon(progress) => {
                        Some(progress.layout.blueprint.theme.display_name())
                    }
                    RunProgress::Penalty(_) => None,
                };
                for member in &run.members {
                    if let Some(member) = players.get(member) {
                        let _ = member.teleport(region.spawn, 0.0, 0.0);
                        if let Some(theme) = theme {
                            member.send_message(
                                &TextComponent::plain(format!("Подземелье: {theme}."))
                                    .color(Color::DarkPurple),
                            );
                        }
                    }
                }
            }
            if tick.saturating_sub(run.started_tick) >= run.duration_ticks {
                run.finishing = true;
                match run.progress {
                    RunProgress::Dungeon(_) => {
                        actions.extend(members.iter().copied().map(RunAction::KillPlayer));
                    }
                    RunProgress::Penalty(_) => {
                        actions.extend(members.iter().copied().map(|uuid| {
                            RunAction::ReturnPlayer {
                                uuid,
                                world: source_world.clone(),
                            }
                        }));
                    }
                }
                continue;
            }

            let world = player.get_world();
            match &mut run.progress {
                RunProgress::Dungeon(progress) => {
                    let context = RunContext {
                        state,
                        world: &world,
                        player,
                        uuid,
                        members: &run.members,
                        region,
                        source_world: &source_world,
                    };
                    let action = Self::tick_dungeon_run(progress, &context, tick);
                    if let Some(kind) = progress.boss_chest.take() {
                        for survivor in progress.survivors.drain(..) {
                            self.pending_loot.push((survivor, kind.stack()));
                        }
                    }
                    if let Some(action) = action {
                        if matches!(action, RunAction::BossAbility(_)) {
                            actions.push(action);
                            continue;
                        }
                        run.finishing = true;
                        match action {
                            RunAction::ReturnPlayer { world, .. } => {
                                actions.extend(members.into_iter().map(|uuid| {
                                    RunAction::ReturnPlayer {
                                        uuid,
                                        world: world.clone(),
                                    }
                                }));
                            }
                            action => actions.push(action),
                        }
                    }
                }
                RunProgress::Penalty(progress) => {
                    if tick < progress.next_wave_tick {
                        continue;
                    }
                    progress.next_wave_tick = tick + PENALTY_ZONE_WAVE_INTERVAL_TICKS;
                    progress
                        .remaining_mobs
                        .retain(|id| world.get_entity_by_id(*id).is_some());
                    let alive = u8::try_from(progress.remaining_mobs.len()).unwrap_or(u8::MAX);
                    let count = penalty_wave_size(alive);
                    if count > 0 {
                        progress
                            .remaining_mobs
                            .extend(spawn_penalty_wave(&world, region, count));
                    }
                }
            }
        }
        actions
    }

    /// Returns hunters left inside the instance world without a live run,
    /// which happens after a disconnect or a stale run from an earlier boot.
    fn return_strays(&mut self, server: &Arc<Server>) -> Vec<RunAction> {
        let mut actions = Vec::new();
        for player in server.get_players() {
            let uuid = player.gameprofile.id();
            if self.is_busy(uuid) || player.is_world_change_pending() {
                continue;
            }
            let world = player.get_world();
            if world.key.path.as_ref() != INSTANCE_WORLD_PATH {
                continue;
            }
            let Some(default_world) = server.worlds.default_world(world.domain()) else {
                log::error!(
                    "sololeveling: domain {} has no default world to return a hunter to",
                    world.domain()
                );
                continue;
            };
            actions.push(RunAction::ReturnPlayer {
                uuid,
                world: default_world.key.clone(),
            });
        }
        actions
    }
}

/// Applies one deferred run action after the runtime lock was released.
pub fn apply_action(state: &SoloLevelingState, server: &Arc<Server>, action: &RunAction) {
    match action {
        RunAction::BossAbility(action) => action.apply(server),
        RunAction::KillPlayer(uuid) => {
            let Some(player) = find_player(server, *uuid) else {
                state.complete_offline_run(*uuid);
                return;
            };
            let world = player.get_world();
            player.hurt(
                &world,
                &DamageSource::environment(&vanilla_damage_types::GENERIC_KILL),
                f32::MAX,
            );
            // The death hook releases the run; damage-immune hunters are
            // released here so their region cannot stay allocated.
            state.complete_run(&player);
        }
        RunAction::ReturnPlayer { uuid, world } => {
            let Some(player) = find_player(server, *uuid) else {
                state.complete_offline_run(*uuid);
                return;
            };
            state.complete_run(&player);
            if let Some(target_world) = server.worlds.get(world)
                && let Err(error) = server.queue_player_world_selection(player, target_world)
            {
                log::warn!(
                    "sololeveling: unable to return hunter {uuid} from an instance: {error}"
                );
            }
        }
        RunAction::PenaltyLoss { uuid, roll } => {
            let _ = state.apply_penalty_failure(*uuid, *roll);
        }
    }
}

/// Splits `count` items as evenly as possible across `members`, giving the
/// first `count % members` of them one extra item.
fn even_share(count: u64, members: usize, index: usize) -> u64 {
    let members = members as u64;
    count / members + u64::from((index as u64) < count % members)
}

pub(crate) fn find_player(server: &Server, uuid: Uuid) -> Option<Arc<Player>> {
    server
        .get_players()
        .into_iter()
        .find(|player| player.gameprofile.id() == uuid)
}

/// Returns the world automatic gates appear in.
///
/// The main world is the default world of the server's default domain, so a
/// renamed overworld still works and every other dimension stays gate-free.
fn main_world(server: &Server) -> Option<&Arc<World>> {
    server.worlds.server_default_world()
}

fn instance_world_of(server: &Arc<Server>, world: &World) -> Option<Arc<World>> {
    server
        .worlds
        .values()
        .find(|candidate| {
            candidate.domain() == world.domain()
                && candidate.key.path.as_ref() == INSTANCE_WORLD_PATH
        })
        .cloned()
}

/// Picks a site in the [`GATE_MIN_RADIUS`]..=[`GATE_MAX_RADIUS`] ring around
/// the world spawn.
fn random_gate_candidate(center: BlockPos) -> BlockPos {
    let mut random = rand::rng();
    let radius = random.random_range(GATE_MIN_RADIUS..=GATE_MAX_RADIUS);
    let angle = random.random_range(0.0..TAU);
    BlockPos::new(
        center.x() + (radius * angle.cos()) as i32,
        0,
        center.z() + (radius * angle.sin()) as i32,
    )
}

/// Picks the column a prototype gate is requested at, roughly
/// [`TEST_GATE_DISTANCE`] away from the hunter.
fn test_gate_candidate(around: DVec3) -> BlockPos {
    let angle = rand::rng().random_range(0.0..TAU);
    BlockPos::new(
        (around.x + TEST_GATE_DISTANCE * angle.cos()).round() as i32,
        0,
        (around.z + TEST_GATE_DISTANCE * angle.sin()).round() as i32,
    )
}

fn squared_distance(first: BlockPos, second: BlockPos) -> f64 {
    let dx = f64::from(first.x() - second.x());
    let dz = f64::from(first.z() - second.z());
    dx * dx + dz * dz
}

fn players_too_close(world: &World, position: BlockPos, server: &Server) -> bool {
    server.get_players().into_iter().any(|player| {
        if player.get_world().key != world.key {
            return false;
        }
        let player_position = player.position();
        let dx = player_position.x - f64::from(position.x());
        let dz = player_position.z - f64::from(position.z());
        dx * dx + dz * dz < GATE_MIN_DISTANCE.powi(2)
    })
}
