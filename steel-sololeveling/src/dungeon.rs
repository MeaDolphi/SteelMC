//! Dungeon-instance allocation and rank-scaled combat rules.

pub mod boss;
pub mod generator;
pub mod instance;
pub mod mobs;
pub mod runtime;
pub mod skills;
pub mod tower;

pub use boss::{BossKind, boss_for_gate};

use std::fs;
use std::io::{self, ErrorKind};
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use glam::DVec3;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use steel_core::world::World;
use steel_utils::{BlockPos, Identifier};

use self::generator::Blueprint;

use crate::castle;
use crate::quest::{
    PENALTY_ZONE_DURATION_SECONDS, PENALTY_ZONE_WAVE_INTERVAL_SECONDS,
    PENALTY_ZONE_ZOMBIES_PER_WAVE,
};
use crate::rank::HunterRank;

/// Server ticks in one second.
const TICKS_PER_SECOND: u64 = 20;
/// Horizontal space reserved for one isolated instance region.
pub const INSTANCE_REGION_STRIDE: i32 = 512;
/// Number of region columns before allocation advances to the next row.
const INSTANCE_REGION_COLUMNS: u64 = 1_024;
/// Spawn height inside the empty instance world.
pub const INSTANCE_FLOOR_Y: i32 = 64;
/// Directory name of the shared world that hosts every instance region.
pub const INSTANCE_WORLD_PATH: &str = "sololeveling_instances";
/// Number of automatic gates created in the main world every ten minutes.
pub const GATES_PER_SPAWN_CYCLE: u8 = 3;
/// Delay between automatic gate-spawn cycles.
pub const GATE_SPAWN_INTERVAL_TICKS: u64 = TICKS_PER_SECOND * 60 * 10;
/// Chance out of one hundred that a newly discovered gate is a red gate.
pub const RED_GATE_CHANCE_PERCENT: u8 = 10;
/// Multiplier red gates apply on top of their bumped rank: mob count, health,
/// attack damage, and the reward.
pub const RED_GATE_SCALE: f64 = 1.5;
/// Lifetime of a gate no player has entered.
pub const UNUSED_GATE_LIFETIME_TICKS: u64 = TICKS_PER_SECOND * 60 * 60;
/// Dungeon time limit after its first participant enters.
pub const DUNGEON_TIME_LIMIT_TICKS: u64 = TICKS_PER_SECOND * 60 * 20;
/// Survival time that clears a penalty-zone instance.
pub const PENALTY_ZONE_DURATION_TICKS: u64 = TICKS_PER_SECOND * PENALTY_ZONE_DURATION_SECONDS;
/// Delay between penalty-zone zombie waves.
pub const PENALTY_ZONE_WAVE_INTERVAL_TICKS: u64 =
    TICKS_PER_SECOND * PENALTY_ZONE_WAVE_INTERVAL_SECONDS;
/// Zombies one penalty arena keeps alive before pausing further waves.
///
/// A ten-minute climb with a group every ten seconds would otherwise pile up
/// hundreds of live mobs on a narrow ramp.
pub const PENALTY_ZONE_MAX_LIVE_ZOMBIES: u8 = 25;
/// Persistent lifecycle state for a discovered gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    /// Stable server-local gate id.
    pub id: u64,
    /// Main-world location of the quartz entrance.
    pub world: Identifier,
    /// Bottom-center of the entrance frame.
    pub entrance: BlockPos,
    /// Chosen dungeon rank; a red gate already has its rank bump applied.
    pub rank: HunterRank,
    /// Whether this is a red gate: one rank higher and noticeably deadlier.
    pub red: bool,
    /// Server tick when this gate appeared.
    pub created_tick: u64,
    /// Unix timestamp used to expire gates across server restarts.
    pub created_at_unix_seconds: u64,
    /// Whether a hunter has already entered this gate.
    pub entered: bool,
    /// Demon Castle floor this gate opens; `None` for an ordinary gate.
    pub floor: Option<u32>,
}

/// One-time cleanup request for a legacy gate frame left without a registry record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyGateCleanup {
    /// World containing the old frame.
    pub world: Identifier,
    /// Bottom-left block of the old frame.
    pub entrance: [i32; 3],
    /// Whether the old frame used red stained glass.
    pub red: bool,
}

impl LegacyGateCleanup {
    /// Builds the expected frame metadata used by the guarded cleanup.
    #[must_use]
    pub fn gate(&self) -> Gate {
        Gate {
            id: u64::MAX,
            world: self.world.clone(),
            entrance: BlockPos::new(self.entrance[0], self.entrance[1], self.entrance[2]),
            rank: HunterRank::D,
            red: self.red,
            created_tick: 0,
            created_at_unix_seconds: 0,
            entered: false,
            floor: None,
        }
    }
}

