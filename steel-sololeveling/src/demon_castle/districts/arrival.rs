//! The arrival hall: a ringed sigil floor where the hunters land, four
//! columns, hanging lanterns and statues flanking every gate.

use super::{Ctx, DOOR_HI, DOOR_LO, Fill, column, hanging_lantern, ring};
use crate::demon_castle::DISTRICT_SIZE;
use crate::demon_castle::plan::Side;

/// Centres of the four columns.
const COLUMNS: [(i32, i32); 4] = [(20, 20), (20, 59), (59, 20), (59, 59)];
/// Columns of the four hanging lanterns.
const LANTERNS: [(i32, i32); 4] = [(30, 30), (30, 49), (49, 30), (49, 49)];
/// How far into the hall the gate statues stand.
const STATUE_DEPTH: i32 = 5;

pub(super) fn fill(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Fill {
    let palette = ctx.palette;
    if layer == 0 {
        let d = ring(u, v);
        if d <= 3 {
            return Fill::of(palette.floor_light);
        }
        if d == 13 && (u + v) % 2 == 0 {
            return Fill::of(palette.floor_light);
        }
        if matches!(d, 11 | 23 | 35 | 47) {
            return Fill::of(palette.floor_trim);
        }
        return Fill::Default;
    }
    for center in COLUMNS {
        if let Some(fill) = column(ctx, u, v, layer, center, 1) {
            return fill;
        }
    }
    if LANTERNS.contains(&(u, v))
        && let Some(fill) = hanging_lantern(ctx, 3, layer)
    {
        return fill;
    }
    if let Some(fill) = statue(ctx, u, v, layer) {
        return fill;
    }
    Fill::Default
}

/// Two statues flank each gate a few blocks into the hall.
fn statue(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Option<Fill> {
    let palette = ctx.palette;
    let far = DISTRICT_SIZE - 1 - STATUE_DEPTH;
    let at = ctx.district.door_sides().any(|side| {
        let (along, depth) = match side {
            Side::North => (u, v == STATUE_DEPTH),
            Side::South => (u, v == far),
            Side::West => (v, u == STATUE_DEPTH),
            Side::East => (v, u == far),
        };
        depth && (along == DOOR_LO - 4 || along == DOOR_HI + 4)
    });
    if !at {
        return None;
    }
    match layer {
        1 => Some(Fill::of(palette.trim)),
        2 | 3 => Some(Fill::of(palette.statue)),
        4 => Some(Fill::of(palette.capital)),
        _ => None,
    }
}
