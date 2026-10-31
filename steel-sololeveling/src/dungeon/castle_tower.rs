//! The Demon Castle tower: square dungeon floors stacked into an endless climb.
//!
//! Every floor is one square hall walled in polished blackstone bricks. A wave
//! of the floor's rank guards the hall; once it falls, a stair of blackstone
//! blocks along one wall leads through the ceiling slab into the next floor,
//! which is generated only when a hunter commits to the climb. Each floor is
//! the same plan rotated a quarter turn, so the arrival hole of one floor
//! never lands inside the staircase of the next.
//!
//! Floors are generated one at a time, outside the game tick, by a server job
//! modelled on `DungeonGenerationJob`; [`floor_block`] is the pure block
//! function that function builds from and the tests walk.

use std::sync::Arc;

use glam::DVec3;
use steel_core::world::World;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::vanilla_blocks;
use steel_utils::{BlockPos, BlockStateId, ChunkPos};

use super::generator::Theme;
use super::instance::{
    SpawnedMob, at, chunks_covering, clear_box_of, footprint_chunks, set_fixture, spawn_class_mob,
    warn_unwritten,
};
use super::mobs::MobClass;
use super::{DungeonWave, Footprint, INSTANCE_FLOOR_Y, InstanceRegion};
use crate::rank::HunterRank;

/// Chebyshev radius of the room interior; walls stand at `TOWER_HALF + 1` and
/// `TOWER_HALF + 2`, so the tower footprint is `2 * TOWER_HALF + 5` wide.
pub const TOWER_HALF: i32 = 13;
/// Inner wall ring.
const WALL_RADIUS: i32 = TOWER_HALF + 1;
/// Outer wall ring; also the reach of the floor slabs.
const WALL_OUTER: i32 = TOWER_HALF + 2;
/// Vertical distance between the walking surfaces of two floors.
pub const FLOOR_STEP: i32 = 8;
/// Air layers between a floor's surface and its ceiling slab, which is the
/// next floor's foundation: one slab serves as both.
const CLEARANCE: i32 = FLOOR_STEP - 1;
/// Canonical staircase columns: two columns along the south wall.
const RAMP_DZ: [i32; 2] = [-12, -13];
/// First staircase column, at floor level.
const RAMP_DX_MIN: i32 = -12;
/// Last staircase column, one below the ceiling slab.
const RAMP_DX_MAX: i32 = 12;
/// Columns the staircase gains in height.
const RISE_EVERY: i32 = 3;
/// First canonical column of the shaft through the ceiling slab. The
/// staircase leaves the room below at rise five and climbs the rest inside
/// the shaft, where there is no slab above a hunter's head.
const HOLE_START: i32 = 6;
/// First canonical column of the exit run, where the staircase tops out at
/// the next floor's level.
const EXIT_RUN_START: i32 = RAMP_DX_MAX - 2;
/// Height, in blocks, of the staircase top above the floor surface.
const RAMP_TOP: i32 = CLEARANCE + 1;

/// Returns the Y coordinate of the walking surface of `floor`.
#[must_use]
pub const fn floor_surface_y(floor: u32) -> i32 {
    INSTANCE_FLOOR_Y + (floor as i32 - 1) * FLOOR_STEP
}

/// Returns how many tower floors fit below `max_y`, the world's highest block.
#[must_use]
pub const fn max_floors(max_y: i32) -> u32 {
    // Floor `n` reaches its ceiling slab at `floor_surface_y(n) + CLEARANCE`.
    (((max_y - CLEARANCE - INSTANCE_FLOOR_Y) / FLOOR_STEP) as u32).saturating_add(1)
}

/// Returns which quarter turn one floor's plan is rotated by.
const fn quarter(floor: u32) -> u32 {
    floor.wrapping_sub(1) % 4
}

/// Rotates `(x, z)` by `q` quarter turns around the tower axis.
const fn rot(x: i32, z: i32, q: u32) -> (i32, i32) {
    match q % 4 {
        0 => (x, z),
        1 => (-z, x),
        2 => (-x, -z),
        _ => (z, -x),
    }
}