impl Gate {
    /// Returns the lowest hunter rank allowed to enter this gate.
    ///
    /// A hunter may enter one rank below the gate itself; E gates therefore
    /// remain available to E-rank hunters.
    #[must_use]
    pub const fn minimum_hunter_rank(&self) -> HunterRank {
        // A castle floor is opened for its climber, whatever its rank.
        if self.floor.is_some() {
            return HunterRank::E;
        }
        match self.rank {
            HunterRank::E | HunterRank::D => HunterRank::E,
            HunterRank::C => HunterRank::D,
            HunterRank::B => HunterRank::C,
            HunterRank::A => HunterRank::B,
            HunterRank::S => HunterRank::A,
        }
    }

    /// Returns whether a hunter standing in the two-block-wide entrance may enter.
    #[must_use]
    pub fn accepts_hunter(&self, rank: HunterRank, position: DVec3) -> bool {
        rank >= self.minimum_hunter_rank() && instance::opening_contains(self.entrance, position)
    }
}

/// Returns whether `world` is the dimension gates lead into.
///
/// Dungeon rules (sealed walls, crystal ore) apply to this dimension alone, so
/// the ordinary world keeps every vanilla building and mining rule.
#[must_use]
pub fn is_instance_world(world: &World) -> bool {
    world.key.path.as_ref() == INSTANCE_WORLD_PATH
}

/// Gate lifecycle bookkeeping independent of physical block placement.
#[derive(Debug, Default)]
pub struct GateRegistry {
    next_id: u64,
    gates: FxHashMap<u64, Gate>,
    revision: u64,
    persisted_revision: u64,
    pending_revision: Option<u64>,
    persisted: GateRegistrySnapshot,
    committed_gates: FxHashMap<u64, Gate>,
    retired_gates: FxHashMap<u64, (Gate, GateRetirementReason)>,
    pending_created: Vec<Gate>,
    pending_removed: Vec<(Gate, GateRetirementReason)>,
    persistence_failed: bool,
}

/// The durable gate metadata, independent of runtime-only enter locks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateRegistrySnapshot {
    next_id: u64,
    gates: Vec<PersistedGate>,
    #[serde(default)]
    retired_gates: Vec<PersistedGateRetirement>,
}

/// Stable serialized form that avoids requiring `BlockPos` or `HunterRank` serde support.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedGate {
    id: u64,
    world: Identifier,
    entrance: [i32; 3],
    rank: String,
    red: bool,
    created_tick: u64,
    #[serde(default)]
    created_at_unix_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    floor: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedGateRetirement {
    gate: PersistedGate,
    reason: GateRetirementReason,
}

impl PersistedGate {
    fn from_gate(gate: &Gate) -> Self {
        Self {
            id: gate.id,
            world: gate.world.clone(),
            entrance: [gate.entrance.x(), gate.entrance.y(), gate.entrance.z()],
            rank: gate.rank.name().to_owned(),
            red: gate.red,
            created_tick: gate.created_tick,
            created_at_unix_seconds: gate.created_at_unix_seconds,
            floor: gate.floor,
        }
    }

    fn into_gate(self) -> Result<Gate, String> {
        let rank = HunterRank::from_name(&self.rank)
            .ok_or_else(|| format!("unknown gate rank {:?}", self.rank))?;
        Ok(Gate {
            id: self.id,
            world: self.world,
            entrance: BlockPos::new(self.entrance[0], self.entrance[1], self.entrance[2]),
            rank,
            red: self.red,
            created_tick: self.created_tick,
            created_at_unix_seconds: self.created_at_unix_seconds,
            entered: false,
            floor: self.floor,
        })
    }
}

/// A gate snapshot staged for one persistence transaction.
#[derive(Debug, Clone)]
pub struct GateRegistryWrite {
    /// Revision expected to become durable.
    pub revision: u64,
    /// Gate state to publish.
    pub snapshot: GateRegistrySnapshot,
}

/// Physical side effects to run only after a gate snapshot commits.
#[derive(Debug, Default)]
pub struct GateRegistryChanges {
    /// Newly committed gate frames.
    pub created: Vec<Gate>,
    /// Gate frames whose removal is now durable.
    pub removed: Vec<(Gate, GateRetirementReason)>,
}

/// Why a committed gate was retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateRetirementReason {
    /// The unused lifetime expired.
    Expired,
    /// A hunter entered the gate.
    Consumed,
}

