//! Dungeon-instance allocation and rank-scaled combat rules.

pub mod boss;
pub mod instance;
pub mod runtime;

pub use boss::{BossAbilitySet, boss_for_gate};

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::world::World;
use steel_utils::{BlockPos, Identifier};

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
/// A ten-minute challenge at twenty zombies every ten seconds would otherwise
/// reach well over a thousand live mobs, far past what a vanilla client can be
/// expected to render and tick.
pub const PENALTY_ZONE_MAX_LIVE_ZOMBIES: u8 = 40;
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
    /// Whether a hunter has already entered this gate.
    pub entered: bool,
}

impl Gate {
    /// Returns the lowest hunter rank allowed to enter this gate.
    ///
    /// A hunter may enter one rank below the gate itself; E gates therefore
    /// remain available to E-rank hunters.
    #[must_use]
    pub const fn minimum_hunter_rank(&self) -> HunterRank {
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
}

impl GateRegistry {
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
            entered: false,
        };
        self.gates.insert(id, gate.clone());
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
            .filter_map(|id| self.gates.remove(&id))
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

    /// Iterates all currently discovered gates.
    pub fn values(&self) -> impl Iterator<Item = &Gate> {
        self.gates.values()
    }

    /// Removes one gate that was consumed by a hunter.
    pub fn remove(&mut self, id: u64) -> Option<Gate> {
        self.gates.remove(&id)
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

/// Room progression generated inside one instance region.
///
/// The final room is deliberately reserved for the Phase 6 boss. In Phase 5,
/// clearing every combat room unlocks the exit from that reserved room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DungeonLayout {
    /// Number of combat rooms between the entrance and the reserved boss room.
    pub combat_room_count: u8,
}

impl DungeonLayout {
    /// Selects two through five combat rooms from a random roll.
    #[must_use]
    pub const fn from_roll(roll: u8) -> Self {
        Self {
            combat_room_count: 2 + roll % 4,
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

/// Allocates non-overlapping regions and reuses released slots.
#[derive(Debug, Default)]
pub struct InstanceRegionAllocator {
    next_slot: u64,
    reusable_slots: Vec<u64>,
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

/// Returns how many zombies the next penalty wave may spawn.
///
/// Waves pause while [`PENALTY_ZONE_MAX_LIVE_ZOMBIES`] zombies are still alive,
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
    fn layout_keeps_the_boss_exit_room_after_combat_rooms() {
        let layout = DungeonLayout::from_roll(3);
        assert_eq!(layout.combat_room_count, 5);
        assert_eq!(layout.total_room_count(), 7);
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
