//! Live gate and instance orchestration.

use std::f64::consts::TAU;
use std::fmt::{self, Debug, Formatter};
use std::fs;
use std::io::{self, ErrorKind};
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use glam::DVec3;
use rand::RngExt as _;
use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::chunk::chunk_request::{
    ChunkRequest, ChunkRequestHandle, ChunkRequestState, ChunkTicketKind,
};
use steel_core::chunk::status::ChunkStatus;
use steel_core::entity::damage::DamageSource;
use steel_core::entity::{Entity as _, LivingEntity as _, RemovalReason};
use steel_core::player::Player;
use steel_core::player::connection::NetworkConnection as _;
use steel_core::server::Server;
use steel_core::server::jobs::{JobPoll, ServerJob, ServerJobContext};
use steel_core::world::World;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_damage_types;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, Identifier};
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use super::boss::{BossAbilityAction, BossFight};
use super::generator::{Room, RoomRole, Theme};
use super::instance::{
    SpawnedMob, build_dungeon, dungeon_chunks, erase_footprint, footprint_chunks,
    gate_frame_chunks, gate_frame_is_clear, gate_frame_matches, gate_ground_is_dry,
    gate_space_is_clear, open_dungeon_exit, opening_contains, place_gate_frame, remove_gate_frame,
    spawn_city_break_mob, spawn_dungeon_boss, spawn_dungeon_wave, spawn_room_mobs,
};
use super::mobs::{MobClass, MobLabel};
use super::skills::{MobSkills, SkillAction, refresh_plates};
use super::tower;
use super::{
    DUNGEON_TIME_LIMIT_TICKS, DungeonLayout, DungeonWave, Footprint,
    GATE_BREAK_PARTICIPATION_RADIUS, GATE_BREAK_WAVE_INTERVAL_TICKS, GATE_BREAK_WAVE_REWARD_BASE,
    GATE_BREAK_WAVES, GATE_SPAWN_INTERVAL_TICKS, GATES_PER_SPAWN_CYCLE, Gate, GateRegistry,
    GateRegistryChanges, GateRegistrySnapshot, GateRegistryWrite, GateRetirementReason,
    INSTANCE_WORLD_PATH, InstanceRegion, InstanceRegionAllocator, LegacyGateCleanup,
    MINE_DURATION_TICKS, PENALTY_ZONE_DURATION_TICKS, PENALTY_ZONE_WAVE_INTERVAL_TICKS,
    TICKS_PER_SECOND, penalty_wave_size, wave_for_gate, wave_for_rank,
};
use crate::castle;
use crate::city;
use crate::item::chest::{self, ChestKind};
use crate::item::crystal;
use crate::item::{key, material, pouch};
use crate::organization::{Event, rank_multiplier};
use crate::party::RaidContribution;
use crate::rank::HunterRank;
use crate::state::{GateClear, SoloLevelingState};

/// Horizontal distance kept between two gates. The four gate plazas are
/// small, so the gap only has to keep two frames from touching; a wider gap
/// once left room for a single gate per plaza and starved the rank demand.
const GATE_MIN_DISTANCE: f64 = 10.0;
/// Horizontal distance an automatic gate keeps from every hunter, so it does
/// not appear on top of someone.
const GATE_PLAYER_DISTANCE: f64 = 16.0;

/// Prototype switch: place one gate next to a hunter who joins the overworld,
/// so a dungeon can be reached without waiting for the automatic cycle.
///
/// Set to `false` to restore vanilla gate scheduling only.
pub const TEST_GATE_ON_JOIN: bool = false;
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
/// Unentered gates kept open for every rank a hunter online holds.
const DEMAND_GATES_PER_RANK: usize = 2;
/// How long a rank may run short of gates before one is added.
const DEMAND_DELAY_TICKS: u64 = TICKS_PER_SECOND * 120;
/// How often the open gates are counted against the online ranks.
const DEMAND_CHECK_TICKS: u64 = TICKS_PER_SECOND * 10;
/// Gates the demand check never adds beyond, in the main world.
const MAX_LIVE_GATES: usize = 18;
/// Sites one spawn cycle may test before settling for fewer gates, so a crowded
/// world cannot keep the scheduler busy forever.
const GATE_MAX_SITE_ATTEMPTS: u8 = 12;
/// Ticks between repeated explanations of why a gate refuses a hunter.
const GATE_REFUSAL_INTERVAL_TICKS: u64 = 60;

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
    /// Executes a mob ability outside the runtime lock for the same reason.
    MobSkill(SkillAction),
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
    /// Start the mine phase after a dungeon boss is defeated.
    StartMine,
    /// Pay defenders for a cleared gate-break wave.
    CityDefenseWave {
        /// Hunters present near the gate.
        participants: Vec<Uuid>,
        /// Rank the gate was fought at.
        rank: HunterRank,
    },
    /// Pay miners when a mine shift ends.
    CreditMine {
        /// Hunters present inside the mine.
        participants: Vec<Uuid>,
        /// Rank the mine crystals sell at.
        rank: HunterRank,
        /// Crystals mined during the shift.
        mined: u32,
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
    /// Hunters whose instance region is being prepared by a [`DungeonGenerationJob`].
    generating_entries: FxHashMap<Uuid, GeneratingEntry>,
    /// Results produced by generation jobs, drained by [`finish_entries`].
    completed_generations: Arc<SyncMutex<Vec<GenerationResult>>>,
    runs: FxHashMap<Uuid, InstanceRun>,
    /// Gate frames to tear down once a [`Server`] handle is available again.
    spent_gates: Vec<u64>,
    /// Gate frames waiting for their chunks to load before removal.
    pending_gate_cleanup: Vec<PendingGateCleanup>,
    /// Committed gates waiting for their frame chunks before placement/repair.
    pending_gate_placement: Vec<PendingGatePlacement>,
    /// Gates whose frame has been verified in the current process.
    verified_gate_frames: FxHashSet<u64>,
    /// Gates whose frame site was occupied and must not be overwritten.
    blocked_gate_frames: FxHashSet<u64>,
    /// One-time cleanup records for gate frames orphaned by older releases.
    legacy_gate_cleanups: Vec<LegacyGateCleanup>,
    pending_legacy_gate_cleanups: Vec<PendingLegacyGateCleanup>,
    completed_legacy_gate_cleanups: FxHashSet<usize>,
    blocked_legacy_gate_cleanups: FxHashSet<usize>,
    /// Instance mobs to discard once a [`Server`] handle is available again.
    spent_mobs: Vec<(Identifier, i32)>,
    /// Loot removed from instance entities and delivered after the runtime lock.
    pending_loot: Vec<(Uuid, ItemStack)>,
    /// Hunters waiting for a gate placed next to them: prototype gates and
    /// gates opened with a dungeon key.
    test_gates: FxHashMap<Uuid, NearbyGate>,
    /// When each rank held by an online hunter first ran short of open gates.
    demand_since: FxHashMap<HunterRank, u64>,
    /// Tick each hunter was last told why a gate refused them.
    gate_refusals: FxHashMap<Uuid, u64>,
    /// Test-gate ids and the hunter who requested them.
    pending_test_gate_players: FxHashMap<u64, Uuid>,
}

impl Debug for DungeonRuntime {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DungeonRuntime")
            .field("gates", &self.gates)
            .field("regions", &self.regions)
            .field("gate_cycle", &self.gate_cycle.is_some())
            .field("pending_gates", &self.pending_gates.len())
            .field("generating_entries", &self.generating_entries.len())
            .field(
                "completed_generations",
                &self.completed_generations.lock().len(),
            )
            .field("runs", &self.runs.len())
            .field("spent_gates", &self.spent_gates.len())
            .field("pending_gate_cleanup", &self.pending_gate_cleanup.len())
            .field("pending_gate_placement", &self.pending_gate_placement.len())
            .field("verified_gate_frames", &self.verified_gate_frames.len())
            .field("blocked_gate_frames", &self.blocked_gate_frames.len())
            .field("legacy_gate_cleanups", &self.legacy_gate_cleanups.len())
            .field(
                "pending_legacy_gate_cleanups",
                &self.pending_legacy_gate_cleanups.len(),
            )
            .field(
                "completed_legacy_gate_cleanups",
                &self.completed_legacy_gate_cleanups.len(),
            )
            .field(
                "blocked_legacy_gate_cleanups",
                &self.blocked_legacy_gate_cleanups.len(),
            )
            .field("spent_mobs", &self.spent_mobs.len())
            .field("pending_loot", &self.pending_loot.len())
            .field("test_gates", &self.test_gates.len())
            .field("demand_since", &self.demand_since.len())
            .field(
                "pending_test_gate_players",
                &self.pending_test_gate_players.len(),
            )
            .field("gate_refusals", &self.gate_refusals.len())
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
    /// Set for a gate the demand check asks for, in the rank that ran short.
    rank: Option<HunterRank>,
}

struct PendingGateCleanup {
    world: Arc<World>,
    gate: Gate,
    request: ChunkRequestHandle,
}

struct PendingGatePlacement {
    world: Arc<World>,
    gate: Gate,
    request: ChunkRequestHandle,
}

struct PendingLegacyGateCleanup {
    index: usize,
    world: Arc<World>,
    gate: Gate,
    request: ChunkRequestHandle,
}

/// A gate requested next to one hunter.
#[derive(Debug, Clone, Copy)]
struct NearbyGate {
    /// Placement attempts already spent.
    attempts: u8,
    /// Rank of the gate.
    rank: HunterRank,
    /// Whether a dungeon key paid for it; a failed placement returns the key.
    key: bool,
    /// Demon Castle floor the gate opens, if it is a castle floor.
    floor: Option<u32>,
}