impl GateRegistry {
    /// Replaces the live registry with the last committed snapshot.
    pub fn restore(&mut self, snapshot: GateRegistrySnapshot) -> Result<(), String> {
        let gates = snapshot
            .gates
            .iter()
            .cloned()
            .map(PersistedGate::into_gate)
            .collect::<Result<Vec<_>, _>>()?;
        let retired_gates = snapshot
            .retired_gates
            .iter()
            .cloned()
            .map(|retirement| {
                retirement
                    .gate
                    .into_gate()
                    .map(|gate| (gate, retirement.reason))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.next_id = snapshot.next_id;
        self.gates = gates.iter().cloned().map(|gate| (gate.id, gate)).collect();
        self.committed_gates = self.gates.clone();
        self.retired_gates = retired_gates
            .into_iter()
            .map(|(gate, reason)| (gate.id, (gate, reason)))
            .collect();
        self.persisted = snapshot;
        self.revision = 0;
        self.persisted_revision = 0;
        self.pending_revision = None;
        self.pending_created.clear();
        self.pending_removed.clear();
        self.persistence_failed = false;
        Ok(())
    }

    /// Returns the currently committed gates, for menus and navigation.
    #[must_use]
    pub fn committed_gates(&self) -> Vec<Gate> {
        self.committed_gates.values().cloned().collect()
    }

    /// Returns whether a write is pending or the gate document has failed.
    #[must_use]
    pub const fn persistence_blocks_gate_actions(&self) -> bool {
        self.persistence_failed
            || self.pending_revision.is_some()
            || self.revision != self.persisted_revision
    }

    /// Returns the next changed gate snapshot, marking its revision pending.
    pub fn begin_persistence(&mut self) -> Option<GateRegistryWrite> {
        if self.persistence_failed
            || self.pending_revision.is_some()
            || self.revision == self.persisted_revision
        {
            return None;
        }
        self.pending_revision = Some(self.revision);
        Some(GateRegistryWrite {
            revision: self.revision,
            snapshot: self.snapshot(),
        })
    }

    /// Accepts a committed snapshot and exposes its physical changes.
    pub fn commit_persistence(&mut self, revision: u64) -> GateRegistryChanges {
        if self.pending_revision != Some(revision) {
            return GateRegistryChanges::default();
        }
        self.persisted = self.snapshot();
        self.committed_gates = self
            .gates
            .iter()
            .map(|(id, gate)| {
                let mut gate = gate.clone();
                gate.entered = false;
                (*id, gate)
            })
            .collect();
        self.persisted_revision = revision;
        self.pending_revision = None;
        GateRegistryChanges {
            created: mem::take(&mut self.pending_created),
            removed: mem::take(&mut self.pending_removed),
        }
    }

    /// Restores the last committed registry after a failed write and fences future writes.
    pub fn fail_persistence(&mut self, revision: u64) -> Vec<Gate> {
        if self.pending_revision != Some(revision) {
            return Vec::new();
        }
        let created = mem::take(&mut self.pending_created);
        self.next_id = self.persisted.next_id;
        self.gates = self.committed_gates.clone();
        self.revision = self.persisted_revision;
        self.pending_revision = None;
        self.pending_removed.clear();
        self.retired_gates = self
            .persisted
            .retired_gates
            .iter()
            .filter_map(|retirement| {
                retirement
                    .gate
                    .clone()
                    .into_gate()
                    .ok()
                    .map(|gate| (gate.id, (gate, retirement.reason)))
            })
            .collect();
        self.persistence_failed = true;
        created
    }

    /// Returns the last committed gate id lookup, if present.
    #[must_use]
    pub fn committed_gate(&self, id: u64) -> Option<&Gate> {
        self.committed_gates.get(&id)
    }

    /// Returns whether a committed registry entry already owns this frame.
    #[must_use]
    pub fn has_committed_gate_at(&self, world: &Identifier, entrance: BlockPos) -> bool {
        self.committed_gates
            .values()
            .any(|gate| gate.world == *world && gate.entrance == entrance)
    }

    /// Returns whether either the pending or committed registry owns this frame.
    #[must_use]
    pub fn has_gate_at(&self, world: &Identifier, entrance: BlockPos) -> bool {
        self.gates
            .values()
            .any(|gate| gate.world == *world && gate.entrance == entrance)
            || self.has_committed_gate_at(world, entrance)
    }

    /// Returns durable removals whose physical frames still need cleanup.
    #[must_use]
    pub fn retired_gates(&self) -> Vec<(Gate, GateRetirementReason)> {
        self.retired_gates.values().cloned().collect()
    }

    /// Removes a retirement tombstone after its frame has been verified clear.
    pub fn acknowledge_gate_cleanup(&mut self, id: u64) -> bool {
        if self.retired_gates.remove(&id).is_some() {
            self.changed();
            true
        } else {
            false
        }
    }

    fn snapshot(&self) -> GateRegistrySnapshot {
        let mut gates = self
            .gates
            .values()
            .map(PersistedGate::from_gate)
            .collect::<Vec<_>>();
        gates.sort_unstable_by_key(|gate| gate.id);
        let mut retired_gates = self
            .retired_gates
            .values()
            .map(|(gate, reason)| PersistedGateRetirement {
                gate: PersistedGate::from_gate(gate),
                reason: *reason,
            })
            .collect::<Vec<_>>();
        retired_gates.sort_unstable_by_key(|retirement| retirement.gate.id);
        GateRegistrySnapshot {
            next_id: self.next_id,
            gates,
            retired_gates,
        }
    }

    const fn changed(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    /// Creates a gate from a deterministic rank roll and red-gate roll.
    ///
    /// A red gate is rolled first, then promoted one rank, so the roll
    /// distribution of ordinary gates is unaffected.
    pub fn create(
        &mut self,
        world: Identifier,
        entrance: BlockPos,
        tick: u64,
        rank_roll: u8,
        red_roll: u8,
    ) -> Gate {
        let red = red_roll < RED_GATE_CHANCE_PERCENT;
        let rank = gate_rank_for_roll(rank_roll);
        self.create_ranked(
            world,
            entrance,
            tick,
            if red { rank.promoted() } else { rank },
            red,
        )
    }

    /// Creates a gate with an explicit rank, skipping the rank and red rolls.
    ///
    /// Used where the rank is decided elsewhere, such as the prototype gate
    /// placed next to a hunter for testing.
    pub fn create_ranked(
        &mut self,
        world: Identifier,
        entrance: BlockPos,
        tick: u64,
        rank: HunterRank,
        red: bool,
    ) -> Gate {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let gate = Gate {
            id,
            world,
            entrance,
            rank,
            red,
            created_tick: tick,
            created_at_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
            entered: false,
            floor: None,
        };
        self.gates.insert(id, gate.clone());
        self.pending_created.push(gate.clone());
        self.changed();
        gate
    }

    /// Creates the gate of Demon Castle `floor`, ranked by the floor.
    pub fn create_castle(
        &mut self,
        world: Identifier,
        entrance: BlockPos,
        tick: u64,
        floor: u32,
    ) -> Gate {
        let mut gate =
            self.create_ranked(world, entrance, tick, castle::rank_for_floor(floor), false);
        gate.floor = Some(floor);
        if let Some(stored) = self.gates.get_mut(&gate.id) {
            stored.floor = Some(floor);
        }
        if let Some(staged) = self
            .pending_created
            .iter_mut()
            .find(|staged| staged.id == gate.id)
        {
            staged.floor = Some(floor);
        }
        gate
    }

    /// Removes unused gates that have reached their one-hour lifetime.
    pub fn expire_unused(&mut self, tick: u64) -> Vec<Gate> {
        let expired = self
            .gates
            .values()
            .filter(|gate| {
                !gate.entered
                    && tick.saturating_sub(gate.created_tick) >= UNUSED_GATE_LIFETIME_TICKS
            })
            .map(|gate| gate.id)
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|id| self.remove_with_reason(id, GateRetirementReason::Expired))
            .collect()
    }

    /// Removes gates whose wall-clock lifetime elapsed, including while the server was offline.
    pub fn expire_unused_at(&mut self, now_unix_seconds: u64) -> Vec<Gate> {
        let expired = self
            .gates
            .values()
            .filter(|gate| {
                !gate.entered
                    && now_unix_seconds.saturating_sub(gate.created_at_unix_seconds)
                        >= UNUSED_GATE_LIFETIME_TICKS / TICKS_PER_SECOND
            })
            .map(|gate| gate.id)
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|id| self.remove_with_reason(id, GateRetirementReason::Expired))
            .collect()
    }

    /// Finds an enterable gate in a world and marks it used.
    pub fn enter_at(
        &mut self,
        world: &Identifier,
        hunter_rank: HunterRank,
        position: DVec3,
    ) -> Option<Gate> {
        let gate = self.gates.values_mut().find(|gate| {
            gate.world == *world && !gate.entered && gate.accepts_hunter(hunter_rank, position)
        })?;
        gate.entered = true;
        Some(gate.clone())
    }

    /// Makes a gate usable again when an attempted entry could not be completed.
    pub fn reopen(&mut self, id: u64) {
        if let Some(gate) = self.gates.get_mut(&id) {
            gate.entered = false;
        }
    }

    /// Restores a consumed gate when no hunter could be moved into its instance.
    pub fn restore_gate(&mut self, mut gate: Gate) {
        gate.entered = false;
        self.next_id = self.next_id.max(gate.id.saturating_add(1));
        self.gates.insert(gate.id, gate.clone());
        self.pending_created.push(gate);
        self.changed();
    }

    /// Returns the unentered gate whose opening contains `position`, whatever
    /// the hunter's rank.
    #[must_use]
    pub fn at_opening(&self, world: &Identifier, position: DVec3) -> Option<&Gate> {
        self.gates.values().find(|gate| {
            gate.world == *world
                && !gate.entered
                && instance::opening_contains(gate.entrance, position)
        })
    }

    /// Iterates all currently discovered gates.
    pub fn values(&self) -> impl Iterator<Item = &Gate> {
        self.gates.values()
    }

    /// Removes one gate that was consumed by a hunter.
    pub fn remove(&mut self, id: u64) -> Option<Gate> {
        self.remove_with_reason(id, GateRetirementReason::Consumed)
    }

    fn remove_with_reason(&mut self, id: u64, reason: GateRetirementReason) -> Option<Gate> {
        let gate = self.gates.remove(&id)?;
        self.retired_gates.insert(id, (gate.clone(), reason));
        self.pending_removed.push((gate.clone(), reason));
        self.changed();
        Some(gate)
    }
}

/// A reusable spatial slot in the shared instance world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstanceRegion {
    /// Stable allocator slot.
    pub slot: u64,
    /// Spawn position for players entering this instance.
    pub spawn: DVec3,
}