/// Returns the canonical coordinates of the world column `(dx, dz)` on `floor`.
const fn canonical(floor: u32, dx: i32, dz: i32) -> (i32, i32) {
    rot(dx, dz, (4 - quarter(floor)) % 4)
}

/// Per-floor salt of the run seed.
fn floor_seed(seed: u64, floor: u32) -> u64 {
    seed ^ u64::from(floor).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// Hashes coordinates into a stable pseudo-random value.
const fn mix(seed: u64, x: i32, z: i32) -> u64 {
    let mut value = seed
        ^ (x as u64).wrapping_mul(0x632B_E9BD_9B39_3B19)
        ^ (z as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^ (value >> 32)
}

/// Returns whether the canonical column is inside the shaft the staircase
/// exits through.
fn is_exit_hole(x: i32, z: i32) -> bool {
    RAMP_DZ.contains(&z) && (HOLE_START..=RAMP_DX_MAX).contains(&x)
}

/// Returns whether the world column of `floor` is the hole its foundation
/// keeps above the staircase top of the floor below.
///
/// The hole is rotation-aware in world space: it sits above the rotated exit
/// of the previous floor, which the canonical block function cannot see.
fn is_arrival_hole(floor: u32, dx: i32, dz: i32) -> bool {
    let (x, z) = rot(dx, dz, quarter(floor.wrapping_sub(1)));
    RAMP_DZ.contains(&z) && (HOLE_START..=RAMP_DX_MAX).contains(&x)
}

/// Returns the staircase height, in blocks above the floor surface, at the
/// canonical column `x` of the staircase strip.
///
/// The staircase rises one block every few columns up to just below the
/// ceiling slab, then climbs the last blocks inside the shaft, two columns
/// per block, topping out at the next floor's level.
fn ramp_rise(x: i32) -> i32 {
    let index = x - RAMP_DX_MIN;
    if index < HOLE_START - RAMP_DX_MIN {
        index / RISE_EVERY
    } else {
        let under_slab = CLEARANCE - 2;
        under_slab + 1 + ((index - (HOLE_START - RAMP_DX_MIN)) / 2).min(RAMP_TOP - under_slab - 1)
    }
}

/// Returns the four canonical pillar columns of one floor, jittered by the
/// per-floor seed.
const fn pillar_columns(seed: u64) -> [(i32, i32); 4] {
    let mut columns = [(0, 0); 4];
    let mut index = 0;
    while index < 4 {
        let (sx, sz) = (
            if index % 2 == 0 { -1 } else { 1 },
            if index < 2 { -1 } else { 1 },
        );
        let jx = (mix(seed, index as i32, 7) % 5) as i32 - 2;
        let jz = (mix(seed, index as i32, 13) % 5) as i32 - 2;
        columns[index] = (sx * 7 + jx, sz * 7 + jz);
        index += 1;
    }
    columns
}

/// Returns the two canonical ceiling light columns of one floor.
fn light_columns(seed: u64) -> [(i32, i32); 2] {
    let first = mix(seed, 3, 21) % 4;
    let second = (first + 1 + mix(seed, 5, 33) % 3) % 4;
    let corner = |quadrant: u64| {
        let (sx, sz) = (
            if quadrant % 2 == 0 { -1 } else { 1 },
            if quadrant < 2 { -1 } else { 1 },
        );
        (sx * 4, sz * 4)
    };
    [corner(first), corner(second)]
}

/// Returns the block of the canonical column `(x, z)` at `layer` above the
/// floor surface, or `None` for air.
fn canonical_block(seed: u64, x: i32, z: i32, layer: i32) -> Option<BlockStateId> {
    let chebyshev = x.abs().max(z.abs());
    if chebyshev > WALL_OUTER {
        return None;
    }
    let on_ramp = RAMP_DZ.contains(&z) && (RAMP_DX_MIN..=RAMP_DX_MAX).contains(&x);
    let rise = if on_ramp { ramp_rise(x) } else { 0 };
    // Staircase: a solid blackstone core with stair blocks on top, rising
    // one block every few columns and climbing the shaft through the
    // ceiling hole.
    if on_ramp && rise > 0 && layer >= 0 && layer < rise {
        if layer == rise - 1 {
            return Some(
                vanilla_blocks::POLISHED_BLACKSTONE_STAIRS
                    .default_state()
                    .set_value(&BlockStateProperties::HORIZONTAL_FACING, Direction::East),
            );
        }
        return Some(vanilla_blocks::BLACKSTONE.default_state());
    }
    if layer == -1 {
        return Some(if mix(seed, x, z) % 5 == 0 {
            vanilla_blocks::BLACKSTONE.default_state()
        } else {
            vanilla_blocks::POLISHED_BLACKSTONE.default_state()
        });
    }
    if layer > CLEARANCE {
        return None;
    }
    if layer == CLEARANCE {
        // The ceiling slab is the next floor's foundation; the staircase
        // exits through the hole at its shaft columns.
        if is_exit_hole(x, z) {
            return None;
        }
        return Some(vanilla_blocks::POLISHED_BLACKSTONE_BRICKS.default_state());
    }
    if chebyshev >= WALL_RADIUS {
        return Some(if mix(seed, x, layer * 31 + z) % 6 == 0 {
            vanilla_blocks::OBSIDIAN.default_state()
        } else {
            vanilla_blocks::POLISHED_BLACKSTONE_BRICKS.default_state()
        });
    }
    if pillar_columns(seed).contains(&(x, z)) && layer <= 4 {
        return Some(vanilla_blocks::OBSIDIAN.default_state());
    }
    if layer == CLEARANCE - 1 && light_columns(seed).contains(&(x, z)) {
        return Some(vanilla_blocks::GLOWSTONE.default_state());
    }
    None
}

/// Returns the block of the tower column `(dx, dz)` of `floor` at absolute
/// height `y`, or `None` for air and for cells that must keep what a previous
/// floor built.
#[must_use]
pub fn floor_block(seed: u64, floor: u32, dx: i32, dz: i32, y: i32) -> Option<BlockStateId> {
    let layer = y - floor_surface_y(floor);
    if !(-1..=CLEARANCE).contains(&layer) {
        return None;
    }
    // The foundation keeps the hole the staircase of the floor below exits
    // through; everything else on the layer is generated as usual.
    if layer == -1 && floor > 1 && is_arrival_hole(floor, dx, dz) {
        return None;
    }
    let (x, z) = canonical(floor, dx, dz);
    canonical_block(floor_seed(seed, floor), x, z, layer)
}

/// Returns every chunk column the tower footprint touches, plus the chunks
/// the previous footprint has to be erased from.
#[must_use]
pub fn castle_chunks(region: InstanceRegion, previous: Option<Footprint>) -> Vec<ChunkPos> {
    let mut chunks = tower_chunks(region);
    for chunk in footprint_chunks(region, previous) {
        if !chunks.contains(&chunk) {
            chunks.push(chunk);
        }
    }
    chunks
}

/// Returns the chunk columns the tower footprint touches.
#[must_use]
pub fn tower_chunks(region: InstanceRegion) -> Vec<ChunkPos> {
    let origin = BlockPos::from(region.spawn);
    chunks_covering(
        BlockPos::new(origin.x() - WALL_OUTER, 0, origin.z() - WALL_OUTER),
        BlockPos::new(origin.x() + WALL_OUTER, 0, origin.z() + WALL_OUTER),
    )
}

/// Builds one floor of the tower and returns the number of blocks written.
pub fn build_floor(world: &Arc<World>, region: InstanceRegion, seed: u64, floor: u32) -> usize {
    let origin = BlockPos::from(region.spawn);
    let surface = floor_surface_y(floor);
    let mut failed = 0;
    let mut blocks = 0;
    for dx in -WALL_OUTER..=WALL_OUTER {
        for dz in -WALL_OUTER..=WALL_OUTER {
            for y in surface - 1..=surface + CLEARANCE {
                let Some(block) = floor_block(seed, floor, dx, dz, y) else {
                    continue;
                };
                blocks += 1;
                set_fixture(
                    world,
                    BlockPos::new(origin.x() + dx, y, origin.z() + dz),
                    block,
                    &mut failed,
                );
            }
        }
    }
    warn_unwritten(
        "castle tower floor",
        at(region, (0, (floor as i32 - 1) * FLOOR_STEP, 0)),
        failed,
    );
    blocks
}

/// Removes every block a tower climb could have built in `region`.
///
/// The erasure always covers the whole buildable height, not only the floors
/// a footprint records: a floor job that raced the end of its run may have
/// built above the recorded count, and unwritten-air checks make the extra
/// reads cheap.
pub fn erase_castle(world: &Arc<World>, region: InstanceRegion) {
    let origin = BlockPos::from(region.spawn);
    let top = floor_surface_y(max_floors(world.get_max_y())) + CLEARANCE;
    let mut failed = 0;
    clear_box_of(
        world,
        (
            BlockPos::new(
                origin.x() - WALL_OUTER,
                INSTANCE_FLOOR_Y - 1,
                origin.z() - WALL_OUTER,
            ),
            BlockPos::new(origin.x() + WALL_OUTER, top, origin.z() + WALL_OUTER),
        ),
        &mut failed,
    );
    warn_unwritten("castle tower", origin, failed);
}

/// Returns whether `position` stands inside the hall of `floor`.
#[must_use]
pub fn floor_contains(region: InstanceRegion, floor: u32, position: DVec3) -> bool {
    let origin = BlockPos::from(region.spawn);
    let dx = position.x - f64::from(origin.x());
    let dz = position.z - f64::from(origin.z());
    let surface = f64::from(floor_surface_y(floor));
    (position.y >= surface - 0.25 && position.y < surface + f64::from(FLOOR_STEP))
        && dx.abs() <= f64::from(WALL_OUTER)
        && dz.abs() <= f64::from(WALL_OUTER)
}

/// Returns whether `position` stands on top of the staircase of `floor`, at
/// the hole into the floor above.
#[must_use]
pub fn at_ramp_top(region: InstanceRegion, floor: u32, position: DVec3) -> bool {
    let origin = BlockPos::from(region.spawn);
    let dx = position.x.floor() as i32 - origin.x();
    let dz = position.z.floor() as i32 - origin.z();
    let (x, z) = canonical(floor, dx, dz);
    position.y >= f64::from(floor_surface_y(floor)) + f64::from(RAMP_TOP) - 0.5
        && RAMP_DZ.contains(&z)
        && (EXIT_RUN_START..=RAMP_DX_MAX).contains(&x)
}

/// Returns where a hunter arrives on `floor`: on the staircase top that leads
/// through the hole in the foundation slab.
#[must_use]
pub fn floor_entry(region: InstanceRegion, floor: u32) -> DVec3 {
    let (dx, dz) = rot(9, -12, quarter(floor.wrapping_sub(1)));
    let pos = at(region, (dx, (floor as i32 - 1) * FLOOR_STEP, dz));
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()),
        f64::from(pos.z()) + 0.5,
    )
}