/// State of one gate placement next to a hunter.
struct PendingTestGate {
    /// Hunter the gate is placed next to.
    player: Uuid,
    /// Placement attempts already spent, including this one.
    attempts: u8,
    /// Rank of the gate.
    rank: HunterRank,
    /// Whether a dungeon key paid for it.
    key: bool,
    /// Demon Castle floor the gate opens, if it is a castle floor.
    floor: Option<u32>,
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

/// Lightweight bookkeeping for one dungeon/penalty generation job.
///
/// The heavy region preparation lives in [`DungeonGenerationJob`] so the
/// runtime lock is not held while blocks are written.
struct GeneratingEntry {
    /// Players waiting for this instance; their UUIDs are enough to detect busyness.
    players: Vec<Uuid>,
    #[expect(dead_code, reason = "kept for diagnostics and future expansion")]
    region: InstanceRegion,
    /// Gate consumed by this entry, restored if generation is cancelled.
    gate_id: Option<u64>,
    /// Shared with the generation job so disconnects and persistence failures
    /// can cancel work without reaching into the job queue.
    cancelled: Arc<AtomicBool>,
}

/// Outcome of a [`DungeonGenerationJob`], drained by [`finish_entries`].
struct GenerationResult {
    leader: Uuid,
    players: Vec<Arc<Player>>,
    source_world: Identifier,
    instance_world: Arc<World>,
    region: InstanceRegion,
    start: RunStart,
    /// Keeps the generated chunks loaded until the run is admitted or discarded.
    request: ChunkRequestHandle,
    footprint: Footprint,
    cancelled: bool,
    #[expect(dead_code, reason = "kept for diagnostics and future expansion")]
    erase_micros: u64,
    #[expect(dead_code, reason = "kept for diagnostics and future expansion")]
    build_micros: u64,
    #[expect(dead_code, reason = "kept for diagnostics and future expansion")]
    block_count: usize,
    #[expect(dead_code, reason = "kept for diagnostics and future expansion")]
    chunk_count: usize,
}

/// Server job that waits for instance chunks, clears the previous footprint,
/// builds the new dungeon/tower and records the footprint.
///
/// The job runs on the serialized game-tick task, so world block mutation is
/// safe; it only avoids doing that work inside the world-tick and gameplay-hook
/// phases.  Cancellation is cooperative via [`AtomicBool`].
struct DungeonGenerationJob {
    leader: Uuid,
    players: Option<Vec<Arc<Player>>>,
    source_world: Option<Identifier>,
    instance_world: Option<Arc<World>>,
    region: InstanceRegion,
    start: Option<RunStart>,
    previous: Option<Footprint>,
    request: Option<ChunkRequestHandle>,
    cancelled: Arc<AtomicBool>,
    completed: Arc<SyncMutex<Vec<GenerationResult>>>,
}

impl DungeonGenerationJob {
    fn push_result(
        &mut self,
        footprint: Footprint,
        erase_micros: u64,
        build_micros: u64,
        block_count: usize,
        chunk_count: usize,
        cancelled: bool,
    ) {
        let Some(request) = self.request.take() else {
            return;
        };
        self.completed.lock().push(GenerationResult {
            leader: self.leader,
            players: self.players.take().unwrap_or_default(),
            source_world: self.source_world.take().unwrap_or_default(),
            instance_world: self.instance_world.take().unwrap_or_else(|| {
                // The instance world is never used after this point without a
                // real handle; a default is unreachable because it was always set.
                unreachable!("DungeonGenerationJob ran without an instance world")
            }),
            region: self.region,
            start: self.start.take().expect("generation result missing start"),
            request,
            footprint,
            cancelled,
            erase_micros,
            build_micros,
            block_count,
            chunk_count,
        });
    }

    fn finish_cancelled(&mut self) {
        if let Some(mut request) = self.request.take() {
            request.cancel();
        }
        self.push_result(Footprint::Tower, 0, 0, 0, 0, true);
    }
}

impl ServerJob for DungeonGenerationJob {
    fn poll(&mut self, _context: &mut ServerJobContext) -> JobPoll {
        if self.cancelled.load(Ordering::Relaxed) {
            self.finish_cancelled();
            return JobPoll::Finished;
        }
        let Some(request) = self.request.as_ref() else {
            self.finish_cancelled();
            return JobPoll::Finished;
        };
        match request.poll() {
            ChunkRequestState::Pending { .. } => JobPoll::Pending,
            ChunkRequestState::Cancelled => {
                self.finish_cancelled();
                JobPoll::Finished
            }
            ChunkRequestState::Ready => {
                if request.ready_chunks().is_none() {
                    return JobPoll::Pending;
                }
                if self.cancelled.load(Ordering::Relaxed) {
                    self.finish_cancelled();
                    return JobPoll::Finished;
                }
                let chunk_count = request.positions().len();
                let instance_world = self
                    .instance_world
                    .as_ref()
                    .expect("generation job missing instance world");
                let erase_start = Instant::now();
                erase_footprint(instance_world, self.region, self.previous);
                let erase_micros = erase_start.elapsed().as_micros() as u64;

                let (footprint, block_count, build_micros) = match self
                    .start
                    .as_ref()
                    .expect("generation job missing start")
                {
                    RunStart::Gate { gate, layout } => {
                        let build_start = Instant::now();
                        build_dungeon(instance_world, self.region, layout, gate.rank);
                        let build_micros = build_start.elapsed().as_micros() as u64;
                        log::info!(
                            "sololeveling: cleared previous instance footprint in {:.2} ms (region {}, {} chunks)",
                            erase_micros as f64 / 1000.0,
                            self.region.slot,
                            chunk_count
                        );
                        log::info!(
                            "sololeveling: built a {} dungeon in {:.2} ms (region {}, seed {}, {} blocks, {} chunks)",
                            layout.blueprint.theme.display_name(),
                            build_micros as f64 / 1000.0,
                            self.region.slot,
                            layout.seed,
                            layout.blueprint.blocks.len(),
                            chunk_count
                        );
                        (
                            Footprint::Dungeon { seed: layout.seed },
                            layout.blueprint.blocks.len(),
                            build_micros,
                        )
                    }
                    RunStart::Penalty => {
                        let build_start = Instant::now();
                        tower::build_tower(instance_world, self.region);
                        let build_micros = build_start.elapsed().as_micros() as u64;
                        log::info!(
                            "sololeveling: cleared previous instance footprint in {:.2} ms (region {}, {} chunks)",
                            erase_micros as f64 / 1000.0,
                            self.region.slot,
                            chunk_count
                        );
                        log::info!(
                            "sololeveling: built a penalty tower in {:.2} ms (region {}, {} chunks)",
                            build_micros as f64 / 1000.0,
                            self.region.slot,
                            chunk_count
                        );
                        (Footprint::Tower, 0, build_micros)
                    }
                };
                self.push_result(
                    footprint,
                    erase_micros,
                    build_micros,
                    block_count,
                    chunk_count,
                    false,
                );
                JobPoll::Finished
            }
        }
    }

    fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.finish_cancelled();
    }

    fn finish_shutdown(
        mut self: Box<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(mut request) = self.request.take() {
            request.cancel();
        }
        Box::pin(async {})
    }
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
    /// Whether the entrance is locked: no voluntary exit until the boss falls.
    locked: bool,
}

enum RunProgress {
    Dungeon(Box<DungeonProgress>),
    Penalty(PenaltyProgress),
    Mine(MineProgress),
}

struct MineProgress {
    rank: HunterRank,
    #[expect(
        dead_code,
        reason = "preserved for run metadata and future payout rules"
    )]
    red: bool,
    #[expect(
        dead_code,
        reason = "preserved for run metadata and future payout rules"
    )]
    floor: Option<u32>,
    exit: BlockPos,
    mined: u32,
    quota: u32,
    started_tick: u64,
    duration_ticks: u64,
    boss_materials: Vec<(Uuid, ItemStack)>,
    boss_chest: Option<ChestKind>,
    survivors: Vec<Uuid>,
}

impl MineProgress {
    fn from_dungeon(progress: &mut DungeonProgress, tick: u64, exit: BlockPos) -> Self {
        // A shift is done when every vein placed in the dungeon was mined, not
        // by wave size: waves far outnumber the ore that physically exists.
        let quota = progress.rank.crystal_vein_count() as u32;
        Self {
            rank: progress.rank,
            red: progress.red,
            floor: progress.floor,
            exit,
            mined: 0,
            quota,
            started_tick: tick,
            duration_ticks: MINE_DURATION_TICKS,
            boss_materials: mem::take(&mut progress.boss_materials),
            boss_chest: progress.boss_chest.take(),
            survivors: mem::take(&mut progress.survivors),
        }
    }

    fn remaining_ticks(&self, tick: u64) -> u64 {
        self.duration_ticks
            .saturating_sub(tick.saturating_sub(self.started_tick))
    }

    fn finished(&self, tick: u64) -> bool {
        self.mined >= self.quota || tick.saturating_sub(self.started_tick) >= self.duration_ticks
    }
}

struct DungeonProgress {
    rank: HunterRank,
    /// Hunters of the run who died inside it.
    deaths: u32,
    /// Demon Castle floor of the run, if it climbs the castle.
    floor: Option<u32>,
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
    /// Whether the wave of `combat_room` already appeared; a wave waits for a
    /// hunter to enter its room.
    wave_spawned: bool,
    /// Side chambers whose guards already appeared, by room index.
    guarded_chambers: Vec<usize>,
    /// Chamber guards and hatched larvae: fought and rewarded like wave mobs,
    /// but not needed to open the next room.
    extra_mobs: Vec<i32>,
    /// Abilities of the run's shamans, bombers and broodmothers.
    skills: MobSkills,
    /// Abilities to apply once the runtime lock is released.
    pending_skills: Vec<SkillAction>,
    /// Classes of the wave mobs and chamber guards; hatched larvae have none
    /// and so drop no materials.
    classes: FxHashMap<i32, MobClass>,
    /// Boss materials owed to survivors, delivered with the boss chest.
    boss_materials: Vec<(Uuid, ItemStack)>,
}

