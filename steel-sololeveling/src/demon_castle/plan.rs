//! The layout of one castle floor: a 4×4 grid of districts joined by gates.
//!
//! A floor is planned from the world seed and its number alone, so the world
//! generator and the runtime agree on it without sharing state. The main path
//! is a self-avoiding walk from the arrival hall on the outer ring to the
//! throne room; every other district hangs off it, and a couple of extra
//! gates close loops so the floor is not a single corridor. The throne room
//! keeps exactly one gate, which the runtime seals during the fight.

use std::iter;

use steel_utils::BlockStateId;

use super::districts::{self, Ctx, DOOR_HEIGHT, DOOR_HI, DOOR_LO, WALL};
use super::theme::{CastlePalette, CastleTheme};
use super::{DISTRICT_SIZE, GRID, HALL_HALF};
use crate::seeded::SplitMix;

/// What one district is built as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archetype {
    /// The hall hunters arrive in.
    Arrival,
    /// An open court with a lava fountain, colonnade and planters.
    Courtyard,
    /// A tall nave with a gallery reached by two flights of stairs.
    GreatHall,
    /// A lava chasm crossed by bridges to a central island.
    Bridge,
    /// A warren of bunk rooms walled off in a grid.
    Barracks,
    /// Rows of barred cages along narrow corridors.
    Prison,
    /// The boss arena with the throne and the exit gate.
    Throne,
    /// The round arena of the giga boss of every tenth floor.
    GigaThrone,
}

impl Archetype {
    /// Air layers between the floor and the ceiling.
    #[must_use]
    pub const fn height(self) -> i32 {
        match self {
            Self::Arrival => 14,
            Self::Courtyard => 26,
            Self::GreatHall => 34,
            Self::Bridge => 24,
            Self::Barracks => 18,
            Self::Prison => 16,
            Self::Throne => 30,
            Self::GigaThrone => 36,
        }
    }
}

/// A side of a district; north is towards negative Z.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Towards negative Z.
    North,
    /// Towards positive X.
    East,
    /// Towards positive Z.
    South,
    /// Towards negative X.
    West,
}

impl Side {
    /// Every side.
    pub const ALL: [Self; 4] = [Self::North, Self::East, Self::South, Self::West];

    /// Returns the grid step towards this side.
    #[must_use]
    pub const fn offset(self) -> (i32, i32) {
        match self {
            Self::North => (0, -1),
            Self::East => (1, 0),
            Self::South => (0, 1),
            Self::West => (-1, 0),
        }
    }

    /// Returns the side across.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::East => Self::West,
            Self::South => Self::North,
            Self::West => Self::East,
        }
    }

    const fn bit(self) -> u8 {
        match self {
            Self::North => 1,
            Self::East => 2,
            Self::South => 4,
            Self::West => 8,
        }
    }
}

/// One district of a floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct District {
    /// What the district is built as.
    pub archetype: Archetype,
    doors: u8,
    /// Per-district salt for its cosmetic rolls.
    pub salt: u64,
    /// Whether a treasure cache waits against the district's far wall,
    /// guarded by an elite.
    pub treasure: bool,
}

impl District {
    /// Returns whether the district has a gate on `side`.
    #[must_use]
    pub const fn has_door(self, side: Side) -> bool {
        self.doors & side.bit() != 0
    }

    /// Returns the sides the district has gates on.
    pub fn door_sides(self) -> impl Iterator<Item = Side> {
        Side::ALL
            .into_iter()
            .filter(move |side| self.has_door(*side))
    }

    /// Returns the only gate of a district with one gate, the throne room.
    #[must_use]
    pub fn single_door(self) -> Option<Side> {
        let mut sides = self.door_sides();
        let side = sides.next()?;
        sides.next().is_none().then_some(side)
    }

    /// Returns the column of the district's treasure cache: against the wall
    /// across from its first gate, or another wall without a gate.
    #[must_use]
    pub fn treasure_spot(self) -> Option<(i32, i32)> {
        if !self.treasure {
            return None;
        }
        let first = self.door_sides().next()?;
        let side = iter::once(first.opposite())
            .chain(Side::ALL)
            .find(|side| !self.has_door(*side))?;
        let near = TREASURE_DEPTH;
        let far = DISTRICT_SIZE - 1 - TREASURE_DEPTH;
        let middle = DISTRICT_SIZE / 2 - 1;
        Some(match side {
            Side::North => (middle, near),
            Side::South => (middle, far),
            Side::West => (near, middle),
            Side::East => (far, middle),
        })
    }
}

/// Floors between two giga bosses; their floor numbers are its multiples.
pub const GIGA_EVERY: u32 = 10;

/// Columns between a district's outer wall face and its treasure cache.
pub const TREASURE_DEPTH: i32 = 4;
/// Fewest treasure caches a floor holds.
const MIN_TREASURES: usize = 2;

