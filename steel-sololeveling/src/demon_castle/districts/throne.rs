//! The throne room: a carpeted aisle between columns up to a two-tier dais
//! with the throne, braziers and a great chandelier. The boss waits before
//! the dais; the gates up and home open beside the aisle once it falls.
//!
//! The room is laid out with its only gate on the north side and rotated to
//! whichever side the gate really faces.

use steel_registry::blocks::properties::Direction;

use super::{Ctx, Fill, bottom_slab, chain, column, stairs};
use crate::demon_castle::DISTRICT_SIZE;
use crate::demon_castle::plan::Side;

/// Canonical column the boss appears on.
pub const BOSS_SPOT: (i32, i32) = (40, 58);
/// Canonical column of the entrance block of the gate up to the next floor.
pub const EXIT_SPOT: (i32, i32) = (33, 46);
/// Canonical column of the entrance block of the gate home, across the aisle.
pub const HOME_SPOT: (i32, i32) = (45, 46);
/// Canonical columns of the aisle carpet.
pub(super) const CARPET_U: (i32, i32) = (37, 42);
/// Last canonical row of the carpet, at the foot of the dais.
pub(super) const CARPET_END: i32 = 61;
/// Column centres beside the aisle.
const COLUMNS_U: [i32; 2] = [28, 51];
const COLUMNS_V: [i32; 4] = [14, 26, 38, 50];
/// Canonical extent of the dais.
const DAIS_U: (i32, i32) = (26, 53);
const DAIS_FIRST: i32 = 62;
const DAIS_SECOND: i32 = 66;
const DAIS_END: i32 = 74;
/// Canonical row of the throne seat; its back stands one row behind.
const THRONE_V: i32 = 71;
/// Length of the chain of the great chandelier.
const CHANDELIER_CHAIN: i32 = 6;

/// Rotates real district coordinates into the canonical room whose gate
/// faces north.
#[must_use]
pub const fn to_canonical(door: Side, u: i32, v: i32) -> (i32, i32) {
    let last = DISTRICT_SIZE - 1;
    match door {
        Side::North => (u, v),
        Side::South => (last - u, last - v),
        Side::West => (last - v, u),
        Side::East => (v, last - u),
    }
}

/// Rotates canonical room coordinates back into the real district.
#[must_use]
pub const fn from_canonical(door: Side, cu: i32, cv: i32) -> (i32, i32) {
    let last = DISTRICT_SIZE - 1;
    match door {
        Side::North => (cu, cv),
        Side::South => (last - cu, last - cv),
        Side::West => (cv, last - cu),
        Side::East => (last - cv, cu),
    }
}

/// Rotates a canonical facing into the real district.
pub(super) const fn real_facing(door: Side, canonical: Direction) -> Direction {
    let (du, dv) = match canonical {
        Direction::North => (0, -1),
        Direction::South => (0, 1),
        Direction::East => (1, 0),
        _ => (-1, 0),
    };
    let (ru, rv) = match door {
        Side::North => (du, dv),
        Side::South => (-du, -dv),
        Side::West => (dv, -du),
        Side::East => (-dv, du),
    };
    match (ru, rv) {
        (0, -1) => Direction::North,
        (0, 1) => Direction::South,
        (1, 0) => Direction::East,
        _ => Direction::West,
    }
}

