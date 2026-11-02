//! The courtyard: a checkered court around a lava fountain with an obelisk,
//! ringed by a colonnade, with demon statues and planters in the corners.

use steel_registry::vanilla_blocks;

use super::{Ctx, Fill, column, in_aisle, ring};
use crate::demon_castle::DISTRICT_SIZE;
use crate::seeded::hash;

/// Ring of the fountain rim; the lava pool lies inside it.
const FOUNTAIN_RIM: i32 = 9;
/// Ring of the obelisk rising from the pool.
const OBELISK: i32 = 1;
/// Top layer of the obelisk shaft.
const OBELISK_TOP: i32 = 7;
/// Ring of the colonnade.
const COLONNADE: i32 = 51;
/// Ring of the flush lights around the plaza.
const PLAZA_LIGHTS: i32 = 17;
/// Corners of the four statues.
const STATUES: [(i32, i32); 4] = [(22, 22), (22, 56), (56, 22), (56, 56)];
/// Inner corner of the planter in the north-west quadrant; the others mirror it.
const PLANTER_LO: i32 = 6;
const PLANTER_HI: i32 = 13;

pub(super) fn fill(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Fill {
    let palette = ctx.palette;
    let d = ring(u, v);
    if d <= FOUNTAIN_RIM {
        return fountain(ctx, d, layer);
    }
    if layer == 0 {
        if d == PLAZA_LIGHTS && (u + v) % 2 == 0 {
            return Fill::of(palette.floor_light);
        }
        if !in_aisle(ctx.district, u, v) && (u / 4 + v / 4) % 2 == 0 {
            return Fill::of(palette.floor_alt);
        }
        return Fill::Default;
    }
    if d == COLONNADE
        && !in_aisle(ctx.district, u, v)
        && (u + v) % 6 == 0
        && let Some(fill) = column(ctx, u, v, layer, (u, v), 0)
    {
        return fill;
    }
    for corner in STATUES {
        if let Some(fill) = statue(ctx, u, v, layer, corner) {
            return fill;
        }
    }
    if let Some(fill) = planter(ctx, u, v, layer) {
        return fill;
    }
    Fill::Default
}

fn fountain(ctx: &Ctx<'_>, d: i32, layer: i32) -> Fill {
    let palette = ctx.palette;
    if d <= OBELISK {
        return match layer {
            0..OBELISK_TOP => Fill::of(palette.trim),
            OBELISK_TOP => Fill::of(palette.capital),
            _ => Fill::Default,
        };
    }
    if d == FOUNTAIN_RIM {
        return match layer {
            0 => Fill::of(palette.floor_trim),
            1 => Fill::of(palette.trim),
            _ => Fill::Default,
        };
    }
    match layer {
        0 => Fill::of(&vanilla_blocks::LAVA),
        _ => Fill::Default,
    }
}

/// A 2×2 statue on a two-block pedestal at `corner`.
fn statue(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32, corner: (i32, i32)) -> Option<Fill> {
    if !(corner.0..=corner.0 + 1).contains(&u) || !(corner.1..=corner.1 + 1).contains(&v) {
        return None;
    }
    let palette = ctx.palette;
    match layer {
        1 | 2 => Some(Fill::of(palette.trim)),
        3..=6 => Some(Fill::of(palette.statue)),
        7 => Some(Fill::of(palette.capital)),
        _ => None,
    }
}

/// Raised planters in the four corners of the court.
fn planter(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Option<Fill> {
    let mirror = |c: i32| {
        if c >= DISTRICT_SIZE / 2 {
            DISTRICT_SIZE - 1 - c
        } else {
            c
        }
    };
    let (mu, mv) = (mirror(u), mirror(v));
    let inside = |c: i32| (PLANTER_LO..=PLANTER_HI).contains(&c);
    if !inside(mu) || !inside(mv) {
        return None;
    }
    let palette = ctx.palette;
    let rim = mu == PLANTER_LO || mu == PLANTER_HI || mv == PLANTER_LO || mv == PLANTER_HI;
    match layer {
        1 if rim => Some(Fill::of(palette.trim)),
        1 => Some(Fill::of(palette.soil)),
        2 if !rim => palette
            .plant
            .filter(|_| hash(ctx.district.salt, u, 2, v).is_multiple_of(3))
            .map(Fill::of),
        _ => None,
    }
}
