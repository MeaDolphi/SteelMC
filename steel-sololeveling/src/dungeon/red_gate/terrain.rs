//! The valley floor: snow, trails with lanterns, a frozen lake, the spruce
//! forest and the cliffs that close the valley in.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::vanilla_blocks;
use steel_utils::BlockStateId;

use super::{CELL, GRID_RADIUS, MARGIN, Sketch, TRAIL_HALF, Zone};
use crate::dungeon::generator::Part;
use crate::seeded::{SplitMix, hash};

/// Thickness of the cliff ring.
const CLIFF_THICKNESS: i32 = 2;
/// Lowest and highest cliff top above the valley floor.
const CLIFF_HEIGHT: (i32, i32) = (12, 19);
/// Radius of the frozen lake.
const LAKE_RADIUS: i32 = 10;
/// Side of the jittered grid trees are planted on.
const TREE_GRID: i32 = 5;
/// Blocks between two lantern posts along a trail.
const LANTERN_SPACING: i32 = 8;

/// The rectangle the valley covers, cliffs included.
#[derive(Debug, Clone, Copy)]
pub(super) struct Valley {
    min: (i32, i32),
    max: (i32, i32),
}

impl Valley {
    const fn contains_inner(self, x: i32, z: i32, inset: i32) -> bool {
        x >= self.min.0 + inset
            && x <= self.max.0 - inset
            && z >= self.min.1 + inset
            && z <= self.max.1 - inset
    }
}

/// Returns the rectangle around every clearing with a forest margin.
pub(super) fn valley(zones: &[Zone]) -> Valley {
    let reach = |zone: &Zone| zone.radius + MARGIN;
    let min_x = zones
        .iter()
        .map(|z| z.center().0 - reach(z))
        .min()
        .unwrap_or(0);
    let max_x = zones
        .iter()
        .map(|z| z.center().0 + reach(z))
        .max()
        .unwrap_or(0);
    let min_z = zones
        .iter()
        .map(|z| z.center().1 - reach(z))
        .min()
        .unwrap_or(0);
    let max_z = zones
        .iter()
        .map(|z| z.center().1 + reach(z))
        .max()
        .unwrap_or(0);
    Valley {
        min: (min_x, min_z),
        max: (max_x, max_z),
    }
}

/// Picks a free grid cell inside the valley for the frozen lake, if any.
pub(super) fn lake_cell(zones: &[Zone], rng: &mut SplitMix) -> Option<(i32, i32)> {
    let (min_x, max_x) = span(zones.iter().map(|zone| zone.cell.0));
    let (min_z, max_z) = span(zones.iter().map(|zone| zone.cell.1));
    let mut free = Vec::new();
    for x in min_x.max(-GRID_RADIUS)..=max_x.min(GRID_RADIUS) {
        for z in min_z.max(-GRID_RADIUS)..=max_z.min(GRID_RADIUS) {
            if zones.iter().all(|zone| zone.cell != (x, z)) {
                free.push((x, z));
            }
        }
    }
    (!free.is_empty()).then(|| free[rng.index(free.len())])
}

fn span(values: impl Iterator<Item = i32> + Clone) -> (i32, i32) {
    (values.clone().min().unwrap_or(0), values.max().unwrap_or(0))
}

fn lake_covers(lake: Option<(i32, i32)>, x: i32, z: i32, extra: i32) -> bool {
    lake.is_some_and(|(cx, cz)| {
        let reach = LAKE_RADIUS + extra;
        (x - cx * CELL).pow(2) + (z - cz * CELL).pow(2) <= reach * reach
    })
}

/// Returns whether column `(x, z)` lies on a trail, widened by `extra`.
pub(super) fn on_trail(zones: &[Zone], x: i32, z: i32, extra: i32) -> bool {
    let half = TRAIL_HALF + extra;
    zones.iter().any(|zone| {
        let Some(from) = zone.from else {
            return false;
        };
        let (ax, az) = zone.center();
        let (bx, bz) = (from.0 * CELL, from.1 * CELL);
        if az == bz {
            (ax.min(bx)..=ax.max(bx)).contains(&x) && (z - az).abs() <= half
        } else {
            (az.min(bz)..=az.max(bz)).contains(&z) && (x - ax).abs() <= half
        }
    })
}

/// Lays the snow, the trails with their lanterns and the frozen lake.
pub(super) fn ground(
    sketch: &mut Sketch,
    zones: &[Zone],
    valley: Valley,
    lake: Option<(i32, i32)>,
    seed: u64,
) {
    for x in valley.min.0..=valley.max.0 {
        for z in valley.min.1..=valley.max.1 {
            let in_clearing = zones.iter().any(|zone| zone.covers(x, z, 0));
            let part = if !in_clearing && on_trail(zones, x, z, 0) {
                Part::FloorAlt
            } else if lake_covers(lake, x, z, 0) {
                // Packed and blue ice never melt, whatever light reaches them.
                let ice = if hash(seed ^ 0x1CE, x, 0, z).is_multiple_of(5) {
                    &vanilla_blocks::BLUE_ICE
                } else {
                    &vanilla_blocks::PACKED_ICE
                };
                Part::Block(ice.default_state())
            } else {
                Part::Floor
            };
            sketch.set(x, -1, z, part);
        }
    }
    trail_lanterns(sketch, zones);
}