impl InstanceRegion {
    /// Returns the region assigned to `slot`.
    #[must_use]
    pub fn from_slot(slot: u64) -> Self {
        let row = slot / INSTANCE_REGION_COLUMNS;
        let column = slot % INSTANCE_REGION_COLUMNS;
        let x = i32::try_from(column)
            .unwrap_or(i32::MAX)
            .saturating_mul(INSTANCE_REGION_STRIDE);
        let z = i32::try_from(row)
            .unwrap_or(i32::MAX)
            .saturating_mul(INSTANCE_REGION_STRIDE);
        Self {
            slot,
            spawn: DVec3::new(
                f64::from(x) + 0.5,
                f64::from(INSTANCE_FLOOR_Y),
                f64::from(z) + 0.5,
            ),
        }
    }
}

/// The procedurally generated dungeon of one run.
///
/// Waves fill the combat rooms in order; the final room is reserved for the
/// boss and, once it falls, the exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DungeonLayout {
    /// Seed the whole dungeon is generated from.
    pub seed: u64,
    /// Number of combat rooms between the entrance and the reserved boss room.
    pub combat_room_count: u8,
    /// Rooms, corridors and blocks of the dungeon.
    pub blueprint: Arc<Blueprint>,
}

impl DungeonLayout {
    /// Generates the dungeon of `seed`, with two through five combat rooms.
    #[must_use]
    pub fn from_seed(seed: u64) -> Self {
        let combat_room_count = 2 + u8::try_from((seed >> 56) % 4).unwrap_or(0);
        Self {
            seed,
            combat_room_count,
            blueprint: Arc::new(Blueprint::generate(seed, combat_room_count)),
        }
    }

