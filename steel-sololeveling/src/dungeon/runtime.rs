//! Live gate and instance orchestration.

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
use steel_core::entity::{Entity, LivingEntity as _, MobEffectInstance, RemovalReason};
use steel_core::player::Player;
use steel_core::player::connection::NetworkConnection as _;
use steel_core::server::Server;
use steel_core::server::jobs::{JobPoll, ServerJob, ServerJobContext};
use steel_core::world::World;
use steel_protocol::packets::game::MapDecoration;
use steel_registry::data_components::components::MapId;
use steel_registry::item_stack::ItemStack;
use steel_registry::{vanilla_damage_types, vanilla_mob_effects};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, Identifier, WorldAabb};
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::content::Content;
use text_components::format::Color;
use uuid::Uuid;

use super::boss::{BossAbilityAction, BossFight};
use super::gate_site;
use super::gate_vortex::GateVortices;
use super::generator::{Room, RoomRole, Theme};
use super::instance::{
    FrameStyle, SpawnedMob, build_dungeon, dungeon_chunks, erase_footprint, footprint_chunks,
    gate_footprint, gate_frame_chunks, gate_frame_cleanup_complete, gate_frame_matches,
    gate_ground_is_level, gate_space_is_clear, open_dungeon_exit, opening_contains,
    place_gate_frame, remove_gate_frame, spawn_city_break_mob, spawn_dungeon_boss,
    spawn_dungeon_wave, spawn_gate_label, spawn_room_mobs,
};
use super::map::{self, DungeonMap, ZoneStatus};
use super::mobs::{MobClass, MobLabel};
use super::skills::{MobSkills, SkillAction, refresh_plates};
use super::tower;
use super::{
    DUNGEON_TIME_LIMIT_TICKS, DungeonLayout, DungeonWave, Footprint,
    GATE_BREAK_PARTICIPATION_RADIUS, GATE_BREAK_WAVE_INTERVAL_TICKS, GATE_BREAK_WAVES,
    GATE_SPAWN_INTERVAL_TICKS, GATES_PER_SPAWN_CYCLE, Gate, GateBreak, GatePhase, GateRegistry,
    GateRegistryChanges, GateRegistrySnapshot, GateRegistryWrite, GateRetirementReason,
    INSTANCE_WORLD_PATH, InstanceRegion, InstanceRegionAllocator, LegacyGateCleanup,
    MINE_DURATION_TICKS, PENALTY_ZONE_DURATION_TICKS, PENALTY_ZONE_WAVE_INTERVAL_TICKS,
    RED_GATE_CHANCE_PERCENT, TICKS_PER_SECOND, gate_break_wave_reward, penalty_wave_size,
    wave_for_gate, wave_for_rank,
};
use crate::castle;
use crate::city;
use crate::combat_reward::HunterReward;
use crate::demon_castle::elites::EliteAction;
use crate::demon_castle::{self, CastleSlot, CastleSlots};
use crate::item::chest::{self, ChestKind};
use crate::item::crystal;
use crate::item::{key, material, pouch};
use crate::organization::{Event, rank_multiplier};
use crate::party::RaidContribution;
use crate::rank::HunterRank;
use crate::state::{GateClear, SoloLevelingState};

mod castle_climb;

use castle_climb::{CastleClimb, FLOOR_TIME_LIMIT_TICKS, FloorBoss, TreasureOpen};

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
/// Radius around a hunter that already holds a gate, which makes the prototype
/// gate redundant.
const TEST_GATE_REUSE_RADIUS: f64 = 64.0;
/// Sites the prototype gate placement tries before giving up.
const TEST_GATE_MAX_ATTEMPTS: u8 = 8;
/// Unentered gates kept open for every rank a hunter online holds.
const DEMAND_GATES_PER_RANK: usize = 2;
/// Ordinary E-rank gates kept on the map even while their runs are active.
const MIN_E_GATES: usize = 3;
/// How long a rank may run short of gates before one is added.
const DEMAND_DELAY_TICKS: u64 = TICKS_PER_SECOND * 120;
/// How often the open gates are counted against the online ranks.
const DEMAND_CHECK_TICKS: u64 = TICKS_PER_SECOND * 10;
/// Gates the demand check never adds beyond, in the main world.
const MAX_LIVE_GATES: usize = 18;
/// Sites one spawn cycle may test before settling for fewer gates, so a crowded
/// world cannot keep the scheduler busy forever.
const GATE_MAX_SITE_ATTEMPTS: u8 = 12;
/// Health shares below which the snow elf chieftain calls its tribe.
const TRIBE_CALLS: [f32; 2] = [0.66, 0.33];
/// Ticks between two checks that every standing gate shows its swirl.
const VORTEX_SYNC_TICKS: u64 = 20;
/// Ticks between repeated explanations of why a gate refuses a hunter.
const GATE_REFUSAL_INTERVAL_TICKS: u64 = 60;

/// Share of an ordinary dungeon-mob kill that a Demon Castle floor mob pays
/// in experience and system points.
///
/// The castle is an endless climb: its mobs funnel into one hall, waves
/// repeat every floor, and every floor clear already pays loot and ranking
/// progress on top, so per-kill currency runs at half rate to keep the
/// endless farm from outpacing finite gates. Floor-clear and ranking
/// payouts are settled separately and stay untouched.
const CASTLE_MOB_REWARD_PERCENT: u64 = 50;

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
    /// Executes a cache guard's class kit with its current targets.
    EliteSkill {
        /// Ability prepared by the cache guard.
        action: EliteAction,
        /// Hunters whose bodies and allied shadows may be hit.
        members: Vec<Uuid>,
    },
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
    /// Copies of the Demon Castle handed to climbing parties.
    castle_slots: CastleSlots,
    /// Live runs by their unique run id, never by a player id: a run outlives
    /// any single member (the party leaves one by one), so a leader who died
    /// and entered new gates must not replace the run his party still fights.
    runs: FxHashMap<RunId, InstanceRun>,
    /// Chunk tickets keeping each run's region loaded for as long as the run
    /// lives, stored apart from [`InstanceRun`] as the RAII lease they are
    /// and released with the run.
    run_tickets: FxHashMap<RunId, ChunkRequestHandle>,
    /// Next run id; ids are handed out once and never reused.
    next_run_id: RunId,
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
    /// Abilities and nameplates of gate-break mobs, by gate id; break mobs
    /// live in the city world, outside instance runs.
    gate_break_tracks: FxHashMap<u64, GateBreakTrack>,
    /// The swirls filling the openings of standing gates.
    vortices: GateVortices,
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
            .field("castle_slots", &self.castle_slots)
            .field("runs", &self.runs.len())
            .field("run_tickets", &self.run_tickets.len())
            .field("next_run_id", &self.next_run_id)
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
            .field("gate_break_tracks", &self.gate_break_tracks.len())
            .finish_non_exhaustive()
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

/// Lightweight bookkeeping for one dungeon/penalty/castle generation job.
///
/// The heavy region preparation lives in [`DungeonGenerationJob`] so the
/// runtime lock is not held while blocks are written.
struct GeneratingEntry {
    /// Players waiting for this instance; their UUIDs are enough to detect busyness.
    players: Vec<Uuid>,
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
    /// What the build left in the instance region; a castle floor leaves
    /// nothing, its world regenerates it.
    footprint: Option<Footprint>,
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
        footprint: Option<Footprint>,
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
        self.push_result(None, 0, 0, 0, 0, true);
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
                let start = self.start.as_ref().expect("generation job missing start");
                let erase_start = Instant::now();
                // A castle floor is built by its world's generator; the job
                // only waited for the floor's chunks.
                if !matches!(start, RunStart::Castle { .. }) {
                    erase_footprint(instance_world, self.region, self.previous);
                }
                let erase_micros = erase_start.elapsed().as_micros() as u64;

                let (footprint, block_count, build_micros) = match start {
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
                            Some(layout.footprint()),
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
                        (Some(Footprint::Tower), 0, build_micros)
                    }
                    RunStart::Castle { gate, .. } => {
                        log::info!(
                            "sololeveling: castle floor {} is ready (slot {}, {} chunks)",
                            gate.floor.unwrap_or(1),
                            self.region.slot,
                            chunk_count
                        );
                        (None, 0, 0)
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
    /// A Demon Castle climb starting at the floor the portal names, in the
    /// castle world rather than the instance world.
    Castle { gate: Gate },
    /// A penalty-zone challenge for an unfinished system quest.
    Penalty,
}

impl RunStart {
    /// Returns whether the entry climbs the Demon Castle.
    const fn is_castle(&self) -> bool {
        matches!(self, Self::Castle { .. })
    }

    /// Returns the gate the entry consumed, if any.
    const fn gate(&self) -> Option<&Gate> {
        match self {
            Self::Gate { gate, .. } | Self::Castle { gate, .. } => Some(gate),
            Self::Penalty => None,
        }
    }
}

/// Unique identity of one live instance run.
///
/// Never a player uuid: a run outlives any single member (the party leaves
/// one by one after a death), so keying runs by the entering leader would
/// let his next gate entry replace the run his party is still fighting in.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
struct RunId(u64);

struct InstanceRun {
    /// Members who entered this particular instance, in party join order.
    members: Vec<Uuid>,
    source_world: Identifier,
    instance_world: Identifier,
    region: InstanceRegion,
    started_tick: u64,
    duration_ticks: u64,
    progress: RunProgress,
    /// Live map for castle and ordinary-dungeon runs.
    map: Option<DungeonMap>,
    /// Gate consumed by this run, removed from the world when the run ends.
    gate: Option<u64>,
    /// Members already teleported to the region spawn. A member is placed at
    /// most once, on the first tick their world change into the instance
    /// world has completed; the rest of the run never re-places anyone.
    placed: FxHashSet<Uuid>,
    /// Set once an action was queued so the run stops ticking before it ends.
    finishing: bool,
    /// Whether the entrance is locked: no voluntary exit until the boss falls.
    locked: bool,
}

enum RunProgress {
    Dungeon(Box<DungeonProgress>),
    /// The Demon Castle climb: floor after floor until everyone falls or leaves.
    Castle(Box<CastleClimb>),
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
    /// Times the snow elf chieftain has called its tribe to the arena.
    tribe_calls: u8,
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
    /// Label above the exit, removed with the run.
    visuals: Vec<i32>,
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
    /// Room each chamber guard or hatched larva belongs to.
    extra_mob_rooms: FxHashMap<i32, usize>,
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
        for mob in &spawned {
            self.extra_mobs.push(mob.id);
            self.extra_mob_rooms.insert(mob.id, index);
        }
        self.adopt(spawned, tick);
    }