impl DungeonProgress {
    /// Spawns the wave of the current combat room and remembers its plates.
    fn spawn_wave(&mut self, world: &Arc<World>, region: InstanceRegion, tick: u64) {
        let spawned = spawn_dungeon_wave(
            world,
            region,
            &self.layout,
            self.wave,
            self.rank,
            self.combat_room,
        );
        self.remaining_mobs = spawned.iter().map(|mob| mob.id).collect();
        self.adopt(spawned, tick);
        self.wave_spawned = true;
    }

    /// Spawns the guards of side chamber `index`, a third of a wave.
    fn spawn_guards(
        &mut self,
        world: &Arc<World>,
        region: InstanceRegion,
        index: usize,
        tick: u64,
    ) {
        self.guarded_chambers.push(index);
        let Some(room) = self.layout.blueprint.rooms.get(index) else {
            return;
        };
        let count = (self.wave.mob_count / 3).max(2);
        let salt = self.layout.seed ^ (index as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93);
        let spawned = spawn_room_mobs(
            world,
            region,
            self.layout.blueprint.theme,
            room,
            count,
            self.wave,
            self.rank,
            salt,
        );
        self.extra_mobs.extend(spawned.iter().map(|mob| mob.id));
        self.adopt(spawned, tick);
    }

    fn adopt(&mut self, spawned: Vec<SpawnedMob>, tick: u64) {
        for mob in spawned {
            self.skills.track(mob.id, mob.class, tick);
            self.classes.insert(mob.id, mob.class);
            self.labels.insert(mob.id, mob.label);
        }
    }

    /// Returns whether `id` is one of the run's ordinary mobs.
    fn tracks(&self, id: i32) -> bool {
        self.remaining_mobs.contains(&id) || self.extra_mobs.contains(&id)
    }

    /// Runs the mob abilities and the plate visibility of one tick.
    fn tick_mobs(&mut self, world: &Arc<World>, hunters: &[DVec3], tick: u64) {
        self.extra_mobs
            .retain(|id| world.get_entity_by_id(*id).is_some());
        let allies = self
            .remaining_mobs
            .iter()
            .chain(&self.extra_mobs)
            .copied()
            .collect::<Vec<_>>();
        let power = (5.0 + 5.0 * self.wave.damage_multiplier) as f32;
        let out = self.skills.tick(world, hunters, &allies, power, tick);
        self.pending_skills.extend(out.actions);
        for (id, label) in out.hatched {
            self.extra_mobs.push(id);
            self.labels.insert(id, label);
        }
        if tick.is_multiple_of(10) {
            self.labels
                .retain(|id, _| world.get_entity_by_id(*id).is_some());
            refresh_plates(world, hunters, self.labels.keys());
        }
    }
}

/// Returns whether `position` stands inside the bounding rectangle of `room`.
fn room_contains(region: InstanceRegion, room: &Room, position: DVec3) -> bool {
    let dx = position.x - region.spawn.x.floor();
    let dz = position.z - region.spawn.z.floor();
    (dx - f64::from(room.center.0) - 0.5).abs() <= f64::from(room.half.0) + 0.5
        && (dz - f64::from(room.center.1) - 0.5).abs() <= f64::from(room.half.1) + 0.5
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
    /// Rank the hunter entered at; scales the tower's mobs.
    rank: HunterRank,
    /// Groups spawned so far, which varies the classes they roll.
    groups: u64,
    /// Nameplates of the tower's mobs, by entity id.
    labels: FxHashMap<i32, MobLabel>,
    skills: MobSkills,
    pending_skills: Vec<SkillAction>,
}