    /// Returns the total room count including entrance and boss/exit rooms.
    #[must_use]
    pub const fn total_room_count(&self) -> u8 {
        self.combat_room_count + 2
    }

    /// Returns the room after the last combat room, reserved for the boss and
    /// the exit of a cleared dungeon.
    #[must_use]
    pub const fn boss_room(&self) -> u8 {
        self.combat_room_count + 1
    }
}

/// What the last run built in a region, so the next one can erase it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Footprint {
    /// A generated dungeon; the seed rebuilds the exact blocks to erase.
    Dungeon {
        /// Seed of the dungeon.
        seed: u64,
    },
    /// The flat penalty arena used before the tower.
    Arena,
    /// The spiral penalty tower.
    Tower,
}

/// Allocates non-overlapping regions and reuses released slots.
#[derive(Debug, Default)]
pub struct InstanceRegionAllocator {
    next_slot: u64,
    reusable_slots: Vec<u64>,
    /// What each region holds; a missing entry may hold a pre-generator build.
    footprints: FxHashMap<u64, Footprint>,
    /// Where `footprints` is saved, if anywhere.
    path: Option<PathBuf>,
}

impl InstanceRegionAllocator {
    /// Allocates one region.
    pub fn allocate(&mut self) -> InstanceRegion {
        let slot = self.reusable_slots.pop().unwrap_or_else(|| {
            let slot = self.next_slot;
            self.next_slot = self.next_slot.saturating_add(1);
            slot
        });
        InstanceRegion::from_slot(slot)
    }

