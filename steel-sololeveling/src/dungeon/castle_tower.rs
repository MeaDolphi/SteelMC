//! The Demon Castle tower: vast dungeon halls stacked into an endless climb.
//!
//! Every floor is one square hall — a whole world, not a literal storey — walled
//! in polished blackstone bricks. Rows of glowing columns grid the hall, a
//! gallery ledge rings the walls, and an open arena in the centre holds the
//! exit portal. A wave of the floor's rank guards the hall; once it falls, a
//! monumental eight-wide staircase rises out of the south strip, climbs one
//! half-block step per column, and punches through a tight framed stairwell in
//! the ceiling slab onto a landing gallery flush with the next floor, which is
//! generated only when a hunter commits to the climb. Each floor is the same
//! plan rotated a quarter turn, so the arrival aisle of one floor never lands
//! inside the staircase of the next.
//!
//! The hall is lit without ever raising a block above the floor: the glowing
//! inlay ring, the avenue markers and the arena pattern sit flush in the
//! foundation itself, while the capitals, wall lanterns, lava moat and
//! chandelier hang at height. A hunter never steps around a light.
//!
//! Floors are generated one at a time, outside the game tick, by a server job
//! modelled on `DungeonGenerationJob`; [`floor_block`] is the pure block
//! function that build builds from and the tests walk.

use std::sync::Arc;

use glam::DVec3;
use steel_core::world::World;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction, SlabType};
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
pub const TOWER_HALF: i32 = 48;
/// Inner wall ring.
const WALL_RADIUS: i32 = TOWER_HALF + 1;
/// Outer wall ring; also the reach of the floor slabs.
const WALL_OUTER: i32 = TOWER_HALF + 2;
/// Vertical distance between the walking surfaces of two floors.
pub const FLOOR_STEP: i32 = 12;
/// Air layers between a floor's surface and its ceiling slab, which is the
/// next floor's foundation: one slab serves as both.
const CLEARANCE: i32 = FLOOR_STEP - 1;
/// Walking height of the landing top, in blocks above the floor surface; the
/// next floor's surface.
const RAMP_TOP: i32 = CLEARANCE + 1;

/// Canonical staircase strip: the eight-wide south band the approach, the
/// flight and the landing gallery occupy.
const STRIP_X_MIN: i32 = -28;
const STRIP_X_MAX: i32 = 28;
const STRIP_Z_MIN: i32 = -30;
const STRIP_Z_MAX: i32 = -23;
/// The staircase rises one column per half-block step, climbing east; stair `i`
/// stands at `FLIGHT_X_MIN + i` at layer `i`.
const FLIGHT_X_MIN: i32 = -14;
/// Last stair column, at the ceiling-slab layer, flush with the landing.
const FLIGHT_X_MAX: i32 = FLIGHT_X_MIN + CLEARANCE;
/// First landing column: full blackstone blocks at the ceiling-slab layer,
/// flush with the next floor's surface.
const LANDING_X_MIN: i32 = FLIGHT_X_MAX + 1;
/// First flight column whose stair needs the ceiling slab open overhead: the
/// slab closes over the approach and opens only into the framed stairwell.
const HOLE_X_MIN: i32 = FLIGHT_X_MIN + (CLEARANCE - 2);
/// Railing columns flanking the flight and the approach.
const BALUSTRADE_Z: [i32; 2] = [STRIP_Z_MIN - 1, STRIP_Z_MAX + 1];

/// Chebyshev radius of the open central arena the exit portal stands in and
/// the wave rings through.
const ARENA_RADIUS: i32 = 14;
/// Chebyshev radius of the glowing inlay ring fencing the arena; its
/// shroomlights sit flush in the foundation, so the floor stays flat.
const INLAY_RADIUS: i32 = ARENA_RADIUS + 1;
/// Column rows stand on this grid on both axes.
const COLUMN_GRID: [i32; 6] = [-40, -28, -16, 16, 28, 40];
/// Column shafts rise through these layers, crowned by a glowstone capital.
const COLUMN_TOP: i32 = 5;
/// Wall-hugging gallery: a ledge of half slabs with a raised inner railing.
const GALLERY_LEDGE_RADIUS: i32 = 46;
const GALLERY_RAIL_RADIUS: i32 = 45;
const GALLERY_LEDGE_LAYER: i32 = 7;
const GALLERY_RAIL_LAYER: i32 = 8;
/// Inner wall face ring, where pilasters and soul lanterns hang.
const WALL_FACE_RADIUS: i32 = WALL_RADIUS - 1;
const LANTERN_LAYER: i32 = 3;
/// Lava moat rings, caged behind the glass parapet ring.
const LAVA_INNER: i32 = 44;
const LAVA_OUTER: i32 = 45;
const PARAPET_RADIUS: i32 = 42;
/// Nylium terrace rings behind the gallery and the inner slab tier.
const TERRACE_INNER: i32 = 44;
const TERRACE_OUTER: i32 = 47;
const TERRACE_SLAB_RADIUS: i32 = 41;

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

/// Returns whether the canonical column is inside the framed stairwell the
/// flight exits through: the two headroom columns of the topmost stairs.
fn is_exit_hole(x: i32, z: i32) -> bool {
    (STRIP_Z_MIN..=STRIP_Z_MAX).contains(&z) && (HOLE_X_MIN..=STRIP_X_MAX).contains(&x)
}

/// Returns whether the world column of `floor` is the hole its foundation
/// keeps above the staircase top of the floor below.
///
/// The hole is rotation-aware in world space: it sits above the rotated exit
/// of the previous floor, which the canonical block function cannot see.
fn is_arrival_hole(floor: u32, dx: i32, dz: i32) -> bool {
    let (x, z) = rot(dx, dz, (4 - quarter(floor.wrapping_sub(1))) % 4);
    (STRIP_Z_MIN..=STRIP_Z_MAX).contains(&z) && (HOLE_X_MIN..=STRIP_X_MAX).contains(&x)
}

/// Returns the layer of the stair block standing at the canonical strip
/// column `x`, or `None` outside the flight.
fn stair_layer(x: i32) -> Option<i32> {
    (FLIGHT_X_MIN..=FLIGHT_X_MAX)
        .contains(&x)
        .then_some(x - FLIGHT_X_MIN)
}

/// Returns whether the canonical column is inside the staircase strip — the
/// eight-wide south band the approach, flight and landing occupy.
fn in_strip(x: i32, z: i32) -> bool {
    (STRIP_X_MIN..=STRIP_X_MAX).contains(&x) && (STRIP_Z_MIN..=STRIP_Z_MAX).contains(&z)
}

/// Returns whether the canonical column is inside the west aisle the arrival
/// of the floor below lands in: this floor's strip turned back one quarter.
/// The climb below hangs over these columns, so the foundation yields to its
/// landing and no decoration may intrude.
fn in_arrival_band(x: i32, z: i32) -> bool {
    (STRIP_Z_MIN..=STRIP_Z_MAX).contains(&x) && (-STRIP_X_MAX..=STRIP_X_MAX).contains(&z)
}

