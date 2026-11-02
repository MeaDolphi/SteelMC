use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::chunk::light::{get_light_block_into, get_light_opacity};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::{init_vanilla_registry, vanilla_blocks};
use steel_utils::Direction;
use steel_utils::axis::Axis;

use super::great_hall::GALLERY_LAYER;
use super::{MAX_LAYER, MIN_LAYER, PLAZA_LO};
use crate::demon_castle::HALL_HALF;
use crate::demon_castle::encounter::floor_packs;
use crate::demon_castle::plan::{Archetype, FloorPlan};

/// Highest step a hunter takes without jumping.
const STEP: f64 = 0.6;
/// Air a hunter needs above their feet.
const HEADROOM: f64 = 1.8;

/// Feet heights a hunter can stand at in one column, relative to the floor
/// surface (layer 0's top is 1.0), plus whether the column is a stair.
struct Column {
    stands: Vec<f64>,
}

/// A grid walk over one generated floor with vanilla step and headroom
/// rules. Stairs offer both of their half steps; the flights never take a
/// climber from the side, so the walk needs no facing rules.
struct Walk<'a> {
    plan: &'a FloorPlan,
    columns: FxHashMap<(i32, i32), Column>,
}

impl<'a> Walk<'a> {
    fn new(plan: &'a FloorPlan) -> Self {
        Self {
            plan,
            columns: FxHashMap::default(),
        }
    }

    fn column(&mut self, lx: i32, lz: i32) -> &Column {
        let plan = self.plan;
        self.columns.entry((lx, lz)).or_insert_with(|| {
            let mut boxes: Vec<(f64, f64)> = Vec::new();
            let mut tops: Vec<f64> = Vec::new();
            for layer in MIN_LAYER..=MAX_LAYER + 2 {
                let Some(block) = plan.block(lx, lz, layer) else {
                    continue;
                };
                let base = f64::from(layer);
                let stair = block
                    .try_get_value(&BlockStateProperties::STAIRS_SHAPE)
                    .is_some();
                if stair {
                    // Only the low step counts against headroom, or the high
                    // half would bury the stair's own lower step.
                    boxes.push((base, base + 0.5));
                    tops.push(base + 0.5);
                    tops.push(base + 1.0);
                    continue;
                }
                for aabb in block.get_static_collision_shape() {
                    let (min, max) = (base + aabb.min(Axis::Y), base + aabb.max(Axis::Y));
                    boxes.push((min, max));
                    tops.push(max);
                }
            }
            tops.sort_by(f64::total_cmp);
            tops.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            let stands = tops
                .into_iter()
                .filter(|&feet| {
                    boxes
                        .iter()
                        .all(|&(min, max)| max <= feet + 1e-6 || min >= feet + HEADROOM - 1e-6)
                })
                .collect();
            Column { stands }
        })
    }

    /// Flood-fills from `start` and returns every reached `(lx, lz, feet)`,
    /// with feet heights in sixteenths of a block.
    fn reachable(&mut self, start: (i32, i32, f64)) -> FxHashSet<(i32, i32, i32)> {
        let key = |feet: f64| (feet * 16.0).round() as i32;
        let mut seen = FxHashSet::default();
        let mut queue = VecDeque::new();
        assert!(
            self.column(start.0, start.1)
                .stands
                .iter()
                .any(|feet| (feet - start.2).abs() < 1e-6),
            "the start {start:?} is no stand"
        );
        seen.insert((start.0, start.1, key(start.2)));
        queue.push_back(start);
        while let Some((lx, lz, feet)) = queue.pop_front() {
            for (nx, nz) in [(lx + 1, lz), (lx - 1, lz), (lx, lz + 1), (lx, lz - 1)] {
                if !(-HALL_HALF..HALL_HALF).contains(&nx) || !(-HALL_HALF..HALL_HALF).contains(&nz)
                {
                    continue;
                }
                let stands = self.column(nx, nz).stands.clone();
                for next in stands {
                    // Up by a step at most, down by any fall, but never into
                    // a stand that is under the ceiling we are standing on.
                    if next - feet > STEP + 1e-6 {
                        continue;
                    }
                    let lower = stands_between(self.column(nx, nz), next, feet);
                    if lower {
                        continue;
                    }
                    if seen.insert((nx, nz, key(next))) {
                        queue.push_back((nx, nz, next));
                    }
                }
            }
        }
        seen
    }
}

