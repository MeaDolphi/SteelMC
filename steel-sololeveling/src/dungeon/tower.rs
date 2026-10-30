//! The spiral tower of the penalty zone.
//!
//! A round tower with a solid core and a ramp winding up between the core and
//! the outer wall. The ramp rises in half blocks, so hunters and mobs walk it
//! without jumping. Mobs of every dungeon class keep appearing on the ramp a
//! quarter turn above the hunter; reaching the platform at the top ends the
//! challenge early.

use std::f64::consts::TAU;
use std::sync::Arc;

use glam::DVec3;
use steel_core::world::World;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, SlabType};
use steel_registry::vanilla_blocks;
use steel_utils::{BlockPos, BlockStateId, ChunkPos};

use super::generator::Theme;
use super::instance::{
    SpawnedMob, at, chunks_covering, clear_box_of, set_fixture, spawn_class_mob, warn_unwritten,
};
use super::mobs::MobClass;
use super::{DungeonWave, InstanceRegion};
use crate::rank::HunterRank;

/// Horizontal offset of the tower axis from the region spawn, so the spawn
/// lands at the foot of the ramp.
const CENTER_OFFSET: (i32, i32) = (-8, 0);
/// Radius of the solid core.
const CORE_RADIUS: f64 = 3.5;
/// Outer radius of the walkable ramp.
const RAMP_RADIUS: f64 = 11.0;
/// Outer radius of the wall.
const WALL_RADIUS: f64 = 12.5;
/// Full turns of the ramp.
pub const TURNS: i32 = 5;
/// Height the ramp gains in one turn.
const TURN_HEIGHT: i32 = 8;
/// Height of the top platform above the tower floor.
const TOP: i32 = TURNS * TURN_HEIGHT;
/// Air above the top platform before the roof.
const ROOF_CLEARANCE: i32 = 6;
/// Share of the last turn left open so the ramp comes out on the platform.
const PLATFORM_GAP: f64 = 0.45;
/// Radius mobs are placed at on the ramp.
const SPAWN_RADIUS: f64 = 7.5;
/// Lead, in turns, of a new group over the hunter it hunts.
const SPAWN_LEAD: f64 = 0.3;
/// Spread, in turns, between the mobs of one group.
const SPAWN_SPREAD: f64 = 0.04;
/// Theme the tower's mobs are named after.
const THEME: Theme = Theme::Crypt;

fn center(region: InstanceRegion) -> BlockPos {
    at(region, (CENTER_OFFSET.0, 0, CENTER_OFFSET.1))
}

/// Returns the box the tower occupies, roof and floor included.
fn tower_box(region: InstanceRegion) -> (BlockPos, BlockPos) {
    let center = center(region);
    let reach = WALL_RADIUS.ceil() as i32;
    (
        BlockPos::new(center.x() - reach, center.y() - 1, center.z() - reach),
        BlockPos::new(
            center.x() + reach,
            center.y() + TOP + ROOF_CLEARANCE,
            center.z() + reach,
        ),
    )
}

/// Returns the chunk columns the tower writes.
#[must_use]
pub fn tower_chunks(region: InstanceRegion) -> Vec<ChunkPos> {
    let (min, max) = tower_box(region);
    chunks_covering(min, max)
}

/// Clears everything the tower may have built.
pub fn erase_tower(world: &Arc<World>, region: InstanceRegion, failed: &mut u32) {
    clear_box_of(world, tower_box(region), failed);
}

/// Returns the share of a full turn at offset `(dx, dz)` from the axis, in
/// `[0, 1)`, counting from the region spawn's side.
fn turn_fraction(dx: f64, dz: f64) -> f64 {
    (dz.atan2(dx) / TAU).rem_euclid(1.0)
}

/// Returns the block of a tower column at height `y` above the floor, or
/// `None` for air.
fn tower_block(dx: i32, dz: i32, y: i32) -> Option<BlockStateId> {
    let (fx, fz) = (f64::from(dx), f64::from(dz));
    let radius = fx.hypot(fz);
    if radius > WALL_RADIUS {
        return None;
    }
    if y == -1 || y == TOP + ROOF_CLEARANCE {
        return Some(vanilla_blocks::POLISHED_BLACKSTONE.default_state());
    }
    if radius > RAMP_RADIUS {
        let lit = y % 4 == 2 && (turn_fraction(fx, fz) * 24.0).fract() < 0.12;
        return Some(if lit {
            vanilla_blocks::GLOWSTONE.default_state()
        } else {
            vanilla_blocks::DEEPSLATE_BRICKS.default_state()
        });
    }
    if radius <= CORE_RADIUS {
        return Some(if y % 6 == 3 {
            vanilla_blocks::CRYING_OBSIDIAN.default_state()
        } else {
            vanilla_blocks::OBSIDIAN.default_state()
        });
    }
    let fraction = turn_fraction(fx, fz);
    // The top platform covers the annulus, except where the ramp comes up.
    if y == TOP - 1 && fraction < 1.0 - PLATFORM_GAP {
        return Some(vanilla_blocks::POLISHED_ANDESITE.default_state());
    }
    for turn in 0..TURNS {
        // Surface height in half blocks at this column on this turn.
        let halves = ((f64::from(turn) + fraction) * f64::from(TURN_HEIGHT) * 2.0).floor() as i32;
        let base = halves.div_euclid(2);
        if y == base - 1 {
            return Some(vanilla_blocks::POLISHED_ANDESITE.default_state());
        }
        if halves % 2 == 1 && y == base {
            return Some(
                vanilla_blocks::POLISHED_ANDESITE_SLAB
                    .default_state()
                    .set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Bottom),
            );
        }
    }
    None
}

