//! Block functions of the castle districts.
//!
//! Every district is an 80×80 cell: two-block walls on each side, so two
//! neighbours share a four-block wall, a ceiling slab over its own height and
//! a foundation under the floor. Gates are eight blocks wide and nine tall in
//! the middle of a side. The shell here builds walls, gates, ceiling and the
//! default floor; each archetype fills the interior.
//!
//! District-local coordinates `(u, v)` run `0..80` towards +X and +Z; layer 0
//! is the floor block, hunters stand at layer 1. Inside every district the
//! aisles from each gate to the central plaza stay clear at walking height,
//! which keeps every gate reachable whatever the archetype puts around them.

pub mod arrival;
pub mod barracks;
pub mod bridge;
pub mod courtyard;
pub mod giga_throne;
pub mod great_hall;
pub mod prison;
pub mod throne;

use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction, SlabType};
use steel_registry::vanilla_blocks;
use steel_utils::BlockStateId;

use super::DISTRICT_SIZE;
use super::plan::{Archetype, District, Side, TREASURE_DEPTH};
use super::theme::CastlePalette;
use crate::seeded::hash;

/// Thickness of a district wall.
pub const WALL: i32 = 2;
/// First column of a gate opening along its side.
pub const DOOR_LO: i32 = 36;
/// Last column of a gate opening along its side.
pub const DOOR_HI: i32 = 43;
/// Air layers of a gate opening.
pub const DOOR_HEIGHT: i32 = 9;
/// Layers of foundation under the floor.
const FOUNDATION: i32 = 4;
/// Depth of the lava chasm under a bridge district, and so of every wall.
pub const CHASM_DEPTH: i32 = 14;
/// Lowest layer any district builds.
pub const MIN_LAYER: i32 = -CHASM_DEPTH;
/// Highest layer any district builds: the tallest ceiling.
pub const MAX_LAYER: i32 = 36;
/// Last interior column; the first is [`WALL`].
const INNER_MAX: i32 = DISTRICT_SIZE - WALL - 1;
/// The central plaza every aisle leads to.
pub const PLAZA_LO: i32 = 32;
/// Last column of the central plaza.
pub const PLAZA_HI: i32 = 47;

/// What a district is built from at one column.
pub struct Ctx<'a> {
    /// The district.
    pub district: District,
    /// Blocks of the floor's theme.
    pub palette: &'a CastlePalette,
    /// Air layers between floor and ceiling.
    pub height: i32,
    /// Theme cycles below the floor; later cycles weather their walls more.
    pub cycle: u32,
}

/// What an archetype puts into one interior cell.
pub enum Fill {
    /// A block.
    Block(BlockStateId),
    /// Air.
    Air,
    /// Whatever the shell builds there by default.
    Default,
}

impl Fill {
    /// Fills with the default state of `block`.
    #[must_use]
    pub fn of(block: BlockRef) -> Self {
        Self::Block(block.default_state())
    }
}