pub(super) fn fill(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Fill {
    let door = ctx.district.single_door().unwrap_or(Side::North);
    let (cu, cv) = to_canonical(door, u, v);
    let palette = ctx.palette;
    if layer == 0 {
        if (cu - 39).abs() + (cv - 40).abs() == 12 {
            return Fill::of(palette.floor_light);
        }
        return Fill::Default;
    }
    for column_u in COLUMNS_U {
        for column_v in COLUMNS_V {
            if let Some(fill) = column(ctx, cu, cv, layer, (column_u, column_v), 1) {
                if layer == 8 {
                    return Fill::of(palette.capital);
                }
                return fill;
            }
        }
    }
    if let Some(fill) = dais(ctx, door, cu, cv, layer) {
        return fill;
    }
    if let Some(fill) = chandelier(ctx, cu, cv, layer) {
        return fill;
    }
    if layer == 1 && (CARPET_U.0..=CARPET_U.1).contains(&cu) && cv <= CARPET_END {
        return Fill::of(palette.carpet);
    }
    Fill::Default
}

/// The two-tier dais with the throne and its braziers.
fn dais(ctx: &Ctx<'_>, door: Side, cu: i32, cv: i32, layer: i32) -> Option<Fill> {
    let palette = ctx.palette;
    if !(DAIS_U.0..=DAIS_U.1).contains(&cu) || !(DAIS_FIRST..=DAIS_END).contains(&cv) {
        return None;
    }
    // Half steps up each tier from the arena.
    let tier = |first: i32, at: i32| -> Option<Fill> {
        if cv == first && layer == at {
            return Some(Fill::Block(bottom_slab(palette.slab)));
        }
        (cv > first && layer == at).then(|| Fill::of(palette.trim))
    };
    if let Some(fill) = tier(DAIS_FIRST, 1).or_else(|| tier(DAIS_SECOND, 2)) {
        return Some(fill);
    }
    let corner = (cu == DAIS_U.0 + 1 || cu == DAIS_U.1 - 1) && cv == DAIS_FIRST + 1;
    if corner {
        return match layer {
            2 => Some(Fill::of(palette.trim)),
            3 => Some(Fill::of(palette.capital)),
            _ => None,
        };
    }
    let seat = (39..=40).contains(&cu);
    if seat && cv == THRONE_V && layer == 3 {
        return Some(Fill::Block(stairs(
            palette.stairs,
            real_facing(door, Direction::South),
        )));
    }
    if seat && cv == THRONE_V + 1 {
        return match layer {
            3..=6 => Some(Fill::of(palette.statue)),
            7 => Some(Fill::of(palette.capital)),
            _ => None,
        };
    }
    if (cu == 38 || cu == 41) && cv == THRONE_V && layer == 3 {
        return Some(Fill::Block(bottom_slab(palette.slab)));
    }
    None
}

/// The great chandelier over the middle of the arena.
fn chandelier(ctx: &Ctx<'_>, cu: i32, cv: i32, layer: i32) -> Option<Fill> {
    let palette = ctx.palette;
    let top = ctx.height;
    let cluster = top - CHANDELIER_CHAIN;
    let (du, dv) = (cu - 39, cv - 40);
    if (du == 0 || du == 1) && dv == 0 && (cluster + 1..=top).contains(&layer) {
        return Some(Fill::Block(chain()));
    }
    if (-2..=3).contains(&du) && (-2..=2).contains(&dv) && layer == cluster {
        let rim = du == -2 || du == 3 || dv.abs() == 2;
        return Some(if rim {
            Fill::of(palette.capital)
        } else {
            Fill::Block(bottom_slab(palette.slab))
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{from_canonical, to_canonical};
    use crate::demon_castle::plan::Side;

    #[test]
    fn canonical_rotation_round_trips_and_puts_the_gate_north() {
        for side in Side::ALL {
            for (u, v) in [(0, 0), (79, 0), (36, 1), (12, 70)] {
                let (cu, cv) = to_canonical(side, u, v);
                assert_eq!(from_canonical(side, cu, cv), (u, v), "{side:?}");
            }
            // The middle of the real gate lands on the canonical north wall.
            let gate = match side {
                Side::North => (39, 0),
                Side::South => (40, 79),
                Side::West => (0, 40),
                Side::East => (79, 39),
            };
            let (cu, cv) = to_canonical(side, gate.0, gate.1);
            assert_eq!(cv, 0, "{side:?}");
            assert!((36..=43).contains(&cu), "{side:?}");
        }
    }
}
