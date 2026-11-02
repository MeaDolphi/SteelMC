//! The giga throne room of every tenth floor: a round arena ringed by
//! columns, with lava pooling in its corners behind railings, glowing magma
//! plates the guardian's eruptions break through, and a towering throne on a
//! two-tier dais. Gate, boss and exit spots match the ordinary throne room.

use std::f64::consts::TAU;

use steel_registry::blocks::properties::Direction;
use steel_registry::vanilla_blocks;

use super::throne::{CARPET_END, CARPET_U, real_facing, to_canonical};
use super::{Ctx, Fill, bottom_slab, chain, hanging, stairs};
use crate::demon_castle::plan::Side;

/// Canonical centre of the arena, doubled so it stays an integer.
const CENTER2: (i32, i32) = (79, 88);
/// Radius of the column ring.
const COLUMN_RING: f64 = 28.0;
/// Columns on the ring before the entry aisle removes its share.
const COLUMNS: u32 = 12;
/// Radius beyond which the floor gives way to lava, past a railing ring.
const LAVA_RING: f64 = 35.0;
/// Canonical corners of the 2×2 magma plates.
const VENTS: [(i32, i32); 6] = [(22, 36), (56, 36), (24, 54), (54, 54), (30, 26), (48, 26)];
/// Canonical extent of the dais.
const DAIS_U: (i32, i32) = (28, 51);
const DAIS_FIRST: i32 = 64;
const DAIS_SECOND: i32 = 67;
const DAIS_END: i32 = 74;
/// Canonical row of the throne seat; its back rises one row behind.
const THRONE_V: i32 = 71;
/// Top layer of the throne back.
const THRONE_TOP: i32 = 9;
/// Canonical corners of the two guardian statues beside the throne.
const STATUES: [(i32, i32); 2] = [(31, 70), (47, 70)];
/// Length of the chain of the great chandelier.
const CHANDELIER_CHAIN: i32 = 8;

/// Returns the distance of canonical `(cu, cv)` from the arena centre.
fn distance(cu: i32, cv: i32) -> f64 {
    let dx = f64::from(2 * cu - CENTER2.0);
    let dz = f64::from(2 * cv - CENTER2.1);
    dx.hypot(dz) / 2.0
}

/// Returns whether canonical `(cu, cv)` lies on the aisle from the gate to
/// the arena centre.
const fn on_entry(cu: i32, cv: i32) -> bool {
    cu >= 36 && cu <= 43 && cv * 2 <= CENTER2.1
}