/// Returns whether the canonical column must stay open at every walking
/// layer because the climb or the arrival passes through it. No decoration
/// may intrude here, whatever the archetype.
fn in_shaft_corridor(x: i32, z: i32) -> bool {
    in_strip(x, z) || in_arrival_band(x, z)
}

/// Returns whether the canonical column is one the exit portal rotates
/// across on the four quarter turns. The portal stands at a fixed world
/// position on every floor, so its box sweeps this orbit through the plan;
/// every cell of it must stay clear at walking height.
fn in_exit_orbit(x: i32, z: i32) -> bool {
    (-1..=2).contains(&x) && z == 6
        || x == -6 && (-1..=2).contains(&z)
        || (-2..=1).contains(&x) && z == -6
        || x == 6 && (-2..=1).contains(&z)
}

/// Returns the slab layer of the balustrade curb flanking the approach and
/// the flight at canonical `x`, or `None` off the railing line.
fn balustrade_layer(x: i32, z: i32) -> Option<i32> {
    if !BALUSTRADE_Z.contains(&z) || !(STRIP_X_MIN..=FLIGHT_X_MAX - 1).contains(&x) {
        return None;
    }
    Some(stair_layer(x).map_or(1, |stair| stair + 1))
}

/// Returns the world facing of the canonical east-facing staircase after the
/// floor's quarter turn. Rotating the plan rotates the climb direction, and
/// the stair blocks must rotate with it: a stair facing across the climb is a
/// full block to step onto, and the flight becomes unclimbable.
const fn stair_facing(q: u32) -> Direction {
    match q % 4 {
        0 => Direction::East,
        1 => Direction::South,
        2 => Direction::West,
        _ => Direction::North,
    }
}

/// Rotates the stair facing with the floor's quarter turn; every other block
/// the tower builds is either a full cube or rotation-symmetric.
fn face_stairs(block: BlockStateId, q: u32) -> BlockStateId {
    if q == 0
        || block
            .try_get_value(&BlockStateProperties::STAIRS_SHAPE)
            .is_none()
    {
        return block;
    }
    block.set_value(&BlockStateProperties::HORIZONTAL_FACING, stair_facing(q))
}

/// Returns the east-facing stair block the flight is built from; the floor
/// rotation re-faces it per quarter turn.
fn east_stairs() -> BlockStateId {
    vanilla_blocks::POLISHED_BLACKSTONE_STAIRS
        .default_state()
        .set_value(&BlockStateProperties::HORIZONTAL_FACING, Direction::East)
}

/// Returns a bottom half slab of polished blackstone bricks.
fn bottom_slab() -> BlockStateId {
    vanilla_blocks::POLISHED_BLACKSTONE_BRICK_SLAB
        .default_state()
        .set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Bottom)
}

/// The plan one floor's hall is decorated with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Archetype {
    /// Carpeted avenues from the arrival aisle to the arena and on to the
    /// staircase mouth.
    Colonnade,
    /// A lava moat along the hall's edge, caged behind a glass parapet.
    LavaArena,
    /// A nylium garden ring behind the gallery with an inner slab tier.
    Terraces,
    /// A throne dais against the far wall with dense lighting, every tenth floor.
    Throne,
}

impl Archetype {
    /// Returns the plan of `floor`: the throne hall every tenth floor,
    /// otherwise a seeded pick from the run seed that never repeats the
    /// previous floor's plan.
    fn of(seed: u64, floor: u32) -> Self {
        const CYCLE: [Archetype; 3] = [
            Archetype::Colonnade,
            Archetype::LavaArena,
            Archetype::Terraces,
        ];
        if floor.is_multiple_of(10) {
            return Self::Throne;
        }
        let mut index = (mix(floor_seed(seed, floor), 11, 17) % CYCLE.len() as u64) as usize;
        // Compare against the previous floor's effective plan, not its raw
        // pick: the previous floor may itself have been shifted off its pick.
        if floor > 1 && Self::of(seed, floor - 1) == CYCLE[index] {
            index = (index + 1) % CYCLE.len();
        }
        CYCLE[index]
    }

    /// Returns the block of the archetype's plan at the canonical column
    /// `(x, z)` at `layer`, or `None` where the plan leaves the cell to the
    /// shared architecture (or air).
    fn block_at(self, salt: u64, x: i32, z: i32, layer: i32) -> Option<BlockStateId> {
        match self {
            Self::Colonnade => colonnade_block(x, z, layer),
            Self::LavaArena => lava_arena_block(x, z, layer),
            Self::Terraces => terraces_block(salt, x, z, layer),
            Self::Throne => throne_block(x, z, layer),
        }
    }
}

/// Returns the colonnade plan: carpeted avenues from the arrival aisle west
/// of the arena to the centre, and from the centre south into the staircase
/// mouth.
fn colonnade_block(x: i32, z: i32, layer: i32) -> Option<BlockStateId> {
    if layer != 0 {
        return None;
    }
    if (z == -1 || z == 0) && (-22..=-15).contains(&x) {
        return Some(vanilla_blocks::RED_CARPET.default_state());
    }
    if (x == -15 || x == -14) && (-22..=-2).contains(&z) {
        return Some(vanilla_blocks::RED_CARPET.default_state());
    }
    None
}

/// Returns the lava arena plan: a lava moat glowing through the gallery
/// along the hall's edge at walking level, caged behind a two-block glass
/// parapet, with stone piers on the axes.
fn lava_arena_block(x: i32, z: i32, layer: i32) -> Option<BlockStateId> {
    let ring = x.abs().max(z.abs());
    let pier = (x == 0 || z == 0) && (LAVA_INNER..=LAVA_OUTER).contains(&ring);
    if pier && layer == 0 {
        return Some(vanilla_blocks::POLISHED_BLACKSTONE.default_state());
    }
    if (LAVA_INNER..=LAVA_OUTER).contains(&ring) && layer == 0 {
        return Some(vanilla_blocks::LAVA.default_state());
    }
    if ring == PARAPET_RADIUS && (0..=1).contains(&layer) {
        return Some(vanilla_blocks::GLASS.default_state());
    }
    None
}

/// Returns the terraces plan: a nylium garden ring along the walls behind
/// the gallery and a half-slab tier inside it, planted with a crimson and
/// warped garden.
fn terraces_block(salt: u64, x: i32, z: i32, layer: i32) -> Option<BlockStateId> {
    let ring = x.abs().max(z.abs());
    if (TERRACE_INNER..=TERRACE_OUTER).contains(&ring) {
        let soil = mix(salt, x, z) % 6;
        if layer == 0 {
            let terrace: BlockRef = match soil {
                0 => &vanilla_blocks::WARPED_NYLIUM,
                5 => &vanilla_blocks::POLISHED_BLACKSTONE,
                _ => &vanilla_blocks::CRIMSON_NYLIUM,
            };
            return Some(terrace.default_state());
        }
        if layer == 1 && soil != 5 && mix(salt, x, z + 977).is_multiple_of(5) {
            let plant: BlockRef = match mix(salt, x, z + 1999) % 3 {
                0 => &vanilla_blocks::CRIMSON_FUNGUS,
                1 => &vanilla_blocks::WARPED_FUNGUS,
                _ => &vanilla_blocks::NETHER_SPROUTS,
            };
            return Some(plant.default_state());
        }
        return None;
    }
    if (TERRACE_SLAB_RADIUS..=TERRACE_SLAB_RADIUS + 2).contains(&ring) && layer == 0 {
        return Some(bottom_slab());
    }
    None
}