    /// Releases a region after its dungeon was cleaned up.
    pub fn release(&mut self, region: InstanceRegion) {
        self.reusable_slots.push(region.slot);
    }

    /// Loads the saved region footprints and keeps saving them to `path`.
    ///
    /// The instance world keeps its blocks across restarts, so the record of
    /// what stands in each region has to outlive the process as well.
    pub fn load_footprints(&mut self, path: PathBuf) -> io::Result<()> {
        self.footprints = match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?,
            Err(error) if error.kind() == ErrorKind::NotFound => FxHashMap::default(),
            Err(error) => return Err(error),
        };
        self.path = Some(path);
        Ok(())
    }

    /// Returns what the region currently holds; `None` is unknown.
    #[must_use]
    pub fn footprint(&self, region: InstanceRegion) -> Option<Footprint> {
        self.footprints.get(&region.slot).copied()
    }

    /// Records what was just built in a region.
    pub fn record(&mut self, region: InstanceRegion, footprint: Footprint) {
        self.footprints.insert(region.slot, footprint);
        let Some(path) = &self.path else {
            return;
        };
        let result = serde_json::to_vec(&self.footprints)
            .map_err(io::Error::other)
            .and_then(|json| {
                let temporary = path.with_extension("json.tmp");
                fs::write(&temporary, json)?;
                fs::rename(temporary, path)
            });
        if let Err(error) = result {
            log::warn!("sololeveling: instance region records were not saved: {error}");
        }
    }
}

/// Returns the automatically selected gate rank for a roll in `0..100`.
#[must_use]
pub const fn gate_rank_for_roll(roll: u8) -> HunterRank {
    match roll {
        0..=34 => HunterRank::E,
        35..=69 => HunterRank::D,
        70..=84 => HunterRank::C,
        85..=92 => HunterRank::B,
        93..=97 => HunterRank::A,
        _ => HunterRank::S,
    }
}

/// Rank-scaled normal-wave parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DungeonWave {
    /// Number of zombies and skeletons in the wave.
    pub mob_count: u8,
    /// Multiplier applied to vanilla maximum health.
    pub health_multiplier: f64,
    /// Multiplier applied to vanilla attack damage.
    pub damage_multiplier: f64,
}

/// Returns normal-wave parameters for one combat room.
#[must_use]
pub const fn wave_for_rank(rank: HunterRank) -> DungeonWave {
    match rank {
        HunterRank::E => DungeonWave {
            mob_count: 8,
            health_multiplier: 1.0,
            damage_multiplier: 1.0,
        },
        HunterRank::D => DungeonWave {
            mob_count: 10,
            health_multiplier: 1.25,
            damage_multiplier: 1.2,
        },
        HunterRank::C => DungeonWave {
            mob_count: 12,
            health_multiplier: 1.6,
            damage_multiplier: 1.45,
        },
        HunterRank::B => DungeonWave {
            mob_count: 14,
            health_multiplier: 2.1,
            damage_multiplier: 1.8,
        },
        HunterRank::A => DungeonWave {
            mob_count: 18,
            health_multiplier: 2.8,
            damage_multiplier: 2.3,
        },
        HunterRank::S => DungeonWave {
            mob_count: 24,
            health_multiplier: 3.75,
            damage_multiplier: 3.0,
        },
    }
}

/// Returns the wave of a dungeon gate, with red gates scaled up.
#[must_use]
pub fn wave_for_gate(rank: HunterRank, red: bool) -> DungeonWave {
    let wave = wave_for_rank(rank);
    if !red {
        return wave;
    }
    DungeonWave {
        mob_count: (f64::from(wave.mob_count) * RED_GATE_SCALE).ceil() as u8,
        health_multiplier: wave.health_multiplier * RED_GATE_SCALE,
        damage_multiplier: wave.damage_multiplier * RED_GATE_SCALE,
    }
}