/// Returns whether a fall from `from` to `to` would pass through another
/// stand of the column: a hunter lands on the first floor below them.
fn stands_between(column: &Column, to: f64, from: f64) -> bool {
    column
        .stands
        .iter()
        .any(|&feet| feet > to + 1e-6 && feet < from + STEP - 1e-6 && feet <= from + 1e-6)
}

fn reached(set: &FxHashSet<(i32, i32, i32)>, lx: i32, lz: i32, feet: f64) -> bool {
    set.contains(&(lx, lz, (feet * 16.0).round() as i32))
}

/// Propagates block light through the generated blocks with vanilla opacity
/// and face-occlusion rules, independently of chunk loading.
fn block_light(plan: &FloorPlan) -> FxHashMap<(i32, i32, i32), u8> {
    let air = vanilla_blocks::AIR.default_state();
    let state = |lx, lz, layer| plan.block(lx, lz, layer).unwrap_or(air);
    let mut levels = FxHashMap::default();
    let mut queue = VecDeque::new();
    for lx in -HALL_HALF..HALL_HALF {
        for lz in -HALL_HALF..HALL_HALF {
            for layer in MIN_LAYER..=MAX_LAYER {
                let emission = state(lx, lz, layer).get_light_emission();
                if emission > 0 {
                    levels.insert((lx, lz, layer), emission);
                    queue.push_back((lx, lz, layer, emission));
                }
            }
        }
    }
    while let Some((lx, lz, layer, level)) = queue.pop_front() {
        for (dx, dz, dy, direction) in [
            (1, 0, 0, Direction::East),
            (-1, 0, 0, Direction::West),
            (0, 1, 0, Direction::South),
            (0, -1, 0, Direction::North),
            (0, 0, 1, Direction::Up),
            (0, 0, -1, Direction::Down),
        ] {
            let (nx, nz, ny) = (lx + dx, lz + dz, layer + dy);
            if !(-HALL_HALF..HALL_HALF).contains(&nx)
                || !(-HALL_HALF..HALL_HALF).contains(&nz)
                || !(MIN_LAYER..=MAX_LAYER).contains(&ny)
            {
                continue;
            }
            let from = state(lx, lz, layer);
            let to = state(nx, nz, ny);
            let cost = get_light_block_into(from, to, direction, get_light_opacity(to));
            let Some(next) = level.checked_sub(cost).filter(|next| *next > 0) else {
                continue;
            };
            if levels.get(&(nx, nz, ny)).is_some_and(|old| *old >= next) {
                continue;
            }
            levels.insert((nx, nz, ny), next);
            queue.push_back((nx, nz, ny, next));
        }
    }
    levels
}