/// Grid coordinates of one district.
pub type Cell = (usize, usize);

/// The plan of one castle floor.
#[derive(Debug, Clone)]
pub struct FloorPlan {
    /// Floor number.
    pub floor: u32,
    /// Theme of the floor.
    pub theme: CastleTheme,
    palette: CastlePalette,
    cycle: u32,
    districts: [District; GRID * GRID],
    /// District the hunters arrive in.
    pub arrival: Cell,
    /// District of the throne room.
    pub throne: Cell,
    /// Main path from the arrival hall to the throne room.
    pub path: Vec<Cell>,
}

impl FloorPlan {
    /// Plans `floor` of a castle whose world uses `world_seed`.
    #[must_use]
    pub fn generate(world_seed: u64, floor: u32) -> Self {
        let seed = world_seed ^ u64::from(floor).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut rng = SplitMix::new(seed);
        let path = main_path(&mut rng);
        let arrival = path[0];
        let throne = path[path.len() - 1];

        let mut doors = [0_u8; GRID * GRID];
        let mut assigned = [false; GRID * GRID];
        for pair in path.windows(2) {
            connect(&mut doors, pair[0], pair[1]);
        }
        for cell in &path {
            assigned[index(*cell)] = true;
        }
        // Every other district hangs off the ones already reachable; the
        // throne room stays a dead end so its one gate can be sealed.
        loop {
            let candidates: Vec<(Cell, Cell)> = cells()
                .filter(|cell| !assigned[index(*cell)])
                .flat_map(|cell| {
                    neighbours(cell)
                        .filter(|(_, other)| assigned[index(*other)] && *other != throne)
                        .map(move |(_, other)| (cell, other))
                })
                .collect();
            if candidates.is_empty() {
                break;
            }
            let (cell, anchor) = candidates[rng.index(candidates.len())];
            connect(&mut doors, cell, anchor);
            assigned[index(cell)] = true;
        }
        // A couple of loops so the floor is no single corridor.
        for _ in 0..2 {
            let loops: Vec<(Cell, Cell)> = cells()
                .flat_map(|cell| neighbours(cell).map(move |(_, other)| (cell, other)))
                .filter(|(a, b)| {
                    *a != throne
                        && *b != throne
                        && assigned[index(*a)]
                        && assigned[index(*b)]
                        && !connected(&doors, *a, *b)
                })
                .collect();
            if loops.is_empty() {
                break;
            }
            let (a, b) = loops[rng.index(loops.len())];
            connect(&mut doors, a, b);
        }

        let mut districts = [District {
            archetype: Archetype::Courtyard,
            doors: 0,
            salt: 0,
            treasure: false,
        }; GRID * GRID];
        let mut previous = None;
        for cell in path.iter().copied().chain(cells()) {
            let slot = &mut districts[index(cell)];
            if slot.salt != 0 {
                continue;
            }
            let archetype = if cell == arrival {
                Archetype::Arrival
            } else if cell == throne && floor.is_multiple_of(GIGA_EVERY) {
                Archetype::GigaThrone
            } else if cell == throne {
                Archetype::Throne
            } else {
                let mut pick = HALLS[rng.index(HALLS.len())];
                if Some(pick) == previous {
                    pick = HALLS[rng.index(HALLS.len())];
                }
                previous = Some(pick);
                pick
            };
            *slot = District {
                archetype,
                doors: doors[index(cell)],
                salt: rng.next() | 1,
                treasure: false,
            };
        }
        place_treasures(&mut districts, arrival, throne, &mut rng);

        let theme = CastleTheme::of_floor(floor);
        Self {
            floor,
            theme,
            palette: theme.palette(),
            cycle: CastleTheme::cycle(floor),
            districts,
            arrival,
            throne,
            path,
        }
    }

    /// Returns the district at `cell`.
    #[must_use]
    pub const fn district(&self, cell: Cell) -> District {
        self.districts[cell.1 * GRID + cell.0]
    }

