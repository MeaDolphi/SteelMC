//! The great hall: a tall nave of massive columns under chandeliers, with a
//! gallery ringing the walls, reached by two flights of stairs. Archers hold
//! the gallery.

use steel_registry::blocks::properties::Direction;

use super::{
    Ctx, DOOR_HI, DOOR_LO, Fill, WALL, bottom_slab, column, hanging, in_aisle, runner, stairs,
};
use crate::demon_castle::DISTRICT_SIZE;

/// Layer of the gallery deck; hunters stand on the gallery one layer up.
pub const GALLERY_LAYER: i32 = 11;
/// Last wall ring of the gallery deck, counted from the outer wall face.
const GALLERY_INNER: i32 = 5;
/// Column centres on both axes; aisles and plaza stay clear of them.
const COLUMNS: [i32; 4] = [12, 24, 55, 67];
/// Columns of the west flight; the east flight mirrors it.
const FLIGHT_U: (i32, i32) = (6, 9);
/// First stair row of the west flight; stair `i` stands one row further at
/// layer `i + 1`, climbing south.
const FLIGHT_V: i32 = 14;
/// Rows of the west landing joining the flight to the gallery.
const LANDING_V: (i32, i32) = (FLIGHT_V + GALLERY_LAYER, FLIGHT_V + GALLERY_LAYER + 1);
/// Chandelier centres.
const CHANDELIERS: [(i32, i32); 4] = [(28, 28), (28, 51), (51, 28), (51, 51)];
/// Length of a chandelier chain.
const CHANDELIER_CHAIN: i32 = 7;

/// Returns the wall ring of `(u, v)`: 0 at the outer face of the walls.
const fn wall_ring(u: i32, v: i32) -> i32 {
    let a = if u < v { u } else { v };
    let b = DISTRICT_SIZE - 1 - if u > v { u } else { v };
    if a < b { a } else { b }
}

/// Mirrors a column of the east flight onto the west one.
const fn mirrored(u: i32, v: i32) -> (i32, i32, bool) {
    if u >= DISTRICT_SIZE / 2 {
        (DISTRICT_SIZE - 1 - u, DISTRICT_SIZE - 1 - v, true)
    } else {
        (u, v, false)
    }
}

/// Returns whether `(u, v)` belongs to a landing, where the gallery railing
/// opens onto the flight.
fn on_landing(u: i32, v: i32) -> bool {
    let (mu, mv, _) = mirrored(u, v);
    (LANDING_V.0..=LANDING_V.1).contains(&mv) && (WALL..=FLIGHT_U.1).contains(&mu)
}

pub(super) fn fill(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Fill {
    let palette = ctx.palette;
    if layer == 0 {
        if in_aisle(ctx.district, u, v) && (u % 4 == 0 || v % 4 == 0) {
            return Fill::of(palette.floor_alt);
        }
        return Fill::Default;
    }
    if let Some(fill) = flight(ctx, u, v, layer) {
        return fill;
    }
    let ring = wall_ring(u, v);
    if (WALL..=GALLERY_INNER).contains(&ring) {
        if layer == GALLERY_LAYER {
            if (u - 3).rem_euclid(4) == 0 && (v - 3).rem_euclid(4) == 0 {
                return Fill::of(palette.floor_light);
            }
            return Fill::of(palette.floor_trim);
        }
        if layer == GALLERY_LAYER + 1 && ring == GALLERY_INNER && !on_landing(u, v) {
            return Fill::of(palette.rail);
        }
        // Posts carry the deck between the gates.
        let along = if ring == u || ring == DISTRICT_SIZE - 1 - u {
            v
        } else {
            u
        };
        if ring == GALLERY_INNER
            && layer < GALLERY_LAYER
            && along % 8 == 4
            && !in_aisle(ctx.district, u, v)
            && !(DOOR_LO - 2..=DOOR_HI + 2).contains(&along)
        {
            return Fill::of(palette.pillar);
        }
    }
    for cu in COLUMNS {
        for cv in COLUMNS {
            if let Some(fill) = column(ctx, u, v, layer, (cu, cv), 1) {
                if layer == GALLERY_LAYER + 1 {
                    return Fill::of(palette.capital);
                }
                return fill;
            }
        }
    }
    if let Some(fill) = chandelier(ctx, u, v, layer) {
        return fill;
    }
    if layer == 1 && in_aisle(ctx.district, u, v) && runner(ctx.district, u, v) {
        return Fill::of(palette.carpet);
    }
    Fill::Default
}

/// The stairs, their filled underside and the landing.
fn flight(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Option<Fill> {
    let palette = ctx.palette;
    let (mu, mv, east) = mirrored(u, v);
    if !(FLIGHT_U.0..=FLIGHT_U.1).contains(&mu) {
        return None;
    }
    if (LANDING_V.0..=LANDING_V.1).contains(&mv) && layer == GALLERY_LAYER {
        return Some(Fill::of(palette.floor_trim));
    }
    let step = mv - FLIGHT_V;
    if !(0..GALLERY_LAYER).contains(&step) {
        return None;
    }
    let stair = step + 1;
    if layer == stair {
        let facing = if east {
            Direction::North
        } else {
            Direction::South
        };
        return Some(Fill::Block(stairs(palette.stairs, facing)));
    }
    (layer < stair).then(|| Fill::of(palette.trim))
}

/// A chain from the ceiling holding a glowing cluster with lanterns under
/// its corners.
fn chandelier(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Option<Fill> {
    let palette = ctx.palette;
    let top = ctx.height;
    let cluster = top - CHANDELIER_CHAIN;
    for (cu, cv) in CHANDELIERS {
        let (du, dv) = (u - cu, v - cv);
        if du == 0 && dv == 0 && (cluster + 1..=top).contains(&layer) {
            return Some(Fill::Block(super::chain()));
        }
        if du.abs() <= 1 && dv.abs() <= 1 && layer == cluster {
            return Some(if du == 0 || dv == 0 {
                Fill::of(palette.capital)
            } else {
                Fill::Block(bottom_slab(palette.slab))
            });
        }
        if du.abs() == 1 && dv.abs() == 1 && layer == cluster - 1 {
            return Some(Fill::Block(hanging(palette.lantern)));
        }
    }
    None
}
