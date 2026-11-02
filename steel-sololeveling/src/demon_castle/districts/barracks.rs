//! The barracks: a warren of bunk rooms walled off in a grid, each with
//! two-tier bunks, a candlelit table and weapon racks, opening onto every
//! neighbour through doorways in the middle of its walls.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::vanilla_blocks;

use super::{Ctx, Fill, bottom_slab, hanging_lantern, in_aisle, runner};
use crate::demon_castle::DISTRICT_SIZE;

/// First room wall line; the strip between it and the outer wall stays open.
const FIRST_LINE: i32 = 6;
/// Distance between two room wall lines.
const ROOM: i32 = 12;
/// Layers of a room wall; the cap sits one layer higher.
const WALL_TOP: i32 = 5;

/// Returns the position of `c` inside its room, 0 on a wall line, or `None`
/// in the open strip along the outer walls.
const fn room_coord(c: i32) -> Option<i32> {
    if c < FIRST_LINE || c >= DISTRICT_SIZE - 2 {
        return None;
    }
    Some((c - FIRST_LINE) % ROOM)
}

pub(super) fn fill(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Fill {
    let palette = ctx.palette;
    if layer == 0 {
        return Fill::Default;
    }
    if in_aisle(ctx.district, u, v) {
        if layer == 1 && runner(ctx.district, u, v) {
            return Fill::of(palette.carpet);
        }
        return Fill::Default;
    }
    let (Some(ru), Some(rv)) = (room_coord(u), room_coord(v)) else {
        return Fill::Default;
    };
    let doorway = |c: i32| (5..=7).contains(&c);
    match (ru == 0, rv == 0) {
        (true, true) => {
            return match layer {
                1..=WALL_TOP => Fill::of(palette.pillar),
                _ if layer == WALL_TOP + 1 => Fill::of(palette.capital),
                _ => Fill::Default,
            };
        }
        (true, false) | (false, true) => {
            let along = if ru == 0 { rv } else { ru };
            if doorway(along) && layer <= 3 {
                return Fill::Air;
            }
            return match layer {
                1..=WALL_TOP => Fill::of(palette.wall),
                _ if layer == WALL_TOP + 1 => Fill::Block(bottom_slab(palette.slab)),
                _ => Fill::Default,
            };
        }
        (false, false) => {}
    }
    furniture(ctx, ru, rv, layer)
}

/// Bunks along one wall, a table with candles in the middle, racks along the
/// opposite wall and a lantern overhead.
fn furniture(ctx: &Ctx<'_>, ru: i32, rv: i32, layer: i32) -> Fill {
    let palette = ctx.palette;
    let bunk = (2..=3).contains(&ru) && ((2..=4).contains(&rv) || (8..=10).contains(&rv));
    if bunk {
        return match layer {
            1 | 3 => Fill::Block(bottom_slab(palette.slab)),
            2 if ru == 2 && (rv == 2 || rv == 10) => Fill::of(palette.rail),
            _ => Fill::Default,
        };
    }
    if (6..=7).contains(&ru) && (6..=7).contains(&rv) {
        return match layer {
            1 => Fill::of(palette.trim),
            2 if ru == 6 && rv == 6 => Fill::Block(
                vanilla_blocks::CANDLE
                    .default_state()
                    .set_value(&BlockStateProperties::CANDLES, 3)
                    .set_value(&BlockStateProperties::LIT, true),
            ),
            _ => Fill::Default,
        };
    }
    if ru == ROOM - 1 && matches!(rv, 2 | 3 | 9 | 10) && (1..=2).contains(&layer) {
        return Fill::of(palette.rail);
    }
    if ru == 6
        && rv == 9
        && let Some(fill) = hanging_lantern(ctx, 6, layer)
    {
        return fill;
    }
    Fill::Default
}