    /// Returns every district with its cell.
    pub fn districts(&self) -> impl Iterator<Item = (Cell, District)> + '_ {
        cells().map(|cell| (cell, self.district(cell)))
    }

    /// Returns the block of the hall at `(lx, lz)` and `layer` above the
    /// walking surface, or `None` for air.
    #[must_use]
    pub fn block(&self, lx: i32, lz: i32, layer: i32) -> Option<BlockStateId> {
        let (cell, u, v) = Self::district_at(lx, lz)?;
        let district = self.district(cell);
        let ctx = Ctx {
            district,
            palette: &self.palette,
            height: district.archetype.height(),
            cycle: self.cycle,
        };
        districts::block(&ctx, u, v, layer)
    }

    /// Returns the district under hall coordinates `(lx, lz)` and the
    /// district-local coordinates of the column.
    #[must_use]
    pub fn district_at(lx: i32, lz: i32) -> Option<(Cell, i32, i32)> {
        let x = lx + HALL_HALF;
        let z = lz + HALL_HALF;
        if !(0..2 * HALL_HALF).contains(&x) || !(0..2 * HALL_HALF).contains(&z) {
            return None;
        }
        let cell = (
            usize::try_from(x / DISTRICT_SIZE).ok()?,
            usize::try_from(z / DISTRICT_SIZE).ok()?,
        );
        Some((cell, x % DISTRICT_SIZE, z % DISTRICT_SIZE))
    }

    /// Returns the hall coordinates of district-local `(u, v)` in `cell`.
    #[must_use]
    pub fn hall_coords(cell: Cell, u: i32, v: i32) -> (i32, i32) {
        let ox = i32::try_from(cell.0).unwrap_or(0) * DISTRICT_SIZE - HALL_HALF;
        let oz = i32::try_from(cell.1).unwrap_or(0) * DISTRICT_SIZE - HALL_HALF;
        (ox + u, oz + v)
    }

    /// Returns the hall columns of every treasure cache of the floor.
    pub fn treasures(&self) -> impl Iterator<Item = (Cell, (i32, i32))> + '_ {
        self.districts().filter_map(|(cell, district)| {
            let (u, v) = district.treasure_spot()?;
            Some((cell, Self::hall_coords(cell, u, v)))
        })
    }

    /// Returns the hall column the hunters land on, in the arrival hall.
    #[must_use]
    pub fn entry(&self) -> (i32, i32) {
        Self::hall_coords(self.arrival, DISTRICT_SIZE / 2, DISTRICT_SIZE / 2)
    }

    /// Returns the side the throne room opens towards.
    #[must_use]
    pub fn throne_door(&self) -> Side {
        self.district(self.throne)
            .single_door()
            .unwrap_or(Side::North)
    }

    /// Returns the hall column the boss appears on.
    #[must_use]
    pub fn boss_spawn(&self) -> (i32, i32) {
        self.throne_spot(districts::throne::BOSS_SPOT)
    }

    /// Returns the hall column of the gate up to the next floor; its frame
    /// is placed there once the boss falls.
    #[must_use]
    pub fn exit_gate(&self) -> (i32, i32) {
        self.throne_spot(districts::throne::EXIT_SPOT)
    }

    /// Returns the hall column of the gate home, opened beside the gate up.
    #[must_use]
    pub fn home_gate(&self) -> (i32, i32) {
        self.throne_spot(districts::throne::HOME_SPOT)
    }

    fn throne_spot(&self, (cu, cv): (i32, i32)) -> (i32, i32) {
        let (u, v) = districts::throne::from_canonical(self.throne_door(), cu, cv);
        Self::hall_coords(self.throne, u, v)
    }

    /// Returns the hall columns and layers of the throne room's gate opening,
    /// which the runtime bars while the boss fight lasts.
    #[must_use]
    pub fn throne_seal(&self) -> Vec<(i32, i32, i32)> {
        let side = self.throne_door();
        let mut cells = Vec::new();
        for along in DOOR_LO..=DOOR_HI {
            let (u, v) = match side {
                Side::North => (along, WALL - 1),
                Side::South => (along, DISTRICT_SIZE - WALL),
                Side::West => (WALL - 1, along),
                Side::East => (DISTRICT_SIZE - WALL, along),
            };
            let (lx, lz) = Self::hall_coords(self.throne, u, v);
            for layer in 1..=DOOR_HEIGHT {
                cells.push((lx, lz, layer));
            }
        }
        cells
    }
}

/// Archetypes of the districts between the arrival hall and the throne room.
const HALLS: [Archetype; 5] = [
    Archetype::Courtyard,
    Archetype::GreatHall,
    Archetype::Bridge,
    Archetype::Barracks,
    Archetype::Prison,
];

/// Hides the treasure caches: in every dead end, and in random districts
/// until the floor holds a few.
fn place_treasures(
    districts: &mut [District; GRID * GRID],
    arrival: Cell,
    throne: Cell,
    rng: &mut SplitMix,
) {
    let halls: Vec<usize> = cells()
        .filter(|cell| *cell != arrival && *cell != throne)
        .map(index)
        .filter(|at| districts[*at].doors != 0)
        .collect();
    for &at in &halls {
        if districts[at].single_door().is_some() {
            districts[at].treasure = true;
        }
    }
    while halls.iter().filter(|at| districts[**at].treasure).count() < MIN_TREASURES {
        let open: Vec<usize> = halls
            .iter()
            .copied()
            .filter(|at| !districts[*at].treasure)
            .collect();
        if open.is_empty() {
            break;
        }
        districts[open[rng.index(open.len())]].treasure = true;
    }
}

