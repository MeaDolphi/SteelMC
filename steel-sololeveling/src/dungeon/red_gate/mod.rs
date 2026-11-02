//! The snow valley behind a red gate.
//!
//! A red gate does not open onto a cave: its hunters land in a valley under
//! the open sky, ringed by icy cliffs, where trails wind through a spruce
//! forest from one snow elf camp to the next and end at the ice palace of the
//! chieftain. The valley is still an ordinary [`Blueprint`]: its clearings
//! are the rooms, so waves, the boss, the exit, the map and the erasure of a
//! reused region all work as in any other dungeon.
//!
//! Positions follow the blueprint convention: offsets from the region spawn,
//! `y = 0` the first air block above the ground.

mod camps;
mod palace;
mod terrain;

use rustc_hash::FxHashMap;
use steel_utils::BlockStateId;

use super::generator::{Blueprint, Part, Placement, Room, RoomRole, Shape, Theme};
use crate::seeded::{SplitMix, hash};

/// Blocks between the centres of two neighbouring grid cells.
const CELL: i32 = 32;
/// Grid cells from the arrival clearing to the grid edge.
const GRID_RADIUS: i32 = 2;
/// Radius of an elf camp's clearing.
const CAMP_RADIUS: i32 = 11;
/// Radius of the arrival clearing and the hunting lodges.
const SMALL_RADIUS: i32 = 9;
/// Radius of the chieftain's arena.
const ARENA_RADIUS: i32 = 15;
/// Half width of a trail; trails are five blocks wide.
const TRAIL_HALF: i32 = 2;
/// Forest between the outermost clearing and the cliffs.
const MARGIN: i32 = 12;
/// Most spawn spots remembered per clearing.
const SPAWN_SPOTS: usize = 24;

const DIRECTIONS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// One clearing of the valley.
#[derive(Debug, Clone, Copy)]
struct Zone {
    role: RoomRole,
    cell: (i32, i32),
    radius: i32,
    /// Cell the trail into this clearing comes from; the arrival has none.
    from: Option<(i32, i32)>,
}

impl Zone {
    const fn center(self) -> (i32, i32) {
        (self.cell.0 * CELL, self.cell.1 * CELL)
    }

    /// Returns whether column `(x, z)` lies within `extra` blocks of the clearing.
    const fn covers(self, x: i32, z: i32, extra: i32) -> bool {
        let (cx, cz) = self.center();
        let reach = self.radius + extra;
        (x - cx).pow(2) + (z - cz).pow(2) <= reach * reach
    }

    /// Returns the cardinal directions trails leave this clearing in.
    fn trail_sides(self, zones: &[Self]) -> Vec<(i32, i32)> {
        let mut sides = Vec::new();
        if let Some(from) = self.from {
            sides.push((from.0 - self.cell.0, from.1 - self.cell.1));
        }
        for other in zones {
            if other.from == Some(self.cell) {
                sides.push((other.cell.0 - self.cell.0, other.cell.1 - self.cell.1));
            }
        }
        sides
    }
}

/// Every block of the valley before it is flattened into placements; later
/// writes replace earlier ones.
#[derive(Default)]
struct Sketch {
    blocks: FxHashMap<(i32, i32, i32), Part>,
}

impl Sketch {
    fn set(&mut self, x: i32, y: i32, z: i32, part: Part) {
        self.blocks.insert((x, y, z), part);
    }

    fn block(&mut self, x: i32, y: i32, z: i32, state: BlockStateId) {
        self.set(x, y, z, Part::Block(state));
    }

    fn is_free(&self, x: i32, y: i32, z: i32) -> bool {
        !self.blocks.contains_key(&(x, y, z))
    }

    /// Returns the placements bottom up, so every block that needs support
    /// (lanterns, leaves, campfires) is built after what holds it.
    fn into_placements(self) -> Vec<Placement> {
        let mut placements: Vec<Placement> = self
            .blocks
            .into_iter()
            .map(|(offset, part)| Placement { offset, part })
            .collect();
        placements.sort_unstable_by_key(|placement| {
            let (x, y, z) = placement.offset;
            (y, x, z)
        });
        placements
    }
}

/// Generates the snow valley of `seed` with `combat_rooms` elf camps.
#[must_use]
pub fn generate(seed: u64, combat_rooms: u8) -> Blueprint {
    let mut rng = SplitMix::new(seed ^ 0x5A0F_E1F5);
    let zones = plan_zones(&mut rng, usize::from(combat_rooms));
    let mut sketch = Sketch::default();
    let lake = terrain::lake_cell(&zones, &mut rng);
    let valley = terrain::valley(&zones);
    terrain::ground(&mut sketch, &zones, valley, lake, seed);
    for zone in &zones {
        let sides = zone.trail_sides(&zones);
        match zone.role {
            RoomRole::Entrance => camps::arrival(&mut sketch, *zone),
            RoomRole::Combat => camps::camp(&mut sketch, *zone, &sides, seed),
            RoomRole::Chamber => camps::lodge(&mut sketch, *zone, &sides, seed),
            RoomRole::Boss => palace::palace(&mut sketch, *zone),
        }
    }
    terrain::forest(&mut sketch, &zones, valley, lake, seed);
    terrain::cliffs(&mut sketch, valley, seed);

    let rooms = zones
        .iter()
        .map(|zone| Room {
            role: zone.role,
            shape: Shape::Rotunda,
            center: zone.center(),
            half: (zone.radius, zone.radius),
            height: 12,
            spawns: spawn_spots(&sketch, *zone, seed),
        })
        .collect();
    let placements = sketch.into_placements();
    let bounds = bounds(&placements);
    Blueprint {
        seed,
        theme: Theme::SnowElves,
        rooms,
        blocks: placements,
        bounds,
    }
}