/// Stands a lantern post beside every trail, alternating sides.
fn trail_lanterns(sketch: &mut Sketch, zones: &[Zone]) {
    for zone in zones {
        let Some(from) = zone.from else {
            continue;
        };
        let (ax, az) = zone.center();
        let (bx, bz) = (from.0 * CELL, from.1 * CELL);
        let length = (bx - ax).abs() + (bz - az).abs();
        let (step_x, step_z) = ((bx - ax).signum(), (bz - az).signum());
        for (index, along) in (0..=length).step_by(LANTERN_SPACING as usize).enumerate() {
            let side = if index % 2 == 0 { 1 } else { -1 } * (TRAIL_HALF + 1);
            let x = ax + step_x * along + step_z.abs() * side;
            let z = az + step_z * along + step_x.abs() * side;
            if zones.iter().any(|zone| zone.covers(x, z, 1)) {
                continue;
            }
            lantern_post(sketch, x, z);
        }
    }
}

/// A spruce fence post two blocks high carrying a lantern.
pub(super) fn lantern_post(sketch: &mut Sketch, x: i32, z: i32) {
    for y in 0..=1 {
        sketch.block(x, y, z, vanilla_blocks::SPRUCE_FENCE.default_state());
    }
    sketch.set(x, 2, z, Part::Light);
}

/// Plants the spruce forest and its boulders wherever nothing else stands.
pub(super) fn forest(
    sketch: &mut Sketch,
    zones: &[Zone],
    valley: Valley,
    lake: Option<(i32, i32)>,
    seed: u64,
) {
    let grid_min = (
        valley.min.0.div_euclid(TREE_GRID),
        valley.min.1.div_euclid(TREE_GRID),
    );
    let grid_max = (
        valley.max.0.div_euclid(TREE_GRID),
        valley.max.1.div_euclid(TREE_GRID),
    );
    for gx in grid_min.0..=grid_max.0 {
        for gz in grid_min.1..=grid_max.1 {
            let roll = hash(seed ^ 0x7EE5, gx, 0, gz);
            let x = gx * TREE_GRID + 1 + i32::try_from(roll % 3).unwrap_or(0);
            let z = gz * TREE_GRID + 1 + i32::try_from((roll >> 8) % 3).unwrap_or(0);
            let open = valley.contains_inner(x, z, CLIFF_THICKNESS + 2)
                && !zones.iter().any(|zone| zone.covers(x, z, 3))
                && !on_trail(zones, x, z, 2)
                && !lake_covers(lake, x, z, 2);
            if !open {
                continue;
            }
            match (roll >> 16) % 10 {
                0..=5 => spruce(
                    sketch,
                    x,
                    z,
                    6 + i32::try_from((roll >> 24) % 4).unwrap_or(0),
                ),
                6 => boulder(sketch, x, z, roll >> 32),
                _ => {}
            }
        }
    }
}

/// Radius of the spruce crown, layer by layer from the top.
const CROWN: [i32; 9] = [0, 1, 1, 2, 1, 2, 3, 2, 3];

/// A snowy spruce with a trunk of `height` logs.
fn spruce(sketch: &mut Sketch, x: i32, z: i32, height: i32) {
    for y in 0..height {
        sketch.block(x, y, z, vanilla_blocks::SPRUCE_LOG.default_state());
    }
    // Placed leaves must not decay: nothing in a dungeon is meant to change.
    let leaves = vanilla_blocks::SPRUCE_LEAVES
        .default_state()
        .set_value(&BlockStateProperties::PERSISTENT, true);
    for (layer, &radius) in CROWN.iter().enumerate() {
        let y = height - i32::try_from(layer).unwrap_or(0);
        if y < 2 {
            break;
        }
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let corner = radius > 0 && dx.abs() == radius && dz.abs() == radius;
                if corner || (dx == 0 && dz == 0 && y < height) {
                    continue;
                }
                let top = layer == 0;
                let state = if top {
                    vanilla_blocks::SNOW_BLOCK.default_state()
                } else {
                    leaves
                };
                sketch.block(x + dx, y, z + dz, state);
            }
        }
    }
}

/// A lump of stone half buried in snow.
fn boulder(sketch: &mut Sketch, x: i32, z: i32, roll: u64) {
    let stone = |salt: u64| -> BlockStateId {
        if (roll >> salt).is_multiple_of(3) {
            vanilla_blocks::COBBLESTONE.default_state()
        } else {
            vanilla_blocks::STONE.default_state()
        }
    };
    for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        sketch.block(
            x + dx,
            0,
            z + dz,
            stone(u64::try_from(dx + 2 * dz).unwrap_or(0)),
        );
    }
    sketch.block(x, 1, z, vanilla_blocks::SNOW_BLOCK.default_state());
}

/// Raises the cliff ring along the valley's edge.
pub(super) fn cliffs(sketch: &mut Sketch, valley: Valley, seed: u64) {
    for x in valley.min.0..=valley.max.0 {
        for z in valley.min.1..=valley.max.1 {
            if valley.contains_inner(x, z, CLIFF_THICKNESS) {
                continue;
            }
            let roll = hash(seed ^ 0xC11F, x, 0, z);
            let span = u64::try_from(CLIFF_HEIGHT.1 - CLIFF_HEIGHT.0 + 1).unwrap_or(1);
            let top = CLIFF_HEIGHT.0 + i32::try_from(roll % span).unwrap_or(0);
            for y in -1..=top {
                let part = if y >= top - 1 {
                    Part::Block(vanilla_blocks::SNOW_BLOCK.default_state())
                } else if hash(seed ^ 0xC120, x, y, z).is_multiple_of(4) {
                    Part::Wall
                } else {
                    Part::Block(vanilla_blocks::STONE.default_state())
                };
                sketch.set(x, y, z, part);
            }
        }
    }
}
