//! The chieftain's ice palace: a round arena of packed and blue ice ringed by
//! pillars, walled in behind the throne, where the exit opens once the
//! chieftain falls.

use std::f64::consts::TAU;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::vanilla_blocks;

use super::{Sketch, Zone};

/// Pillars around the arena.
const PILLARS: u32 = 12;
/// Distance of the pillar ring from the centre.
const PILLAR_RING: f64 = 12.0;
/// Height of a pillar below its cap.
const PILLAR_HEIGHT: i32 = 7;
/// Ring of the lights set into the arena floor.
const FLOOR_LIGHT_RING: f64 = 8.0;
/// Height of the wall behind the throne.
const WALL_HEIGHT: i32 = 9;
/// Row of the throne's backrest, south of the centre where the boss waits.
const THRONE_ROW: i32 = 15;

/// Builds the palace in `zone`. The trail never arrives from the south (see
/// [`super::walk`]), so the throne and its wall stand there.
pub(super) fn palace(sketch: &mut Sketch, zone: Zone) {
    let (cx, cz) = zone.center();
    floor(sketch, zone);
    let entry = zone
        .from
        .map(|from| (from.0 - zone.cell.0, from.1 - zone.cell.1));
    for index in 0..PILLARS {
        let angle = (f64::from(index) + 0.5) * TAU / f64::from(PILLARS);
        let (dir_x, dir_z) = (angle.cos(), angle.sin());
        // No pillar blocks the trail's way in.
        if entry.is_some_and(|(ex, ez)| dir_x * f64::from(ex) + dir_z * f64::from(ez) > 0.85) {
            continue;
        }
        let x = cx + (dir_x * PILLAR_RING).round() as i32;
        let z = cz + (dir_z * PILLAR_RING).round() as i32;
        pillar(sketch, x, z);
    }
    back_wall(sketch, zone);
    throne(sketch, cx, cz);
}

/// Concentric rings of blue ice in packed ice, with lights in the floor.
fn floor(sketch: &mut Sketch, zone: Zone) {
    let (cx, cz) = zone.center();
    let radius = zone.radius;
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            let distance = f64::from(dx * dx + dz * dz).sqrt();
            if distance > f64::from(radius) {
                continue;
            }
            let angle = f64::from(dz).atan2(f64::from(dx)).rem_euclid(TAU);
            let lit =
                (distance - FLOOR_LIGHT_RING).abs() < 0.5 && (angle / (TAU / 8.0)).fract() < 0.08;
            let state = if lit {
                vanilla_blocks::SEA_LANTERN.default_state()
            } else if (distance.round() as i32).rem_euclid(4) == 0 {
                vanilla_blocks::BLUE_ICE.default_state()
            } else {
                vanilla_blocks::PACKED_ICE.default_state()
            };
            sketch.block(cx + dx, -1, cz + dz, state);
        }
    }
}

/// A cross-shaped ice pillar with a blue cap and a light on top.
fn pillar(sketch: &mut Sketch, x: i32, z: i32) {
    for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
        for y in 0..PILLAR_HEIGHT {
            sketch.block(
                x + dx,
                y,
                z + dz,
                vanilla_blocks::PACKED_ICE.default_state(),
            );
        }
        sketch.block(
            x + dx,
            PILLAR_HEIGHT,
            z + dz,
            vanilla_blocks::BLUE_ICE.default_state(),
        );
    }
    sketch.block(
        x,
        PILLAR_HEIGHT + 1,
        z,
        vanilla_blocks::SEA_LANTERN.default_state(),
    );
}

/// The curved wall closing the arena behind the throne.
fn back_wall(sketch: &mut Sketch, zone: Zone) {
    let (cx, cz) = zone.center();
    let outer = zone.radius + 1;
    for dx in -outer..=outer {
        for dz in 5..=outer {
            let distance = f64::from(dx * dx + dz * dz).sqrt();
            if (distance - f64::from(outer)).abs() > 0.7 {
                continue;
            }
            for y in 0..WALL_HEIGHT {
                let state = if y == 4 {
                    vanilla_blocks::BLUE_ICE.default_state()
                } else if y == WALL_HEIGHT - 1 {
                    vanilla_blocks::QUARTZ_BLOCK.default_state()
                } else {
                    vanilla_blocks::PACKED_ICE.default_state()
                };
                sketch.block(cx + dx, y, cz + dz, state);
            }
        }
    }
}

/// The throne on its dais, flanked by soul fires.
fn throne(sketch: &mut Sketch, cx: i32, cz: i32) {
    let quartz = vanilla_blocks::QUARTZ_BLOCK.default_state();
    for dx in -2..=2 {
        for dz in THRONE_ROW - 2..=THRONE_ROW {
            sketch.block(cx + dx, 0, cz + dz, quartz);
        }
    }
    sketch.block(
        cx,
        1,
        cz + THRONE_ROW - 1,
        vanilla_blocks::QUARTZ_STAIRS
            .default_state()
            .set_value(&BlockStateProperties::HORIZONTAL_FACING, Direction::South),
    );
    for y in 1..=4 {
        sketch.block(
            cx,
            y,
            cz + THRONE_ROW,
            vanilla_blocks::QUARTZ_PILLAR.default_state(),
        );
    }
    for dx in [-1, 1] {
        for y in 1..=3 {
            sketch.block(
                cx + dx,
                y,
                cz + THRONE_ROW,
                vanilla_blocks::BLUE_ICE.default_state(),
            );
        }
    }
    for dx in [-4, 4] {
        sketch.block(
            cx + dx,
            0,
            cz + THRONE_ROW - 2,
            vanilla_blocks::SOUL_CAMPFIRE.default_state(),
        );
    }
}