/// Returns the bottom of the exit frame of `floor`, standing in the middle
/// of the hall.
///
/// The frame cannot lean on a wall: the staircase rotates through all four
/// walls, and a frame in its way would be torn apart on some floor.
#[must_use]
pub fn exit_entrance(region: InstanceRegion, floor: u32) -> BlockPos {
    at(region, (0, (floor as i32 - 1) * FLOOR_STEP, 6))
}

/// Returns the seeded free columns of `floor` the wave spawns across.
#[must_use]
pub fn spawn_spots(seed: u64, floor: u32) -> Vec<(i32, i32)> {
    let salt = floor_seed(seed, floor);
    let pillars = pillar_columns(salt);
    let mut spots = Vec::new();
    for index in 0..12 {
        let gx = (mix(salt, index as i32, 101) % 21) as i32 - 10;
        let gz = (mix(salt, index as i32, 202) % 15) as i32 - 3;
        if pillars.contains(&(gx, gz)) {
            continue;
        }
        let (dx, dz) = rot(gx, gz, quarter(floor));
        // Never inside the exit frame standing in the middle of the hall.
        if (-1..=2).contains(&dx) && dz == 6 {
            continue;
        }
        spots.push((dx, dz));
    }
    if spots.is_empty() {
        spots.push((0, 0));
    }
    spots
}