#[test]
fn every_reachable_stand_is_lit() {
    init_vanilla_registry();
    for (seed, floor) in [(7_u64, 1_u32), (42, 3), (1_337, 24), (99, 37), (7, 10)] {
        let plan = FloorPlan::generate(seed, floor);
        let (ex, ez) = plan.entry();
        let stands = Walk::new(&plan).reachable((ex, ez, 1.0));
        let light = block_light(&plan);
        for &(lx, lz, feet) in &stands {
            let layer = feet.div_euclid(16);
            let level = light.get(&(lx, lz, layer)).copied().unwrap_or(0);
            let district = FloorPlan::district_at(lx, lz)
                .map(|(cell, u, v)| (plan.district(cell).archetype, u, v));
            assert!(
                level >= 7,
                "seed {seed} floor {floor}: stand ({lx}, {lz}, {layer}) in {district:?} has light {level}"
            );
        }
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "every reach check of one floor walk reads best side by side"
)]
fn every_district_gallery_mob_boss_and_exit_is_reachable_from_the_arrival() {
    init_vanilla_registry();
    let mut seen = Vec::new();
    for (seed, floor) in [
        (7_u64, 1_u32),
        (7, 2),
        (42, 3),
        (42, 11),
        (1_337, 24),
        (99, 37),
        (5, 48),
        (3, 6),
        (11, 15),
        (64, 29),
        (7, 10),
        (42, 20),
        (1_337, 30),
        (99, 40),
    ] {
        let plan = FloorPlan::generate(seed, floor);
        for (_, district) in plan.districts() {
            if !seen.contains(&district.archetype) {
                seen.push(district.archetype);
            }
        }
        let (ex, ez) = plan.entry();
        let reached_set = Walk::new(&plan).reachable((ex, ez, 1.0));
        for (cell, district) in plan.districts() {
            let (lx, lz) = FloorPlan::hall_coords(cell, PLAZA_LO + 1, PLAZA_LO + 1);
            assert!(
                reached(&reached_set, lx, lz, 1.0),
                "seed {seed} floor {floor}: the plaza of {:?} at {cell:?} is unreachable",
                district.archetype
            );
            if district.archetype == Archetype::GreatHall {
                for (u, v) in [(3, 40), (76, 40), (40, 3), (40, 76)] {
                    let (lx, lz) = FloorPlan::hall_coords(cell, u, v);
                    assert!(
                        reached(&reached_set, lx, lz, f64::from(GALLERY_LAYER + 1)),
                        "seed {seed} floor {floor}: the gallery at ({u}, {v}) of {cell:?} is unreachable"
                    );
                }
            }
        }
        // A hunter reaches a side of every treasure cache to open it; on a
        // bridge that is the walkway behind the railing line.
        for (cell, district) in plan.districts() {
            let Some((tu, tv)) = district.treasure_spot() else {
                continue;
            };
            let (lx, lz) = FloorPlan::hall_coords(cell, tu, tv);
            assert!(
                [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .into_iter()
                    .any(|(dx, dz)| reached(&reached_set, lx + dx, lz + dz, 1.0)),
                "seed {seed} floor {floor}: the cache of {:?} at {cell:?} is unreachable",
                district.archetype
            );
        }
        let (bx, bz) = plan.boss_spawn();
        assert!(
            Walk::new(&plan)
                .column(bx, bz)
                .stands
                .iter()
                .any(|feet| *feet > 1.0 && *feet < 1.1),
            "seed {seed} floor {floor}: the boss spot is no floor"
        );
        for pack in floor_packs(&plan, seed) {
            for &(lx, lz, layer) in &pack.spots {
                assert!(
                    reached(&reached_set, lx, lz, f64::from(layer)),
                    "seed {seed} floor {floor}: a mob spot at ({lx}, {lz}, {layer}) is unreachable"
                );
            }
        }
        for (gx, gz) in [plan.exit_gate(), plan.home_gate()] {
            assert!(
                reached(&reached_set, gx, gz, 1.0),
                "seed {seed} floor {floor}: the gate at ({gx}, {gz}) is unreachable"
            );
            // The frame runs four blocks along X and five layers up from the
            // floor; the runtime only places it into open air.
            for dx in -1..=2 {
                for layer in 1..=5 {
                    assert!(
                        plan.block(gx + dx, gz, layer).is_none(),
                        "seed {seed} floor {floor}: the gate frame at ({gx}, {gz}) hits a block"
                    );
                }
            }
        }
    }
    // The sample must walk every kind of district.
    for archetype in [
        Archetype::Arrival,
        Archetype::Courtyard,
        Archetype::GreatHall,
        Archetype::Bridge,
        Archetype::Barracks,
        Archetype::Prison,
        Archetype::Throne,
        Archetype::GigaThrone,
    ] {
        assert!(
            seen.contains(&archetype),
            "no sampled floor holds {archetype:?}"
        );
    }
}
