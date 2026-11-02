//! The prison: rows of barred cages along narrow corridors, chains hanging
//! from the vault and a slab cot in every cell. Every cage door stands open,
//! and whatever was locked inside walks free when the hunters come near.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::vanilla_blocks;
use steel_utils::BlockStateId;

use super::{Ctx, Fill, bottom_slab, chain, hanging_lantern, in_aisle};
use crate::demon_castle::DISTRICT_SIZE;
use crate::demon_castle::plan::Side;

/// First column of the first cage row; the strip along the walls stays open.
const FIRST_CAGE: i32 = 7;
/// Distance between two cage rows: the cage and a corridor.
const PERIOD: i32 = 10;
/// Side of a cage, bars included.
const CAGE: i32 = 7;
/// Layers of bars; the cage roof sits on top.
const BARS_TOP: i32 = 3;

/// Returns the position of `c` inside its cage, or `None` in a corridor.
const fn cage_coord(c: i32) -> Option<i32> {
    if c < FIRST_CAGE || c > DISTRICT_SIZE - FIRST_CAGE - 1 {
        return None;
    }
    let local = (c - FIRST_CAGE) % PERIOD;
    if local < CAGE { Some(local) } else { None }
}

/// Returns iron bars joined towards `sides`, so a run of bars is a wall
/// rather than a row of posts a hunter slips between.
#[must_use]
pub fn bars(sides: &[Side]) -> BlockStateId {
    vanilla_blocks::IRON_BARS
        .default_state()
        .set_value(&BlockStateProperties::NORTH, sides.contains(&Side::North))
        .set_value(&BlockStateProperties::EAST, sides.contains(&Side::East))
        .set_value(&BlockStateProperties::SOUTH, sides.contains(&Side::South))
        .set_value(&BlockStateProperties::WEST, sides.contains(&Side::West))
}

/// Returns whether the cage around `(u, v)` touches an aisle; such cages are
/// left out so the aisles stay clear.
fn cage_in_aisle(ctx: &Ctx<'_>, u: i32, v: i32, cu: i32, cv: i32) -> bool {
    let (u0, v0) = (u - cu, v - cv);
    [(0, 0), (CAGE - 1, 0), (0, CAGE - 1), (CAGE - 1, CAGE - 1)]
        .into_iter()
        .any(|(du, dv)| in_aisle(ctx.district, u0 + du, v0 + dv))
}

pub(super) fn fill(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Fill {
    let palette = ctx.palette;
    if layer == 0 {
        return Fill::Default;
    }
    let (Some(cu), Some(cv)) = (cage_coord(u), cage_coord(v)) else {
        return corridor(ctx, u, v, layer);
    };
    if cage_in_aisle(ctx, u, v, cu, cv) {
        return Fill::Default;
    }
    let last = CAGE - 1;
    let rim = cu == 0 || cv == 0 || cu == last || cv == last;
    if layer == BARS_TOP + 1 {
        return Fill::of(palette.trim);
    }
    if layer > BARS_TOP + 1 {
        return Fill::Default;
    }
    if rim {
        // The door: two open columns in the south side.
        if cv == last && (3..=4).contains(&cu) {
            return Fill::Air;
        }
        // Sides join along the side, rows along the row; the bars beside the
        // door stop short of the opening.
        let side = cu == 0 || cu == last;
        let row = cv == 0 || cv == last;
        let north = side && cv > 0;
        let south = side && cv < last;
        let west = row && cu > 0 && !(cv == last && cu == 5);
        let east = row && cu < last && !(cv == last && cu == 2);
        let joined: Vec<Side> = [
            (north, Side::North),
            (east, Side::East),
            (south, Side::South),
            (west, Side::West),
        ]
        .into_iter()
        .filter_map(|(join, side)| join.then_some(side))
        .collect();
        return Fill::Block(bars(&joined));
    }
    if cu == 3 && cv == 3 && (2..=BARS_TOP).contains(&layer) {
        return Fill::Block(chain());
    }
    if cu == 1 && (1..=2).contains(&cv) && layer == 1 {
        return Fill::Block(bottom_slab(palette.slab));
    }
    Fill::Default
}

/// Chains and lanterns hanging over the corridors between the cages.
fn corridor(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Fill {
    let between = |c: i32| c >= FIRST_CAGE && (c - FIRST_CAGE) % PERIOD == CAGE + 1;
    if between(u)
        && between(v)
        && !in_aisle(ctx.district, u, v)
        && let Some(fill) = hanging_lantern(ctx, 9, layer)
    {
        return fill;
    }
    Fill::Default
}