    /// Lets the snow elf chieftain call its tribe as it weakens: below two
    /// thirds of its health and again below one third, when it also flies
    /// into a frenzy.
    fn call_the_tribe(
        &mut self,
        world: &Arc<World>,
        region: InstanceRegion,
        boss_id: i32,
        members: &[Uuid],
        tick: u64,
    ) {
        let Some(&threshold) = TRIBE_CALLS.get(usize::from(self.tribe_calls)) else {
            return;
        };
        let Some(boss) = world.get_entity_by_id(boss_id) else {
            return;
        };
        let Some(living) = boss.as_living_entity() else {
            return;
        };
        if living.get_health() > living.get_max_health() * threshold {
            return;
        }
        self.tribe_calls += 1;
        let blueprint = Arc::clone(&self.layout.blueprint);
        let Some(index) = blueprint
            .rooms
            .iter()
            .position(|room| room.role == RoomRole::Boss)
        else {
            return;
        };
        let count = (self.wave.mob_count / 3).max(3);
        let salt =
            self.layout.seed ^ u64::from(self.tribe_calls).wrapping_mul(0xA076_1D64_78BD_642F);
        let spawned = spawn_room_mobs(
            world,
            region,
            blueprint.theme,
            &blueprint.rooms[index],
            count,
            self.wave,
            self.rank,
            salt,
        );
        for mob in &spawned {
            self.extra_mobs.push(mob.id);
            self.extra_mob_rooms.insert(mob.id, index);
        }
        self.adopt(spawned, tick);
        let frenzy = usize::from(self.tribe_calls) == TRIBE_CALLS.len();
        if frenzy {
            for (effect, amplifier) in [
                (vanilla_mob_effects::SPEED, 1),
                (vanilla_mob_effects::STRENGTH, 0),
            ] {
                let _ = living.add_mob_effect(MobEffectInstance::with_duration(
                    effect,
                    i32::MAX,
                    amplifier,
                ));
            }
        }
        let line = if frenzy {
            "Хьярн в неистовстве и зовёт последних воинов племени!"
        } else {
            "Хьярн трубит в рог: племя спешит на помощь вождю!"
        };
        for member in members {
            if let Some(hunter) = world.players.get_by_uuid(member) {
                hunter.send_action_bar(TextComponent::plain(line).color(Color::Aqua));
            }
        }
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
            self.extra_mob_rooms
                .insert(id, usize::from(self.combat_room));
            self.labels.insert(id, label);
        }
        if tick.is_multiple_of(10) {
            self.labels
                .retain(|id, _| world.get_entity_by_id(*id).is_some());
            refresh_plates(world, hunters, self.labels.keys());
        }
    }

    fn map_canvas(&self) -> [u8; map::SIZE * map::SIZE] {
        let statuses = self
            .layout
            .blueprint
            .rooms
            .iter()
            .enumerate()
            .map(|(index, room)| match room.role {
                RoomRole::Entrance => ZoneStatus::Cleared,
                RoomRole::Combat => {
                    let current = usize::from(self.combat_room);
                    if index < current
                        || (index == current && self.wave_spawned && self.remaining_mobs.is_empty())
                    {
                        ZoneStatus::Cleared
                    } else if index == current {
                        ZoneStatus::Combat
                    } else {
                        ZoneStatus::Unexplored
                    }
                }
                RoomRole::Chamber => {
                    let alive = self
                        .extra_mobs
                        .iter()
                        .any(|id| self.extra_mob_rooms.get(id) == Some(&index));
                    if alive {
                        ZoneStatus::Combat
                    } else if self.guarded_chambers.contains(&index) {
                        ZoneStatus::Cleared
                    } else {
                        ZoneStatus::Unexplored
                    }
                }
                RoomRole::Boss => match self.boss {
                    BossState::Defeated => ZoneStatus::ThroneCleared,
                    BossState::Waiting | BossState::Alive(_) => ZoneStatus::Throne,
                },
            })
            .collect::<Vec<_>>();
        map::gate::canvas(&self.layout.blueprint, &statuses)
    }

    fn map_decorations(
        &self,
        world: &Arc<World>,
        region: InstanceRegion,
        viewer: &Player,
        members: &[Uuid],
    ) -> Vec<MapDecoration> {
        let projection = map::gate::GateProjection::new(&self.layout.blueprint);
        let viewer_id = viewer.gameprofile.id();
        let point = |position: DVec3| {
            projection.point(
                (position.x - region.spawn.x.floor()).floor() as i32,
                (position.z - region.spawn.z.floor()).floor() as i32,
            )
        };
        let mut decorations = Vec::new();
        for member in members {
            let Some(hunter) = world.players.get_by_uuid(member) else {
                continue;
            };
            let (x, y) = point(hunter.position());
            decorations.push(map::decoration(
                if *member == viewer_id {
                    map::PLAYER
                } else {
                    map::FRAME
                },
                x,
                y,
                map::player_rotation(hunter.rotation().0),
                None,
            ));
        }
        if let BossState::Alive(id) = self.boss
            && let Some(boss) = world.get_entity_by_id(id)
        {
            let (x, y) = point(boss.position());
            decorations.push(map::decoration(map::RED_MARKER, x, y, 0, Some("Босс")));
        }
        if let Some(exit) = self.exit {
            let (x, y) = point(DVec3::new(
                f64::from(exit.x()) + 0.5,
                f64::from(exit.y()),
                f64::from(exit.z()) + 0.5,
            ));
            decorations.push(map::decoration(
                map::BANNER_LIGHT_BLUE,
                x,
                y,
                0,
                Some("Выход"),
            ));
        }
        decorations
    }
}

/// Live mobs of one breaking gate: their abilities, nameplates and classes.
///
/// Break mobs spawn in the city world, not inside an instance run, so the run
/// bookkeeping cannot adopt them; this mirrors the slice of
/// [`DungeonProgress`] a gate break needs.
#[derive(Debug, Default)]
struct GateBreakTrack {
    /// World the break's mobs live in, to match nameplates by world.
    world: Identifier,
    /// Blast damage of a bomber's explosion, from the gate rank's wave.
    blast_power: f32,
    /// Abilities of the break's shamans, bombers and broodmothers.
    skills: MobSkills,
    /// Nameplates by entity id, refreshed after damage.
    labels: FxHashMap<i32, MobLabel>,
    /// Classes by entity id.
    classes: FxHashMap<i32, MobClass>,
    /// Abilities to apply once the runtime lock is released.
    pending_skills: Vec<SkillAction>,
}