/// Spawns the wave of one floor across its spawn spots.
pub fn spawn_floor_wave(
    world: &Arc<World>,
    region: InstanceRegion,
    seed: u64,
    floor: u32,
    wave: DungeonWave,
    rank: HunterRank,
) -> Vec<SpawnedMob> {
    let salt = floor_seed(seed, floor);
    let spots = spawn_spots(seed, floor);
    (0..wave.mob_count)
        .filter_map(|index| {
            let (dx, dz) = spots[usize::from(index) % spots.len()];
            let roll = salt.wrapping_add(u64::from(index).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let class = MobClass::from_roll(roll ^ (roll >> 29));
            spawn_class_mob(
                world,
                Theme::Nether,
                at(region, (dx, (floor as i32 - 1) * FLOOR_STEP, dz)),
                class,
                wave,
                rank,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::{
        CLEARANCE, EXIT_RUN_START, FLOOR_STEP, HOLE_START, RAMP_DX_MAX, RAMP_DX_MIN, RAMP_DZ,
        TOWER_HALF, WALL_OUTER, floor_block, floor_entry, floor_surface_y, is_arrival_hole,
        max_floors, quarter, ramp_rise, rot, spawn_spots,
    };
    use crate::dungeon::InstanceRegion;

    /// Runs the flood fill a hunter could walk and returns the reached cells
    /// as `(dx, dz, layer above the floor surface)`.
    fn flood_fill(seed: u64, floor: u32) -> Vec<(i32, i32, i32)> {
        let surface = floor_surface_y(floor);
        // A cell is enterable when it is air; the arrival hole's foundation
        // layer is open as well, holding the staircase top of the floor below.
        let is_air = |dx: i32, dz: i32, layer: i32| {
            if layer == -1 && floor > 1 && is_arrival_hole(floor, dx, dz) {
                return true;
            }
            floor_block(seed, floor, dx, dz, surface + layer).is_none()
        };
        let supported = |dx: i32, dz: i32, layer: i32| layer == -1 || !is_air(dx, dz, layer - 1);
        let (sx, sz) = rot(9, -12, quarter(floor.wrapping_sub(1)));
        let mut visited = vec![(sx, sz, 0)];
        let mut queue = vec![(sx, sz, 0i32)];
        while let Some((dx, dz, layer)) = queue.pop() {
            for (nx, nz) in [(dx + 1, dz), (dx - 1, dz), (dx, dz + 1), (dx, dz - 1)] {
                if nx.abs() > WALL_OUTER || nz.abs() > WALL_OUTER {
                    continue;
                }
                // Step up at most one block; falling any distance is fine.
                let mut next = None;
                for candidate in (layer - 6..=layer + 1).rev() {
                    if candidate >= 0 && is_air(nx, nz, candidate) && supported(nx, nz, candidate) {
                        next = Some(candidate);
                        break;
                    }
                }
                let Some(next) = next else {
                    continue;
                };
                if visited.contains(&(nx, nz, next)) {
                    continue;
                }
                visited.push((nx, nz, next));
                queue.push((nx, nz, next));
            }
        }
        visited
    }

    #[test]
    fn a_floor_connects_arrival_to_every_spot_and_the_stair_top() {
        init_vanilla_registry();
        for seed in [7_u64, 42, 1_337] {
            for floor in 1_u32..=6 {
                let reached = flood_fill(seed, floor);
                for spot in spawn_spots(seed, floor) {
                    assert!(
                        reached.contains(&(spot.0, spot.1, 0)),
                        "seed {seed} floor {floor}: spot {spot:?} is unreachable"
                    );
                }
                let (sx, sz) = rot(9, -12, quarter(floor.wrapping_sub(1)));
                assert!(
                    reached.contains(&(sx, sz, 0)),
                    "seed {seed} floor {floor}: the arrival hole is buried"
                );
                for gx in EXIT_RUN_START..=RAMP_DX_MAX {
                    for gz in RAMP_DZ {
                        let (dx, dz) = rot(gx, gz, quarter(floor));
                        assert!(
                            reached.contains(&(dx, dz, 8)),
                            "seed {seed} floor {floor}: staircase top ({gx}, {gz}) is unreachable"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_staircase_rises_in_walkable_steps_to_the_ceiling_hole() {
        init_vanilla_registry();
        // Floors 1 and 5 share the unrotated plan; other rotations are walked
        // by the flood-fill test.
        for floor in [1_u32, 5] {
            let surface = floor_surface_y(floor);
            for dz in RAMP_DZ {
                let mut rise = 0;
                for dx in RAMP_DX_MIN..=RAMP_DX_MAX {
                    let expected = ramp_rise(dx);
                    if expected > 0 {
                        assert!(
                            floor_block(9, floor, dx, dz, surface + expected - 1).is_some(),
                            "stair block at {dx}"
                        );
                    }
                    assert!(
                        (expected - rise).abs() <= 1,
                        "floor {floor} column {dx}: {rise} -> {expected}"
                    );
                    rise = expected;
                }
                assert_eq!(rise, CLEARANCE + 1, "the staircase ends at the hole");
            }
        }
    }

    #[test]
    fn stairs_leave_headroom_under_the_slab() {
        init_vanilla_registry();
        // Below the shaft the staircase stays low enough for a hunter's head
        // to clear the ceiling slab.
        for dx in RAMP_DX_MIN..HOLE_START {
            assert!(
                ramp_rise(dx) <= CLEARANCE - 2,
                "column {dx} rises into the slab"
            );
        }
    }

    #[test]
    fn floors_rotate_and_fit_below_the_world_ceiling() {
        init_vanilla_registry();
        let mut entries = Vec::new();
        for floor in 1_u32..=5 {
            let entry = floor_entry(InstanceRegion::from_slot(0), floor);
            entries.push((entry.x.round() as i32, entry.z.round() as i32));
            assert_eq!(entry.y as i32, floor_surface_y(floor));
        }
        for pair in entries.windows(2) {
            assert_ne!(pair[0], pair[1], "each floor must arrive somewhere new");
        }
        assert_eq!(entries[0], entries[4], "the plan repeats every four floors");
        // The default overworld build height admits a long climb.
        assert!(max_floors(320) >= 30);
        // And the highest admitted floor stays fully inside the world.
        let top = floor_surface_y(max_floors(320)) + CLEARANCE;
        assert!(top <= 320);
        assert!((320 - top) / FLOOR_STEP <= 1);
    }

    #[test]
    fn the_room_stays_inside_the_tower_walls() {
        init_vanilla_registry();
        let surface = floor_surface_y(1);
        for spot in spawn_spots(5, 1) {
            assert!(spot.0.abs() <= TOWER_HALF && spot.1.abs() <= TOWER_HALF);
        }
        // The exit frame stands in the middle of the hall, clear of the
        // staircase, pillars and lights on every rotation.
        for floor in 1_u32..=4 {
            let surface = floor_surface_y(floor);
            for y in surface..=surface + 4 {
                for x in -1..=2 {
                    assert!(
                        floor_block(5, floor, x, 6, y).is_none(),
                        "floor {floor} exit cell {x}, {y}"
                    );
                }
            }
        }
    }
}