/// Plans the clearings: the arrival, the camps in order, the palace, then
/// the hunting lodges off the trail.
fn plan_zones(rng: &mut SplitMix, combat_rooms: usize) -> Vec<Zone> {
    let path = walk(rng, combat_rooms + 2);
    let mut zones: Vec<Zone> = path
        .iter()
        .enumerate()
        .map(|(index, &cell)| {
            let (role, radius) = match index {
                0 => (RoomRole::Entrance, SMALL_RADIUS),
                _ if index == path.len() - 1 => (RoomRole::Boss, ARENA_RADIUS),
                _ => (RoomRole::Combat, CAMP_RADIUS),
            };
            Zone {
                role,
                cell,
                radius,
                from: index.checked_sub(1).map(|previous| path[previous]),
            }
        })
        .collect();
    let lodges = 1 + usize::try_from(rng.next() % 2).unwrap_or(0);
    for _ in 0..lodges {
        let camps: Vec<(i32, i32)> = zones
            .iter()
            .filter(|zone| zone.role == RoomRole::Combat)
            .map(|zone| zone.cell)
            .collect();
        let options: Vec<((i32, i32), (i32, i32))> = camps
            .iter()
            .flat_map(|&camp| {
                DIRECTIONS
                    .iter()
                    .map(move |(dx, dz)| (camp, (camp.0 + dx, camp.1 + dz)))
            })
            .filter(|&(_, cell)| in_grid(cell) && zones.iter().all(|zone| zone.cell != cell))
            .collect();
        if options.is_empty() {
            break;
        }
        let (camp, cell) = options[rng.index(options.len())];
        zones.push(Zone {
            role: RoomRole::Chamber,
            cell,
            radius: SMALL_RADIUS,
            from: Some(camp),
        });
    }
    zones
}

const fn in_grid((x, z): (i32, i32)) -> bool {
    x.abs() <= GRID_RADIUS && z.abs() <= GRID_RADIUS
}

/// Walks a self-avoiding path of `length` cells from the arrival cell.
///
/// The last step must not come from the south: the boss waits on the south
/// side of its arena, under the throne, so the trail has to arrive from
/// anywhere else.
fn walk(rng: &mut SplitMix, length: usize) -> Vec<(i32, i32)> {
    for _ in 0..64 {
        let mut path = vec![(0, 0)];
        while path.len() < length {
            let last = path[path.len() - 1];
            let options: Vec<(i32, i32)> = DIRECTIONS
                .iter()
                .map(|(dx, dz)| (last.0 + dx, last.1 + dz))
                .filter(|cell| in_grid(*cell) && !path.contains(cell))
                .collect();
            if options.is_empty() {
                break;
            }
            path.push(options[rng.index(options.len())]);
        }
        if path.len() == length && arrives_from_north_or_side(&path) {
            return path;
        }
    }
    // A snake out of the centre always fits, and whatever its length, its
    // last step never comes from the south.
    [(0, 0), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 2), (0, 2)][..length].to_vec()
}

const fn arrives_from_north_or_side(path: &[(i32, i32)]) -> bool {
    let [.., previous, last] = path else {
        return true;
    };
    previous.1 <= last.1
}

/// Returns the free stands of a clearing, spread across it.
fn spawn_spots(sketch: &Sketch, zone: Zone, seed: u64) -> Vec<(i32, i32)> {
    let (cx, cz) = zone.center();
    let mut spots: Vec<(i32, i32)> = Vec::new();
    for x in cx - zone.radius..=cx + zone.radius {
        for z in cz - zone.radius..=cz + zone.radius {
            let distance = (x - cx).pow(2) + (z - cz).pow(2);
            if distance < 9 || !zone.covers(x, z, -2) {
                continue;
            }
            if !sketch.is_free(x, -1, z) && sketch.is_free(x, 0, z) && sketch.is_free(x, 1, z) {
                spots.push((x, z));
            }
        }
    }
    spots.sort_unstable_by_key(|&(x, z)| (hash(seed ^ 0x51_0751, x, 0, z), x, z));
    spots.truncate(SPAWN_SPOTS);
    spots
}

fn bounds(placements: &[Placement]) -> ((i32, i32, i32), (i32, i32, i32)) {
    let mut min = (i32::MAX, i32::MAX, i32::MAX);
    let mut max = (i32::MIN, i32::MIN, i32::MIN);
    for placement in placements {
        let (x, y, z) = placement.offset;
        min = (min.0.min(x), min.1.min(y), min.2.min(z));
        max = (max.0.max(x), max.1.max(y), max.2.max(z));
    }
    (min, max)
}

#[cfg(test)]
mod tests;
