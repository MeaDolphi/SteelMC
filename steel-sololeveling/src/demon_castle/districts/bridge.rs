//! The bridge hall: a lava chasm crossed by railed bridges from every gate to
//! a central island, with a walkway along the walls and great piers rising
//! out of the lava.

use steel_registry::vanilla_blocks;

use super::{
    CHASM_DEPTH, Ctx, DOOR_HI, DOOR_LO, Fill, PLAZA_HI, PLAZA_LO, WALL, column, default_floor,
    hanging_lantern, ring,
};
use crate::demon_castle::DISTRICT_SIZE;
use crate::demon_castle::plan::Side;

/// Last wall ring of the walkway; its railing stands on this ring.
const WALKWAY_INNER: i32 = 4;
/// Layer of the lava surface.
const LAVA_LAYER: i32 = 2 - CHASM_DEPTH;
/// Ring of the island edge.
const ISLAND_EDGE: i32 = PLAZA_HI - PLAZA_LO;
/// Centres of the four piers.
const PIERS: [(i32, i32); 4] = [(16, 16), (16, 63), (63, 16), (63, 63)];
/// Columns of the lanterns hanging over the chasm.
const LANTERNS: [(i32, i32); 4] = [(24, 40), (55, 40), (40, 24), (40, 55)];

/// Returns the wall ring of `(u, v)`: 0 at the outer face of the walls.
const fn wall_ring(u: i32, v: i32) -> i32 {
    let a = if u < v { u } else { v };
    let b = DISTRICT_SIZE - 1 - if u > v { u } else { v };
    if a < b { a } else { b }
}

/// Returns the bridge `(u, v)` lies on and its position across the bridge,
/// railings included; `None` off every bridge.
fn bridge_at(ctx: &Ctx<'_>, u: i32, v: i32) -> Option<i32> {
    let band = DOOR_LO - 1..=DOOR_HI + 1;
    ctx.district.door_sides().find_map(|side| {
        let (across, toward_island) = match side {
            Side::North => (u, v <= PLAZA_LO),
            Side::South => (u, v >= PLAZA_HI),
            Side::West => (v, u <= PLAZA_LO),
            Side::East => (v, u >= PLAZA_HI),
        };
        (band.contains(&across) && toward_island).then_some(across)
    })
}

pub(super) fn fill(ctx: &Ctx<'_>, u: i32, v: i32, layer: i32) -> Fill {
    let palette = ctx.palette;
    for center in PIERS {
        if (u - center.0).abs() <= 1 && (v - center.1).abs() <= 1 && layer <= 0 {
            return Fill::of(palette.pillar);
        }
        if let Some(fill) = column(ctx, u, v, layer, center, 1) {
            return fill;
        }
    }
    let walkway = (WALL..=WALKWAY_INNER).contains(&wall_ring(u, v));
    let island = ring(u, v) <= ISLAND_EDGE;
    let bridge = bridge_at(ctx, u, v);
    if walkway || island || bridge.is_some() {
        return ground(ctx, u, v, layer, walkway, island, bridge);
    }
    // The chasm: rock at the bottom, then lava, then air up to the floor.
    if layer < LAVA_LAYER {
        return Fill::of(palette.foundation);
    }
    if layer == LAVA_LAYER {
        return Fill::of(&vanilla_blocks::LAVA);
    }
    if LANTERNS.contains(&(u, v))
        && let Some(fill) = hanging_lantern(ctx, 8, layer)
    {
        return fill;
    }
    Fill::Air
}

/// The walkway, the island and the bridges: solid rock to the floor, with
/// railings along every drop.
fn ground(
    ctx: &Ctx<'_>,
    u: i32,
    v: i32,
    layer: i32,
    walkway: bool,
    island: bool,
    bridge: Option<i32>,
) -> Fill {
    let palette = ctx.palette;
    if layer < 0 {
        // Bridges are two-block decks on piers every eight blocks; the
        // walkway and the island are cliffs down to the chasm floor.
        if let Some(across) = bridge
            && !walkway
            && !island
            && layer < -1
        {
            if (u + v) % 8 == 0 && across == i32::midpoint(DOOR_LO, DOOR_HI) {
                return Fill::of(palette.pillar);
            }
            return match layer {
                _ if layer < LAVA_LAYER => Fill::of(palette.foundation),
                LAVA_LAYER => Fill::of(&vanilla_blocks::LAVA),
                _ => Fill::Air,
            };
        }
        return Fill::of(palette.foundation);
    }
    if layer == 0 {
        if bridge.is_some_and(|across| across == DOOR_LO - 1 || across == DOOR_HI + 1)
            && !walkway
            && !island
        {
            return Fill::of(palette.floor_trim);
        }
        return Fill::Block(default_floor(ctx, u, v));
    }
    if layer != 1 {
        return Fill::Default;
    }
    let on_bridge_deck = bridge.is_some_and(|across| (DOOR_LO..=DOOR_HI).contains(&across));
    let railing = if island {
        ring(u, v) == ISLAND_EDGE && !on_bridge_deck
    } else if walkway {
        wall_ring(u, v) == WALKWAY_INNER && !on_bridge_deck && bridge.is_none()
    } else {
        bridge.is_some_and(|across| across == DOOR_LO - 1 || across == DOOR_HI + 1)
    };
    if railing {
        Fill::of(palette.rail)
    } else {
        Fill::Default
    }
}