/// Returns the throne plan: a red nether brick dais against the far wall
/// with a blackstone throne, shroomlight inlay and a red carpet from the
/// portal to the dais steps.
fn throne_block(x: i32, z: i32, layer: i32) -> Option<BlockStateId> {
    // The carpet stops at the exit portal and resumes past it to the steps.
    if (-1..=2).contains(&x) && (7..=38).contains(&z) && layer == 0 {
        return Some(vanilla_blocks::RED_CARPET.default_state());
    }
    if (-8..=9).contains(&x) && (40..=47).contains(&z) {
        match layer {
            0 => return Some(vanilla_blocks::RED_NETHER_BRICKS.default_state()),
            1 => {
                if (x == -8 || x == 9) && (z == 40 || z == 47) {
                    return Some(vanilla_blocks::SHROOMLIGHT.default_state());
                }
                return Some(vanilla_blocks::RED_NETHER_BRICKS.default_state());
            }
            2 => {
                if (x == -2 || x == 3) && z == 46 {
                    return Some(vanilla_blocks::SHROOMLIGHT.default_state());
                }
                if (-1..=2).contains(&x) && (45..=46).contains(&z) {
                    return Some(vanilla_blocks::BLACKSTONE.default_state());
                }
                return None;
            }
            3..=5 if (-1..=2).contains(&x) && z == 47 => {
                return Some(vanilla_blocks::BLACKSTONE.default_state());
            }
            _ => {}
        }
        return None;
    }
    // The wide step in front of the dais: a half slab, then the dais block.
    if (-8..=9).contains(&x) && z == 39 && layer == 0 {
        return Some(bottom_slab());
    }
    None
}

/// Returns the foundation block of the canonical cell `(x, z)`: a patterned
/// blackstone floor whose light sources sit flush in it, so the walking
/// surface stays flat — a hunter never steps around a glowing block.
fn foundation_block(salt: u64, x: i32, z: i32) -> BlockStateId {
    if x.abs().max(z.abs()) == INLAY_RADIUS {
        return vanilla_blocks::SHROOMLIGHT.default_state();
    }
    if is_avenue_marker(x, z) {
        return vanilla_blocks::GLOWSTONE.default_state();
    }
    if mix(salt, x, z).is_multiple_of(5) {
        vanilla_blocks::BLACKSTONE.default_state()
    } else {
        vanilla_blocks::POLISHED_BLACKSTONE.default_state()
    }
}

/// Returns whether the foundation cell lights the way from the arrival aisle
/// west of the arena to the staircase mouth: a lit avenue to the centre and a
/// lit arm south into the strip's approach.
fn is_avenue_marker(x: i32, z: i32) -> bool {
    (z == 0 && (-22..=-15).contains(&x) && x % 4 == 0)
        || (x == -15 && (-22..=-2).contains(&z) && z % 4 == 0)
        || (z == -26 && (STRIP_X_MIN..=-16).contains(&x) && x % 4 == 0)
}

/// Rows of blackstone columns on the grid, crowned with glowstone capitals.
fn column_block(x: i32, z: i32, layer: i32) -> Option<BlockStateId> {
    if !COLUMN_GRID.contains(&x) || !COLUMN_GRID.contains(&z) {
        return None;
    }
    if layer < COLUMN_TOP {
        return Some(vanilla_blocks::POLISHED_BLACKSTONE_BRICKS.default_state());
    }
    (layer == COLUMN_TOP).then_some(vanilla_blocks::GLOWSTONE.default_state())
}

/// A wall-hugging gallery: a ledge of half slabs with a raised inner railing.
fn gallery_block(chebyshev: i32, layer: i32) -> Option<BlockStateId> {
    if (GALLERY_LEDGE_RADIUS..=GALLERY_LEDGE_RADIUS + 1).contains(&chebyshev)
        && layer == GALLERY_LEDGE_LAYER
    {
        return Some(bottom_slab());
    }
    if chebyshev == GALLERY_RAIL_RADIUS && layer == GALLERY_RAIL_LAYER {
        return Some(bottom_slab());
    }
    None
}

/// Returns whether the canonical column is a wall pilaster: a full-height
/// pier protruding one block into the hall on an eight-block grid.
fn is_pilaster(x: i32, z: i32) -> bool {
    x.abs().max(z.abs()) == WALL_FACE_RADIUS && (x % 8 == 0 || z % 8 == 0)
}

/// Wall pilasters protruding one block into the hall on an eight-block grid.
fn pilaster_block(x: i32, z: i32, layer: i32) -> Option<BlockStateId> {
    if !is_pilaster(x, z) || layer > CLEARANCE {
        return None;
    }
    Some(vanilla_blocks::POLISHED_BLACKSTONE.default_state())
}

/// The shared elevated lighting: soul lanterns along the walls and the
/// chandelier over the arena. Floor-level light lives in the foundation.
fn common_light(salt: u64, x: i32, z: i32, layer: i32) -> Option<BlockStateId> {
    if layer == LANTERN_LAYER
        && x.abs().max(z.abs()) == WALL_FACE_RADIUS
        && mix(salt, x, z).is_multiple_of(6)
        && !is_pilaster(x, z)
    {
        return Some(vanilla_blocks::SOUL_LANTERN.default_state());
    }
    if (0..=1).contains(&x) && (0..=1).contains(&z) {
        if (9..=10).contains(&layer) {
            return Some(vanilla_blocks::IRON_CHAIN.default_state());
        }
        if layer == 8 {
            let cluster = if (x + z) % 2 == 0 {
                &vanilla_blocks::SHROOMLIGHT
            } else {
                &vanilla_blocks::GLOWSTONE
            };
            return Some(cluster.default_state());
        }
    }
    None
}