/// Returns the block of district-local `(u, v)` at `layer`, or `None` for air.
#[must_use]
pub fn block(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Option<BlockStateId> {
    let height = ctx.height;
    if !(MIN_LAYER..=height + 2).contains(&layer) {
        return None;
    }
    if layer > height {
        return Some(ceiling_block(ctx, u, v, layer));
    }
    if is_wall(u, v) {
        return wall_block(ctx, u, v, layer);
    }
    if let Some(fill) = treasure(ctx, u, v, layer) {
        return match fill {
            Fill::Block(block) => Some(block),
            Fill::Air => None,
            Fill::Default => default_interior(ctx, u, v, layer),
        };
    }
    let fill = match ctx.district.archetype {
        Archetype::Arrival => arrival::fill(ctx, u, v, layer),
        Archetype::Courtyard => courtyard::fill(ctx, u, v, layer),
        Archetype::GreatHall => great_hall::fill(ctx, u, v, layer),
        Archetype::Bridge => bridge::fill(ctx, u, v, layer),
        Archetype::Barracks => barracks::fill(ctx, u, v, layer),
        Archetype::Prison => prison::fill(ctx, u, v, layer),
        Archetype::Throne => throne::fill(ctx, u, v, layer),
        Archetype::GigaThrone => giga_throne::fill(ctx, u, v, layer),
    };
    match fill {
        Fill::Block(block) => Some(block),
        Fill::Air => None,
        Fill::Default => default_interior(ctx, u, v, layer),
    }
}

/// Returns whether `(u, v)` is part of the district's walls.
#[must_use]
pub const fn is_wall(u: i32, v: i32) -> bool {
    u < WALL || v < WALL || u > INNER_MAX || v > INNER_MAX
}

/// Returns the wall side of a wall column, its depth into the wall (0 at
/// the outer face) and its position along the side; corners have no side.
const fn wall_side(u: i32, v: i32) -> Option<(Side, i32, i32)> {
    let north = v < WALL;
    let south = v > INNER_MAX;
    let west = u < WALL;
    let east = u > INNER_MAX;
    match (north, south, west, east) {
        (true, false, false, false) => Some((Side::North, v, u)),
        (false, true, false, false) => Some((Side::South, DISTRICT_SIZE - 1 - v, u)),
        (false, false, true, false) => Some((Side::West, u, v)),
        (false, false, false, true) => Some((Side::East, DISTRICT_SIZE - 1 - u, v)),
        _ => None,
    }
}

fn wall_block(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Option<BlockStateId> {
    let palette = ctx.palette;
    if let Some((side, depth, along)) = wall_side(u, v) {
        let door = ctx.district.has_door(side);
        if door && (DOOR_LO..=DOOR_HI).contains(&along) {
            if (1..=DOOR_HEIGHT).contains(&layer) {
                return None;
            }
            if layer == 0 {
                return Some(palette.floor_trim.default_state());
            }
            if layer == DOOR_HEIGHT + 1 {
                return Some(palette.trim.default_state());
            }
        }
        if door
            && (along == DOOR_LO - 1 || along == DOOR_HI + 1)
            && (0..=DOOR_HEIGHT + 1).contains(&layer)
        {
            return Some(palette.trim.default_state());
        }
        let near_door = door && (DOOR_LO - 4..=DOOR_HI + 4).contains(&along);
        let lit_row = layer == 5 || (ctx.height >= 20 && layer == 16);
        if depth == WALL - 1 && lit_row && along % 8 == 4 && !near_door {
            return Some(palette.wall_light.default_state());
        }
        if depth == WALL - 1 && layer == 1 {
            return Some(palette.floor_trim.default_state());
        }
    }
    Some(weathered(ctx, palette.wall, palette.wall_alt, u, layer, v))
}

fn ceiling_block(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> BlockStateId {
    let palette = ctx.palette;
    if layer == ctx.height + 1
        && !is_wall(u, v)
        && (u - 3).rem_euclid(8) == 0
        && (v - 3).rem_euclid(8) == 0
    {
        return palette.ceiling_light.default_state();
    }
    palette.ceiling.default_state()
}

fn default_interior(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Option<BlockStateId> {
    match layer {
        _ if layer < -FOUNDATION => None,
        _ if layer < 0 => Some(ctx.palette.foundation.default_state()),
        0 => Some(default_floor(ctx, u, v)),
        _ => None,
    }
}

/// Returns the default floor pattern: a trimmed border, a trim grid every
/// ten blocks and flush lights between the lines.
#[must_use]
pub fn default_floor(ctx: &Ctx<'_>, u: i32, v: i32) -> BlockStateId {
    let palette = ctx.palette;
    if u == WALL || v == WALL || u == INNER_MAX || v == INNER_MAX {
        return palette.floor_trim.default_state();
    }
    if u % 10 == 5 && v % 10 == 5 && !in_aisle(ctx.district, u, v) {
        return palette.floor_light.default_state();
    }
    if u % 10 == 0 || v % 10 == 0 {
        return palette.floor_trim.default_state();
    }
    weathered(ctx, palette.floor, palette.floor_alt, u, 0, v)
}

/// Mixes `alt` into `main` by position, more often on later theme cycles.
fn weathered(ctx: &Ctx<'_>, main: BlockRef, alt: BlockRef, x: i32, y: i32, z: i32) -> BlockStateId {
    let every = 7_u64.saturating_sub(u64::from(ctx.cycle.min(4))).max(3);
    if hash(ctx.district.salt, x, y, z).is_multiple_of(every) {
        alt.default_state()
    } else {
        main.default_state()
    }
}

/// Returns whether `(u, v)` lies on an aisle from a gate to the plaza or on
/// the plaza itself: cells no decoration may block at walking height.
#[must_use]
pub fn in_aisle(district: District, u: i32, v: i32) -> bool {
    if (PLAZA_LO..=PLAZA_HI).contains(&u) && (PLAZA_LO..=PLAZA_HI).contains(&v) {
        return true;
    }
    let band = DOOR_LO..=DOOR_HI;
    district.door_sides().any(|side| match side {
        Side::North => band.contains(&u) && v <= PLAZA_HI,
        Side::South => band.contains(&u) && v >= PLAZA_LO,
        Side::West => band.contains(&v) && u <= PLAZA_HI,
        Side::East => band.contains(&v) && u >= PLAZA_LO,
    })
}

/// The treasure cache: a barrel between two braziers against the far wall.
fn treasure(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Option<Fill> {
    let (tu, tv) = ctx.district.treasure_spot()?;
    let palette = ctx.palette;
    if (u, v) == (tu, tv) {
        return match layer {
            0 => Some(Fill::of(palette.floor_trim)),
            1 => Some(Fill::Block(
                vanilla_blocks::BARREL
                    .default_state()
                    .set_value(&BlockStateProperties::FACING, Direction::Up),
            )),
            _ => Some(Fill::Default),
        };
    }
    // The braziers flank the cache along its wall.
    let along_u = tv == TREASURE_DEPTH || tv == DISTRICT_SIZE - 1 - TREASURE_DEPTH;
    let brazier = if along_u {
        v == tv && (u - tu).abs() == 2
    } else {
        u == tu && (v - tv).abs() == 2
    };
    if !brazier {
        return None;
    }
    Some(match layer {
        1 => Fill::of(palette.trim),
        2 => Fill::of(palette.capital),
        _ => Fill::Default,
    })
}

/// Returns whether `(u, v)` lies on the carpet runner down the middle of an
/// aisle.
#[must_use]
pub fn runner(district: District, u: i32, v: i32) -> bool {
    let middle = DOOR_LO + 2..=DOOR_HI - 2;
    district.door_sides().any(|side| match side {
        Side::North | Side::South => middle.contains(&u),
        Side::West | Side::East => middle.contains(&v),
    })
}

/// Returns the Chebyshev distance from the district centre, doubled so it
/// stays an integer: 1 for the four centre cells, 79 at the outer face.
#[must_use]
pub const fn ring(u: i32, v: i32) -> i32 {
    let du = (2 * u - (DISTRICT_SIZE - 1)).abs();
    let dv = (2 * v - (DISTRICT_SIZE - 1)).abs();
    if du > dv { du } else { dv }
}

/// Returns a bottom half slab of `slab`.
#[must_use]
pub fn bottom_slab(slab: BlockRef) -> BlockStateId {
    slab.default_state()
        .set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Bottom)
}

/// Returns `stairs` climbing towards `facing`.
#[must_use]
pub fn stairs(stairs: BlockRef, facing: Direction) -> BlockStateId {
    stairs
        .default_state()
        .set_value(&BlockStateProperties::HORIZONTAL_FACING, facing)
}

/// Returns a lantern hanging from the block above.
#[must_use]
pub fn hanging(lantern: BlockRef) -> BlockStateId {
    lantern
        .default_state()
        .set_value(&BlockStateProperties::HANGING, true)
}

/// Returns a vertical iron chain.
#[must_use]
pub fn chain() -> BlockStateId {
    vanilla_blocks::IRON_CHAIN.default_state()
}

/// Returns the fill of `layer` in a column holding a chain of `length`
/// blocks under the ceiling with a lantern under it, or `None` for the layers
/// the chain leaves to the rest of the district.
#[must_use]
pub fn hanging_lantern(ctx: &Ctx<'_>, length: i32, layer: i32) -> Option<Fill> {
    let top = ctx.height;
    if (top - length + 1..=top).contains(&layer) {
        return Some(Fill::Block(chain()));
    }
    (layer == top - length).then(|| Fill::Block(hanging(ctx.palette.lantern)))
}

/// A square column of `half` blocks around `(cu, cv)` from the floor to the
/// ceiling with a trim base and a glowing capital.
#[must_use]
pub fn column(
    ctx: &Ctx<'_>,
    u: i32,
    v: i32,
    layer: i32,
    (cu, cv): (i32, i32),
    half: i32,
) -> Option<Fill> {
    if (u - cu).abs() > half || (v - cv).abs() > half || !(1..=ctx.height).contains(&layer) {
        return None;
    }
    let palette = ctx.palette;
    Some(match layer {
        1 => Fill::of(palette.trim),
        _ if layer == ctx.height => Fill::of(palette.capital),
        _ => Fill::of(palette.pillar),
    })
}

#[cfg(test)]
mod tests;
