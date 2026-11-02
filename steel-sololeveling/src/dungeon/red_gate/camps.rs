//! The clearings the elves live in: the arrival glade, the camps with their
//! huts and palisades, and the hunting lodges with crystal in their rocks.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{Axis, BlockStateProperties, Direction, SlabType};
use steel_registry::vanilla_blocks;

use super::terrain::lantern_post;
use super::{Sketch, TRAIL_HALF, Zone};
use crate::dungeon::generator::Part;
use crate::seeded::hash;

/// Offsets of the hut centres from a camp's centre, on the diagonals so the
/// lanes the trails enter by stay open.
const HUTS: [(i32, i32); 4] = [(6, 6), (-6, 6), (6, -6), (-6, -6)];
/// Lantern posts inside a camp, clear of the huts and the lanes.
const CAMP_LANTERNS: [(i32, i32); 4] = [(9, 4), (-9, -4), (4, -9), (-4, 9)];
/// Lantern posts around the arrival glade and a lodge.
const SMALL_LANTERNS: [(i32, i32); 4] = [(5, 5), (-5, 5), (5, -5), (-5, -5)];

/// The arrival glade: open snow under four lanterns.
pub(super) fn arrival(sketch: &mut Sketch, zone: Zone) {
    let (cx, cz) = zone.center();
    for (dx, dz) in SMALL_LANTERNS {
        lantern_post(sketch, cx + dx, cz + dz);
    }
}

/// An elf camp: huts around a campfire with log benches, lanterns, and a
/// palisade that leaves the trails' lanes open.
pub(super) fn camp(sketch: &mut Sketch, zone: Zone, sides: &[(i32, i32)], seed: u64) {
    let (cx, cz) = zone.center();
    sketch.block(cx, 0, cz, vanilla_blocks::CAMPFIRE.default_state());
    for (dx, dz, axis) in [
        (2, 0, Axis::Z),
        (-2, 0, Axis::Z),
        (0, 2, Axis::X),
        (0, -2, Axis::X),
    ] {
        sketch.block(
            cx + dx,
            0,
            cz + dz,
            vanilla_blocks::SPRUCE_LOG
                .default_state()
                .set_value(&BlockStateProperties::AXIS, axis),
        );
    }
    let skipped = usize::try_from(hash(seed, cx, 1, cz) % 5).unwrap_or(4);
    for (index, (dx, dz)) in HUTS.into_iter().enumerate() {
        if index != skipped {
            hut(sketch, cx + dx, cz + dz, (-dx.signum(), -dz.signum()));
        }
    }
    for (dx, dz) in CAMP_LANTERNS {
        lantern_post(sketch, cx + dx, cz + dz);
    }
    palisade(sketch, zone, sides);
}

/// A hunting lodge: a hut beside an icy outcrop veined with crystal.
pub(super) fn lodge(sketch: &mut Sketch, zone: Zone, sides: &[(i32, i32)], seed: u64) {
    let (cx, cz) = zone.center();
    // The outcrop rises on the side facing away from the trail.
    let (ax, az) = sides.first().copied().unwrap_or((0, -1));
    let (ox, oz) = (cx - ax * 5, cz - az * 5);
    for dx in -3_i32..=3 {
        for dz in -3_i32..=3 {
            let distance = dx * dx + dz * dz;
            if distance > 10 {
                continue;
            }
            let height = 4 - distance / 3;
            for y in 0..height {
                let surface = y == height - 1 || distance >= 7;
                let part = if surface && !hash(seed ^ 0x0E, ox + dx, y, oz + dz).is_multiple_of(3) {
                    Part::Ore
                } else {
                    Part::Wall
                };
                sketch.set(ox + dx, y, oz + dz, part);
            }
        }
    }
    // The hut stands beside the outcrop, its door towards the glade centre.
    let (px, pz) = (-az, ax);
    hut(sketch, cx + px * 5, cz + pz * 5, (-px, -pz));
    for (dx, dz) in SMALL_LANTERNS {
        let (x, z) = (cx + dx, cz + dz);
        if sketch.is_free(x, 0, z) && sketch.is_free(x, 1, z) {
            lantern_post(sketch, x, z);
        }
    }
}

/// A five-by-five spruce hut with log corners, a doorway facing `door`, and
/// a snowed-over roof.
fn hut(sketch: &mut Sketch, x: i32, z: i32, door: (i32, i32)) {
    let planks = vanilla_blocks::SPRUCE_PLANKS.default_state();
    let log = vanilla_blocks::SPRUCE_LOG.default_state();
    // The door goes in the wall along the larger offset of `door`.
    let door_cell = if door.0 != 0 {
        (door.0 * 2, 0)
    } else {
        (0, door.1 * 2)
    };
    for dx in -2_i32..=2 {
        for dz in -2_i32..=2 {
            let wall = dx.abs() == 2 || dz.abs() == 2;
            let corner = dx.abs() == 2 && dz.abs() == 2;
            for y in 0..=2 {
                if !wall || ((dx, dz) == door_cell && y <= 1) {
                    continue;
                }
                sketch.block(x + dx, y, z + dz, if corner { log } else { planks });
            }
            sketch.block(
                x + dx,
                3,
                z + dz,
                vanilla_blocks::SPRUCE_SLAB
                    .default_state()
                    .set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Bottom),
            );
            if dx.abs() <= 1 && dz.abs() <= 1 {
                sketch.block(
                    x + dx,
                    3,
                    z + dz,
                    vanilla_blocks::SNOW_BLOCK.default_state(),
                );
            }
        }
    }
    // Inside, a barrel of supplies against the back wall.
    let back = (-door_cell.0 / 2, -door_cell.1 / 2);
    sketch.block(
        x + back.0,
        0,
        z + back.1,
        vanilla_blocks::BARREL
            .default_state()
            .set_value(&BlockStateProperties::FACING, Direction::Up),
    );
}

/// The ring of pointed logs around a camp, open where a trail comes in.
fn palisade(sketch: &mut Sketch, zone: Zone, sides: &[(i32, i32)]) {
    let (cx, cz) = zone.center();
    let ring = zone.radius + 1;
    let log = vanilla_blocks::SPRUCE_LOG.default_state();
    for dx in -ring..=ring {
        for dz in -ring..=ring {
            let distance = f64::from(dx * dx + dz * dz).sqrt();
            if (distance - f64::from(ring)).abs() > 0.5 {
                continue;
            }
            let lane = sides.iter().any(|&(sx, sz)| {
                let ahead = dx * sx + dz * sz;
                let across = (dx * sz - dz * sx).abs();
                ahead > 0 && across <= TRAIL_HALF + 1
            });
            if lane {
                continue;
            }
            let height = if (dx + dz).rem_euclid(3) == 0 { 3 } else { 2 };
            for y in 0..height {
                sketch.block(cx + dx, y, cz + dz, log);
            }
        }
    }
}