/// Returns the block of the canonical column `(x, z)` at `layer` above the
/// floor surface, or `None` for air.
fn canonical_block(arch: Archetype, salt: u64, x: i32, z: i32, layer: i32) -> Option<BlockStateId> {
    let chebyshev = x.abs().max(z.abs());
    if chebyshev > WALL_OUTER {
        return None;
    }
    if in_strip(x, z) {
        // The staircase is a bare flight of stair blocks one column per
        // half-block step, air underneath, with a blackstone landing gallery
        // at the ceiling-slab layer under the next floor's arrival walk.
        if let Some(of_stair) = stair_layer(x) {
            if layer == of_stair {
                return Some(east_stairs());
            }
        } else if (LANDING_X_MIN..=STRIP_X_MAX).contains(&x) && layer == CLEARANCE {
            return Some(vanilla_blocks::BLACKSTONE.default_state());
        }
        if (0..=CLEARANCE - 1).contains(&layer) {
            // The approach and the air under the flight and landing stay open.
            return None;
        }
    }
    if let Some(rail) = balustrade_layer(x, z) {
        // The railing yields to the shaft corridor: one quarter turn back its
        // west end hangs over the staircase of the floor below, and a curb
        // there would close the climber's headroom.
        if layer == rail && !in_shaft_corridor(x, z) {
            return Some(bottom_slab());
        }
    }
    if layer == -1 {
        return Some(foundation_block(salt, x, z));
    }
    if layer > CLEARANCE {
        return None;
    }
    if layer == CLEARANCE {
        // The ceiling slab is the next floor's foundation; the staircase
        // exits through the framed stairwell kept at its headroom columns.
        if is_exit_hole(x, z) {
            return None;
        }
        return Some(vanilla_blocks::POLISHED_BLACKSTONE_BRICKS.default_state());
    }
    if chebyshev >= WALL_RADIUS {
        return Some(if mix(salt, x, layer * 31 + z).is_multiple_of(6) {
            vanilla_blocks::OBSIDIAN.default_state()
        } else {
            vanilla_blocks::POLISHED_BLACKSTONE_BRICKS.default_state()
        });
    }
    if in_shaft_corridor(x, z) || in_exit_orbit(x, z) {
        // The strip cells that are neither a stair nor the landing, the
        // arrival aisle of the floor below and the exit portal's orbit all
        // stay open for the climb; no decoration may intrude here.
        return None;
    }
    arch.block_at(salt, x, z, layer)
        .or_else(|| column_block(x, z, layer))
        .or_else(|| gallery_block(chebyshev, layer))
        .or_else(|| pilaster_block(x, z, layer))
        .or_else(|| common_light(salt, x, z, layer))
}

/// Whether the floor under a floor being built already stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorBelow {
    /// The climb built the floor below; its ceiling slab fills the stairwell
    /// the foundation keeps for the staircase.
    Built,
    /// The run entered the tower at this floor and nothing stands below, so
    /// the foundation closes the stairwell instead of opening onto the void.
    Missing,
}

/// Returns the block of the tower column `(dx, dz)` of `floor` at absolute
/// height `y`, or `None` for air and for cells that must keep what a previous
/// floor built.
#[must_use]
pub fn floor_block(seed: u64, floor: u32, dx: i32, dz: i32, y: i32) -> Option<BlockStateId> {
    floor_block_over(seed, floor, FloorBelow::Built, dx, dz, y)
}