impl InstanceRun {
    /// Removes the run's live mob ids, so their entities can be discarded.
    fn take_mobs(&mut self) -> Vec<i32> {
        match &mut self.progress {
            RunProgress::Dungeon(progress) => {
                let mut mobs = mem::take(&mut progress.remaining_mobs);
                mobs.append(&mut progress.extra_mobs);
                if let BossState::Alive(id) = progress.boss {
                    mobs.push(id);
                }
                mobs
            }
            RunProgress::Penalty(progress) => mem::take(&mut progress.remaining_mobs),
            RunProgress::Mine(_) => Vec::new(),
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
    /// Tick the run started, for the clear time.
    started_tick: u64,
    world: &'a Arc<World>,
    player: &'a Player,
    uuid: Uuid,
    members: &'a [Uuid],
    region: InstanceRegion,
    source_world: &'a Identifier,
}

impl DungeonRuntime {
    /// Loads one-time cleanup records for legacy orphan gate frames.
    pub fn load_legacy_gate_cleanups(&mut self, path: PathBuf) -> io::Result<()> {
        self.legacy_gate_cleanups = match fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?,
            Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        Ok(())
    }

    /// Loads what each instance region holds, saving later builds to `path`.
    pub fn load_region_records(&mut self, path: PathBuf) -> io::Result<()> {
        self.regions.load_footprints(path)
    }

    /// Restores the durable gate registry after the main world has been loaded.
    pub fn restore_gate_registry(&mut self, snapshot: GateRegistrySnapshot) -> Result<(), String> {
        self.gates.restore(snapshot, 0)
    }

    /// Starts an asynchronous write for changed gate metadata.
    pub fn begin_gate_persistence(&mut self) -> Option<GateRegistryWrite> {
        self.gates.begin_persistence()
    }

    /// Returns the last committed gates for a discovery menu.
    #[must_use]
    pub fn committed_gates(&self) -> Vec<Gate> {
        self.gates.committed_gates()
    }

    /// Returns one committed gate by id.
    #[must_use]
    pub fn committed_gate(&self, id: u64) -> Option<&Gate> {
        self.gates.committed_gate(id)
    }

    /// Returns whether the current gate registry cannot safely serve mutations.
    #[must_use]
    pub const fn gate_persistence_blocks_actions(&self) -> bool {
        self.gates.persistence_blocks_gate_actions()
    }

    /// Publishes gate effects only after their metadata transaction commits.
    pub fn commit_gate_persistence(&mut self, server: &Server, revision: u64) {
        let changes = self.gates.commit_persistence(revision);
        self.apply_gate_registry_changes(server, changes);
    }

    /// Restores the prior gate registry if a persistence write fails.
    pub fn fail_gate_persistence(&mut self, revision: u64) {
        let created = self.gates.fail_persistence(revision);
        for gate in created {
            self.pending_test_gate_players.remove(&gate.id);
        }
        // Cancel gate entries that are still being generated.  The regions are
        // released once the jobs observe the cancellation and report back.
        for entry in self.generating_entries.values() {
            if entry.gate_id.is_some() {
                entry.cancelled.store(true, Ordering::Relaxed);
            }
        }
        // Any completed gate results that have not been finalized yet are
        // treated as cancelled; finish_entries will release their regions and
        // reopen their gates from the restored registry.
        for result in self.completed_generations.lock().iter_mut() {
            if matches!(result.start, RunStart::Gate { .. }) {
                result.cancelled = true;
            }
        }
        self.gate_cycle = None;
        log::error!("sololeveling: gate registry persistence failed; gate entry is disabled");
    }

    fn apply_gate_registry_changes(&mut self, server: &Server, changes: GateRegistryChanges) {
        for gate in changes.created {
            let Some(world) = server.worlds.get(&gate.world) else {
                log::error!(
                    "sololeveling: committed gate {} has no loaded world {}",
                    gate.id,
                    gate.world
                );
                continue;
            };
            self.queue_gate_placement(Arc::clone(world), gate.clone());
            announce_gate(server, &gate);
            if let Some(test_player) = self.pending_test_gate_players.remove(&gate.id) {
                if let Some(player) = find_player(server, test_player) {
                    player.send_message(
                        &TextComponent::plain(format!(
                            "Test gate: {}-rank at {}, {}, {}",
                            gate.rank.name(),
                            gate.entrance.x(),
                            gate.entrance.y(),
                            gate.entrance.z()
                        ))
                        .color(Color::Aqua),
                    );
                }
            } else if let Some(cycle) = &mut self.gate_cycle
                && cycle.world == gate.world
            {
                cycle.gates_created += 1;
            }
            log::info!(
                "sololeveling: created {}-rank{} gate {} in {} at {:?}",
                gate.rank.name(),
                if gate.red { " red" } else { "" },
                gate.id,
                gate.world,
                gate.entrance
            );
        }
        for (gate, reason) in changes.removed {
            match reason {
                GateRetirementReason::Expired => announce_gate_expired(server, &gate),
                GateRetirementReason::Consumed => announce_gate_consumed(server, &gate),
                GateRetirementReason::Broken => announce_gate_broken(server, &gate),
            }
            if let Some(world) = server.worlds.get(&gate.world) {
                self.queue_gate_cleanup(Arc::clone(world), gate);
            }
        }
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
        self.schedule_legacy_gate_cleanups(server);
        self.finish_legacy_gate_cleanups();
        self.finish_gate_cleanup();
        self.schedule_retired_gate_cleanups(server);
        self.finish_gate_placement();
        self.schedule_committed_gate_placements(server);
        if !self.gates.persistence_blocks_gate_actions() {
            self.flush_spent(server);
        }
        if !self.gates.persistence_blocks_gate_actions() {
            self.expire_gates();
            self.schedule_gates(server, tick);
            self.schedule_demand_gates(state, server, tick);
            self.schedule_test_gates(server, tick);
            self.finish_gate_requests(server);
        }
        if !self.gates.persistence_blocks_gate_actions() {
            self.find_gate_entries(state, server, tick);
        }
        self.find_penalty_entries(state, server);
        self.finish_entries(state, server, tick);
        let mut actions = self.tick_gate_breaks(server, tick);
        actions.extend(self.tick_runs(state, server, tick));
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
                RunProgress::Mine(progress) => Some(progress.rank),
                RunProgress::Penalty(_) => None,
            }
        })
    }

    /// Records `amount` crystals mined in the mine run occupying `world` and `pos`.
    ///
    /// Returns `true` when the block was a mine crystal and its drops should be suppressed.
    pub fn record_mine_crystal(&mut self, world: &World, pos: BlockPos, amount: u32) -> bool {
        let Some(run) = self.runs.values_mut().find(|run| {
            run.instance_world == world.key
                && matches!(run.progress, RunProgress::Mine(_))
                && run.region.contains(pos)
        }) else {
            return false;
        };
        let RunProgress::Mine(progress) = &mut run.progress else {
            return false;
        };
        progress.mined += amount;
        true
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
                RunProgress::Mine(_) => continue,
            };
            if let Some(index) = remaining.iter().position(|id| *id == entity_id) {
                remaining.swap_remove(index);
                return;
            }
            if let RunProgress::Dungeon(progress) = &mut run.progress
                && let Some(index) = progress.extra_mobs.iter().position(|id| *id == entity_id)
            {
                progress.extra_mobs.swap_remove(index);
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
            match &run.progress {
                RunProgress::Dungeon(progress) => progress.labels.get(&entity_id).cloned(),
                RunProgress::Penalty(progress) => progress.labels.get(&entity_id).cloned(),
                RunProgress::Mine(_) => None,
            }
        })
    }

    /// Returns whether both entities are tracked by the same active run.
    #[must_use]
    pub fn are_in_same_run(&self, world: &World, first: i32, second: i32) -> bool {
        self.runs.values().any(|run| {
            if run.instance_world != world.key {
                return false;
            }
            match &run.progress {
                RunProgress::Dungeon(progress) => {
                    progress.labels.contains_key(&first) && progress.labels.contains_key(&second)
                }
                RunProgress::Penalty(progress) => {
                    progress.labels.contains_key(&first) && progress.labels.contains_key(&second)
                }
                RunProgress::Mine(_) => false,
            }
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
                            || progress.tracks(entity_id)
                )
        })
    }

    /// Returns the class and gate rank of a wave mob or chamber guard that
    /// `hunter`'s run is fighting, for its material drop.
    #[must_use]
    pub fn material_source(
        &self,
        world: &World,
        entity_id: i32,
        hunter: Uuid,
    ) -> Option<(MobClass, HunterRank)> {
        self.runs.values().find_map(|run| {
            if run.instance_world != world.key || !run.members.contains(&hunter) {
                return None;
            }
            let RunProgress::Dungeon(progress) = &run.progress else {
                return None;
            };
            if !progress.tracks(entity_id) {
                return None;
            }
            Some((*progress.classes.get(&entity_id)?, progress.rank))
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
            progress.tracks(entity_id).then_some(progress.rank)
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
            if progress.tracks(entity_id) {
                let contribution = progress.contributions.entry(hunter).or_default();
                contribution.mob_kills = contribution.mob_kills.saturating_add(1);
                return;
            }
        }
    }

    /// Returns the entity id of the living boss of a run in `world`.
    pub fn boss_entity(&self, world: &World) -> Option<i32> {
        self.runs.values().find_map(|run| {
            if run.instance_world != world.key {
                return None;
            }
            match &run.progress {
                RunProgress::Dungeon(progress) => match progress.boss {
                    BossState::Alive(id) => Some(id),
                    _ => None,
                },
                RunProgress::Penalty(_) | RunProgress::Mine(_) => None,
            }
        })
    }

    /// Drains loot that must be delivered outside the runtime lock.
    pub fn take_pending_loot(&mut self) -> Vec<(Uuid, ItemStack)> {
        mem::take(&mut self.pending_loot)
    }

    /// Removes a dead hunter from a run, releasing its region and gate only
    /// after the last participant has died.
    pub fn on_player_death(&mut self, player: &Player) -> RunDeath {
        let uuid = player.gameprofile.id();
        self.cancel_generating_entry(uuid);
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
        let death = match &mut run.progress {
            RunProgress::Dungeon(progress) => {
                progress.deaths += 1;
                RunDeath::DungeonFailed
            }
            RunProgress::Penalty(_) => RunDeath::PenaltyFailed {
                roll: rand::rng().random_range(0..5),
            },
            RunProgress::Mine(_) => RunDeath::None,
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
        self.cancel_generating_entry(player_uuid);
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

    fn cancel_generating_entry(&mut self, player_uuid: Uuid) -> bool {
        let Some(entry) = self.generating_entries.get(&player_uuid) else {
            return false;
        };
        entry.cancelled.store(true, Ordering::Relaxed);
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
            self.gates.remove(id);
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

    fn expire_gates(&mut self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        for id in self.gates.unused_ids_at(now) {
            self.gates.start_break(id);
        }
    }

    fn gate_break_participants(server: &Server, gate: &Gate) -> Vec<Uuid> {
        let center = DVec3::new(
            f64::from(gate.entrance.x()),
            f64::from(gate.entrance.y()),
            f64::from(gate.entrance.z()),
        );
        server
            .get_players()
            .into_iter()
            .filter(|player| {
                player.get_world().key == gate.world
                    && player.position().distance_squared(center)
                        <= GATE_BREAK_PARTICIPATION_RADIUS.powi(2)
                    && !player.is_dead_or_dying()
            })
            .map(|player| player.gameprofile.id())
            .collect()
    }

    fn tick_gate_breaks(&mut self, server: &Arc<Server>, tick: u64) -> Vec<RunAction> {
        let mut actions = Vec::new();
        let mut cleared = Vec::new();
        for break_state in self.gates.breaks_mut() {
            if !break_state.announced {
                break_state.announced = true;
                announce_gate_break(server, &break_state.gate);
            }
            let Some(world) = server.worlds.get(&break_state.gate.world) else {
                continue;
            };
            break_state
                .remaining_mobs
                .retain(|id| world.get_entity_by_id(*id).is_some());
            if !break_state.remaining_mobs.is_empty() {
                continue;
            }
            if break_state.wave > 0 {
                let participants = Self::gate_break_participants(server, &break_state.gate);
                actions.push(RunAction::CityDefenseWave {
                    participants,
                    rank: break_state.gate.rank,
                });
            }
            if break_state.wave >= GATE_BREAK_WAVES {
                cleared.push(break_state.gate.id);
                continue;
            }
            break_state.wave += 1;
            break_state.next_wave_tick = tick + GATE_BREAK_WAVE_INTERVAL_TICKS;
            let wave = wave_for_gate(break_state.gate.rank, break_state.gate.red);
            let theme = Theme::Crypt;
            let count = wave.mob_count / 3;
            for _ in 0..count {
                if let Some(mob) = spawn_city_break_mob(
                    world,
                    break_state.gate.entrance,
                    wave,
                    break_state.gate.rank,
                    theme,
                ) {
                    break_state.remaining_mobs.push(mob.id);
                }
            }
        }
        for id in cleared {
            if let Some(gate) = self.gates.clear_break(id) {
                log::info!(
                    "sololeveling: gate break cleared in {} at {:?}",
                    gate.world,
                    gate.entrance
                );
            }
        }
        actions
    }

    fn queue_gate_cleanup(&mut self, world: Arc<World>, gate: Gate) {
        let request = world.chunk_map.request_chunks(ChunkRequest {
            status: ChunkStatus::Full,
            positions: gate_frame_chunks(gate.entrance),
            ticket_kind: ChunkTicketKind::Command,
        });
        self.pending_gate_cleanup.push(PendingGateCleanup {
            world,
            gate,
            request,
        });
    }

    fn queue_gate_placement(&mut self, world: Arc<World>, gate: Gate) {
        let request = world.chunk_map.request_chunks(ChunkRequest {
            status: ChunkStatus::Full,
            positions: gate_frame_chunks(gate.entrance),
            ticket_kind: ChunkTicketKind::Command,
        });
        self.pending_gate_placement.push(PendingGatePlacement {
            world,
            gate,
            request,
        });
    }

    fn schedule_committed_gate_placements(&mut self, server: &Server) {
        for gate in self.gates.committed_gates() {
            if self.verified_gate_frames.contains(&gate.id)
                || self.blocked_gate_frames.contains(&gate.id)
                || self
                    .pending_gate_placement
                    .iter()
                    .any(|pending| pending.gate.id == gate.id)
            {
                continue;
            }
            let Some(world) = server.worlds.get(&gate.world) else {
                continue;
            };
            self.queue_gate_placement(Arc::clone(world), gate);
        }
    }

    fn finish_gate_placement(&mut self) {
        let mut index = 0;
        while index < self.pending_gate_placement.len() {
            match self.pending_gate_placement[index].request.poll() {
                ChunkRequestState::Ready => {
                    let placement = self.pending_gate_placement.swap_remove(index);
                    if place_gate_frame(
                        &placement.world,
                        placement.gate.entrance,
                        placement.gate.red,
                    ) {
                        self.verified_gate_frames.insert(placement.gate.id);
                    } else {
                        self.blocked_gate_frames.insert(placement.gate.id);
                        log::warn!(
                            "sololeveling: gate frame placement refused or incomplete in {} at {:?}; keeping the registry entry",
                            placement.gate.world,
                            placement.gate.entrance
                        );
                    }
                }
                ChunkRequestState::Cancelled => {
                    let placement = self.pending_gate_placement.swap_remove(index);
                    self.queue_gate_placement(Arc::clone(&placement.world), placement.gate);
                }
                ChunkRequestState::Pending { .. } => index += 1,
            }
        }
    }

    fn schedule_retired_gate_cleanups(&mut self, server: &Server) {
        for (gate, _) in self.gates.retired_gates() {
            if self
                .pending_gate_cleanup
                .iter()
                .any(|pending| pending.gate.id == gate.id)
            {
                continue;
            }
            let Some(world) = server.worlds.get(&gate.world) else {
                continue;
            };
            self.queue_gate_cleanup(Arc::clone(world), gate);
        }
    }

    fn schedule_legacy_gate_cleanups(&mut self, server: &Server) {
        for index in 0..self.legacy_gate_cleanups.len() {
            if self.completed_legacy_gate_cleanups.contains(&index)
                || self.blocked_legacy_gate_cleanups.contains(&index)
                || self
                    .pending_legacy_gate_cleanups
                    .iter()
                    .any(|pending| pending.index == index)
            {
                continue;
            }
            let record = &self.legacy_gate_cleanups[index];
            let gate = record.gate();
            if self.gates.has_gate_at(&record.world, gate.entrance) {
                self.completed_legacy_gate_cleanups.insert(index);
                continue;
            }
            let Some(world) = server.worlds.get(&record.world) else {
                continue;
            };
            let request = world.chunk_map.request_chunks(ChunkRequest {
                status: ChunkStatus::Full,
                positions: gate_frame_chunks(gate.entrance),
                ticket_kind: ChunkTicketKind::Command,
            });
            self.pending_legacy_gate_cleanups
                .push(PendingLegacyGateCleanup {
                    index,
                    world: Arc::clone(world),
                    gate,
                    request,
                });
        }
    }

    fn finish_legacy_gate_cleanups(&mut self) {
        let mut index = 0;
        while index < self.pending_legacy_gate_cleanups.len() {
            match self.pending_legacy_gate_cleanups[index].request.poll() {
                ChunkRequestState::Ready => {
                    let cleanup = self.pending_legacy_gate_cleanups.swap_remove(index);
                    let record_index = cleanup.index;
                    let gate = cleanup.gate;
                    if self.gates.has_gate_at(&gate.world, gate.entrance) {
                        self.completed_legacy_gate_cleanups.insert(record_index);
                        continue;
                    }
                    if gate_frame_is_clear(&cleanup.world, &gate) {
                        self.completed_legacy_gate_cleanups.insert(record_index);
                    } else if gate_frame_matches(&cleanup.world, &gate) {
                        remove_gate_frame(&cleanup.world, &gate);
                        self.completed_legacy_gate_cleanups.insert(record_index);
                        log::info!(
                            "sololeveling: removed verified legacy orphan gate frame in {} at {:?}",
                            gate.world,
                            gate.entrance
                        );
                    } else {
                        self.blocked_legacy_gate_cleanups.insert(record_index);
                        log::warn!(
                            "sololeveling: legacy gate cleanup refused for mismatched frame in {} at {:?}",
                            gate.world,
                            gate.entrance
                        );
                    }
                }
                ChunkRequestState::Cancelled => {
                    let cleanup = self.pending_legacy_gate_cleanups.swap_remove(index);
                    let request = cleanup.world.chunk_map.request_chunks(ChunkRequest {
                        status: ChunkStatus::Full,
                        positions: gate_frame_chunks(cleanup.gate.entrance),
                        ticket_kind: ChunkTicketKind::Command,
                    });
                    self.pending_legacy_gate_cleanups
                        .push(PendingLegacyGateCleanup { request, ..cleanup });
                }
                ChunkRequestState::Pending { .. } => index += 1,
            }
        }
    }

    fn finish_gate_cleanup(&mut self) {
        let mut index = 0;
        while index < self.pending_gate_cleanup.len() {
            match self.pending_gate_cleanup[index].request.poll() {
                ChunkRequestState::Ready => {
                    let cleanup = self.pending_gate_cleanup.swap_remove(index);
                    remove_gate_frame(&cleanup.world, &cleanup.gate);
                    if gate_frame_is_clear(&cleanup.world, &cleanup.gate) {
                        self.gates.acknowledge_gate_cleanup(cleanup.gate.id);
                    } else {
                        log::warn!(
                            "sololeveling: gate frame cleanup is incomplete in {} at {:?}; retaining durable retirement",
                            cleanup.gate.world,
                            cleanup.gate.entrance
                        );
                        self.queue_gate_cleanup(Arc::clone(&cleanup.world), cleanup.gate);
                    }
                }
                ChunkRequestState::Cancelled => {
                    let cleanup = self.pending_gate_cleanup.swap_remove(index);
                    let world = Arc::clone(&cleanup.world);
                    self.queue_gate_cleanup(world, cleanup.gate);
                }
                ChunkRequestState::Pending { .. } => index += 1,
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

    /// Keeps open gates available for every rank a hunter online in the main
    /// world holds.
    ///
    /// The ten-minute cycle seeds the world, but a fast hunter clears its few
    /// gates long before the next one. When a rank has fewer than
    /// [`DEMAND_GATES_PER_RANK`] unentered gates for [`DEMAND_DELAY_TICKS`],
    /// one gate of exactly that rank is added, up to [`MAX_LIVE_GATES`].
    fn schedule_demand_gates(
        &mut self,
        state: &SoloLevelingState,
        server: &Arc<Server>,
        tick: u64,
    ) {
        if !tick.is_multiple_of(DEMAND_CHECK_TICKS) {
            return;
        }
        let Some(world) = main_world(server) else {
            return;
        };
        let mut ranks: Vec<HunterRank> = Vec::new();
        world.players.iter_players(|_, player| {
            if let Some(rank) = state.hunter_rank(player.gameprofile.id())
                && !ranks.contains(&rank)
            {
                ranks.push(rank);
            }
            true
        });
        let gates = self.gates.committed_gates();
        let live = gates.iter().filter(|gate| gate.world == world.key).count();
        self.demand_since.retain(|rank, _| ranks.contains(rank));
        if live >= MAX_LIVE_GATES
            || self
                .pending_gates
                .iter()
                .any(|pending| pending.rank.is_some())
        {
            return;
        }
        for rank in ranks {
            let open = gates
                .iter()
                .filter(|gate| gate.world == world.key && gate.rank == rank && !gate.entered)
                .count();
            if open >= DEMAND_GATES_PER_RANK {
                self.demand_since.remove(&rank);
                continue;
            }
            let since = *self.demand_since.entry(rank).or_insert(tick);
            if tick.saturating_sub(since) < DEMAND_DELAY_TICKS {
                continue;
            }
            self.demand_since.insert(rank, tick);
            let spawn = world.level_data.read().data().spawn_pos();
            let candidate = random_gate_candidate(spawn);
            let requests = gate_frame_chunks(candidate)
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
                world: Arc::clone(world),
                candidate,
                requests,
                tick,
                test: None,
                rank: Some(rank),
            });
            log::debug!(
                "sololeveling: hunters of rank {} wait for a gate",
                rank.name()
            );
            // One demand gate in flight at a time keeps sites from colliding.
            return;
        }
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
            rank: None,
        });
    }

    /// Asks for a prototype gate next to one hunter.
    ///
    /// The request is served on a later tick, once the chunks around the
    /// hunter's position are loaded, so it works from the join hook where no
    /// [`Server`] tick is available yet.
    pub fn request_test_gate(&mut self, player: Uuid) {
        self.test_gates.insert(
            player,
            NearbyGate {
                attempts: 0,
                rank: TEST_GATE_RANK,
                key: false,
                floor: None,
            },
        );
    }

    /// Asks for a gate of `rank` next to a hunter who used a dungeon key.
    ///
    /// Returns `false` while an earlier request of the hunter is still being
    /// placed, so the caller keeps the key.
    pub fn request_key_gate(&mut self, player: Uuid, rank: HunterRank, floor: Option<u32>) -> bool {
        let busy = self.test_gates.contains_key(&player)
            || self.pending_gates.iter().any(|pending| {
                pending
                    .test
                    .as_ref()
                    .is_some_and(|test| test.player == player)
            });
        if busy {
            return false;
        }
        self.test_gates.insert(
            player,
            NearbyGate {
                attempts: 0,
                rank,
                key: true,
                floor,
            },
        );
        true
    }

    /// Gives a key back when its gate could not be placed: a castle key for
    /// a castle floor, so even a free entry is not lost.
    fn refund_key(&mut self, player: Uuid, rank: HunterRank, floor: Option<u32>) {
        let stack = if floor.is_some() {
            key::castle_stack()
        } else {
            key::stack(rank)
        };
        self.pending_loot.push((player, stack));
    }

    /// Starts the prototype gate placements the join hook asked for.
    fn schedule_test_gates(&mut self, server: &Arc<Server>, tick: u64) {
        if self.test_gates.is_empty() {
            return;
        }
        let requests = mem::take(&mut self.test_gates);
        for (uuid, request) in requests {
            let refund = |runtime: &mut Self| {
                if request.key {
                    runtime.refund_key(uuid, request.rank, request.floor);
                }
            };
            let Some(player) = find_player(server, uuid) else {
                refund(self);
                continue;
            };
            let world = player.get_world();
            if main_world(server).is_none_or(|main| main.key != world.key) {
                log::debug!("sololeveling: hunter {uuid} is not in the main world; no test gate");
                if request.key {
                    player.send_message(
                        &TextComponent::plain("Ключ открывает врата только в главном мире.")
                            .color(Color::Red),
                    );
                }
                refund(self);
                continue;
            }
            // A key opens its gate even beside another one; only the
            // prototype gate avoids stacking on top of a nearby gate.
            if !request.key
                && self.gates_near(&world.key, player.position(), TEST_GATE_REUSE_RADIUS)
            {
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
                    attempts: request.attempts.saturating_add(1),
                    rank: request.rank,
                    key: request.key,
                    floor: request.floor,
                }),
                rank: None,
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
                self.retry_demand_gate(&pending);
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
                self.retry_demand_gate(&pending);
                log::debug!(
                    "sololeveling: rejected gate site {surface:?} in {} (occupied, crowded, or too wet)",
                    pending.world.key
                );
                continue;
            }
            let test_gate = pending.test.is_some();
            let gate = if let Some(floor) = pending.test.as_ref().and_then(|test| test.floor) {
                self.gates
                    .create_castle(pending.world.key.clone(), surface, pending.tick, floor)
            } else if let Some(test) = &pending.test {
                self.gates.create_ranked(
                    pending.world.key.clone(),
                    surface,
                    pending.tick,
                    test.rank,
                    TEST_GATE_RED,
                )
            } else if let Some(rank) = pending.rank {
                self.gates.create_ranked(
                    pending.world.key.clone(),
                    surface,
                    pending.tick,
                    rank,
                    false,
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
            if let Some(test) = &pending.test {
                self.pending_test_gate_players.insert(gate.id, test.player);
            }
            log::debug!(
                "sololeveling: staged {}-rank{} gate {} in {} at {:?}{}",
                gate.rank.name(),
                if gate.red { " red" } else { "" },
                gate.id,
                gate.world,
                surface,
                if test_gate { " (test gate)" } else { "" }
            );
        }
    }

    /// Lets a rank whose demand gate found no site try again on the next
    /// demand check instead of waiting out the whole delay.
    fn retry_demand_gate(&mut self, pending: &PendingGate) {
        if pending.test.is_none()
            && let Some(rank) = pending.rank
        {
            self.demand_since.insert(rank, 0);
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
            if test.key {
                self.refund_key(test.player, test.rank, test.floor);
            }
            return;
        }
        self.test_gates.insert(
            test.player,
            NearbyGate {
                attempts: test.attempts,
                rank: test.rank,
                key: test.key,
                floor: test.floor,
            },
        );
    }

    fn gates_too_close_within(&self, world: &Identifier, position: BlockPos, radius: f64) -> bool {
        self.gates.values().any(|gate| {
            gate.world == *world && squared_distance(gate.entrance, position) < radius.powi(2)
        })
    }

    fn find_gate_entries(&mut self, state: &SoloLevelingState, server: &Arc<Server>, tick: u64) {
        for player in server.get_players() {
            let uuid = player.gameprofile.id();
            if self.is_busy(uuid) {
                continue;
            }
            let world = player.get_world();
            let Some(standing) = self.gates.at_opening(&world.key, player.position()) else {
                continue;
            };
            let minimum = standing.minimum_hunter_rank();
            let party = state.party_of(uuid);
            let Some(rank) = state.hunter_rank(uuid) else {
                continue;
            };
            let refusal = if state.has_pending_penalty(uuid) {
                Some(penalty_refusal(state.penalty_wait_seconds(uuid)))
            } else if party.as_ref().is_some_and(|party| party.leader() != uuid) {
                Some("Войти во врата может только лидер группы.".to_owned())
            } else if rank < minimum {
                Some(format!(
                    "Ранг охотника слишком низок: нужен {} или выше.",
                    minimum.name()
                ))
            } else {
                None
            };
            if let Some(reason) = refusal {
                let last = self.gate_refusals.get(&uuid).copied();
                if last.is_none_or(|last| tick.saturating_sub(last) >= GATE_REFUSAL_INTERVAL_TICKS)
                {
                    self.gate_refusals.insert(uuid, tick);
                    player.send_message(
                        &TextComponent::plain(format!("Врата не пускают вас. {reason}"))
                            .color(Color::Red),
                    );
                }
                continue;
            }
            let Some(instance_world) = instance_world_of(server, &world) else {
                log::error!(
                    "sololeveling: instance world is not loaded for {}",
                    world.key
                );
                continue;
            };
            let Some(gate) = self.gates.enter_at(&world.key, rank, player.position()) else {
                continue;
            };
            let start = RunStart::Gate {
                gate: gate.clone(),
                layout: DungeonLayout::for_gate(rand::rng().random(), gate.red),
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
            for participant in &participants {
                participant.send_message(
                    &TextComponent::plain(format!(
                        "Врата ранга {} активны. Подготавливается подземелье; вход займёт несколько секунд.",
                        gate.rank.name()
                    ))
                    .color(Color::Aqua),
                );
            }
            self.begin_entry(
                server,
                participants,
                world.key.clone(),
                instance_world,
                start,
            );
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
                server,
                vec![player],
                world.key.clone(),
                instance_world,
                RunStart::Penalty,
            );
        }
    }

    fn is_busy(&self, player_uuid: Uuid) -> bool {
        self.generating_entries
            .values()
            .any(|entry| entry.players.contains(&player_uuid))
            || self
                .runs
                .values()
                .any(|run| run.members.contains(&player_uuid))
    }

    fn begin_entry(
        &mut self,
        server: &Arc<Server>,
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
            RunStart::Penalty => {
                let mut chunks = tower::tower_chunks(region);
                for chunk in footprint_chunks(region, previous) {
                    if !chunks.contains(&chunk) {
                        chunks.push(chunk);
                    }
                }
                chunks
            }
        };
        let request = instance_world.chunk_map.request_chunks(ChunkRequest {
            status: ChunkStatus::Full,
            positions,
            ticket_kind: ChunkTicketKind::Portal,
        });
        let Some(leader) = players.first().map(|player| player.gameprofile.id()) else {
            self.regions.release(region);
            return;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let player_uuids = players
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        let gate_id = match &start {
            RunStart::Gate { gate, .. } => Some(gate.id),
            RunStart::Penalty => None,
        };
        self.generating_entries.insert(
            leader,
            GeneratingEntry {
                players: player_uuids,
                region,
                gate_id,
                cancelled: Arc::clone(&cancelled),
            },
        );
        server.jobs.spawn(DungeonGenerationJob {
            leader,
            players: Some(players),
            source_world: Some(source_world),
            instance_world: Some(Arc::clone(&instance_world)),
            region,
            start: Some(start),
            previous,
            request: Some(request),
            cancelled,
            completed: Arc::clone(&self.completed_generations),
        });
    }

    #[expect(
        clippy::too_many_lines,
        reason = "entering a run stays one reviewable sequence: cancel, wait, then admit"
    )]
    fn finish_entries(&mut self, state: &SoloLevelingState, server: &Arc<Server>, tick: u64) {
        let results = mem::take(&mut *self.completed_generations.lock());
        let mut skipped = Vec::new();
        for result in results {
            if result.cancelled {
                self.discard_generation_result(&result);
                continue;
            }
            let Some(entry) = self.generating_entries.get(&result.leader) else {
                // The entry was already cleaned up; release the region defensively.
                self.regions.release(result.region);
                if let RunStart::Gate { gate, .. } = &result.start {
                    self.gates.reopen(gate.id);
                }
                continue;
            };
            if entry.cancelled.load(Ordering::Relaxed) {
                self.discard_generation_result(&result);
                continue;
            }
            if matches!(result.start, RunStart::Gate { .. })
                && self.gates.persistence_blocks_gate_actions()
            {
                skipped.push(result);
                continue;
            }
            if let RunStart::Gate { gate, .. } = &result.start {
                if self.gates.remove(gate.id).is_some() {
                    // Consume the gate retirement now; finalize the run once the
                    // removal is visible (i.e. on the next tick after persistence commits).
                    skipped.push(result);
                    continue;
                }
            }
            self.generating_entries.remove(&result.leader);
            self.finalize_entry(state, server, tick, result);
        }
        self.completed_generations.lock().extend(skipped);
    }

    fn discard_generation_result(&mut self, result: &GenerationResult) {
        self.generating_entries.remove(&result.leader);
        self.regions.release(result.region);
        if let RunStart::Gate { gate, .. } = &result.start {
            self.gates.reopen(gate.id);
        }
    }

    fn finalize_entry(
        &mut self,
        state: &SoloLevelingState,
        server: &Arc<Server>,
        tick: u64,
        result: GenerationResult,
    ) {
        let GenerationResult {
            leader,
            players,
            source_world,
            instance_world,
            region,
            start,
            request,
            footprint,
            ..
        } = result;
        let mut penalty_rank = HunterRank::E;
        let duration_ticks = match &start {
            RunStart::Gate { .. } => DUNGEON_TIME_LIMIT_TICKS,
            RunStart::Penalty => {
                let Some(rank) = state.claim_penalty(leader) else {
                    self.regions.release(region);
                    return;
                };
                penalty_rank = rank;
                PENALTY_ZONE_DURATION_TICKS
            }
        };
        self.regions.record(region, footprint);
        let mut members = Vec::with_capacity(players.len());
        for player in players {
            let player_uuid = player.gameprofile.id();
            if player.connection.closed() {
                continue;
            }
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
            if let RunStart::Gate { gate, .. } = &start {
                self.gates.restore_gate(gate.clone());
            }
            return;
        }
        let (progress, gate, locked) = match start {
            RunStart::Gate { gate, layout } => {
                let locked = gate.red;
                (
                    RunProgress::Dungeon(Box::new(DungeonProgress {
                        rank: gate.rank,
                        floor: gate.floor,
                        red: gate.red,
                        wave: gate
                            .floor
                            .map_or_else(|| wave_for_gate(gate.rank, gate.red), castle::wave),
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
                        wave_spawned: false,
                        guarded_chambers: Vec::new(),
                        extra_mobs: Vec::new(),
                        skills: MobSkills::default(),
                        pending_skills: Vec::new(),
                        classes: FxHashMap::default(),
                        boss_materials: Vec::new(),
                        deaths: 0,
                    })),
                    Some(gate.id),
                    locked,
                )
            }
            RunStart::Penalty => (
                RunProgress::Penalty(PenaltyProgress {
                    next_wave_tick: tick + PENALTY_ZONE_WAVE_INTERVAL_TICKS,
                    remaining_mobs: Vec::new(),
                    rank: penalty_rank,
                    groups: 0,
                    labels: FxHashMap::default(),
                    skills: MobSkills::default(),
                    pending_skills: Vec::new(),
                }),
                None,
                false,
            ),
        };
        self.runs.insert(
            leader,
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
                locked,
            },
        );
    }

    /// Advances one dungeon run: waves, then the boss, then the exit.
    ///
    /// Returns the action that ends the run, or `None` while the fight goes on.
    ///
    /// A room counts as cleared when its mobs are gone, not when they die: a mob
    /// can be removed without a death event, so the live entities are what the
    /// room is waiting for.
    /// Spawns the tower's groups ahead of the highest hunter and runs their
    /// abilities.
    fn tick_tower(
        progress: &mut PenaltyProgress,
        world: &Arc<World>,
        region: InstanceRegion,
        hunters: &[DVec3],
        tick: u64,
    ) {
        progress
            .remaining_mobs
            .retain(|id| world.get_entity_by_id(*id).is_some());
        let power = (5.0 + 5.0 * wave_for_rank(progress.rank).damage_multiplier) as f32;
        let out = progress
            .skills
            .tick(world, hunters, &progress.remaining_mobs, power, tick);
        progress.pending_skills.extend(out.actions);
        for (id, label) in out.hatched {
            progress.remaining_mobs.push(id);
            progress.labels.insert(id, label);
        }
        if tick.is_multiple_of(10) {
            progress
                .labels
                .retain(|id, _| world.get_entity_by_id(*id).is_some());
            refresh_plates(world, hunters, progress.labels.keys());
        }
        if tick < progress.next_wave_tick {
            return;
        }
        progress.next_wave_tick = tick + PENALTY_ZONE_WAVE_INTERVAL_TICKS;
        let alive = u8::try_from(progress.remaining_mobs.len()).unwrap_or(u8::MAX);
        let count = penalty_wave_size(alive);
        let Some(climb) = hunters
            .iter()
            .map(|hunter| tower::climb_of(region, *hunter))
            .reduce(f64::max)
        else {
            return;
        };
        if count == 0 {
            return;
        }
        progress.groups += 1;
        let spawned = tower::spawn_tower_group(
            world,
            region,
            climb,
            count,
            wave_for_rank(progress.rank),
            progress.rank,
            tick ^ progress.groups.wrapping_mul(0xA24B_AED4_963E_E407),
        );
        for mob in spawned {
            progress.remaining_mobs.push(mob.id);
            progress.skills.track(mob.id, mob.class, tick);
            progress.labels.insert(mob.id, mob.label);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "a dungeon run advances room by room; the stages read best in order"
    )]
    fn tick_dungeon_run(
        progress: &mut DungeonProgress,
        context: &RunContext<'_>,
        tick: u64,
    ) -> Option<RunAction> {
        let RunContext {
            state,
            started_tick,
            world,
            player,
            uuid,
            members,
            region,
            source_world,
        } = *context;
        let hunters = members
            .iter()
            .filter_map(|member| world.players.get_by_uuid(member))
            .filter(|hunter| !hunter.is_dead_or_dying())
            .map(|hunter| hunter.position())
            .collect::<Vec<_>>();
        let inside = |room: &Room| {
            hunters
                .iter()
                .any(|hunter| room_contains(region, room, *hunter))
        };
        progress.tick_mobs(world, &hunters, tick);
        // Side chambers wake up when a hunter walks in.
        let waking = progress
            .layout
            .blueprint
            .rooms
            .iter()
            .enumerate()
            .filter(|(index, room)| {
                room.role == RoomRole::Chamber
                    && !progress.guarded_chambers.contains(index)
                    && inside(room)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        for index in waking {
            progress.spawn_guards(world, region, index, tick);
        }
        if progress.combat_room == 0 {
            progress.combat_room = 1;
            progress.wave_spawned = false;
        }
        if !progress.wave_spawned {
            // A wave waits in its room, unseen, until a hunter walks in.
            if progress
                .layout
                .blueprint
                .combat_room(progress.combat_room)
                .is_some_and(inside)
            {
                progress.spawn_wave(world, region, tick);
            }
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
            progress.wave_spawned = false;
            for member in members {
                if let Some(hunter) = world.players.get_by_uuid(member) {
                    hunter.send_message(
                        &TextComponent::plain(format!(
                            "Комната зачищена ({}/{}). Ищите следующую.",
                            progress.combat_room - 1,
                            progress.layout.combat_room_count
                        ))
                        .color(Color::Gray),
                    );
                }
            }
            return None;
        }
        match progress.boss {
            BossState::Waiting if !inside(progress.layout.blueprint.boss_room()) => {
                return None;
            }
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
        state.on_gate_cleared(&GateClear {
            scores: survivors
                .iter()
                .map(|survivor| {
                    progress
                        .contributions
                        .get(survivor)
                        .copied()
                        .unwrap_or_default()
                        .score()
                })
                .collect(),
            survivors: survivors.clone(),
            rank: progress.rank,
            red: progress.red,
            floor: progress.floor,
            seconds: tick.saturating_sub(started_tick) / TICKS_PER_SECOND,
            deaths: progress.deaths,
            party: progress.contributions.len(),
        });
        state.record_organization_event(survivors.clone(), Event::Boss(progress.rank));
        state.record_organization_event(survivors.clone(), Event::Dungeon(progress.rank));
        // One roll decides whether the squad gets a chest at all; every survivor
        // then receives the same kind, as the boss reward table specifies.
        progress.boss_chest = chest::boss_chest_for(progress.rank, rand::random_range(0..100));
        for survivor in &survivors {
            for stack in material::roll_boss(progress.rank, || rand::random_range(0..100)) {
                progress.boss_materials.push((*survivor, stack));
            }
            progress
                .boss_materials
                .push((*survivor, pouch::stack(pouch::boss_amount(progress.rank))));
            if rand::random_range(0..100) < key::BOSS_KEY_PERCENT {
                progress
                    .boss_materials
                    .push((*survivor, key::stack(progress.rank)));
            }
            if progress.rank >= HunterRank::B
                && rand::random_range(0..100) < castle::CASTLE_KEY_PERCENT
            {
                progress
                    .boss_materials
                    .push((*survivor, key::castle_stack()));
            }
            if let Some(floor) = progress.floor {
                for stack in castle::floor_loot(floor, || rand::random_range(0..100)) {
                    progress.boss_materials.push((*survivor, stack));
                }
            }
        }
        if let Some(floor) = progress.floor {
            let climbers: Vec<(Uuid, String)> = survivors
                .iter()
                .filter_map(|uuid| {
                    let player = world.players.get_by_uuid(uuid)?;
                    Some((*uuid, player.gameprofile.name().to_owned()))
                })
                .collect();
            state.record_castle_clear(&climbers, floor);
        }
        progress.survivors = survivors;
        Some(RunAction::StartMine)
    }

    fn tick_mine_run(
        progress: &mut MineProgress,
        context: &RunContext<'_>,
        tick: u64,
    ) -> Option<RunAction> {
        let RunContext {
            world: _,
            player,
            uuid,
            source_world,
            ..
        } = context;
        if progress.finished(tick) {
            return Some(RunAction::ReturnPlayer {
                uuid: *uuid,
                world: (*source_world).clone(),
            });
        }
        if opening_contains(progress.exit, player.position()) {
            return Some(RunAction::ReturnPlayer {
                uuid: *uuid,
                world: (*source_world).clone(),
            });
        }
        if tick.saturating_sub(progress.started_tick) % (20 * 60) == 0 {
            player.send_message(
                &TextComponent::plain(format!(
                    "Шахта: добыто {}/{} кристаллов, осталось {} мин.",
                    progress.mined,
                    progress.quota,
                    progress.remaining_ticks(tick) / 20 / 60
                ))
                .color(Color::Aqua),
            );
        }
        None
    }

    fn mine_participants(world: &World, region: InstanceRegion, members: &[Uuid]) -> Vec<Uuid> {
        members
            .iter()
            .filter(|member| {
                world.players.get_by_uuid(*member).is_some_and(|player| {
                    !player.is_dead_or_dying() && region.contains(BlockPos::from(player.position()))
                })
            })
            .copied()
            .collect()
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
                    RunProgress::Penalty(_) | RunProgress::Mine(_) => None,
                };
                for member in &run.members {
                    if let Some(member) = players.get(member) {
                        let _ = member.teleport(region.spawn, 0.0, 0.0);
                        if let Some(theme) = theme {
                            member.send_message(
                                &TextComponent::plain(format!("Подземелье: {theme}."))
                                    .color(Color::DarkPurple),
                            );
                        } else {
                            member.send_message(
                                &TextComponent::plain(
                                    "Штрафная зона: поднимитесь на вершину башни или продержитесь до конца отсчёта.",
                                )
                                .color(Color::Red),
                            );
                        }
                        if run.locked {
                            member.send_message(
                                &TextComponent::plain(
                                    "Красные врата заперты. Выход откроется только после смерти босса.",
                                )
                                .color(Color::Red),
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
                    RunProgress::Mine(ref progress) => {
                        let Some(world) = server.worlds.get(&run.instance_world) else {
                            continue;
                        };
                        actions.push(RunAction::CreditMine {
                            participants: Self::mine_participants(world, run.region, &members),
                            rank: progress.rank,
                            mined: progress.mined,
                        });
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
                        started_tick: run.started_tick,
                        world: &world,
                        player,
                        uuid,
                        members: &run.members,
                        region,
                        source_world: &source_world,
                    };
                    let action = Self::tick_dungeon_run(progress, &context, tick);
                    actions.extend(progress.pending_skills.drain(..).map(RunAction::MobSkill));
                    let start_mine = matches!(action, Some(RunAction::StartMine));
                    if !start_mine {
                        self.pending_loot.append(&mut progress.boss_materials);
                        if let Some(kind) = progress.boss_chest.take() {
                            for survivor in progress.survivors.drain(..) {
                                self.pending_loot.push((survivor, kind.stack()));
                            }
                        }
                    }
                    if let Some(action) = action {
                        if matches!(action, RunAction::BossAbility(_)) {
                            actions.push(action);
                            continue;
                        }
                        if matches!(action, RunAction::StartMine) {
                            let exit = progress.exit.expect("exit opened before mine");
                            run.progress =
                                RunProgress::Mine(MineProgress::from_dungeon(progress, tick, exit));
                            run.started_tick = tick;
                            run.duration_ticks = MINE_DURATION_TICKS;
                            for member in &run.members {
                                if let Some(member) = players.get(member) {
                                    member.send_message(
                                        &TextComponent::plain(
                                            "Босс повержен. Подземелье открыто как шахта — добудьте кристаллы.",
                                        )
                                        .color(Color::Gold),
                                    );
                                }
                            }
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
                RunProgress::Mine(progress) => {
                    let context = RunContext {
                        state,
                        started_tick: run.started_tick,
                        world: &world,
                        player,
                        uuid,
                        members: &run.members,
                        region,
                        source_world: &source_world,
                    };
                    let action = Self::tick_mine_run(progress, &context, tick);
                    self.pending_loot.append(&mut progress.boss_materials);
                    if let Some(kind) = progress.boss_chest.take() {
                        for survivor in progress.survivors.drain(..) {
                            self.pending_loot.push((survivor, kind.stack()));
                        }
                    }
                    if let Some(action) = action {
                        run.finishing = true;
                        match action {
                            RunAction::ReturnPlayer {
                                world: target_world,
                                ..
                            } => {
                                if let Some(world) = server.worlds.get(&target_world) {
                                    actions.push(RunAction::CreditMine {
                                        participants: Self::mine_participants(
                                            world, run.region, &members,
                                        ),
                                        rank: progress.rank,
                                        mined: progress.mined,
                                    });
                                }
                                actions.extend(members.into_iter().map(|uuid| {
                                    RunAction::ReturnPlayer {
                                        uuid,
                                        world: target_world.clone(),
                                    }
                                }));
                            }
                            action => actions.push(action),
                        }
                    }
                }
                RunProgress::Penalty(progress) => {
                    let hunters = members
                        .iter()
                        .filter_map(|member| world.players.get_by_uuid(member))
                        .filter(|hunter| !hunter.is_dead_or_dying())
                        .map(|hunter| hunter.position())
                        .collect::<Vec<_>>();
                    if hunters
                        .iter()
                        .any(|hunter| tower::reached_top(region, *hunter))
                    {
                        run.finishing = true;
                        for member in &members {
                            if let Some(hunter) = players.get(member) {
                                hunter.send_message(
                                    &TextComponent::plain(
                                        "Система: вершина башни достигнута. Штраф снят.",
                                    )
                                    .color(Color::Gold),
                                );
                            }
                        }
                        actions.extend(members.iter().copied().map(|uuid| {
                            RunAction::ReturnPlayer {
                                uuid,
                                world: source_world.clone(),
                            }
                        }));
                        continue;
                    }
                    Self::tick_tower(progress, &world, region, &hunters, tick);
                    actions.extend(progress.pending_skills.drain(..).map(RunAction::MobSkill));
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
            // A dead hunter cannot change worlds; the respawn returns them.
            if self.is_busy(uuid) || player.is_world_change_pending() || player.is_dead_or_dying() {
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
        RunAction::MobSkill(action) => action.apply(server),
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
        RunAction::StartMine => {}
        RunAction::CityDefenseWave { participants, rank } => {
            let reward = GATE_BREAK_WAVE_REWARD_BASE * rank_multiplier(*rank);
            for uuid in participants {
                state.credit_won(*uuid, reward);
                state.record_organization_event(vec![*uuid], Event::CityDefense(*rank));
                state.notify(*uuid, &format!("Волна прорыва отбита: +{reward} Won."));
            }
        }
        RunAction::CreditMine {
            participants,
            rank,
            mined,
        } => {
            if participants.is_empty() {
                return;
            }
            let total = u64::from(*mined) * crystal::sale_price(*rank);
            let share = total / participants.len() as u64;
            for uuid in participants {
                state.credit_won(*uuid, share);
                state.notify(*uuid, &format!("Шахтная смена завершена: +{share} Won."));
            }
        }
    }
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

/// Picks a site in one of the city gate plazas.
fn random_gate_candidate(center: BlockPos) -> BlockPos {
    city::gate_candidate(center)
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
        dx * dx + dz * dz < GATE_PLAYER_DISTANCE.powi(2)
    })
}

/// Explains that an owed penalty zone keeps a hunter out of gates.
fn penalty_refusal(wait_seconds: Option<u64>) -> String {
    match wait_seconds {
        Some(wait) => format!(
            "Сначала пройдите штрафную зону за невыполненное задание: она начнётся через {} мин. {} с.",
            wait / 60,
            wait % 60
        ),
        None => "Сначала пройдите штрафную зону за невыполненное задание: она начнётся сейчас."
            .to_owned(),
    }
}

fn announce_gate(server: &Server, gate: &Gate) {
    let color = if gate.red { Color::Red } else { Color::Aqua };
    let kind = if gate.red {
        "красные врата"
    } else {
        "врата"
    };
    let message = TextComponent::plain(format!(
        "Открыты {kind} ранга {}. Минимальный ранг охотника: {}. Врата активны {} минут.",
        gate.rank.name(),
        gate.minimum_hunter_rank().name(),
        super::UNUSED_GATE_LIFETIME_TICKS / (20 * 60)
    ))
    .color(color);
    notify_near_gate(server, gate, message);
}

fn announce_gate_expired(server: &Server, gate: &Gate) {
    notify_near_gate(
        server,
        gate,
        TextComponent::plain(format!(
            "Врата ранга {} закрылись и больше не активны.",
            gate.rank.name()
        ))
        .color(Color::Gray),
    );
}

fn announce_gate_consumed(server: &Server, gate: &Gate) {
    notify_near_gate(
        server,
        gate,
        TextComponent::plain(format!(
            "Врата ранга {} использованы и закрываются.",
            gate.rank.name()
        ))
        .color(Color::Gold),
    );
}

fn announce_gate_broken(server: &Server, gate: &Gate) {
    notify_near_gate(
        server,
        gate,
        TextComponent::plain(format!(
            "Врата ранга {} зачищены: рамка разобрана.",
            gate.rank.name()
        ))
        .color(Color::Green),
    );
}

fn announce_gate_break(server: &Server, gate: &Gate) {
    // A gate break is a city-wide emergency, so every hunter in the city hears
    // of it, not only the ones near the frame.
    let message = TextComponent::plain(format!(
        "Прорыв! Врата ранга {} открылись: монстры идут в город.",
        gate.rank.name()
    ))
    .color(Color::Red);
    for player in server.get_players() {
        if player.get_world().key == gate.world {
            player.send_message(&message);
        }
    }
}

fn notify_near_gate(server: &Server, gate: &Gate, message: TextComponent) {
    let center = DVec3::new(
        f64::from(gate.entrance.x()),
        f64::from(gate.entrance.y()),
        f64::from(gate.entrance.z()),
    );
    for player in server.get_players() {
        let world = player.get_world();
        if world.key == gate.world && player.position().distance_squared(center) <= 96.0_f64.powi(2)
        {
            player.send_message(&message);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use uuid::Uuid;

    use rustc_hash::FxHashMap;
    use steel_utils::BlockPos;

    use super::{BossState, DungeonProgress, DungeonRuntime, GeneratingEntry, MineProgress};
    use crate::dungeon::InstanceRegion;
    use crate::dungeon::skills::MobSkills;
    use crate::dungeon::{DungeonLayout, wave_for_gate};
    use crate::rank::HunterRank;

    #[test]
    fn generating_entry_marks_players_busy() {
        let mut runtime = DungeonRuntime::default();
        let leader = Uuid::new_v4();
        let member = Uuid::new_v4();
        runtime.generating_entries.insert(
            leader,
            GeneratingEntry {
                players: vec![leader, member],
                region: InstanceRegion::from_slot(0),
                gate_id: None,
                cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
        );

        assert!(runtime.is_busy(leader));
        assert!(runtime.is_busy(member));
        assert!(!runtime.is_busy(Uuid::new_v4()));
    }

    #[test]
    fn cancel_generating_entry_sets_shared_flag() {
        let mut runtime = DungeonRuntime::default();
        let leader = Uuid::new_v4();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        runtime.generating_entries.insert(
            leader,
            GeneratingEntry {
                players: vec![leader],
                region: InstanceRegion::from_slot(1),
                gate_id: Some(7),
                cancelled: Arc::clone(&cancelled),
            },
        );

        assert!(runtime.cancel_generating_entry(leader));
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(!runtime.cancel_generating_entry(Uuid::new_v4()));
    }

    #[test]
    fn mine_quota_matches_the_placed_veins() {
        let mut dungeon = DungeonProgress {
            rank: HunterRank::D,
            deaths: 0,
            floor: None,
            red: false,
            wave: wave_for_gate(HunterRank::D, false),
            layout: DungeonLayout::from_seed(1),
            combat_room: 0,
            remaining_mobs: Vec::new(),
            boss: BossState::Waiting,
            fight: None,
            exit: Some(BlockPos::new(10, 64, 10)),
            contributions: FxHashMap::default(),
            boss_chest: None,
            survivors: Vec::new(),
            labels: FxHashMap::default(),
            wave_spawned: false,
            guarded_chambers: Vec::new(),
            extra_mobs: Vec::new(),
            skills: MobSkills::default(),
            pending_skills: Vec::new(),
            classes: FxHashMap::default(),
            boss_materials: Vec::new(),
        };
        let exit = BlockPos::new(12, 64, 12);
        let mine = MineProgress::from_dungeon(&mut dungeon, 100, exit);
        assert_eq!(mine.rank, HunterRank::D);
        assert_eq!(mine.mined, 0);
        assert_eq!(mine.quota, HunterRank::D.crystal_vein_count() as u32);
        assert!(dungeon.boss_materials.is_empty());
        assert_eq!(mine.exit, exit);
    }
}