/// Returns how many mobs the next penalty-tower group may spawn.
///
/// Groups pause while [`PENALTY_ZONE_MAX_LIVE_ZOMBIES`] mobs are still alive,
/// so a ten-minute challenge cannot accumulate an unbounded mob count.
#[must_use]
pub const fn penalty_wave_size(alive: u8) -> u8 {
    let room = PENALTY_ZONE_MAX_LIVE_ZOMBIES.saturating_sub(alive);
    if PENALTY_ZONE_ZOMBIES_PER_WAVE < room {
        PENALTY_ZONE_ZOMBIES_PER_WAVE
    } else {
        room
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_distribution_uses_the_confirmed_weights() {
        assert_eq!(gate_rank_for_roll(34), HunterRank::E);
        assert_eq!(gate_rank_for_roll(35), HunterRank::D);
        assert_eq!(gate_rank_for_roll(69), HunterRank::D);
        assert_eq!(gate_rank_for_roll(97), HunterRank::A);
        assert_eq!(gate_rank_for_roll(98), HunterRank::S);
    }

    #[test]
    fn rank_scaling_uses_the_confirmed_s_rank_wave() {
        let wave = wave_for_rank(HunterRank::S);
        assert_eq!(wave.mob_count, 24);
        assert_eq!(wave.health_multiplier, 3.75);
        assert_eq!(wave.damage_multiplier, 3.0);
    }

    #[test]
    fn allocator_reuses_released_regions() {
        let mut allocator = InstanceRegionAllocator::default();
        let first = allocator.allocate();
        let second = allocator.allocate();
        assert_ne!(first.slot, second.slot);
        allocator.release(first);
        assert_eq!(allocator.allocate().slot, first.slot);
    }

    #[test]
    fn gate_access_allows_exactly_one_rank_below() {
        let mut gates = GateRegistry::default();
        let gate = gates.create(
            Identifier::from_steel("overworld"),
            BlockPos::new(20, 70, 20),
            0,
            70,
            100,
        );
        assert_eq!(gate.rank, HunterRank::C);
        assert_eq!(gate.minimum_hunter_rank(), HunterRank::D);
        assert!(gate.accepts_hunter(HunterRank::D, DVec3::new(20.5, 71.0, 20.5)));
        assert!(!gate.accepts_hunter(HunterRank::E, DVec3::new(20.5, 71.0, 20.5)));
        assert!(!gate.accepts_hunter(HunterRank::D, DVec3::new(22.0, 71.0, 20.5)));
    }

    #[test]
    fn entering_gate_prevents_expiration() {
        let mut gates = GateRegistry::default();
        gates.create(
            Identifier::from_steel("overworld"),
            BlockPos::new(20, 70, 20),
            0,
            0,
            100,
        );
        assert!(
            gates
                .enter_at(
                    &Identifier::from_steel("overworld"),
                    HunterRank::E,
                    DVec3::new(20.5, 71.0, 20.5),
                )
                .is_some()
        );
        assert_eq!(gates.expire_unused(UNUSED_GATE_LIFETIME_TICKS).len(), 0);
    }

    #[test]
    fn gate_snapshots_round_trip_and_expire_using_wall_time() {
        let mut gates = GateRegistry::default();
        let created = gates.create_ranked(
            Identifier::from_steel("city"),
            BlockPos::new(22, 4, 2),
            5,
            HunterRank::D,
            false,
        );
        let write = gates.begin_persistence().expect("new gate is dirty");
        let bytes = serde_json::to_vec(&write.snapshot).expect("snapshot should serialize");
        let snapshot: GateRegistrySnapshot =
            serde_json::from_slice(&bytes).expect("snapshot should deserialize");
        assert_eq!(gates.commit_persistence(write.revision).created[0], created);

        let mut restored = GateRegistry::default();
        restored.restore(snapshot).expect("snapshot should restore");
        let saved = restored.committed_gates();
        assert_eq!(saved.len(), 1);
        assert!(
            !saved[0].entered,
            "a pending entry is reopened after restart"
        );
        assert!(
            restored
                .expire_unused_at(created.created_at_unix_seconds + 3_599)
                .is_empty(),
            "the saved gate remains active for its whole lifetime"
        );
        assert_eq!(
            restored
                .expire_unused_at(created.created_at_unix_seconds + 3_600)
                .len(),
            1,
            "offline time counts toward unused-gate expiration"
        );
    }

    #[test]
    fn retired_gate_tombstone_survives_restart_until_cleanup_is_acknowledged() {
        let mut gates = GateRegistry::default();
        let gate = gates.create_ranked(
            Identifier::from_steel("city"),
            BlockPos::new(22, 4, 2),
            5,
            HunterRank::D,
            false,
        );
        let created = gates.begin_persistence().expect("new gate is dirty");
        gates.commit_persistence(created.revision);
        assert!(gates.remove(gate.id).is_some());

        let retirement = gates
            .begin_persistence()
            .expect("retirement is a durable change");
        let encoded =
            serde_json::to_vec(&retirement.snapshot).expect("retirement snapshot should serialize");
        let snapshot: GateRegistrySnapshot =
            serde_json::from_slice(&encoded).expect("retirement snapshot should deserialize");
        gates.commit_persistence(retirement.revision);

        let mut restored = GateRegistry::default();
        restored
            .restore(snapshot)
            .expect("retirement should restore");
        assert_eq!(restored.committed_gates(), Vec::<Gate>::new());
        assert_eq!(
            restored.retired_gates(),
            vec![(gate.clone(), GateRetirementReason::Consumed)]
        );

        assert!(restored.acknowledge_gate_cleanup(gate.id));
        assert!(
            restored
                .begin_persistence()
                .is_some_and(|write| write.snapshot.retired_gates.is_empty()),
            "the tombstone is removed only in a later durable write"
        );
    }

    #[test]
    fn failed_gate_snapshot_keeps_the_committed_registry_and_fences_changes() {
        let mut gates = GateRegistry::default();
        let gate = gates.create_ranked(
            Identifier::from_steel("city"),
            BlockPos::new(22, 4, 2),
            5,
            HunterRank::D,
            false,
        );
        let write = gates.begin_persistence().expect("new gate is dirty");

        assert_eq!(gates.committed_gates(), Vec::<Gate>::new());
        gates.fail_persistence(write.revision);

        assert_eq!(gates.committed_gates(), Vec::<Gate>::new());
        assert!(gates.persistence_blocks_gate_actions());
        assert!(!gates.values().any(|candidate| candidate.id == gate.id));
    }

    #[test]
    fn a_gate_can_be_reopened_after_entry_preparation_fails() {
        let mut gates = GateRegistry::default();
        let gate = gates.create_ranked(
            Identifier::from_steel("overworld"),
            BlockPos::new(20, 70, 20),
            0,
            HunterRank::E,
            false,
        );
        let position = DVec3::new(20.5, 71.0, 20.5);
        assert!(
            gates
                .enter_at(&gate.world, HunterRank::E, position)
                .is_some()
        );
        gates.reopen(gate.id);
        assert!(
            gates
                .enter_at(&gate.world, HunterRank::E, position)
                .is_some()
        );
    }

    #[test]
    fn layout_keeps_the_boss_exit_room_after_combat_rooms() {
        let layout = DungeonLayout::from_seed(3 << 56);
        assert_eq!(layout.combat_room_count, 5);
        assert_eq!(layout.total_room_count(), 7);
        assert!(layout.blueprint.combat_room(5).is_some());
    }

    #[test]
    fn red_gates_are_promoted_and_scaled() {
        let mut gates = GateRegistry::default();
        let normal = gates.create(
            Identifier::from_steel("overworld"),
            BlockPos::new(20, 70, 20),
            0,
            70,
            RED_GATE_CHANCE_PERCENT,
        );
        assert_eq!(normal.rank, HunterRank::C);
        assert!(!normal.red);

        let red = gates.create(
            Identifier::from_steel("overworld"),
            BlockPos::new(60, 70, 20),
            0,
            70,
            RED_GATE_CHANCE_PERCENT - 1,
        );
        assert!(red.red);
        assert_eq!(red.rank, HunterRank::B);

        // A red S gate cannot be promoted any further.
        let top = gates.create(
            Identifier::from_steel("overworld"),
            BlockPos::new(100, 70, 20),
            0,
            99,
            0,
        );
        assert_eq!(top.rank, HunterRank::S);
    }

    #[test]
    fn red_gate_waves_add_half_again_on_top_of_the_rank() {
        assert_eq!(
            wave_for_gate(HunterRank::C, false),
            wave_for_rank(HunterRank::C)
        );

        let red = wave_for_gate(HunterRank::C, true);
        assert_eq!(red.mob_count, 18);
        assert!((red.health_multiplier - 2.4).abs() < 1e-9);
        assert!((red.damage_multiplier - 2.175).abs() < 1e-9);
    }

    #[test]
    fn penalty_waves_pause_once_the_arena_is_full() {
        assert_eq!(penalty_wave_size(0), PENALTY_ZONE_ZOMBIES_PER_WAVE);
        assert_eq!(
            penalty_wave_size(PENALTY_ZONE_ZOMBIES_PER_WAVE),
            PENALTY_ZONE_ZOMBIES_PER_WAVE
        );
        assert_eq!(penalty_wave_size(PENALTY_ZONE_MAX_LIVE_ZOMBIES - 5), 5);
        assert_eq!(penalty_wave_size(PENALTY_ZONE_MAX_LIVE_ZOMBIES), 0);
    }
}