pub(super) fn fill(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Fill {
    let door = ctx.district.single_door().unwrap_or(Side::North);
    let (cu, cv) = to_canonical(door, u, v);
    let palette = ctx.palette;
    let d = distance(cu, cv);
    let entry = on_entry(cu, cv);
    // Lava fills the corners; the entry aisle crosses them on solid ground.
    if d > LAVA_RING && !entry {
        let beside_entry = (cu == 35 || cu == 44) && cv * 2 <= CENTER2.1;
        if beside_entry {
            return match layer {
                1 => Fill::of(palette.rail),
                _ => Fill::Default,
            };
        }
        return match layer {
            0 => Fill::of(&vanilla_blocks::LAVA),
            _ => Fill::Default,
        };
    }
    if d > LAVA_RING - 1.0 && !entry && layer == 1 {
        return Fill::of(palette.rail);
    }
    if layer == 0 {
        return floor(ctx, cu, cv, d);
    }
    if let Some(fill) = column(ctx, cu, cv, layer) {
        return fill;
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

/// Concentric trim rings, a ring of lights and the magma plates.
fn floor(ctx: &Ctx<'_>, cu: i32, cv: i32, d: f64) -> Fill {
    let palette = ctx.palette;
    let vent = VENTS
        .iter()
        .any(|&(vu, vv)| (vu..=vu + 1).contains(&cu) && (vv..=vv + 1).contains(&cv));
    if vent {
        return Fill::of(&vanilla_blocks::MAGMA_BLOCK);
    }
    let band = d.round() as i32;
    if (d - f64::from(band)).abs() < 0.5 && band > 0 && band % 6 == 0 {
        return Fill::of(if band == 12 && (cu + cv) % 3 == 0 {
            palette.floor_light
        } else {
            palette.floor_trim
        });
    }
    Fill::Default
}

/// The column ring, broken where the entry aisle passes.
fn column(ctx: &Ctx<'_>, cu: i32, cv: i32, layer: i32) -> Option<Fill> {
    let palette = ctx.palette;
    (0..COLUMNS).find_map(|index| {
        let angle = (f64::from(index) + 0.5) * TAU / f64::from(COLUMNS);
        let center_u = (f64::from(CENTER2.0) / 2.0 + COLUMN_RING * angle.cos()).round() as i32;
        let center_v = (f64::from(CENTER2.1) / 2.0 + COLUMN_RING * angle.sin()).round() as i32;
        let near_entry = (33..=46).contains(&center_u) && center_v * 2 <= CENTER2.1;
        if near_entry || (cu - center_u).abs() > 1 || (cv - center_v).abs() > 1 {
            return None;
        }
        Some(match layer {
            1 => Fill::of(palette.trim),
            12 => Fill::of(palette.capital),
            _ if layer == ctx.height => Fill::of(palette.capital),
            _ => Fill::of(palette.pillar),
        })
    })
}

/// The two-tier dais with the towering throne and its guardian statues.
fn dais(ctx: &Ctx<'_>, door: Side, cu: i32, cv: i32, layer: i32) -> Option<Fill> {
    let palette = ctx.palette;
    if !(DAIS_U.0..=DAIS_U.1).contains(&cu) || !(DAIS_FIRST..=DAIS_END).contains(&cv) {
        return None;
    }
    let tier = |first: i32, at: i32| -> Option<Fill> {
        if cv == first && layer == at {
            return Some(Fill::Block(bottom_slab(palette.slab)));
        }
        (cv > first && layer == at).then(|| Fill::of(palette.trim))
    };
    if let Some(fill) = tier(DAIS_FIRST, 1).or_else(|| tier(DAIS_SECOND, 2)) {
        return Some(fill);
    }
    for (su, sv) in STATUES {
        if (su..=su + 1).contains(&cu) && (sv..=sv + 1).contains(&cv) {
            return match layer {
                3..=8 => Some(Fill::of(palette.statue)),
                9 => Some(Fill::of(palette.capital)),
                _ => None,
            };
        }
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
            3..=THRONE_TOP => Some(Fill::of(palette.statue)),
            _ if layer == THRONE_TOP + 1 => Some(Fill::of(palette.capital)),
            _ => None,
        };
    }
    if (cu == 38 || cu == 41) && cv == THRONE_V && layer == 3 {
        return Some(Fill::Block(bottom_slab(palette.slab)));
    }
    None
}

/// The great chandelier over the arena centre, with lanterns at its rim.
fn chandelier(ctx: &Ctx<'_>, cu: i32, cv: i32, layer: i32) -> Option<Fill> {
    let palette = ctx.palette;
    let top = ctx.height;
    let cluster = top - CHANDELIER_CHAIN;
    let (du, dv) = (cu - 39, cv - 44);
    if (0..=1).contains(&du) && (0..=1).contains(&dv) && (cluster + 1..=top).contains(&layer) {
        return Some(Fill::Block(chain()));
    }
    if (-3..=4).contains(&du) && (-3..=4).contains(&dv) && layer == cluster {
        let rim = du == -3 || du == 4 || dv == -3 || dv == 4;
        return Some(if rim {
            Fill::of(palette.capital)
        } else {
            Fill::Block(bottom_slab(palette.slab))
        });
    }
    let corner = (du == -3 || du == 4) && (dv == -3 || dv == 4);
    (corner && layer == cluster - 1).then(|| Fill::Block(hanging(palette.lantern)))
}