impl GateBreakTrack {
    /// Starts tracking `spawned` wave mobs: class abilities and nameplates.
    fn adopt(&mut self, spawned: Vec<SpawnedMob>, tick: u64) {
        for mob in spawned {
            self.skills.track(mob.id, mob.class, tick);
            self.labels.insert(mob.id, mob.label);
            self.classes.insert(mob.id, mob.class);
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

impl RunProgress {
    /// Returns the rank the run is fought and rewarded at.
    fn rank(&self) -> Option<HunterRank> {
        match self {
            Self::Dungeon(progress) => Some(progress.rank),
            Self::Castle(progress) => Some(castle::rank_for_floor(progress.floor)),
            Self::Mine(progress) => Some(progress.rank),
            Self::Penalty(_) => None,
        }
    }

    /// Returns the entity id of the run's living boss.
    const fn live_boss(&self) -> Option<i32> {
        match self {
            Self::Dungeon(progress) => match progress.boss {
                BossState::Alive(id) => Some(id),
                _ => None,
            },
            Self::Castle(progress) => match progress.boss {
                FloorBoss::Alive(id) => Some(id),
                _ => None,
            },
            Self::Penalty(_) | Self::Mine(_) => None,
        }
    }
}

impl InstanceRun {
    /// Ends `member`'s participation in the run.
    ///
    /// Returns `true` once the last member has left: a run outlives any
    /// single member, so one departure never ends the dungeon for the rest
    /// of the party.
    fn finish_member(&mut self, member: Uuid) -> bool {
        self.members.retain(|m| *m != member);
        self.members.is_empty()
    }

    /// Teleports one member to the entry point of its run and announces it.
    fn place_member(&self, member: &Player) {
        let target = match &self.progress {
            RunProgress::Castle(progress) => progress.entry_position(),
            _ => self.region.spawn,
        };
        let _ = member.teleport(target, 0.0, 0.0);
        member.send_message(&self.entry_announcement());
        if self.locked {
            member.send_message(
                &TextComponent::plain(
                    "Красные врата заперты. Выход откроется только после смерти босса.",
                )
                .color(Color::Red),
            );
        }
    }

    /// The entry line for a freshly placed member: the dungeon or castle
    /// theme, or the penalty-zone warning.
    fn entry_announcement(&self) -> TextComponent {
        match &self.progress {
            RunProgress::Dungeon(progress) => TextComponent::plain(format!(
                "Подземелье: {}.",
                progress.layout.blueprint.theme.display_name()
            ))
            .color(Color::DarkPurple),
            RunProgress::Castle(progress) => progress.announcement(),
            RunProgress::Penalty(_) => TextComponent::plain(
                "Штрафная зона: поднимитесь на вершину башни или продержитесь до конца отсчёта.",
            )
            .color(Color::Red),
            RunProgress::Mine(_) => TextComponent::plain(
                "Босс повержен. Подземелье открыто как шахта — добудьте кристаллы.",
            )
            .color(Color::Gold),
        }
    }

    /// Removes the run's live mob ids, so their entities can be discarded.
    fn take_mobs(&mut self) -> Vec<i32> {
        match &mut self.progress {
            RunProgress::Dungeon(progress) => {
                let mut mobs = mem::take(&mut progress.remaining_mobs);
                mobs.append(&mut progress.extra_mobs);
                mobs.append(&mut progress.visuals);
                if let BossState::Alive(id) = progress.boss {
                    mobs.push(id);
                }
                mobs
            }
            RunProgress::Castle(progress) => progress.take_mobs(),
            RunProgress::Penalty(progress) => mem::take(&mut progress.remaining_mobs),
            RunProgress::Mine(_) => Vec::new(),
        }
    }

    /// Removes the boss bar the run showed, if its boss ever appeared.
    ///
    /// The client keeps a boss bar until it is removed, even across the death
    /// screen, so it outlives the run otherwise.
    fn hide_boss_bar(&self, player: &Player) {
        let fight = match &self.progress {
            RunProgress::Dungeon(progress) => progress.fight.as_ref(),
            RunProgress::Castle(progress) => progress.fight.as_ref(),
            RunProgress::Penalty(_) | RunProgress::Mine(_) => None,
        };
        if let Some(fight) = fight {
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
        self.gates.restore(snapshot, 0)?;
        // Breaks are disabled: restored waves never resume, so their old
        // absolute tick schedule is irrelevant to the one-time retirement.
        for break_state in self.gates.breaks_mut() {
            break_state.next_wave_tick = 0;
        }
        Ok(())
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
                "sololeveling: created {}-rank gate {} in {} at {:?}",
                gate.rank.name(),
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
        if tick.is_multiple_of(VORTEX_SYNC_TICKS) {
            self.vortices.sync(server, self.gates.values());
        }
        self.retire_disabled_breaks(server);
        let mut actions = self.tick_runs(state, server, tick);
        actions.extend(self.return_strays(server));
        actions
    }

    /// Returns the map id of the mapped run `player` is currently inside.
    #[must_use]
    pub fn map_id_for(&self, player: &Player) -> Option<MapId> {
        let uuid = player.gameprofile.id();
        let world = player.get_world();
        self.runs
            .values()
            .find(|run| run.instance_world == world.key && run.members.contains(&uuid))
            .and_then(|run| run.map.as_ref().map(DungeonMap::id))
    }

    /// Returns the effective rank of the dungeon run inside `world`.
    ///
    /// The run of a red gate fights one rank above the gate, so this is the
    /// rank the run is fought and rewarded at.
    #[must_use]
    pub fn instance_rank(&self, world: &World) -> Option<HunterRank> {
        self.runs
            .values()
            .find_map(|run| (run.instance_world == world.key).then(|| run.progress.rank())?)
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
            if let RunProgress::Castle(progress) = &mut run.progress {
                if progress.boss == FloorBoss::Alive(entity_id) {
                    progress.boss = FloorBoss::Defeated;
                    return;
                }
                if progress.on_elite_death(entity_id, &run.members) {
                    for member in &run.members {
                        if let Some(hunter) = world.players.get_by_uuid(member) {
                            hunter.send_action_bar(
                                TextComponent::plain(
                                    "Элита повержена. Страж этажа ждёт в тронном зале — он отмечен на карте",
                                )
                                .color(Color::Gold),
                            );
                        }
                    }
                }
            }
            let remaining = match &mut run.progress {
                RunProgress::Dungeon(progress) => &mut progress.remaining_mobs,
                RunProgress::Castle(progress) => &mut progress.remaining_mobs,
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
        for break_state in self.gates.breaks_mut() {
            if let Some(index) = break_state
                .remaining_mobs
                .iter()
                .position(|id| *id == entity_id)
            {
                break_state.remaining_mobs.swap_remove(index);
                break;
            }
        }
    }

    /// Returns the nameplate of a dungeon, penalty or gate-break mob in `world`.
    ///
    /// Gate-break mobs live in the city world outside instance runs, so their
    /// plates are kept in `gate_break_tracks`; both kinds are returned here
    /// so a hurt mob's plate is refreshed through the same hook path.
    #[must_use]
    pub fn mob_label(&self, world: &World, entity_id: i32) -> Option<MobLabel> {
        let run_label = self.runs.values().find_map(|run| {
            if run.instance_world != world.key {
                return None;
            }
            match &run.progress {
                RunProgress::Dungeon(progress) => progress.labels.get(&entity_id).cloned(),
                RunProgress::Castle(progress) => progress.labels.get(&entity_id).cloned(),
                RunProgress::Penalty(progress) => progress.labels.get(&entity_id).cloned(),
                RunProgress::Mine(_) => None,
            }
        });
        run_label.or_else(|| {
            self.gate_break_tracks.values().find_map(|track| {
                (track.world == world.key)
                    .then(|| track.labels.get(&entity_id))
                    .flatten()
                    .cloned()
            })
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
                RunProgress::Castle(progress) => {
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

    /// Returns the reduced kill reward for a Demon Castle floor mob of the
    /// climb `hunter` fights in `world`, or `None` for any other mob.
    ///
    /// Castle mobs are no ordinary dungeon mobs (`ordinary_mob_rank` skips
    /// them): their climb already pays floor loot and ranking progress on
    /// every clear, so the per-kill currency runs at
    /// [`CASTLE_MOB_REWARD_PERCENT`].
    #[must_use]
    pub fn castle_mob_reward(
        &self,
        world: &World,
        entity_id: i32,
        hunter: Uuid,
    ) -> Option<HunterReward> {
        self.castle_mob_reward_key(&world.key, entity_id, hunter)
    }

    fn castle_mob_reward_key(
        &self,
        world_key: &Identifier,
        entity_id: i32,
        hunter: Uuid,
    ) -> Option<HunterReward> {
        for run in self.runs.values() {
            if run.instance_world != *world_key || !run.members.contains(&hunter) {
                continue;
            }
            let RunProgress::Castle(progress) = &run.progress else {
                continue;
            };
            // Both lookups must hit: `on_living_death` drops the id from
            // `remaining_mobs` once the kill is observed, which keeps a
            // duplicate death event from paying twice before the plates are
            // pruned.
            if !progress.tracks(entity_id) {
                continue;
            }
            let mut reward = HunterReward::for_mob(castle::rank_for_floor(progress.floor));
            reward.experience = reward.experience * CASTLE_MOB_REWARD_PERCENT / 100;
            reward.system_points = reward.system_points * CASTLE_MOB_REWARD_PERCENT / 100;
            return Some(reward);
        }
        None
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

    /// Returns the rank of the run whose living boss is `entity_id`, or
    /// `None` when it is no run's boss.
    ///
    /// Several runs share one world (every castle climb does), so a boss is
    /// matched by its id rather than by the world alone.
    #[must_use]
    pub fn boss_rank(&self, world: &World, entity_id: i32) -> Option<HunterRank> {
        self.runs.values().find_map(|run| {
            (run.instance_world == world.key && run.progress.live_boss() == Some(entity_id))
                .then(|| run.progress.rank())?
        })
    }

    /// Opens the castle treasure cache at `pos` for `hunter`; returns whether
    /// they got loot, or `None` when no cache of their climb stands there.
    pub fn open_castle_treasure(
        &mut self,
        world: &World,
        pos: BlockPos,
        hunter: Uuid,
    ) -> Option<bool> {
        let opened = self.runs.values_mut().find_map(|run| {
            if run.instance_world != world.key || !run.members.contains(&hunter) {
                return None;
            }
            let RunProgress::Castle(progress) = &mut run.progress else {
                return None;
            };
            progress.open_treasure(pos, hunter)
        })?;
        Some(match opened {
            TreasureOpen::Looted(loot) => {
                self.pending_loot
                    .extend(loot.into_iter().map(|stack| (hunter, stack)));
                true
            }
            TreasureOpen::Empty => false,
        })
    }

    /// Returns the living boss of the run `hunter` fights in.
    #[must_use]
    pub fn boss_of(&self, hunter: Uuid) -> Option<i32> {
        self.runs
            .values()
            .find(|run| run.members.contains(&hunter))
            .and_then(|run| run.progress.live_boss())
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
        let last = run.finish_member(uuid);
        let death = match &mut run.progress {
            RunProgress::Dungeon(progress) => {
                progress.deaths += 1;
                RunDeath::DungeonFailed
            }
            // A castle death ends the climb with the same fall back to the
            // checkpoint the entry already moved the hunter to.
            RunProgress::Castle(_) => RunDeath::DungeonFailed,
            RunProgress::Penalty(_) => RunDeath::PenaltyFailed {
                roll: rand::rng().random_range(0..5),
            },
            RunProgress::Mine(_) => RunDeath::None,
        };
        if !last {
            log::debug!(
                "sololeveling: hunter {uuid} died inside an instance; {} participant(s) remain",
                run.members.len()
            );
            return death;
        }
        let Some(run) = self.take_run(run_key) else {
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

    /// Ends the participation of the player whose end action was applied.
    ///
    /// Only that member leaves the run: with a party inside, the run lives
    /// until its last member has left, so one return never tears the dungeon
    /// down for everyone else.
    pub fn complete_run(&mut self, player: &Player) {
        let player_uuid = player.gameprofile.id();
        let Some(run_key) = self
            .runs
            .iter()
            .find_map(|(key, run)| run.members.contains(&player_uuid).then_some(*key))
        else {
            return;
        };
        let last = {
            let Some(run) = self.runs.get_mut(&run_key) else {
                return;
            };
            run.hide_boss_bar(player);
            run.finish_member(player_uuid)
        };
        if !last {
            return;
        }
        let Some(run) = self.take_run(run_key) else {
            return;
        };
        self.release_run(run);
    }

    /// Ends the participation of a hunter that is no longer online, whose
    /// client can no longer receive a boss bar removal.
    pub fn complete_offline_run(&mut self, player_uuid: Uuid) {
        let Some(run_key) = self
            .runs
            .iter()
            .find_map(|(key, run)| run.members.contains(&player_uuid).then_some(*key))
        else {
            return;
        };
        let last = {
            let Some(run) = self.runs.get_mut(&run_key) else {
                return;
            };
            run.finish_member(player_uuid)
        };
        if !last {
            return;
        }
        let Some(run) = self.take_run(run_key) else {
            return;
        };
        self.release_run(run);
    }

    /// Frees the region, gate, and mobs a finished run held.
    fn release_run(&mut self, mut run: InstanceRun) {
        self.free_region(run.region, matches!(run.progress, RunProgress::Castle(_)));
        self.spend_gate(run.gate);
        self.spend_mobs(&mut run);
    }

    /// Gives a region back to the allocator it came from: castle climbs hold a
    /// castle slot in the region's place.
    fn free_region(&mut self, region: InstanceRegion, castle: bool) {
        if castle {
            self.castle_slots.release(CastleSlot(region.slot));
        } else {
            self.regions.release(region);
        }
    }

    /// Admits a finished generation as a live run under a fresh id.
    ///
    /// The id identifies the run, never the entering leader: his next entry
    /// after a death starts a second run next to the one his party still
    /// fights in, and two live runs must not collide on one key.
    fn adopt_run(&mut self, mut run: InstanceRun) -> RunId {
        let id = self.next_run_id;
        self.next_run_id.0 += 1;
        if matches!(
            run.progress,
            RunProgress::Dungeon(_) | RunProgress::Castle(_)
        ) {
            run.map = Some(DungeonMap::new(id.0));
        }
        self.runs.insert(id, run);
        id
    }

    /// Removes a finished run and releases the chunk tickets that kept its
    /// region loaded.
    fn take_run(&mut self, id: RunId) -> Option<InstanceRun> {
        self.run_tickets.remove(&id);
        self.runs.remove(&id)
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
        self.gates.expire_unused_at(now);
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

    /// Positions of the hunters a breaking gate's mobs react to and whose
    /// plates they show for: the same defenders the wave reward pays.
    fn gate_break_hunter_positions(server: &Server, gate: &Gate) -> Vec<DVec3> {
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
            .map(|player| player.position())
            .collect()
    }

    fn retire_disabled_breaks(&mut self, server: &Arc<Server>) {
        for break_state in self.gates.breaks() {
            let id = break_state.gate.id;
            if break_state.wave > 0
                && let Some(world) = server.worlds.get(&break_state.gate.world)
                && let Some(active) = self.gates.break_mut(id)
            {
                Self::adopt_gate_break_orphans(&mut self.gate_break_tracks, active, world, 0);
            }
            if let Some(track) = self.gate_break_tracks.remove(&id) {
                for entity_id in track.labels.keys() {
                    if let Some(entity) = server
                        .worlds
                        .get(&track.world)
                        .and_then(|world| world.get_entity_by_id(*entity_id))
                    {
                        entity.set_removed(RemovalReason::Discarded);
                    }
                }
            }
            for entity_id in break_state.remaining_mobs {
                if let Some(entity) = server
                    .worlds
                    .get(&break_state.gate.world)
                    .and_then(|world| world.get_entity_by_id(entity_id))
                {
                    entity.set_removed(RemovalReason::Discarded);
                }
            }
            let Some(gate) = self.gates.clear_break(id) else {
                continue;
            };
            let Some(world) = server.worlds.get(&gate.world) else {
                continue;
            };
            remove_gate_frame(world, &gate);
            if gate_frame_cleanup_complete(world, &gate) {
                self.gates.acknowledge_gate_cleanup(id);
            } else {
                self.queue_gate_cleanup(Arc::clone(world), gate);
            }
        }
    }

    #[expect(dead_code, reason = "Прорывы временно отключены")]
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
            // A mob out of sight is unloaded, not dead: keep it on the wave.
            // Only mobs removed from the world count as fallen; deaths also
            // remove ids explicitly in `on_living_death`.
            break_state.remaining_mobs.retain(|id| {
                world
                    .get_entity_by_id(*id)
                    .is_none_or(|entity| !entity.is_removed())
            });
            if break_state.wave > 0 && tick.is_multiple_of(20) {
                Self::adopt_gate_break_orphans(
                    &mut self.gate_break_tracks,
                    break_state,
                    world,
                    tick,
                );
            }
            Self::tick_gate_break_mobs(
                &mut self.gate_break_tracks,
                server,
                break_state,
                world,
                tick,
                &mut actions,
            );
            if !break_state.remaining_mobs.is_empty() {
                continue;
            }
            // The defense reward is a one-time transition: between waves the
            // list is empty for the whole interval, and an empty list must
            // never pay twice.
            if break_state.take_wave_reward() {
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
            // Waves keep their interval even when the previous one fell fast;
            // without the gate they spawn back to back and swamp the defenders.
            if !break_state.wave_due(tick) {
                continue;
            }
            break_state.wave += 1;
            break_state.next_wave_tick = tick + GATE_BREAK_WAVE_INTERVAL_TICKS;
            let wave = wave_for_gate(break_state.gate.rank, break_state.gate.red);
            let theme = Theme::Crypt;
            let count = wave.mob_count / 3;
            let mut spawned = Vec::new();
            for _ in 0..count {
                if let Some(mob) = spawn_city_break_mob(
                    world,
                    break_state.gate.entrance,
                    wave,
                    break_state.gate.rank,
                    theme,
                ) {
                    break_state.remaining_mobs.push(mob.id);
                    spawned.push(mob);
                }
            }
            // The same blast power a dungeon wave of this rank casts with.
            let blast_power = (5.0 + 5.0 * wave.damage_multiplier) as f32;
            let track = self
                .gate_break_tracks
                .entry(break_state.gate.id)
                .or_insert_with(|| GateBreakTrack {
                    world: break_state.gate.world.clone(),
                    ..GateBreakTrack::default()
                });
            track.blast_power = blast_power;
            track.adopt(spawned, tick);
        }
        for id in cleared {
            if let Some(gate) = self.gates.clear_break(id) {
                // Larvae hatched by a broodmother outlive the wave they came
                // with; discard them like a finished run discards its extras.
                if let (Some(track), Some(world)) = (
                    self.gate_break_tracks.remove(&id),
                    server.worlds.get(&gate.world),
                ) {
                    for entity_id in track.labels.keys() {
                        if let Some(entity) = world.get_entity_by_id(*entity_id) {
                            entity.set_removed(RemovalReason::Discarded);
                        }
                    }
                }
                // The fight just happened here, so the frame chunks are
                // loaded: remove the frame right away instead of waiting for
                // the asynchronous cleanup, and retire the tombstone when the
                // site is verified clear.
                if let Some(world) = server.worlds.get(&gate.world) {
                    remove_gate_frame(world, &gate);
                    if gate_frame_cleanup_complete(world, &gate)
                        && self.gates.acknowledge_gate_cleanup(id)
                    {
                        log::info!(
                            "sololeveling: removed the gate frame in {} at {:?}",
                            gate.world,
                            gate.entrance
                        );
                    }
                }
                log::info!(
                    "sololeveling: gate break cleared in {} at {:?}",
                    gate.world,
                    gate.entrance
                );
            }
        }
        actions
    }

    /// Runs the abilities and nameplate visibility of one break's live mobs.
    ///
    /// Class abilities mirror a dungeon wave: the mobs were spawned by the
    /// same [`spawn_city_break_mob`], only their skill state lives here
    /// instead of in a run's [`DungeonProgress`].
    fn tick_gate_break_mobs(
        tracks: &mut FxHashMap<u64, GateBreakTrack>,
        server: &Server,
        break_state: &mut GateBreak,
        world: &Arc<World>,
        tick: u64,
        actions: &mut Vec<RunAction>,
    ) {
        let Some(track) = tracks.get_mut(&break_state.gate.id) else {
            return;
        };
        let hunters = Self::gate_break_hunter_positions(server, &break_state.gate);
        let power = track.blast_power;
        let out = track
            .skills
            .tick(world, &hunters, &break_state.remaining_mobs, power, tick);
        track.pending_skills.extend(out.actions);
        actions.extend(track.pending_skills.drain(..).map(RunAction::MobSkill));
        for (id, label) in out.hatched {
            // Like a dungeon run's larvae: fought for loot, but a wave is
            // cleared by its spawned mobs, not by the brood it hatched.
            track.labels.insert(id, label);
        }
        if tick.is_multiple_of(10) {
            track
                .labels
                .retain(|id, _| world.get_entity_by_id(*id).is_some());
            track
                .classes
                .retain(|id, _| world.get_entity_by_id(*id).is_some());
            refresh_plates(world, &hunters, track.labels.keys());
        }
    }

    /// Adopts break wave mobs that lost their track back into the break.
    ///
    /// The break state persists across restarts, but its mobs live in chunks,
    /// so after a restore they are back in the world untracked. Adoption also
    /// lets disabled breaks find and discard those mobs without resuming them.
    fn adopt_gate_break_orphans(
        tracks: &mut FxHashMap<u64, GateBreakTrack>,
        break_state: &mut GateBreak,
        world: &Arc<World>,
        tick: u64,
    ) {
        let center = DVec3::new(
            f64::from(break_state.gate.entrance.x()),
            f64::from(break_state.gate.entrance.y()),
            f64::from(break_state.gate.entrance.z()),
        );
        // The same radius the break's mobs hunt hunters in: wave mobs spawn
        // closer, but chase defenders across the whole participation zone.
        let reach = GATE_BREAK_PARTICIPATION_RADIUS;
        let area = WorldAabb::new(
            center.x - reach,
            center.y - 8.0,
            center.z - reach,
            center.x + reach,
            center.y + 16.0,
            center.z + reach,
        );
        let mut adopted = Vec::new();
        for entity in world.get_entities_in_aabb_matching(&area, is_adoptable_break_mob) {
            let id = entity.id();
            let known = tracks
                .get(&break_state.gate.id)
                .is_some_and(|track| track.labels.contains_key(&id));
            if known || break_state.remaining_mobs.contains(&id) {
                continue;
            }
            let Some(class) = entity.custom_name().as_ref().and_then(break_mob_class) else {
                continue;
            };
            adopted.push(SpawnedMob {
                id,
                class,
                label: MobLabel::mob(Theme::Crypt, class),
            });
        }
        if adopted.is_empty() {
            return;
        }
        let wave = wave_for_gate(break_state.gate.rank, break_state.gate.red);
        let track = tracks
            .entry(break_state.gate.id)
            .or_insert_with(|| GateBreakTrack {
                world: break_state.gate.world.clone(),
                ..GateBreakTrack::default()
            });
        // Adopted bombers explode with the same blast a fresh wave casts.
        track.blast_power = (5.0 + 5.0 * wave.damage_multiplier) as f32;
        for mob in &adopted {
            break_state.remaining_mobs.push(mob.id);
        }
        track.adopt(adopted, tick);
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
                        FrameStyle::from_gate(&placement.gate),
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
                    if gate_frame_cleanup_complete(&cleanup.world, &gate) {
                        self.completed_legacy_gate_cleanups.insert(record_index);
                    } else if gate_frame_matches(&cleanup.world, &gate) {
                        remove_gate_frame(&cleanup.world, &gate);
                        if gate_frame_cleanup_complete(&cleanup.world, &gate) {
                            self.completed_legacy_gate_cleanups.insert(record_index);
                            log::info!(
                                "sololeveling: removed verified legacy orphan gate frame in {} at {:?}",
                                gate.world,
                                gate.entrance
                            );
                        } else {
                            log::warn!(
                                "sololeveling: legacy gate cleanup is incomplete in {} at {:?}; retrying",
                                gate.world,
                                gate.entrance
                            );
                        }
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
                    if gate_frame_cleanup_complete(&cleanup.world, &cleanup.gate) {
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
    /// world holds and at least three ordinary E gates even with no E hunter.
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
        if let Some(index) = ranks.iter().position(|rank| *rank == HunterRank::E) {
            ranks.remove(index);
        }
        ranks.insert(0, HunterRank::E);
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
            let present = ordinary_gate_count(&gates, &world.key, rank);
            let required = required_gate_count(rank);
            if present >= required {
                self.demand_since.remove(&rank);
                continue;
            }
            let since = *self.demand_since.entry(rank).or_insert(tick);
            if rank != HunterRank::E && tick.saturating_sub(since) < DEMAND_DELAY_TICKS {
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
            let around = (
                player.position().x.floor() as i32,
                player.position().z.floor() as i32,
            );
            let frames = self.frame_entrances(&world.key);
            let site = if city::is_city_world(&world) {
                gate_site::nearby_site(
                    around,
                    usize::from(request.attempts),
                    city::is_gate_ground,
                    &frames,
                )
            } else {
                gate_site::nearby_site(around, usize::from(request.attempts), |_, _| true, &frames)
            };
            let Some((x, z)) = site else {
                log::warn!("sololeveling: no free street site for a gate next to hunter {uuid}");
                if request.key {
                    player.send_message(
                        &TextComponent::plain("Рядом нет свободного места для врат.")
                            .color(Color::Red),
                    );
                }
                refund(self);
                continue;
            };
            let candidate = BlockPos::new(x, 0, z);
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
                gate_site::NEARBY_GATE_MIN_DISTANCE
            } else {
                GATE_MIN_DISTANCE
            };
            let crowded =
                pending.test.is_none() && players_too_close(&pending.world, surface, server);
            let frames = self.frame_entrances(&pending.world.key);
            let off_street = city::is_city_world(&pending.world)
                && !gate_footprint(surface)
                    .into_iter()
                    .all(|(x, z)| city::is_gate_ground(x, z));
            if !gate_site::is_free(surface.x(), surface.z(), &frames, min_distance)
                || crowded
                || off_street
                || !gate_space_is_clear(&pending.world, surface)
                || !gate_ground_is_level(&pending.world, surface)
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
                )
            } else if let Some(rank) = pending.rank {
                self.gates
                    .create_ranked(pending.world.key.clone(), surface, pending.tick, rank)
            } else {
                self.gates.create(
                    pending.world.key.clone(),
                    surface,
                    pending.tick,
                    rand::rng().random_range(0..100),
                )
            };
            if let Some(test) = &pending.test {
                self.pending_test_gate_players.insert(gate.id, test.player);
            }
            log::debug!(
                "sololeveling: staged {}-rank gate {} in {} at {:?}{}",
                gate.rank.name(),
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

    /// Returns every frame standing or about to stand in `world`, including
    /// the sites still waiting for their chunks.
    fn frame_entrances(&self, world: &Identifier) -> Vec<BlockPos> {
        let mut frames = self.gates.frame_entrances(world);
        frames.extend(
            self.pending_gates
                .iter()
                .filter(|pending| pending.world.key == *world)
                .map(|pending| pending.candidate),
        );
        frames
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
            let phase = standing.phase;
            let (gate_id, red, castle) = (standing.id, standing.red, standing.floor.is_some());
            let party = state.party_of(uuid);
            let Some(rank) = state.hunter_rank(uuid) else {
                continue;
            };
            // Only the first hunter in needs to lead a party: a blue gate that
            // is already open takes anyone of its rank into its run.
            let refusal = if let Some(reason) = phase_refusal(phase, red, castle) {
                Some(reason.to_owned())
            } else if state.has_pending_penalty(uuid) {
                Some(penalty_refusal(state.penalty_wait_seconds(uuid)))
            } else if phase == GatePhase::Open
                && party.as_ref().is_some_and(|party| party.leader() != uuid)
            {
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
            if phase == GatePhase::Active {
                self.join_run(server, &player, gate_id);
                continue;
            }
            let red = rand::rng().random_range(0..100) < RED_GATE_CHANCE_PERCENT;
            let Some(gate) = self
                .gates
                .enter_at(&world.key, rank, player.position(), red)
            else {
                continue;
            };
            if gate.red {
                // The frame turns red at once, so the hunters left outside see
                // the gate close behind the ones who stepped in.
                place_gate_frame(&world, gate.entrance, FrameStyle::Red);
                announce_gate_red(server, &gate);
            }
            let target_world = if gate.floor.is_some() {
                castle_world_of(server, &world)
            } else {
                instance_world_of(server, &world)
            };
            let Some(instance_world) = target_world else {
                log::error!(
                    "sololeveling: the world behind gate {} is not loaded for {}",
                    gate.id,
                    world.key
                );
                self.gates.reopen(gate.id);
                continue;
            };
            let start = if gate.floor.is_some() {
                log::info!(
                    "sololeveling: hunter {} entered the Demon Castle portal {} (floor {})",
                    uuid,
                    gate.id,
                    gate.floor.unwrap_or(1)
                );
                RunStart::Castle { gate: gate.clone() }
            } else {
                log::info!(
                    "sololeveling: hunter {} entered {}-rank gate {}",
                    uuid,
                    gate.rank.name(),
                    gate.id
                );
                RunStart::Gate {
                    gate: gate.clone(),
                    layout: DungeonLayout::for_gate(rand::rng().random(), gate.red),
                }
            };
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
                let line = if gate.red {
                    TextComponent::plain(format!(
                        "Врата покраснели! Подземелье ранга {} запирается за вами; выход откроется только после смерти босса.",
                        gate.run_rank().name()
                    ))
                    .color(Color::Red)
                } else {
                    TextComponent::plain(format!(
                        "Врата ранга {} активны. Подготавливается подземелье; вход займёт несколько секунд.",
                        gate.rank.name()
                    ))
                    .color(Color::Aqua)
                };
                participant.send_message(&line);
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

    /// Takes `player` into the live run behind the blue gate `gate_id`.
    fn join_run(&mut self, server: &Arc<Server>, player: &Arc<Player>, gate_id: u64) {
        let uuid = player.gameprofile.id();
        let Some(run) = self
            .runs
            .values_mut()
            .find(|run| run.gate == Some(gate_id) && !run.finishing)
        else {
            return;
        };
        let Some(instance_world) = server.worlds.get(&run.instance_world) else {
            return;
        };
        if let Err(error) = server.queue_player_world_selection(Arc::clone(player), instance_world)
        {
            log::warn!(
                "sololeveling: unable to move hunter {uuid} into a running dungeon: {error}"
            );
            return;
        }
        run.members.push(uuid);
        if let RunProgress::Dungeon(progress) = &mut run.progress {
            progress.contributions.entry(uuid).or_default();
        }
        log::info!("sololeveling: hunter {uuid} joined the run behind gate {gate_id}");
        player.send_message(
            &TextComponent::plain("Врата открыты: вы присоединяетесь к рейду внутри.")
                .color(Color::Aqua),
        );
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
        let castle = matches!(start, RunStart::Castle { .. });
        let region = if let RunStart::Castle { gate } = &start {
            // A castle climb holds a slot of the castle world in the region's
            // place; its spawn is never used, members land on the floor entry.
            let slot = self.castle_slots.allocate();
            InstanceRegion {
                slot: slot.0,
                spawn: slot.center(gate.floor.unwrap_or(1)).0.as_dvec3(),
            }
        } else {
            self.regions.allocate()
        };
        // The whole build, and the erasure of what the region held before, has
        // to be loaded before the first block is written.
        let previous = if castle {
            None
        } else {
            self.regions.footprint(region)
        };
        let positions = match &start {
            RunStart::Gate { layout, .. } => dungeon_chunks(region, layout, previous),
            RunStart::Castle { gate } => {
                CastleClimb::entry_request(CastleSlot(region.slot), gate.floor.unwrap_or(1))
                    .positions
            }
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
            self.free_region(region, castle);
            return;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let player_uuids = players
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        let gate_id = start.gate().map(|gate| gate.id);
        self.generating_entries.insert(
            leader,
            GeneratingEntry {
                players: player_uuids,
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

    fn finish_entries(&mut self, state: &SoloLevelingState, server: &Arc<Server>, tick: u64) {
        let results = mem::take(&mut *self.completed_generations.lock());
        for result in results {
            if result.cancelled {
                self.discard_generation_result(&result);
                continue;
            }
            let Some(entry) = self.generating_entries.get(&result.leader) else {
                // The entry was already cleaned up; release the region defensively.
                self.free_region(result.region, result.start.is_castle());
                if let Some(gate) = result.start.gate() {
                    self.gates.reopen(gate.id);
                }
                continue;
            };
            if entry.cancelled.load(Ordering::Relaxed) {
                self.discard_generation_result(&result);
                continue;
            }
            self.generating_entries.remove(&result.leader);
            self.finalize_entry(state, server, tick, result);
        }
    }

    fn discard_generation_result(&mut self, result: &GenerationResult) {
        self.generating_entries.remove(&result.leader);
        self.free_region(result.region, result.start.is_castle());
        if let Some(gate) = result.start.gate() {
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
        let castle = start.is_castle();
        let mut penalty_rank = HunterRank::E;
        let duration_ticks = match &start {
            // A red gate keeps its hunters until the boss or the last of them
            // falls; there is no timer to wait out.
            RunStart::Gate { gate, .. } if gate.red => u64::MAX,
            RunStart::Gate { .. } => DUNGEON_TIME_LIMIT_TICKS,
            RunStart::Castle { .. } => FLOOR_TIME_LIMIT_TICKS,
            RunStart::Penalty => {
                let Some(rank) = state.claim_penalty(leader) else {
                    self.regions.release(region);
                    return;
                };
                penalty_rank = rank;
                PENALTY_ZONE_DURATION_TICKS
            }
        };
        if let Some(footprint) = footprint {
            self.regions.record(region, footprint);
        }
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
            self.free_region(region, castle);
            if let Some(gate) = start.gate() {
                self.gates.reopen(gate.id);
            }
            return;
        }
        // A climb holds its floors' chunk tickets itself, floor by floor.
        let mut request = Some(request);
        let (progress, gate, locked) = match start {
            RunStart::Gate { gate, layout } => {
                let locked = gate.red;
                (
                    RunProgress::Dungeon(Box::new(DungeonProgress {
                        rank: gate.run_rank(),
                        floor: gate.floor,
                        red: gate.red,
                        wave: gate
                            .floor
                            .map_or_else(|| wave_for_gate(gate.run_rank(), gate.red), castle::wave),
                        layout,
                        combat_room: 0,
                        remaining_mobs: Vec::new(),
                        boss: BossState::Waiting,
                        fight: None,
                        exit: None,
                        visuals: Vec::new(),
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
                        extra_mob_rooms: FxHashMap::default(),
                        skills: MobSkills::default(),
                        pending_skills: Vec::new(),
                        classes: FxHashMap::default(),
                        boss_materials: Vec::new(),
                        deaths: 0,
                        tribe_calls: 0,
                    })),
                    Some(gate.id),
                    locked,
                )
            }
            RunStart::Castle { gate } => (
                RunProgress::Castle(Box::new(CastleClimb::new(
                    CastleSlot(region.slot),
                    gate.floor.unwrap_or(1),
                    instance_world.seed() as u64,
                    request.take(),
                ))),
                Some(gate.id),
                false,
            ),
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
        if let Some(gate) = gate {
            // A castle portal carries its one climber up and is spent; any
            // other gate stays standing for as long as its run lasts.
            if castle {
                self.gates.remove(gate);
            } else {
                self.gates.activate(gate);
            }
        }
        let run_id = self.adopt_run(InstanceRun {
            members,
            source_world,
            instance_world: instance_world.key.clone(),
            region,
            started_tick: tick,
            duration_ticks,
            progress,
            map: None,
            gate,
            placed: FxHashSet::default(),
            finishing: false,
            locked,
        });
        // Keeps the region's chunks loaded for as long as the run is live:
        // waves spawn several rooms ahead of the hunter, past any ticket the
        // hunter's own loading distance keeps ready.
        if let Some(request) = request {
            self.run_tickets.insert(run_id, request);
        }
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
            BossState::Alive(boss_id) => {
                if progress.red {
                    progress.call_the_tribe(world, region, boss_id, members, tick);
                }
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
            let exit = open_dungeon_exit(world, region, &progress.layout);
            progress.visuals.extend(spawn_gate_label(
                world,
                exit,
                TextComponent::plain("Выход").color(Color::Aqua),
            ));
            progress.exit = Some(exit);
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

    /// Builds the payout of a finished mine shift.
    ///
    /// The timer expiry and the exit walk both pay through this helper, so
    /// they cannot drift apart: participants are read from the instance world
    /// the party still stands in, never from the world it is about to return
    /// to (where nobody has been teleported yet).
    fn mine_credit_action(
        server: &Arc<Server>,
        instance_world: &Identifier,
        region: InstanceRegion,
        members: &[Uuid],
        rank: HunterRank,
        mined: u32,
    ) -> Option<RunAction> {
        let world = server.worlds.get(instance_world)?;
        Some(RunAction::CreditMine {
            participants: Self::mine_participants(world, region, members),
            rank,
            mined,
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
            let instance_key = run.instance_world.clone();
            for member in pending_placements(&run.members, &mut run.placed, |member| {
                players
                    .get(member)
                    .is_some_and(|player| player.get_world().key == instance_key)
            }) {
                let Some(member) = players.get(&member) else {
                    continue;
                };
                run.place_member(member);
            }
            if tick.saturating_sub(run.started_tick) >= run.duration_ticks {
                run.finishing = true;
                match run.progress {
                    RunProgress::Dungeon(_) | RunProgress::Castle(_) => {
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
                        let Some(credit) = Self::mine_credit_action(
                            server,
                            &run.instance_world,
                            run.region,
                            &members,
                            progress.rank,
                            progress.mined,
                        ) else {
                            continue;
                        };
                        actions.push(credit);
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
                RunProgress::Castle(progress) => {
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
                    if let Some(action) = progress.tick(&context, tick) {
                        actions.push(action);
                    }
                    actions.extend(progress.pending_skills.drain(..).map(RunAction::MobSkill));
                    actions.extend(progress.pending_elite_skills.drain(..).map(|action| {
                        RunAction::EliteSkill {
                            action,
                            members: context.members.to_vec(),
                        }
                    }));
                    self.pending_loot.append(&mut progress.floor_loot);
                    actions.extend(progress.leaving.drain(..).map(|uuid| {
                        RunAction::ReturnPlayer {
                            uuid,
                            world: source_world.clone(),
                        }
                    }));
                    if mem::take(&mut progress.advanced) {
                        // Every floor gets its own time limit.
                        run.started_tick = tick;
                    }
                }
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
                            if let Some(gate) = run.gate {
                                self.gates.close(gate);
                            }
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
                                if let Some(credit) = Self::mine_credit_action(
                                    server,
                                    &run.instance_world,
                                    run.region,
                                    &members,
                                    progress.rank,
                                    progress.mined,
                                ) {
                                    actions.push(credit);
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
            if tick.is_multiple_of(10) && !run.finishing {
                let Some(dungeon_map) = &mut run.map else {
                    continue;
                };
                let canvas = match &run.progress {
                    RunProgress::Dungeon(progress) => progress.map_canvas(),
                    RunProgress::Castle(progress) => progress.map_canvas(),
                    RunProgress::Penalty(_) | RunProgress::Mine(_) => continue,
                };
                dungeon_map.replace_canvas(&canvas);
                for member in &run.members {
                    let Some(hunter) = players.get(member) else {
                        continue;
                    };
                    if hunter.get_world().key != run.instance_world {
                        continue;
                    }
                    let decorations = match &run.progress {
                        RunProgress::Dungeon(progress) => {
                            progress.map_decorations(&world, region, hunter, &run.members)
                        }
                        RunProgress::Castle(progress) => {
                            progress.map_decorations(&world, hunter, &run.members)
                        }
                        RunProgress::Penalty(_) | RunProgress::Mine(_) => Vec::new(),
                    };
                    dungeon_map.sync(hunter, decorations);
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
            if world.key.path.as_ref() != INSTANCE_WORLD_PATH
                && !demon_castle::is_castle_world(&world)
            {
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

/// Converts a finished shift into each miner's Won share.
///
/// Every living participant inside the mine splits the sale value of the
/// mined crystals equally; the division remainder is not paid out. Both ways
/// of leaving a mine pay through the same [`RunAction::CreditMine`], so this
/// conversion is the single payout formula for the timer expiry and the exit
/// walk alike.
pub fn mine_share(rank: HunterRank, mined: u32, participants: usize) -> u64 {
    if participants == 0 {
        return 0;
    }
    u64::from(mined) * crystal::sale_price(rank) / participants as u64
}

/// Marks and returns the run members due their one-time spawn placement:
/// they have arrived in the instance world and were not placed yet.
///
/// A member is placed at most once, no matter how many ticks their world
/// change took: the set records placements per member, so a late party
/// member lands on the region spawn exactly like the first one.
fn pending_placements(
    members: &[Uuid],
    placed: &mut FxHashSet<Uuid>,
    in_instance: impl Fn(&Uuid) -> bool,
) -> Vec<Uuid> {
    let mut placed_now = Vec::new();
    for member in members {
        if placed.contains(member) || !in_instance(member) {
            continue;
        }
        placed.insert(*member);
        placed_now.push(*member);
    }
    placed_now
}

/// Applies one deferred run action after the runtime lock was released.
pub fn apply_action(state: &SoloLevelingState, server: &Arc<Server>, action: &RunAction) {
    match action {
        RunAction::BossAbility(action) => action.apply(server),
        RunAction::MobSkill(action) => action.apply(server),
        RunAction::EliteSkill { action, members } => action.apply(server, members),
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
            let reward = gate_break_wave_reward(*rank);
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
            let share = mine_share(*rank, *mined, participants.len());
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

fn required_gate_count(rank: HunterRank) -> usize {
    if rank == HunterRank::E {
        MIN_E_GATES
    } else {
        DEMAND_GATES_PER_RANK
    }
}

fn ordinary_gate_count(gates: &[Gate], world: &Identifier, rank: HunterRank) -> usize {
    gates
        .iter()
        .filter(|gate| gate.world == *world && gate.rank == rank && gate.floor.is_none())
        .count()
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

/// Returns the Demon Castle world of `world`'s domain.
fn castle_world_of(server: &Arc<Server>, world: &World) -> Option<Arc<World>> {
    server
        .worlds
        .values()
        .find(|candidate| {
            candidate.domain() == world.domain() && demon_castle::is_castle_world(candidate)
        })
        .cloned()
}

/// Picks a site in one of the city gate plazas.
fn random_gate_candidate(center: BlockPos) -> BlockPos {
    city::gate_candidate(center)
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

/// Whether `entity` can be a break wave mob that lost its track: a living,
/// unallied mob. Allied mobs (a hunter's own shadows) are never touched.
fn is_adoptable_break_mob(entity: &dyn Entity) -> bool {
    entity
        .as_mob()
        .is_some_and(|mob| mob.allied_owner().is_none())
        && entity
            .as_living_entity()
            .is_none_or(|living| !living.is_dead_or_dying())
        && !entity.is_removed()
}

/// Reads the class out of a break mob's plate: the plate text is the
/// creature name with the class in brackets, e.g. `Упырь [Страж]`.
fn break_mob_class(name: &TextComponent) -> Option<MobClass> {
    let Content::Text { text } = &name.content else {
        return None;
    };
    MobClass::ALL
        .iter()
        .copied()
        .find(|class| text.contains(&format!("[{}]", class.display_name())))
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
    let message = TextComponent::plain(format!(
        "Открыты врата ранга {}. Минимальный ранг охотника: {}. Врата активны {} минут.",
        gate.rank.name(),
        gate.minimum_hunter_rank().name(),
        super::UNUSED_GATE_LIFETIME_TICKS / (20 * 60)
    ))
    .color(Color::Aqua);
    notify_near_gate(server, gate, message);
}

fn announce_gate_red(server: &Server, gate: &Gate) {
    notify_near_gate(
        server,
        gate,
        TextComponent::plain(format!(
            "Врата ранга {} покраснели! Внутри заперты охотники; никто больше не войдёт, пока врата не закроются.",
            gate.rank.name()
        ))
        .color(Color::Red),
    );
}

/// Returns why a gate in `phase` turns a hunter away, or `None` when it lets
/// one in.
const fn phase_refusal(phase: GatePhase, red: bool, castle: bool) -> Option<&'static str> {
    match phase {
        GatePhase::Active if red => Some("Красные врата закрыты изнутри."),
        GatePhase::Active if castle => Some("Этот портал замка уже унёс своего охотника наверх."),
        GatePhase::Open | GatePhase::Active => None,
        GatePhase::Opening => Some("Врата открываются…"),
        GatePhase::Closing => Some("Подземелье зачищено, врата закрываются."),
    }
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
            "Врата ранга {} закрылись: рейд внутри окончен.",
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

    use super::{
        BossState, CASTLE_MOB_REWARD_PERCENT, CastleClimb, CastleSlot, DungeonProgress,
        DungeonRuntime, GateBreakTrack, GeneratingEntry, InstanceRun, MineProgress, RunProgress,
    };
    use crate::castle::rank_for_floor;
    use crate::combat_reward::HunterReward;
    use crate::dungeon::InstanceRegion;
    use crate::dungeon::generator::Theme;
    use crate::dungeon::mobs::{MobClass, MobLabel};
    use crate::dungeon::skills::MobSkills;
    use crate::dungeon::{DungeonLayout, wave_for_gate};
    use crate::rank::HunterRank;

    #[test]
    fn gate_break_track_gives_mobs_their_class_skills_and_plates() {
        let mut track = GateBreakTrack::default();
        let shaman = break_mob(1, MobClass::Shaman);
        let bomber = break_mob(2, MobClass::Bomber);
        let warrior = break_mob(3, MobClass::Warrior);

        track.adopt(vec![shaman, bomber, warrior], 0);

        // Only classes with an ability are casters, like inside a dungeon.
        assert!(track.skills.has_caster(1));
        assert!(track.skills.has_caster(2));
        assert!(!track.skills.has_caster(3));
        assert_eq!(track.classes.get(&1), Some(&MobClass::Shaman));

        // The plate carries name, class and a health line that is rewritten
        // from the mob's current health after each hit.
        let label = track.labels.get(&1).expect("shaman plate");
        let plate = format!("{:?}", label.text());
        assert!(plate.contains("Упырь [Шаман]"), "{plate}");
        let health = format!("{:?}", label.health_text(21.2, 30.0));
        assert!(health.contains("❤ 22/30"), "{health}");
    }

    fn break_mob(id: i32, class: MobClass) -> super::SpawnedMob {
        super::SpawnedMob {
            id,
            class,
            label: MobLabel::mob(Theme::Crypt, class),
        }
    }

    #[test]
    fn generating_entry_marks_players_busy() {
        let mut runtime = DungeonRuntime::default();
        let leader = Uuid::new_v4();
        let member = Uuid::new_v4();
        runtime.generating_entries.insert(
            leader,
            GeneratingEntry {
                players: vec![leader, member],
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
                gate_id: Some(7),
                cancelled: Arc::clone(&cancelled),
            },
        );

        assert!(runtime.cancel_generating_entry(leader));
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(!runtime.cancel_generating_entry(Uuid::new_v4()));
    }

    #[test]
    fn each_member_is_placed_once_when_they_arrive_in_the_instance_world() {
        use rustc_hash::FxHashSet;

        let leader = Uuid::new_v4();
        let member = Uuid::new_v4();
        let members = vec![leader, member];
        let mut placed = FxHashSet::default();
        // Only the leader has finished the world change so far.
        let mut arrived = FxHashSet::from_iter([leader]);

        let first = super::pending_placements(&members, &mut placed, |uuid| arrived.contains(uuid));
        assert_eq!(first, vec![leader], "the arrived leader is placed");

        // A later tick does not place the leader again while the member is away.
        assert!(
            super::pending_placements(&members, &mut placed, |uuid| arrived.contains(uuid))
                .is_empty(),
            "a placed member is never placed twice"
        );

        // The member's world change completes on a later tick.
        arrived.insert(member);
        let second =
            super::pending_placements(&members, &mut placed, |uuid| arrived.contains(uuid));
        assert_eq!(
            second,
            vec![member],
            "the late member is placed on their own arrival tick"
        );
        assert_eq!(placed.len(), 2);
    }

    #[test]
    fn restored_break_is_retired_without_spawning_or_rewarding_a_wave() {
        use glam::DVec3;
        use steel_core::entity::{Entity as _, entities::ZombieEntity, next_entity_id};
        use steel_registry::vanilla_entities;
        use steel_utils::WorldAabb;

        use crate::dungeon::GateRegistry;
        use crate::dungeon::instance::{
            gate_frame_chunks, gate_frame_cleanup_complete, place_gate_frame,
        };
        use crate::state::SoloLevelingState;

        let fixture =
            crate::test_support::world(Arc::new(steel_core::gameplay_hooks::NoopGameplayHooks));
        let world = Arc::clone(&fixture.world);
        let server = crate::test_support::server(&fixture);
        let mut gates = GateRegistry::default();
        let gate = gates.create_ranked(
            world.key.clone(),
            BlockPos::new(20, 70, 20),
            500_000,
            HunterRank::C,
        );
        gates.start_break(gate.id).expect("old gate break");
        gates.break_mut(gate.id).expect("old break state").wave = 1;
        let snapshot = gates.begin_persistence().expect("break is dirty").snapshot;

        let mut runtime = DungeonRuntime::default();
        runtime
            .restore_gate_registry(snapshot)
            .expect("snapshot should restore");
        crate::test_support::load_full_chunks(&world, gate_frame_chunks(gate.entrance));
        assert!(place_gate_frame(
            &world,
            gate.entrance,
            super::FrameStyle::Normal,
        ));

        let first = Arc::new(ZombieEntity::new(
            &vanilla_entities::ZOMBIE,
            next_entity_id(),
            DVec3::new(20.5, 71.0, 20.5),
            Arc::downgrade(&world),
        ));
        let second = Arc::new(ZombieEntity::new(
            &vanilla_entities::ZOMBIE,
            next_entity_id(),
            DVec3::new(22.5, 71.0, 20.5),
            Arc::downgrade(&world),
        ));
        second.set_custom_name(Some(MobLabel::mob(Theme::Crypt, MobClass::Shaman).text()));
        world.try_add_entity(first.clone()).expect("wave mob");
        world
            .try_add_entity(second.clone())
            .expect("untracked restored mob");
        runtime
            .gates
            .break_mut(gate.id)
            .expect("restored break")
            .remaining_mobs
            .push(first.id());
        let mut track = GateBreakTrack {
            world: world.key.clone(),
            ..GateBreakTrack::default()
        };
        track.adopt(vec![break_mob(first.id(), MobClass::Warrior)], 0);
        runtime.gate_break_tracks.insert(gate.id, track);

        runtime.retire_disabled_breaks(&server);

        assert!(first.is_removed());
        assert!(second.is_removed());
        assert!(runtime.gates.breaks().is_empty());
        assert!(!runtime.gate_break_tracks.contains_key(&gate.id));
        assert!(runtime.gates.retired_gates().is_empty());
        assert!(gate_frame_cleanup_complete(&world, &gate));

        let directory = std::env::temp_dir().join(format!("gate-retirement-{}", Uuid::new_v4()));
        let state = SoloLevelingState::new(directory.clone()).expect("test state");
        let actions = runtime.tick(&state, &server, 1);
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, super::RunAction::CityDefenseWave { .. }))
        );
        let area = WorldAabb::new(-10.0, -60.0, -10.0, 100.0, 100.0, 100.0);
        assert!(world.get_entities_in_aabb(&area).is_empty());
        std::fs::remove_dir_all(directory).expect("cleanup test state");
    }

    #[test]
    fn unloaded_break_frame_is_queued_not_acknowledged() {
        use crate::dungeon::instance::gate_frame_chunks;

        let fixture =
            crate::test_support::world(Arc::new(steel_core::gameplay_hooks::NoopGameplayHooks));
        let world = Arc::clone(&fixture.world);
        let server = crate::test_support::server(&fixture);
        let mut runtime = DungeonRuntime::default();
        let gate = runtime.gates.create_ranked(
            world.key.clone(),
            BlockPos::new(20, 70, 20),
            0,
            HunterRank::C,
        );
        runtime.gates.start_break(gate.id);

        runtime.retire_disabled_breaks(&server);

        assert!(runtime.gates.breaks().is_empty());
        assert_eq!(runtime.gates.retired_gates().len(), 1);
        assert_eq!(runtime.pending_gate_cleanup.len(), 1);

        crate::test_support::load_full_chunks(&world, gate_frame_chunks(gate.entrance));
        runtime.finish_gate_cleanup();
        assert!(runtime.pending_gate_cleanup.is_empty());
        assert!(runtime.gates.retired_gates().is_empty());
    }

    #[test]
    fn legacy_cleanup_preserves_live_frames_and_removes_orphans() {
        use crate::dungeon::LegacyGateCleanup;
        use crate::dungeon::instance::{
            FrameStyle, gate_frame_chunks, gate_frame_cleanup_complete, place_gate_frame,
            standing_frame_style,
        };

        let fixture =
            crate::test_support::world(Arc::new(steel_core::gameplay_hooks::NoopGameplayHooks));
        let world = Arc::clone(&fixture.world);
        let server = crate::test_support::server(&fixture);
        let mut runtime = DungeonRuntime::default();
        let live = runtime.gates.create_ranked(
            world.key.clone(),
            BlockPos::new(4, 64, 4),
            0,
            HunterRank::E,
        );
        let orphan = LegacyGateCleanup {
            world: world.key.clone(),
            entrance: [20, 64, 4],
            red: false,
        }
        .gate();
        runtime.legacy_gate_cleanups = vec![
            LegacyGateCleanup {
                world: world.key.clone(),
                entrance: [4, 64, 4],
                red: false,
            },
            LegacyGateCleanup {
                world: world.key.clone(),
                entrance: [20, 64, 4],
                red: false,
            },
        ];
        let mut chunks = gate_frame_chunks(live.entrance);
        chunks.extend(gate_frame_chunks(orphan.entrance));
        crate::test_support::load_full_chunks(&world, chunks.clone());
        assert!(place_gate_frame(&world, live.entrance, FrameStyle::Red));
        assert!(place_gate_frame(
            &world,
            orphan.entrance,
            FrameStyle::Castle,
        ));

        runtime.schedule_legacy_gate_cleanups(&server);
        crate::test_support::load_full_chunks(&world, chunks.clone());
        runtime.finish_legacy_gate_cleanups();

        assert!(runtime.pending_legacy_gate_cleanups.is_empty());
        assert!(runtime.completed_legacy_gate_cleanups.contains(&0));
        assert!(runtime.completed_legacy_gate_cleanups.contains(&1));
        assert!(runtime.blocked_legacy_gate_cleanups.is_empty());
        assert_eq!(
            standing_frame_style(&world, live.entrance),
            Some(FrameStyle::Red)
        );
        assert!(gate_frame_cleanup_complete(&world, &orphan));

        runtime.gates.remove(live.id);
        runtime.schedule_retired_gate_cleanups(&server);
        crate::test_support::load_full_chunks(&world, chunks);
        runtime.finish_gate_cleanup();
        assert!(runtime.gates.retired_gates().is_empty());
        assert!(gate_frame_cleanup_complete(&world, &live));
    }

    #[test]
    fn break_mob_class_reads_the_plate_text() {
        use text_components::TextComponent;

        let plate = TextComponent::plain("Упырь [Страж]");
        assert_eq!(super::break_mob_class(&plate), Some(MobClass::Guard));
        let king = TextComponent::plain("Король муравьёв");
        assert_eq!(super::break_mob_class(&king), None);
    }

    #[test]
    fn mine_quota_matches_the_placed_veins() {
        let mut dungeon = DungeonProgress {
            rank: HunterRank::D,
            deaths: 0,
            tribe_calls: 0,
            floor: None,
            red: false,
            wave: wave_for_gate(HunterRank::D, false),
            layout: DungeonLayout::from_seed(1),
            combat_room: 0,
            remaining_mobs: Vec::new(),
            boss: BossState::Waiting,
            fight: None,
            exit: Some(BlockPos::new(10, 64, 10)),
            visuals: Vec::new(),
            contributions: FxHashMap::default(),
            boss_chest: None,
            survivors: Vec::new(),
            labels: FxHashMap::default(),
            wave_spawned: false,
            guarded_chambers: Vec::new(),
            extra_mobs: Vec::new(),
            extra_mob_rooms: FxHashMap::default(),
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

    #[test]
    fn mine_shift_pays_the_same_share_on_exit_and_timer() {
        // Both the exit walk and the timer expiry route through
        // `mine_credit_action` and this conversion, so one formula defines
        // the payout either way; the test pins the formula without depending
        // on the crystal price itself.
        let unit = crate::item::crystal::sale_price(HunterRank::D);
        assert_eq!(super::mine_share(HunterRank::D, 6, 3), unit * 6 / 3);
        assert_eq!(super::mine_share(HunterRank::D, 5, 3), unit * 5 / 3);
        assert_eq!(super::mine_share(HunterRank::D, 6, 0), 0);
        assert_eq!(super::mine_share(HunterRank::D, 0, 3), 0);
    }

    fn test_run(members: Vec<Uuid>, progress: RunProgress) -> InstanceRun {
        use rustc_hash::FxHashSet;
        use steel_utils::Identifier;

        InstanceRun {
            members,
            source_world: Identifier::from_steel("overworld"),
            instance_world: Identifier::from_steel("instance"),
            region: InstanceRegion::from_slot(0),
            started_tick: 0,
            duration_ticks: 100,
            progress,
            map: None,
            gate: None,
            placed: FxHashSet::default(),
            finishing: false,
            locked: false,
        }
    }

    fn test_dungeon_progress() -> DungeonProgress {
        DungeonProgress {
            rank: HunterRank::C,
            deaths: 0,
            tribe_calls: 0,
            floor: None,
            red: false,
            wave: wave_for_gate(HunterRank::C, false),
            layout: DungeonLayout::from_seed(1),
            combat_room: 0,
            remaining_mobs: Vec::new(),
            boss: BossState::Waiting,
            fight: None,
            exit: None,
            visuals: Vec::new(),
            contributions: FxHashMap::default(),
            boss_chest: None,
            survivors: Vec::new(),
            labels: FxHashMap::default(),
            wave_spawned: false,
            guarded_chambers: Vec::new(),
            extra_mobs: Vec::new(),
            extra_mob_rooms: FxHashMap::default(),
            skills: MobSkills::default(),
            pending_skills: Vec::new(),
            classes: FxHashMap::default(),
            boss_materials: Vec::new(),
        }
    }

    fn test_castle_progress(floor: u32) -> Box<CastleClimb> {
        Box::new(CastleClimb::new(CastleSlot(0), floor, 1, None))
    }

    #[test]
    fn leader_reentry_keeps_the_run_his_party_still_fights() {
        let mut runtime = DungeonRuntime::default();
        let leader = Uuid::new_v4();
        let member = Uuid::new_v4();
        // The leader fell inside; his run outlives him while the member fights on.
        let surviving = runtime.adopt_run(test_run(
            vec![member],
            RunProgress::Dungeon(Box::new(test_dungeon_progress())),
        ));
        // The leader respawned and entered new gates: his new run must live
        // next to the old one, not replace it under his key.
        let reentered = runtime.adopt_run(test_run(
            vec![leader],
            RunProgress::Dungeon(Box::new(test_dungeon_progress())),
        ));

        assert_ne!(surviving, reentered);
        assert_eq!(
            runtime.runs.len(),
            2,
            "the reentered leader never evicts the live run"
        );
        assert!(
            runtime.runs.values().any(|run| run.members == [member]),
            "the member's run still lives"
        );
    }

    #[test]
    fn castle_mob_kill_pays_half_an_ordinary_kill() {
        use steel_utils::Identifier;

        steel_registry::init_vanilla_registry();
        let mut runtime = DungeonRuntime::default();
        let hunter = Uuid::new_v4();
        // Floor 11 ranks as a D gate, like the floor spawn table.
        let mut climb = test_castle_progress(11);
        climb.remaining_mobs = vec![7];
        climb
            .labels
            .insert(7, MobLabel::mob(Theme::Crypt, MobClass::Warrior));
        runtime.adopt_run(test_run(vec![hunter], RunProgress::Castle(climb)));
        let world = Identifier::from_steel("instance");

        let full = HunterReward::for_mob(rank_for_floor(11));
        let reward = runtime
            .castle_mob_reward_key(&world, 7, hunter)
            .expect("a castle floor mob pays its kill reward");
        assert_eq!(
            reward.experience,
            full.experience * CASTLE_MOB_REWARD_PERCENT / 100
        );
        assert_eq!(
            reward.system_points,
            full.system_points * CASTLE_MOB_REWARD_PERCENT / 100
        );
        // Half of a D-rank kill: 20 experience and 4 system points full.
        assert_eq!((reward.experience, reward.system_points), (10, 2));

        // Outsiders and untracked mobs get nothing, and neither does a
        // duplicate death event once the kill was observed.
        assert!(
            runtime
                .castle_mob_reward_key(&world, 7, Uuid::new_v4())
                .is_none()
        );
        assert!(runtime.castle_mob_reward_key(&world, 8, hunter).is_none());
        // `on_living_death` drops the id from `remaining_mobs` on the first
        // observation; a second event must not pay again.
        let run = runtime.runs.values_mut().next().expect("climb lives");
        let RunProgress::Castle(progress) = &mut run.progress else {
            panic!("climb stays a castle run");
        };
        progress.remaining_mobs.clear();
        assert!(runtime.castle_mob_reward_key(&world, 7, hunter).is_none());
    }

    #[test]
    fn e_gate_quota_counts_active_gates_but_not_the_castle() {
        use glam::DVec3;
        use steel_utils::Identifier;

        use super::{GateRegistry, ordinary_gate_count, required_gate_count};

        steel_registry::init_vanilla_registry();
        let world = Identifier::from_steel("overworld");
        let mut registry = GateRegistry::default();
        let active =
            registry.create_ranked(world.clone(), BlockPos::new(20, 70, 20), 0, HunterRank::E);
        registry.create_ranked(world.clone(), BlockPos::new(40, 70, 20), 0, HunterRank::E);
        let castle = registry.create_castle(world.clone(), BlockPos::new(60, 70, 20), 0, 1);
        let revision = registry
            .begin_persistence()
            .expect("gate changes should commit")
            .revision;
        registry.commit_persistence(revision);
        registry.activate(active.id);
        let gates = registry.committed_gates();
        assert_eq!(ordinary_gate_count(&gates, &world, HunterRank::E), 2);
        assert_eq!(required_gate_count(HunterRank::E), 3);
        assert_eq!(required_gate_count(HunterRank::D), 2);
        assert_eq!(castle.minimum_hunter_rank(), HunterRank::D);
        assert!(!castle.accepts_hunter(HunterRank::E, DVec3::new(60.5, 71.0, 20.5)));
        assert!(castle.accepts_hunter(HunterRank::D, DVec3::new(60.5, 71.0, 20.5)));
    }

    #[test]
    fn expired_gates_are_removed_without_starting_a_break() {
        use steel_utils::Identifier;

        use super::GateRetirementReason;

        let world = Identifier::from_steel("overworld");
        let mut runtime = DungeonRuntime::default();
        let gate = runtime
            .gates
            .create_ranked(world, BlockPos::new(20, 70, 20), 0, HunterRank::E);
        let mut snapshot = runtime
            .gates
            .begin_persistence()
            .expect("new gate should be dirty")
            .snapshot;
        snapshot.gates[0].created_at_unix_seconds = 0;
        runtime
            .gates
            .restore(snapshot, 0)
            .expect("restore expired gate");
        runtime.expire_gates();
        assert!(runtime.gates.gate(gate.id).is_none());
        assert!(runtime.gates.breaks().is_empty());
        assert!(
            runtime
                .gates
                .retired_gates()
                .iter()
                .any(|(retired, reason)| retired.id == gate.id
                    && *reason == GateRetirementReason::Expired)
        );
    }
}