/// Builds the whole tower, emptying whatever the box held before.
pub fn build_tower(world: &Arc<World>, region: InstanceRegion) {
    let center = center(region);
    let reach = WALL_RADIUS.ceil() as i32;
    let air = vanilla_blocks::AIR.default_state();
    let mut failed = 0;
    for dx in -reach..=reach {
        for dz in -reach..=reach {
            for y in -1..=TOP + ROOF_CLEARANCE {
                let pos = BlockPos::new(center.x() + dx, center.y() + y, center.z() + dz);
                set_fixture(
                    world,
                    pos,
                    tower_block(dx, dz, y).unwrap_or(air),
                    &mut failed,
                );
            }
        }
    }
    // A light on the core marks the goal.
    for dx in -1..=1 {
        for dz in -1..=1 {
            let pos = BlockPos::new(center.x() + dx, center.y() + TOP - 1, center.z() + dz);
            set_fixture(
                world,
                pos,
                vanilla_blocks::SEA_LANTERN.default_state(),
                &mut failed,
            );
        }
    }
    warn_unwritten("penalty tower", center, failed);
}

/// Returns how far up the ramp `position` is, in turns from the foot.
#[must_use]
pub fn climb_of(region: InstanceRegion, position: DVec3) -> f64 {
    let center = center(region);
    let dx = position.x - (f64::from(center.x()) + 0.5);
    let dz = position.z - (f64::from(center.z()) + 0.5);
    let fraction = turn_fraction(dx, dz);
    let height = (position.y - f64::from(center.y())) / f64::from(TURN_HEIGHT);
    let turn = (height - fraction + 0.5)
        .floor()
        .clamp(0.0, f64::from(TURNS - 1));
    (turn + fraction).min(f64::from(TURNS))
}

/// Returns whether `position` stands on the top platform.
#[must_use]
pub fn reached_top(region: InstanceRegion, position: DVec3) -> bool {
    let center = center(region);
    let dx = position.x - (f64::from(center.x()) + 0.5);
    let dz = position.z - (f64::from(center.z()) + 0.5);
    position.y >= f64::from(center.y() + TOP) - 0.1 && dx.hypot(dz) <= WALL_RADIUS
}

/// Returns the ramp column `climb` turns up from the foot.
fn ramp_point(region: InstanceRegion, climb: f64) -> BlockPos {
    let center = center(region);
    let climb = climb.clamp(0.0, f64::from(TURNS) - 0.01);
    let angle = climb.fract() * TAU;
    let x = f64::from(center.x()) + 0.5 + SPAWN_RADIUS * angle.cos();
    let z = f64::from(center.z()) + 0.5 + SPAWN_RADIUS * angle.sin();
    // One block above the surface; the spawn moves down onto the ramp.
    let y = f64::from(center.y()) + climb * f64::from(TURN_HEIGHT) + 1.0;
    BlockPos::containing(x, y, z)
}

/// Spawns `count` mobs on the ramp ahead of a hunter who climbed `climb`
/// turns, rolling their classes from `salt`.
pub fn spawn_tower_group(
    world: &Arc<World>,
    region: InstanceRegion,
    climb: f64,
    count: u8,
    wave: DungeonWave,
    rank: HunterRank,
    salt: u64,
) -> Vec<SpawnedMob> {
    (0..count)
        .filter_map(|index| {
            let roll = salt.wrapping_add(u64::from(index).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let class = MobClass::from_roll(roll ^ (roll >> 29));
            let position = ramp_point(region, climb + SPAWN_LEAD + SPAWN_SPREAD * f64::from(index));
            spawn_class_mob(world, THEME, position, class, wave, rank)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::{CENTER_OFFSET, TOP, TURN_HEIGHT, TURNS, tower_block, turn_fraction};
    use steel_registry::init_vanilla_registry;

    #[test]
    fn the_spawn_column_is_open_at_the_foot_of_the_ramp() {
        init_vanilla_registry();
        let (dx, dz) = (-CENTER_OFFSET.0, -CENTER_OFFSET.1);
        assert!(tower_block(dx, dz, -1).is_some(), "the floor is solid");
        for y in 0..TURN_HEIGHT - 1 {
            assert!(tower_block(dx, dz, y).is_none(), "headroom at {y}");
        }
    }

    #[test]
    fn the_ramp_rises_in_walkable_half_steps() {
        init_vanilla_registry();
        // Walk the whole ramp at radius 8 in small angle steps, always stepping
        // onto the nearest walkable surface, as a hunter would: it never
        // changes by more than one block row, which is a half step.
        let mut height = 0;
        for step in 1..400 * TURNS {
            let angle = f64::from(step) / 400.0 * TAU;
            let dx = (8.0 * angle.cos()).round() as i32;
            let dz = (8.0 * angle.sin()).round() as i32;
            let next = (-1..TOP)
                .filter(|y| {
                    tower_block(dx, dz, *y).is_some() && tower_block(dx, dz, y + 1).is_none()
                })
                .min_by_key(|y| (y - height).abs())
                .expect("the ramp is solid everywhere");
            assert!(
                (next - height).abs() <= 1,
                "step at {step}: {height} -> {next}"
            );
            height = next;
        }
        assert!(height >= TOP - 2, "the walk ends at the top platform");
        assert!(turn_fraction(1.0, 0.0) < 1e-9);
    }
}