/// Returns the block of the tower column like [`floor_block`], over a floor
/// below that may not have been built.
fn floor_block_over(
    seed: u64,
    floor: u32,
    below: FloorBelow,
    dx: i32,
    dz: i32,
    y: i32,
) -> Option<BlockStateId> {
    let layer = y - floor_surface_y(floor);
    if !(-1..=CLEARANCE).contains(&layer) {
        return None;
    }
    // The foundation keeps the hole the staircase of the floor below exits
    // through; everything else on the layer is generated as usual.
    if layer == -1 && floor > 1 && below == FloorBelow::Built && is_arrival_hole(floor, dx, dz) {
        return None;
    }
    let (x, z) = canonical(floor, dx, dz);
    let block = canonical_block(
        Archetype::of(seed, floor),
        floor_seed(seed, floor),
        x,
        z,
        layer,
    );
    block.map(|block| face_stairs(block, quarter(floor)))
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
///
/// Besides the floor itself the build writes the ceiling slab of the floor
/// above (one more slab up): the climb's next hall is generated lazily, and
/// without that cap the stairwell looks straight into the sky of the world
/// until it is built. The slab's own stairwell is rotated away, so no shaft
/// lines up with the one below.
pub fn build_floor(
    world: &Arc<World>,
    region: InstanceRegion,
    seed: u64,
    floor: u32,
    below: FloorBelow,
) -> usize {
    let origin = BlockPos::from(region.spawn);
    let surface = floor_surface_y(floor);
    let mut failed = 0;
    let mut blocks = 0;
    let next_ceiling = floor_surface_y(floor + 1) + CLEARANCE;
    let with_next_ceiling = next_ceiling <= world.get_max_y();
    for dx in -WALL_OUTER..=WALL_OUTER {
        for dz in -WALL_OUTER..=WALL_OUTER {
            for y in surface - 1..=surface + CLEARANCE {
                let Some(block) = floor_block_over(seed, floor, below, dx, dz, y) else {
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
            if with_next_ceiling
                && let Some(block) = floor_block(seed, floor + 1, dx, dz, next_ceiling)
            {
                blocks += 1;
                set_fixture(
                    world,
                    BlockPos::new(origin.x() + dx, next_ceiling, origin.z() + dz),
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
/// the stairwell into the floor above.
#[must_use]
pub fn at_ramp_top(region: InstanceRegion, floor: u32, position: DVec3) -> bool {
    let origin = BlockPos::from(region.spawn);
    let dx = position.x.floor() as i32 - origin.x();
    let dz = position.z.floor() as i32 - origin.z();
    let (x, z) = canonical(floor, dx, dz);
    position.y >= f64::from(floor_surface_y(floor)) + f64::from(RAMP_TOP) - 0.5
        && (STRIP_Z_MIN..=STRIP_Z_MAX).contains(&z)
        && (LANDING_X_MIN..=STRIP_X_MAX).contains(&x)
}

/// Returns where a hunter arrives on `floor`: on the landing gallery of the
/// floor below that leads through the stairwell in the foundation slab.
#[must_use]
pub fn floor_entry(region: InstanceRegion, floor: u32) -> DVec3 {
    let (dx, dz) = rot(12, -26, quarter(floor.wrapping_sub(1)));
    let pos = at(region, (dx, (floor as i32 - 1) * FLOOR_STEP, dz));
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()),
        f64::from(pos.z()) + 0.5,
    )
}

/// Returns where a hunter entering the tower directly at `floor` stands: the
/// middle of that floor's arena.
///
/// A climb only ever builds the floors it reaches, so the floor below — and
/// the landing [`floor_entry`] stands on — does not exist on a directly
/// entered floor.
#[must_use]
pub fn hall_center(region: InstanceRegion, floor: u32) -> DVec3 {
    let pos = at(region, (0, (floor as i32 - 1) * FLOOR_STEP, 0));
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
///
/// A spot is usable only when the cell above the floor surface is open, which
/// keeps every archetype's columns, moats, terraces and dais steps out of the
/// spawn pool with one rule.
#[must_use]
pub fn spawn_spots(seed: u64, floor: u32) -> Vec<(i32, i32)> {
    let salt = floor_seed(seed, floor);
    let surface = floor_surface_y(floor);
    let mut spots = Vec::new();
    for index in 0..16 {
        let gx = (mix(salt, index, 101) % 29) as i32 - 14;
        let gz = (mix(salt, index, 202) % 29) as i32 - 14;
        let (dx, dz) = rot(gx, gz, quarter(floor));
        // Never inside the exit frame standing in the middle of the hall.
        if (-1..=2).contains(&dx) && dz == 6 {
            continue;
        }
        if floor_block(seed, floor, dx, dz, surface).is_some() {
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
    use std::collections::VecDeque;

    use rustc_hash::{FxHashMap, FxHashSet};
    use steel_registry::blocks::block_state_ext::BlockStateExt as _;
    use steel_registry::blocks::properties::{BlockStateProperties, Direction};
    use steel_registry::init_vanilla_registry;
    use steel_registry::vanilla_blocks;
    use steel_utils::BlockPos;
    use steel_utils::axis::Axis;

    use super::{
        Archetype, CLEARANCE, FLIGHT_X_MAX, FLIGHT_X_MIN, FLOOR_STEP, FloorBelow, HOLE_X_MIN,
        INLAY_RADIUS, LANDING_X_MIN, LAVA_INNER, LAVA_OUTER, RAMP_TOP, STRIP_X_MAX, STRIP_X_MIN,
        STRIP_Z_MAX, STRIP_Z_MIN, TOWER_HALF, WALL_OUTER, canonical, east_stairs, floor_block,
        floor_block_over, floor_entry, floor_surface_y, max_floors, quarter, rot, spawn_spots,
        stair_facing, stair_layer,
    };
    use crate::dungeon::InstanceRegion;

    /// Which side of a cell a stand may be entered from. A stair's low step
    /// only receives a climber coming up its facing line, and its high half
    /// only one coming down it; from the sides a stair is a full block wall.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Side {
        /// Any neighbour: full blocks, slabs and open floor.
        Any,
        /// The stair's low step: enterable only from the neighbour the stair
        /// faces away from.
        StairLow,
        /// The stair's high half: enterable only from the neighbour the stair
        /// faces toward.
        StairHigh,
    }

    /// A stand the walker can rest at: cell plus feet height in half blocks
    /// above `floor`'s surface, so stairs split into their two half steps.
    type Stand = (i32, i32, i32);

    #[test]
    fn direct_entry_lands_on_solid_ground_at_any_floor() {
        init_vanilla_registry();
        for floor in 1_u32..=max_floors(320) {
            let surface = floor_surface_y(floor);
            let center = super::hall_center(InstanceRegion::from_slot(0), floor);
            assert_eq!(center.y, f64::from(surface));
            assert!(
                floor_block(9, floor, 0, 0, surface - 1).is_some(),
                "floor {floor}: the arena centre has no floor under it"
            );
            assert!(
                floor_block(9, floor, 0, 0, surface).is_none(),
                "floor {floor}: the arena centre is blocked"
            );
        }
    }

    #[test]
    fn the_next_ceiling_covers_every_stairwell_from_below() {
        init_vanilla_registry();
        // The cap build_floor writes one slab up must cover every open cell of
        // the floor's own stairwell, or a climber at the landing still sees
        // the sky through the ceiling above.
        for floor in 1_u32..6 {
            let surface = floor_surface_y(floor);
            let next_ceiling = floor_surface_y(floor + 1) + CLEARANCE;
            for dx in -WALL_OUTER..=WALL_OUTER {
                for dz in -WALL_OUTER..=WALL_OUTER {
                    if floor_block(7, floor, dx, dz, surface + CLEARANCE).is_none() {
                        assert!(
                            floor_block(7, floor + 1, dx, dz, next_ceiling).is_some(),
                            "floor {floor}: stairwell cell ({dx}, {dz}) sees the sky"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_directly_entered_floor_has_a_whole_foundation() {
        init_vanilla_registry();
        // Nothing stands under the floor a run enters at, so a stairwell kept
        // in its foundation would be a trench straight into the void.
        for floor in 2_u32..6 {
            let surface = floor_surface_y(floor);
            for dx in -WALL_OUTER..=WALL_OUTER {
                for dz in -WALL_OUTER..=WALL_OUTER {
                    assert!(
                        floor_block_over(7, floor, FloorBelow::Missing, dx, dz, surface - 1)
                            .is_some(),
                        "floor {floor}: the foundation is open at ({dx}, {dz})"
                    );
                }
            }
        }
    }

    /// Reads the world column the way the incremental build leaves it: floor
    /// `n + 1` is written over floor `n`'s ceiling slab, except where its
    /// generator steps aside for the staircase below and keeps what it finds.
    fn world_block(
        seed: u64,
        built: u32,
        dx: i32,
        dz: i32,
        y: i32,
    ) -> Option<steel_utils::BlockStateId> {
        for floor in (1..=built).rev() {
            let surface = floor_surface_y(floor);
            if !(surface - 1..=surface + CLEARANCE).contains(&y) {
                continue;
            }
            if y == surface - 1 && floor > 1 {
                if let Some(block) = floor_block(seed, floor, dx, dz, y) {
                    return Some(block);
                }
                continue;
            }
            return floor_block(seed, floor, dx, dz, y);
        }
        None
    }

    /// A grid walk over the generated world with vanilla movement rules: each
    /// step up is at most half a block and the two blocks above the feet must
    /// stay clear. Falls of any height are allowed, like a hunter dropping
    /// off a ledge.
    /// Collision boxes of one column as `(layer, min_y, max_y)` triples
    /// relative to the floor surface.
    type ColumnBoxes = Vec<(i32, f64, f64)>;
    /// Stand heights of one column: feet height halves, the layer of the
    /// supporting box, and the side the stand takes a climber from.
    type ColumnStands = Vec<(i32, i32, Side)>;

    struct Walk {
        seed: u64,
        built: u32,
        surface: i32,
        aabbs: FxHashMap<(i32, i32), ColumnBoxes>,
        stands: FxHashMap<(i32, i32), ColumnStands>,
        facings: FxHashMap<(i32, i32), Option<(i32, i32)>>,
    }

    impl Walk {
        fn new(seed: u64, built: u32, floor: u32) -> Self {
            Self {
                seed,
                built,
                surface: floor_surface_y(floor),
                aabbs: FxHashMap::default(),
                stands: FxHashMap::default(),
                facings: FxHashMap::default(),
            }
        }

        /// Collision boxes of the column at `(dx, dz)`, from the real block
        /// shapes.
        fn aabbs_of(&mut self, dx: i32, dz: i32) -> ColumnBoxes {
            if let Some(aabbs) = self.aabbs.get(&(dx, dz)) {
                return aabbs.clone();
            }
            let mut out = Vec::new();
            for layer in -1..=CLEARANCE + 2 {
                let Some(block) = world_block(self.seed, self.built, dx, dz, self.surface + layer)
                else {
                    continue;
                };
                for aabb in block.get_static_collision_shape() {
                    out.push((
                        layer,
                        aabb.min(Axis::Y) + f64::from(layer),
                        aabb.max(Axis::Y) + f64::from(layer),
                    ));
                }
            }
            self.aabbs.insert((dx, dz), out.clone());
            out
        }

        /// The facing vector of the stair block in the cell, if the cell is
        /// a stair of the flight.
        fn stair_facing_of(&mut self, dx: i32, dz: i32) -> Option<(i32, i32)> {
            if let Some(facing) = self.facings.get(&(dx, dz)) {
                return *facing;
            }
            let mut found = None;
            for layer in 0..=CLEARANCE {
                let Some(block) = world_block(self.seed, self.built, dx, dz, self.surface + layer)
                else {
                    continue;
                };
                if let Some(facing) = block.try_get_value(&BlockStateProperties::HORIZONTAL_FACING)
                {
                    found = Some(match facing {
                        Direction::East => (1, 0),
                        Direction::West => (-1, 0),
                        Direction::South => (0, 1),
                        _ => (0, -1),
                    });
                    break;
                }
            }
            self.facings.insert((dx, dz), found);
            found
        }

        /// Feet heights, in half blocks above the surface, a hunter can stand
        /// at in the cell: the supported tops of every collision box, paired
        /// with the layer of the box they rest on and the side it takes a
        /// climber from.
        fn stands_of(&mut self, dx: i32, dz: i32) -> ColumnStands {
            if let Some(stands) = self.stands.get(&(dx, dz)) {
                return stands.clone();
            }
            let aabbs = self.aabbs_of(dx, dz);
            let stair = self.stair_facing_of(dx, dz).is_some();
            let mut halves: ColumnStands = aabbs
                .iter()
                .map(|&(layer, _, max)| {
                    let half = (max * 2.0).round() as i32;
                    let side = match (stair, half == 2 * layer + 1) {
                        (false, _) => Side::Any,
                        (true, true) => Side::StairLow,
                        (true, false) => Side::StairHigh,
                    };
                    (half, layer, side)
                })
                .collect();
            halves.sort_unstable_by_key(|&(half, _, _)| half);
            halves.dedup();
            halves.retain(|&(half, _, _)| {
                let height = f64::from(half) / 2.0;
                aabbs.iter().any(|&(_, _, max)| (max - height).abs() < 1e-6)
            });
            self.stands.insert((dx, dz), halves.clone());
            halves
        }

        /// Returns whether the two blocks above `half` stay clear. Boxes of
        /// the supporting layer itself never count: the two half steps of a
        /// stair share one layer, and only what stands above that layer can
        /// hit a climber's head.
        fn headroom(&mut self, dx: i32, dz: i32, half: i32, support: i32) -> bool {
            let height = f64::from(half) / 2.0;
            self.aabbs_of(dx, dz).iter().all(|&(layer, min, max)| {
                layer <= support || min >= height + 1.8 - 1e-9 || max <= height + 1e-9
            })
        }

        /// Flood fill from `starts` and return every reached stand.
        fn reachable(&mut self, starts: Vec<Stand>) -> FxHashSet<Stand> {
            let mut seen = FxHashSet::default();
            let mut queue = VecDeque::new();
            for (dx, dz, half) in starts {
                let supported = self
                    .stands_of(dx, dz)
                    .into_iter()
                    .find(|&(stand, _, _)| stand == half)
                    .map(|(_, support, _)| support);
                if let Some(support) = supported
                    && self.headroom(dx, dz, half, support)
                {
                    seen.insert((dx, dz, half));
                    queue.push_back((dx, dz, half));
                }
            }
            while let Some((dx, dz, half)) = queue.pop_front() {
                for (nx, nz) in [
                    (dx + 1, dz),
                    (dx - 1, dz),
                    (dx, dz + 1),
                    (dx, dz - 1),
                    (dx, dz),
                ] {
                    if nx.abs() > WALL_OUTER || nz.abs() > WALL_OUTER {
                        continue;
                    }
                    let move_dir = (nx - dx, nz - dz);
                    let facing = self.stair_facing_of(nx, nz);
                    for &(nhalf, nsupport, side) in &self.stands_of(nx, nz) {
                        if nhalf == half && (nx, nz) == (dx, dz) {
                            continue;
                        }
                        if nhalf - half > 1 {
                            continue;
                        }
                        // A stair only receives a climber along its facing
                        // line: onto the low step coming up from behind, onto
                        // the high half coming down from ahead. From the
                        // sides a stair is a full block wall.
                        if let Some((fx, fz)) = facing
                            && move_dir != (0, 0)
                        {
                            let along_facing = move_dir == (fx, fz);
                            match side {
                                Side::StairLow if !along_facing => continue,
                                Side::StairHigh if along_facing => continue,
                                _ => {}
                            }
                        }
                        let next = (nx, nz, nhalf);
                        if seen.contains(&next) || !self.headroom(nx, nz, nhalf, nsupport) {
                            continue;
                        }
                        seen.insert(next);
                        queue.push_back(next);
                    }
                }
            }
            seen
        }
    }

    /// Returns the cell a hunter arrives on `floor` at (their feet stand on
    /// the landing the floor below leaves under the stairwell).
    fn entry_cell(floor: u32) -> (i32, i32) {
        let region = InstanceRegion::from_slot(0);
        let origin = BlockPos::from(region.spawn);
        let entry = floor_entry(region, floor);
        (
            entry.x.floor() as i32 - origin.x(),
            entry.z.floor() as i32 - origin.z(),
        )
    }

    /// Walks both phases of one floor. Phase A builds only up to `floor` and
    /// climbs the staircase; phase B builds the floor above and walks in from
    /// the arrival teleport. Returns the reached stands of each phase.
    fn climb_phases(seed: u64, floor: u32) -> (FxHashSet<Stand>, Option<FxHashSet<Stand>>) {
        let phase_a = {
            let start = if floor == 1 {
                // The run starts hunters at the region spawn, in the arena.
                (0, 0, 0)
            } else {
                let (dx, dz) = entry_cell(floor);
                (dx, dz, 0)
            };
            Walk::new(seed, floor, floor).reachable(vec![start])
        };
        let phase_b = (floor < max_floors(320)).then(|| {
            let (dx, dz) = entry_cell(floor + 1);
            Walk::new(seed, floor + 1, floor + 1).reachable(vec![(dx, dz, 0)])
        });
        (phase_a, phase_b)
    }

    /// Asserts the walk reached every spawn spot, the exit portal, the
    /// staircase mouth, every stair of the flight and the landing gallery of
    /// `floor`, all in world coordinates.
    fn assert_floor_walked(seed: u64, floor: u32, reached: &FxHashSet<Stand>) {
        let q = quarter(floor);
        for spot in spawn_spots(seed, floor) {
            let (dx, dz) = rot(spot.0, spot.1, q);
            assert!(
                reached.contains(&(dx, dz, 0)),
                "seed {seed} floor {floor}: spawn spot {spot:?} is unreachable"
            );
        }
        // Beside the exit portal frame standing in the middle of the hall.
        for cell in [(0, 5), (1, 7)] {
            let (dx, dz) = rot(cell.0, cell.1, q);
            assert!(
                reached.contains(&(dx, dz, 0)),
                "seed {seed} floor {floor}: the exit portal at {cell:?} is unreachable"
            );
        }
        // The mouth of the staircase, at the west end of the approach.
        let (mx, mz) = rot(FLIGHT_X_MIN - 1, -26, q);
        assert!(
            reached.contains(&(mx, mz, 0)),
            "seed {seed} floor {floor}: the staircase mouth is unreachable"
        );
        // Every stair of the flight, at its exit height, on both flanks.
        for i in 0..=CLEARANCE {
            for z in [STRIP_Z_MIN + 1, STRIP_Z_MAX - 1] {
                let (dx, dz) = rot(FLIGHT_X_MIN + i, z, q);
                assert!(
                    reached.contains(&(dx, dz, 2 * i + 2)),
                    "seed {seed} floor {floor}: stair {i} at ({z}) is unclimbable"
                );
            }
        }
        // The landing gallery at the next floor's height, triggering the climb.
        for x in [LANDING_X_MIN, 12, STRIP_X_MAX] {
            let (dx, dz) = rot(x, -26, q);
            assert!(
                reached.contains(&(dx, dz, 2 * RAMP_TOP)),
                "seed {seed} floor {floor}: landing column {x} is unreachable"
            );
        }
    }

    #[test]
    fn the_climb_walks_every_floor_and_quarter() {
        init_vanilla_registry();
        let floors = [
            1_u32,
            2,
            3,
            4,
            5,
            6,
            11,
            max_floors(320) - 1,
            max_floors(320),
        ];
        for seed in [7_u64, 1_337] {
            for floor in floors {
                let (a, b) = climb_phases(seed, floor);
                assert_floor_walked(seed, floor, &a);
                if let Some(b) = b {
                    assert_floor_walked(seed, floor + 1, &b);
                    let (ex, ez) = entry_cell(floor + 1);
                    assert!(
                        b.contains(&(ex, ez, 0)),
                        "seed {seed} floor {}: the arrival teleport target is buried",
                        floor + 1
                    );
                }
            }
        }
    }

    /// Returns one floor of `seed` for every archetype on every quarter turn
    /// it can appear on, or panics when a hundred and twenty floors cannot
    /// produce them all. The throne hall stands on every tenth floor, and a
    /// multiple of ten is always an odd quarter turn, so the throne can only
    /// ever be met on quarters one and three.
    fn archetype_quarter_matrix(seed: u64) -> Vec<u32> {
        let mut wanted: Vec<(Archetype, u32)> = Vec::new();
        for quarter in 0..4 {
            for arch in [
                Archetype::Colonnade,
                Archetype::LavaArena,
                Archetype::Terraces,
            ] {
                wanted.push((arch, quarter));
            }
        }
        for quarter in [1, 3] {
            wanted.push((Archetype::Throne, quarter));
        }
        let mut matrix: Vec<((Archetype, u32), u32)> = Vec::new();
        for floor in 1_u32..=120 {
            let arch = Archetype::of(seed, floor);
            let slot = (arch, quarter(floor));
            if wanted.contains(&slot) && !matrix.iter().any(|(s, _)| *s == slot) {
                matrix.push((slot, floor));
            }
        }
        assert_eq!(
            matrix.len(),
            wanted.len(),
            "seed {seed}: floors 1..=120 must cover every archetype on its quarter turns, missing {:?}",
            wanted
                .iter()
                .filter(|slot| !matrix.iter().any(|(s, _)| s == *slot))
                .collect::<Vec<_>>()
        );
        matrix.into_iter().map(|(_, floor)| floor).collect()
    }

    #[test]
    fn every_archetype_walks_on_every_quarter_turn() {
        init_vanilla_registry();
        for floor in archetype_quarter_matrix(7) {
            let (a, b) = climb_phases(7, floor);
            assert_floor_walked(7, floor, &a);
            if let Some(b) = b {
                assert_floor_walked(7, floor + 1, &b);
            }
        }
    }

    #[test]
    fn archetypes_rotate_without_immediate_repeats() {
        for seed in [7_u64, 42, 1_337] {
            let mut seen = Vec::new();
            let mut previous = None;
            for floor in 1_u32..=40 {
                let arch = Archetype::of(seed, floor);
                if floor % 10 == 0 {
                    assert_eq!(
                        arch,
                        Archetype::Throne,
                        "floor {floor} must be the throne hall"
                    );
                }
                assert_ne!(
                    Some(arch),
                    previous,
                    "seed {seed} floor {floor}: the archetype repeats the previous floor"
                );
                if !seen.contains(&arch) {
                    seen.push(arch);
                }
                previous = Some(arch);
            }
            assert_eq!(seen.len(), 4, "seed {seed}: every archetype must appear");
        }
    }

    #[test]
    fn the_stairwell_pierces_the_slab_exactly() {
        init_vanilla_registry();
        for floor in 1_u32..=8 {
            let q = quarter(floor);
            let surface = floor_surface_y(floor);
            let facing = stair_facing(q);
            for i in 0..=CLEARANCE {
                for z in STRIP_Z_MIN..=STRIP_Z_MAX {
                    let (dx, dz) = rot(FLIGHT_X_MIN + i, z, q);
                    assert_eq!(
                        floor_block(9, floor, dx, dz, surface + i),
                        Some(
                            east_stairs()
                                .set_value(&BlockStateProperties::HORIZONTAL_FACING, facing)
                        ),
                        "floor {floor}: stair {i} at ({z}) does not face the climb"
                    );
                    // The flight is bare: nothing under the stair.
                    for below in 0..i {
                        assert!(
                            floor_block(9, floor, dx, dz, surface + below).is_none(),
                            "floor {floor}: support under stair {i} at ({z})"
                        );
                    }
                    // Two blocks of air over a climber's head.
                    for above in [i + 1, i + 2] {
                        assert!(
                            floor_block(9, floor, dx, dz, surface + above).is_none(),
                            "floor {floor}: stair {i} at ({z}) lacks headroom at {above}"
                        );
                    }
                }
            }
            // The slab closes over the approach and opens only into the
            // stairwell: the two headroom columns of the topmost stairs.
            for z in STRIP_Z_MIN..=STRIP_Z_MAX {
                for x in STRIP_X_MIN..=STRIP_X_MAX {
                    let (dx, dz) = rot(x, z, q);
                    let block = floor_block(9, floor, dx, dz, surface + CLEARANCE);
                    if x < HOLE_X_MIN {
                        assert_eq!(
                            block,
                            Some(vanilla_blocks::POLISHED_BLACKSTONE_BRICKS.default_state()),
                            "floor {floor}: the slab over the approach at ({x}, {z}) is open"
                        );
                    } else if x == FLIGHT_X_MAX {
                        // The top stair fills its own column.
                        assert!(
                            block.is_some(),
                            "floor {floor}: the top stair at ({x}, {z}) is missing"
                        );
                    } else if x >= LANDING_X_MIN {
                        assert_eq!(
                            block,
                            Some(vanilla_blocks::BLACKSTONE.default_state()),
                            "floor {floor}: the landing at ({x}, {z}) is missing"
                        );
                    } else {
                        assert!(
                            block.is_none(),
                            "floor {floor}: the stairwell at ({x}, {z}) is covered"
                        );
                    }
                }
            }
        }
        // The arrival lands on the floor below's landing with two blocks of
        // air over it, and the whole stairwell stays open into the floor above.
        for floor in 2_u32..=8 {
            let surface = floor_surface_y(floor);
            let (ex, ez) = entry_cell(floor);
            assert!(
                floor_block(7, floor, ex, ez, surface - 1).is_none(),
                "floor {floor}: the foundation buries the arrival"
            );
            assert_eq!(
                world_block(7, floor, ex, ez, surface - 1),
                Some(vanilla_blocks::BLACKSTONE.default_state()),
                "floor {floor}: the arrival does not stand on the landing below"
            );
            for gx in HOLE_X_MIN..=STRIP_X_MAX {
                for gz in STRIP_Z_MIN..=STRIP_Z_MAX {
                    let (dx, dz) = rot(gx, gz, quarter(floor - 1));
                    for layer in [0, 1] {
                        assert!(
                            world_block(7, floor, dx, dz, surface + layer).is_none(),
                            "floor {floor}: the shaft is walled at ({gx}, {gz}, {layer})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn lights_are_flush_with_the_floor() {
        init_vanilla_registry();
        for seed in [7_u64, 42, 1_337] {
            for floor in 1_u32..=12 {
                let surface = floor_surface_y(floor);
                let arch = Archetype::of(seed, floor);
                let mut quadrants = [0_u32; 4];
                for dx in -TOWER_HALF..=TOWER_HALF {
                    for dz in -TOWER_HALF..=TOWER_HALF {
                        for layer in -1..CLEARANCE {
                            let Some(block) = floor_block(seed, floor, dx, dz, surface + layer)
                            else {
                                continue;
                            };
                            if block.get_light_emission() > 0 {
                                let index = match (dx >= 0, dz >= 0) {
                                    (true, true) => 0,
                                    (false, true) => 1,
                                    (false, false) => 2,
                                    (true, false) => 3,
                                };
                                quadrants[index] += 1;
                            }
                            if layer == 0 && block.get_light_emission() > 0 {
                                // The only walking-height light is the caged
                                // lava moat; every other source is flush in
                                // the foundation or hangs at height.
                                let (x, z) = canonical(floor, dx, dz);
                                let ring = x.abs().max(z.abs());
                                assert_eq!(
                                    block,
                                    vanilla_blocks::LAVA.default_state(),
                                    "seed {seed} floor {floor} ({arch:?}): a light source sits on the floor at ({dx}, {dz})"
                                );
                                assert!(
                                    (LAVA_INNER..=LAVA_OUTER).contains(&ring),
                                    "seed {seed} floor {floor}: lava escaped the moat at ({dx}, {dz})"
                                );
                            }
                        }
                    }
                }
                assert!(
                    quadrants.iter().all(|&count| count >= 3),
                    "seed {seed} floor {floor} ({arch:?}): a quadrant has too few light sources: {quadrants:?}"
                );
                // The inlay ring lights the foundation; the surface above it
                // stays open (a throne carpet may dress it, nothing more).
                for d in -INLAY_RADIUS..=INLAY_RADIUS {
                    for (x, z) in [
                        (d, -INLAY_RADIUS),
                        (d, INLAY_RADIUS),
                        (-INLAY_RADIUS, d),
                        (INLAY_RADIUS, d),
                    ] {
                        let (dx, dz) = rot(x, z, quarter(floor));
                        assert_eq!(
                            floor_block(seed, floor, dx, dz, surface - 1),
                            Some(vanilla_blocks::SHROOMLIGHT.default_state()),
                            "seed {seed} floor {floor}: the inlay at ({x}, {z}) is not flush"
                        );
                        let above = floor_block(seed, floor, dx, dz, surface);
                        assert!(
                            above.is_none()
                                || above == Some(vanilla_blocks::RED_CARPET.default_state()),
                            "seed {seed} floor {floor}: the inlay at ({x}, {z}) raises the floor"
                        );
                    }
                }
                // Lit avenues mark the way in and the way up, flush as well.
                for (x, z) in [(-20, 0), (-15, -20), (-20, -26)] {
                    let (dx, dz) = rot(x, z, quarter(floor));
                    assert_eq!(
                        floor_block(seed, floor, dx, dz, surface - 1),
                        Some(vanilla_blocks::GLOWSTONE.default_state()),
                        "seed {seed} floor {floor}: the avenue marker at ({x}, {z}) is not flush"
                    );
                }
            }
        }
    }

    #[test]
    fn no_stair_blocks_outside_the_climb_route() {
        init_vanilla_registry();
        for seed in [7_u64, 42, 1_337] {
            for floor in 1_u32..=6 {
                let surface = floor_surface_y(floor);
                for dx in -WALL_OUTER..=WALL_OUTER {
                    for dz in -WALL_OUTER..=WALL_OUTER {
                        for layer in -1..=CLEARANCE {
                            let Some(block) = floor_block(seed, floor, dx, dz, surface + layer)
                            else {
                                continue;
                            };
                            if block
                                .try_get_value(&BlockStateProperties::STAIRS_SHAPE)
                                .is_none()
                            {
                                continue;
                            }
                            let (x, z) = canonical(floor, dx, dz);
                            assert!(
                                (STRIP_Z_MIN..=STRIP_Z_MAX).contains(&z)
                                    && stair_layer(x) == Some(layer),
                                "seed {seed} floor {floor}: stair block outside the flight at ({dx}, {dz}, {layer})"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_exit_portal_stays_clear_on_every_archetype() {
        init_vanilla_registry();
        // The portal stands at a fixed world position on every floor, so
        // floors 1..=4 walk the exit cell across all four quarter turns.
        let mut covered = Vec::new();
        for seed in [7_u64, 42, 1_337] {
            for floor in 1_u32..=10 {
                let arch = Archetype::of(seed, floor);
                let surface = floor_surface_y(floor);
                for y in surface..=surface + 4 {
                    for x in -1..=2 {
                        assert!(
                            floor_block(seed, floor, x, 6, y).is_none(),
                            "seed {seed} floor {floor} ({arch:?}): exit cell {x} at layer {} is blocked",
                            y - surface
                        );
                    }
                }
                if !covered.contains(&arch) {
                    covered.push(arch);
                }
            }
        }
        assert_eq!(covered.len(), 4, "the sweep must cover every archetype");
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
        // The default overworld build height admits a long climb, and the
        // highest admitted floor stays fully inside the world.
        let max = max_floors(320);
        assert!(
            max >= 15,
            "the climb should reach at least 15 floors, got {max}"
        );
        let top = floor_surface_y(max) + CLEARANCE;
        assert!(top <= 320 && top + FLOOR_STEP > 320);
    }

    #[test]
    fn spawn_spots_stay_in_the_open_arena() {
        init_vanilla_registry();
        for seed in [7_u64, 42, 1_337] {
            for floor in 1_u32..=10 {
                let surface = floor_surface_y(floor);
                for spot in spawn_spots(seed, floor) {
                    assert!(
                        spot.0.abs() <= 14 && spot.1.abs() <= 14,
                        "seed {seed} floor {floor}: spot {spot:?} left the arena"
                    );
                    assert!(
                        !((-1..=2).contains(&spot.0) && spot.1 == 6),
                        "seed {seed} floor {floor}: spot {spot:?} stands in the exit portal"
                    );
                    assert!(
                        floor_block(seed, floor, spot.0, spot.1, surface).is_none(),
                        "seed {seed} floor {floor}: spot {spot:?} is blocked"
                    );
                }
            }
        }
    }
}