/// Walks the main path: a self-avoiding walk from a cell on the outer ring.
fn main_path(rng: &mut SplitMix) -> Vec<Cell> {
    let outer: Vec<Cell> = cells()
        .filter(|(x, z)| *x == 0 || *z == 0 || *x == GRID - 1 || *z == GRID - 1)
        .collect();
    let wanted = usize::try_from(rng.range(7, 10)).unwrap_or(7);
    let mut best: Vec<Cell> = Vec::new();
    for _ in 0..96 {
        let mut path = vec![outer[rng.index(outer.len())]];
        while path.len() < wanted {
            let last = path[path.len() - 1];
            let options: Vec<Cell> = neighbours(last)
                .map(|(_, cell)| cell)
                .filter(|cell| !path.contains(cell))
                .collect();
            if options.is_empty() {
                break;
            }
            path.push(options[rng.index(options.len())]);
        }
        if path.len() > best.len() {
            best = path;
        }
        if best.len() == wanted {
            break;
        }
    }
    best
}

fn cells() -> impl Iterator<Item = Cell> {
    (0..GRID).flat_map(|z| (0..GRID).map(move |x| (x, z)))
}

const fn index(cell: Cell) -> usize {
    cell.1 * GRID + cell.0
}

fn neighbours(cell: Cell) -> impl Iterator<Item = (Side, Cell)> {
    Side::ALL.into_iter().filter_map(move |side| {
        let (dx, dz) = side.offset();
        let x = usize::try_from(i32::try_from(cell.0).ok()? + dx).ok()?;
        let z = usize::try_from(i32::try_from(cell.1).ok()? + dz).ok()?;
        (x < GRID && z < GRID).then_some((side, (x, z)))
    })
}

fn side_towards(from: Cell, to: Cell) -> Option<Side> {
    neighbours(from).find_map(|(side, cell)| (cell == to).then_some(side))
}

fn connect(doors: &mut [u8; GRID * GRID], a: Cell, b: Cell) {
    let Some(side) = side_towards(a, b) else {
        return;
    };
    doors[index(a)] |= side.bit();
    doors[index(b)] |= side.opposite().bit();
}

fn connected(doors: &[u8; GRID * GRID], a: Cell, b: Cell) -> bool {
    side_towards(a, b).is_some_and(|side| doors[index(a)] & side.bit() != 0)
}

#[cfg(test)]
mod tests {
    use super::{Archetype, FloorPlan, GRID};

    #[test]
    fn every_floor_plan_is_one_connected_castle_with_a_dead_end_throne() {
        for seed in [7_u64, 42, 1_337] {
            for floor in 1_u32..=60 {
                let plan = FloorPlan::generate(seed, floor);
                assert!(
                    plan.path.len() >= 6,
                    "floor {floor}: the main path is too short"
                );
                assert_eq!(plan.district(plan.arrival).archetype, Archetype::Arrival);
                let throne = if floor % 10 == 0 {
                    Archetype::GigaThrone
                } else {
                    Archetype::Throne
                };
                assert_eq!(
                    plan.district(plan.throne).archetype,
                    throne,
                    "floor {floor}"
                );
                assert!(
                    plan.district(plan.throne).single_door().is_some(),
                    "seed {seed} floor {floor}: the throne room must keep exactly one gate"
                );
                // Gates are mutual and every district is reachable from the
                // arrival hall through them.
                let mut seen = vec![plan.arrival];
                let mut queue = vec![plan.arrival];
                while let Some(cell) = queue.pop() {
                    for side in plan.district(cell).door_sides() {
                        let (dx, dz) = side.offset();
                        let next = ((cell.0 as i32 + dx) as usize, (cell.1 as i32 + dz) as usize);
                        assert!(next.0 < GRID && next.1 < GRID, "a gate opens onto the void");
                        assert!(
                            plan.district(next).has_door(side.opposite()),
                            "seed {seed} floor {floor}: a one-sided gate at {cell:?} {side:?}"
                        );
                        if !seen.contains(&next) {
                            seen.push(next);
                            queue.push(next);
                        }
                    }
                }
                assert_eq!(
                    seen.len(),
                    GRID * GRID,
                    "seed {seed} floor {floor}: a district is cut off"
                );
            }
        }
    }

    #[test]
    fn plans_are_stable_for_a_seed_and_differ_between_floors() {
        let a = FloorPlan::generate(9, 4);
        let b = FloorPlan::generate(9, 4);
        assert_eq!(a.path, b.path);
        assert!(
            (1_u32..=20).any(|floor| FloorPlan::generate(9, floor).path != a.path),
            "floors must not all share one layout"
        );
    }
}
