//! Procedural dungeon layouts.
//!
//! A dungeon is generated from a seed alone, so the same seed always rebuilds
//! the same blocks: that is how a reused instance region erases the dungeon a
//! previous run left there. Positions are offsets from the region spawn, with
//! `y = 0` the first air block above the floor.
//!
//! Rooms sit on a coarse grid. The main path is a random self-avoiding walk
//! from the entrance cell, so it turns instead of running in a line; side
//! chambers branch off it. Neighbouring rooms are joined by corridors whose
//! doors are offset from each other, so most corridors bend. The shell (floor,
//! walls, ceiling) is derived from the carved air, which keeps rooms and
//! corridors airtight wherever they meet.

use rustc_hash::{FxHashMap, FxHashSet};
use steel_registry::blocks::BlockRef;
use steel_registry::vanilla_blocks;

use crate::rank::HunterRank;

/// Grid cells from the entrance cell to the grid edge.
const GRID_RADIUS: i32 = 3;
/// Blocks between the centres of two neighbouring grid cells.
const CELL: i32 = 24;
/// Air height of a corridor.
const CORRIDOR_HEIGHT: u8 = 3;
/// Blocks between two corridor ceiling lights.
const CORRIDOR_LIGHT_SPACING: i32 = 5;
/// Blocks between two room ceiling lights.
const ROOM_LIGHT_SPACING: i32 = 4;
/// Most spawn spots remembered per room.
const SPAWN_SPOTS: usize = 24;

/// A small deterministic generator, so a seed keeps meaning the same dungeon
/// regardless of the `rand` crate version.
struct SplitMix(u64);

impl SplitMix {
    const fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    /// Returns a value in `low..=high`.
    fn range(&mut self, low: i32, high: i32) -> i32 {
        let span = u64::try_from(high - low + 1).unwrap_or(1).max(1);
        low + i32::try_from(self.next() % span).unwrap_or(0)
    }

    const fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

/// Hashes a position into a stable pseudo-random value.
const fn hash(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    let mut value = seed
        ^ (x as u64).wrapping_mul(73_856_093)
        ^ (y as u64).wrapping_mul(83_492_791)
        ^ (z as u64).wrapping_mul(2_654_435_761);
    value = (value ^ (value >> 31)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^ (value >> 29)
}

/// The look of a dungeon, named after the biome it imitates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Stone-brick crypt.
    Crypt,
    /// Ice cavern.
    Ice,
    /// Nether fortress.
    Nether,
    /// Desert temple.
    Desert,
    /// Overgrown jungle ruins.
    Jungle,
    /// Deep dark ruins.
    DeepDark,
    /// End city ruins.
    End,
}

/// Every theme, in roll order.
pub const THEMES: [Theme; 7] = [
    Theme::Crypt,
    Theme::Ice,
    Theme::Nether,
    Theme::Desert,
    Theme::Jungle,
    Theme::DeepDark,
    Theme::End,
];

/// Blocks one theme builds with.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    floor: BlockRef,
    floor_alt: BlockRef,
    wall: BlockRef,
    wall_alt: BlockRef,
    pillar: BlockRef,
    ceiling: BlockRef,
    light: BlockRef,
}

impl Theme {
    /// Returns the Russian name shown when a hunter enters.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Crypt => "Древняя крипта",
            Self::Ice => "Ледяная пещера",
            Self::Nether => "Адская крепость",
            Self::Desert => "Пустынный храм",
            Self::Jungle => "Руины джунглей",
            Self::DeepDark => "Глубокая тьма",
            Self::End => "Руины Края",
        }
    }

    /// Returns the blocks of this theme.
    #[must_use]
    pub fn palette(self) -> Palette {
        match self {
            Self::Crypt => Palette {
                floor: &vanilla_blocks::POLISHED_ANDESITE,
                floor_alt: &vanilla_blocks::CRACKED_STONE_BRICKS,
                wall: &vanilla_blocks::STONE_BRICKS,
                wall_alt: &vanilla_blocks::MOSSY_STONE_BRICKS,
                pillar: &vanilla_blocks::CHISELED_STONE_BRICKS,
                ceiling: &vanilla_blocks::STONE_BRICKS,
                light: &vanilla_blocks::GLOWSTONE,
            },
            Self::Ice => Palette {
                floor: &vanilla_blocks::PACKED_ICE,
                floor_alt: &vanilla_blocks::SNOW_BLOCK,
                wall: &vanilla_blocks::PACKED_ICE,
                wall_alt: &vanilla_blocks::BLUE_ICE,
                pillar: &vanilla_blocks::BLUE_ICE,
                ceiling: &vanilla_blocks::SNOW_BLOCK,
                light: &vanilla_blocks::SEA_LANTERN,
            },
            Self::Nether => Palette {
                floor: &vanilla_blocks::NETHER_BRICKS,
                floor_alt: &vanilla_blocks::BLACKSTONE,
                wall: &vanilla_blocks::NETHER_BRICKS,
                wall_alt: &vanilla_blocks::RED_NETHER_BRICKS,
                pillar: &vanilla_blocks::POLISHED_BLACKSTONE_BRICKS,
                ceiling: &vanilla_blocks::NETHER_BRICKS,
                light: &vanilla_blocks::SHROOMLIGHT,
            },
            Self::Desert => Palette {
                floor: &vanilla_blocks::SMOOTH_SANDSTONE,
                floor_alt: &vanilla_blocks::CUT_SANDSTONE,
                wall: &vanilla_blocks::SANDSTONE,
                wall_alt: &vanilla_blocks::CUT_SANDSTONE,
                pillar: &vanilla_blocks::CHISELED_SANDSTONE,
                ceiling: &vanilla_blocks::SMOOTH_SANDSTONE,
                light: &vanilla_blocks::OCHRE_FROGLIGHT,
            },
            Self::Jungle => Palette {
                floor: &vanilla_blocks::MOSS_BLOCK,
                floor_alt: &vanilla_blocks::COBBLESTONE,
                wall: &vanilla_blocks::MOSSY_COBBLESTONE,
                wall_alt: &vanilla_blocks::COBBLESTONE,
                pillar: &vanilla_blocks::MOSSY_STONE_BRICKS,
                ceiling: &vanilla_blocks::MOSSY_COBBLESTONE,
                light: &vanilla_blocks::VERDANT_FROGLIGHT,
            },
            Self::DeepDark => Palette {
                floor: &vanilla_blocks::DEEPSLATE_TILES,
                floor_alt: &vanilla_blocks::SCULK,
                wall: &vanilla_blocks::DEEPSLATE_BRICKS,
                wall_alt: &vanilla_blocks::CRACKED_DEEPSLATE_TILES,
                pillar: &vanilla_blocks::POLISHED_DEEPSLATE,
                ceiling: &vanilla_blocks::DEEPSLATE_TILES,
                light: &vanilla_blocks::SEA_LANTERN,
            },
            Self::End => Palette {
                floor: &vanilla_blocks::END_STONE_BRICKS,
                floor_alt: &vanilla_blocks::PURPUR_BLOCK,
                wall: &vanilla_blocks::END_STONE_BRICKS,
                wall_alt: &vanilla_blocks::PURPUR_BLOCK,
                pillar: &vanilla_blocks::PURPUR_PILLAR,
                ceiling: &vanilla_blocks::PURPUR_BLOCK,
                light: &vanilla_blocks::PEARLESCENT_FROGLIGHT,
            },
        }
    }
}

/// What role one shell block plays; the theme decides the concrete block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    /// Walkable floor.
    Floor,
    /// Floor variant mixed into the floor.
    FloorAlt,
    /// Wall.
    Wall,
    /// Wall variant mixed into the wall.
    WallAlt,
    /// A free-standing pillar inside a room.
    Pillar,
    /// Ceiling.
    Ceiling,
    /// A light source set into the ceiling.
    Light,
    /// Mineable crystal ore in a wall.
    Ore,
}

impl Palette {
    /// Returns the block a `part` is built from; ore is not themed.
    #[must_use]
    pub const fn block(&self, part: Part, ore: BlockRef) -> BlockRef {
        match part {
            Part::Floor => self.floor,
            Part::FloorAlt => self.floor_alt,
            Part::Wall => self.wall,
            Part::WallAlt => self.wall_alt,
            Part::Pillar => self.pillar,
            Part::Ceiling => self.ceiling,
            Part::Light => self.light,
            Part::Ore => ore,
        }
    }
}

/// The outline a room is carved in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A plain rectangle.
    Hall,
    /// A rectangle with rounded corners.
    Rotunda,
    /// Two crossing naves.
    Cross,
    /// A rectangle broken up by pillars.
    Colonnade,
    /// An uneven natural cave.
    Cavern,
}

const SHAPES: [Shape; 5] = [
    Shape::Hall,
    Shape::Rotunda,
    Shape::Cross,
    Shape::Colonnade,
    Shape::Cavern,
];

/// What a room is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomRole {
    /// Where the hunters arrive.
    Entrance,
    /// A room that spawns one wave.
    Combat,
    /// The final room with the boss and, later, the exit.
    Boss,
    /// An optional side chamber off the main path with richer ore.
    Chamber,
}

/// One generated room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    /// What the room is for.
    pub role: RoomRole,
    /// The room's outline.
    pub shape: Shape,
    /// Centre column, as an offset from the region spawn.
    pub center: (i32, i32),
    /// Half extents of the bounding rectangle along x and z.
    pub half: (i32, i32),
    /// Air height above the floor.
    pub height: u8,
    /// Free floor columns mobs may spawn on, spread across the room.
    pub spawns: Vec<(i32, i32)>,
}

/// One block of the built dungeon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// Offset from the region spawn.
    pub offset: (i32, i32, i32),
    /// The block's role.
    pub part: Part,
}

/// A fully generated dungeon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blueprint {
    /// Seed this blueprint was generated from.
    pub seed: u64,
    /// The look of the dungeon.
    pub theme: Theme,
    /// Rooms: entrance, combat rooms in order, boss, then side chambers.
    pub rooms: Vec<Room>,
    /// Every shell block.
    pub blocks: Vec<Placement>,
    /// Lowest and highest offsets the shell touches.
    pub bounds: ((i32, i32, i32), (i32, i32, i32)),
}

#[derive(Debug, Clone, Copy)]
struct Column {
    height: u8,
    /// Index of the room the column belongs to; corridors have none.
    room: Option<usize>,
}

const DIRECTIONS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

impl Blueprint {
    /// Generates the dungeon of `seed` with `combat_rooms` wave rooms.
    #[must_use]
    pub fn generate(seed: u64, combat_rooms: u8) -> Self {
        let mut rng = SplitMix(seed);
        let theme = THEMES[usize::try_from(rng.next() % THEMES.len() as u64).unwrap_or(0)];
        Self::generate_with_theme(seed, combat_rooms, theme)
    }

    /// Generates the dungeon of `seed` with a fixed `theme`.
    #[must_use]
    pub fn generate_with_theme(seed: u64, combat_rooms: u8, theme: Theme) -> Self {
        let mut rng = SplitMix(seed);
        let path = main_path(&mut rng, usize::from(combat_rooms) + 2);
        let mut cells: Vec<((i32, i32), RoomRole)> = path
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let role = if index == 0 {
                    RoomRole::Entrance
                } else if index + 1 == path.len() {
                    RoomRole::Boss
                } else {
                    RoomRole::Combat
                };
                (*cell, role)
            })
            .collect();
        let mut links: Vec<(usize, usize)> =
            (1..path.len()).map(|index| (index - 1, index)).collect();
        add_chambers(&mut rng, &mut cells, &mut links);

        let rooms: Vec<Room> = cells
            .iter()
            .map(|(cell, role)| room_for(&mut rng, *cell, *role))
            .collect();

        let mut air: FxHashMap<(i32, i32), Column> = FxHashMap::default();
        for (index, room) in rooms.iter().enumerate() {
            carve_room(&mut air, seed, index, room);
        }
        let mut lights: FxHashSet<(i32, i32)> = FxHashSet::default();
        for (from, to) in &links {
            carve_corridor(&mut rng, &mut air, &mut lights, &rooms[*from], &rooms[*to]);
        }
        for room in &rooms {
            light_room(&air, &mut lights, room);
        }

        let blocks = shell(seed, &air, &lights);
        let mut rooms = rooms;
        for (index, room) in rooms.iter_mut().enumerate() {
            room.spawns = spawn_spots(seed, &air, index, room);
        }
        let bounds = bounds_of(&blocks);
        Self {
            seed,
            theme,
            rooms,
            blocks,
            bounds,
        }
    }

    /// Returns the dungeon's crystal-vein positions in a seed-stable order.
    #[must_use]
    pub fn crystal_ore_positions(&self, count: usize) -> Vec<(i32, i32, i32)> {
        let mut positions = self
            .blocks
            .iter()
            .filter(|placement| placement.part == Part::Ore)
            .map(|placement| placement.offset)
            .collect::<Vec<_>>();
        positions.sort_by_key(|(x, y, z)| {
            (
                hash(self.seed ^ 0xC2B2_AE35_87B2_1F63, *x, *y, *z),
                (*x, *y, *z),
            )
        });
        positions.truncate(count.min(HunterRank::S.crystal_vein_count()));
        positions
    }

    /// Returns the entrance room.
    #[must_use]
    pub fn entrance(&self) -> &Room {
        &self.rooms[0]
    }

    /// Returns the `index`-th combat room, counting from 1.
    #[must_use]
    pub fn combat_room(&self, index: u8) -> Option<&Room> {
        self.rooms
            .iter()
            .filter(|room| room.role == RoomRole::Combat)
            .nth(usize::from(index).checked_sub(1)?)
    }

    /// Returns the boss room.
    #[must_use]
    pub fn boss_room(&self) -> &Room {
        self.rooms
            .iter()
            .find(|room| room.role == RoomRole::Boss)
            .unwrap_or(&self.rooms[0])
    }
}

/// Walks a self-avoiding path of `length` cells from the entrance cell.
fn main_path(rng: &mut SplitMix, length: usize) -> Vec<(i32, i32)> {
    for _ in 0..64 {
        let mut path = vec![(0, 0)];
        let mut heading = DIRECTIONS[usize::try_from(rng.next() % 4).unwrap_or(0)];
        while path.len() < length {
            let (x, z) = *path.last().unwrap_or(&(0, 0));
            let mut options: Vec<(i32, i32)> = DIRECTIONS
                .into_iter()
                .filter(|(dx, dz)| {
                    let next = (x + dx, z + dz);
                    in_grid(next) && !path.contains(&next)
                })
                .collect();
            if options.is_empty() {
                break;
            }
            // Keep the heading now and then so long runs alternate with turns.
            if options.contains(&heading) && rng.chance(35) {
                options = vec![heading];
            }
            heading = options[usize::try_from(rng.next()).unwrap_or(0) % options.len()];
            path.push((x + heading.0, z + heading.1));
        }
        if path.len() == length {
            return path;
        }
    }
    // A snake always fits the grid; it is only a guard for exhausted retries.
    let width = GRID_RADIUS * 2 + 1;
    (0..length)
        .map(|index| {
            let index = i32::try_from(index).unwrap_or(0);
            let row = index / width;
            let column = if row % 2 == 0 {
                index % width
            } else {
                width - 1 - index % width
            };
            (column - GRID_RADIUS, row)
        })
        .collect()
}

const fn in_grid((x, z): (i32, i32)) -> bool {
    x.abs() <= GRID_RADIUS && z.abs() <= GRID_RADIUS
}

/// Branches up to three side chambers off combat rooms into free cells.
fn add_chambers(
    rng: &mut SplitMix,
    cells: &mut Vec<((i32, i32), RoomRole)>,
    links: &mut Vec<(usize, usize)>,
) {
    let wanted = rng.range(1, 3);
    for _ in 0..wanted {
        let anchors: Vec<usize> = cells
            .iter()
            .enumerate()
            .filter(|(_, (_, role))| matches!(role, RoomRole::Combat | RoomRole::Chamber))
            .map(|(index, _)| index)
            .collect();
        if anchors.is_empty() {
            return;
        }
        let anchor = anchors[usize::try_from(rng.next()).unwrap_or(0) % anchors.len()];
        let (x, z) = cells[anchor].0;
        let free: Vec<(i32, i32)> = DIRECTIONS
            .into_iter()
            .map(|(dx, dz)| (x + dx, z + dz))
            .filter(|cell| in_grid(*cell) && cells.iter().all(|(used, _)| used != cell))
            .collect();
        if free.is_empty() {
            continue;
        }
        let cell = free[usize::try_from(rng.next()).unwrap_or(0) % free.len()];
        cells.push((cell, RoomRole::Chamber));
        links.push((anchor, cells.len() - 1));
    }
}

fn room_for(rng: &mut SplitMix, (x, z): (i32, i32), role: RoomRole) -> Room {
    let (shape, half, height) = match role {
        // The entrance is a plain hall so the arrival column is always free.
        RoomRole::Entrance => (Shape::Hall, (rng.range(4, 5), rng.range(4, 5)), 5),
        // The boss room must fit the boss and, later, the exit frame.
        RoomRole::Boss => (
            if rng.chance(50) {
                Shape::Rotunda
            } else {
                Shape::Colonnade
            },
            (rng.range(7, 8), rng.range(7, 8)),
            u8::try_from(rng.range(7, 9)).unwrap_or(7),
        ),
        RoomRole::Combat => (
            SHAPES[usize::try_from(rng.next()).unwrap_or(0) % SHAPES.len()],
            (rng.range(5, 8), rng.range(5, 8)),
            u8::try_from(rng.range(5, 8)).unwrap_or(5),
        ),
        RoomRole::Chamber => (
            if rng.chance(50) {
                Shape::Cavern
            } else {
                Shape::Rotunda
            },
            (rng.range(3, 5), rng.range(3, 5)),
            u8::try_from(rng.range(4, 6)).unwrap_or(4),
        ),
    };
    Room {
        role,
        shape,
        center: (x * CELL, z * CELL),
        half,
        height,
        spawns: Vec::new(),
    }
}

/// Returns whether the room's outline contains the local offset `(u, v)`.
fn shape_contains(seed: u64, room: &Room, u: i32, v: i32) -> bool {
    let (hx, hz) = room.half;
    if u.abs() > hx || v.abs() > hz {
        return false;
    }
    match room.shape {
        Shape::Hall => true,
        Shape::Rotunda => {
            let radius = 3;
            let du = u.abs() - (hx - radius);
            let dv = v.abs() - (hz - radius);
            du <= 0 || dv <= 0 || du * du + dv * dv <= radius * radius
        }
        Shape::Cross => u.abs() <= hx / 2 + 1 || v.abs() <= hz / 2 + 1,
        Shape::Colonnade => {
            let pillar = u.abs() < hx - 1
                && v.abs() < hz - 1
                && u.rem_euclid(4) == 2
                && v.rem_euclid(4) == 2;
            !pillar
        }
        Shape::Cavern => {
            let (cx, cz) = room.center;
            let jitter_u = i32::try_from(hash(seed, cx + v, 7, cz) % 3).unwrap_or(0);
            let jitter_v = i32::try_from(hash(seed, cx, 11, cz + u) % 3).unwrap_or(0);
            u.abs() <= hx - jitter_u && v.abs() <= hz - jitter_v
        }
    }
}

fn carve_room(air: &mut FxHashMap<(i32, i32), Column>, seed: u64, index: usize, room: &Room) {
    let (hx, hz) = room.half;
    let (cx, cz) = room.center;
    for u in -hx..=hx {
        for v in -hz..=hz {
            if shape_contains(seed, room, u, v) {
                air.insert(
                    (cx + u, cz + v),
                    Column {
                        height: room.height,
                        room: Some(index),
                    },
                );
            }
        }
    }
}

/// Carves a corridor column, never lowering a room that is already there.
fn carve_corridor_column(air: &mut FxHashMap<(i32, i32), Column>, column: (i32, i32)) {
    air.entry(column).or_insert(Column {
        height: CORRIDOR_HEIGHT,
        room: None,
    });
}

/// A room seen from a corridor: its centre and half extent along the
/// corridor, and its centre and half extent across it.
struct Axes {
    along: i32,
    reach: i32,
    across: i32,
    width: i32,
}

impl Axes {
    const fn of(room: &Room, along_x: bool) -> Self {
        if along_x {
            Self {
                along: room.center.0,
                reach: room.half.0,
                across: room.center.1,
                width: room.half.1,
            }
        } else {
            Self {
                along: room.center.1,
                reach: room.half.1,
                across: room.center.0,
                width: room.half.0,
            }
        }
    }

    /// Picks a door position across the corridor, near the wall's middle.
    fn door(&self, rng: &mut SplitMix) -> i32 {
        let spread = (self.width / 2 - 1).max(0);
        self.across + rng.range(-spread, spread)
    }
}

/// Joins two rooms in neighbouring cells with a three-wide corridor.
///
/// Each door sits at its own offset along its wall, so the corridor runs out
/// of one room, steps sideways halfway, and enters the other room. The
/// corridor also reaches a few blocks into both rooms, so an uneven wall or a
/// pillar can never seal a doorway.
fn carve_corridor(
    rng: &mut SplitMix,
    air: &mut FxHashMap<(i32, i32), Column>,
    lights: &mut FxHashSet<(i32, i32)>,
    from: &Room,
    to: &Room,
) {
    let along_x = from.center.0 != to.center.0;
    // The corridor runs along one axis and steps sideways across the other.
    let from_axes = Axes::of(from, along_x);
    let to_axes = Axes::of(to, along_x);
    let sign = (to_axes.along - from_axes.along).signum();
    let door_from = from_axes.door(rng);
    let door_to = to_axes.door(rng);
    let start = from_axes.along + sign * (from_axes.reach - 3);
    let end = to_axes.along - sign * (to_axes.reach - 3);
    let middle = i32::midpoint(start, end);

    let mut path: Vec<(i32, i32)> = Vec::new();
    let mut a = start;
    while a != middle {
        path.push((a, door_from));
        a += sign;
    }
    let step = (door_to - door_from).signum();
    let mut b = door_from;
    while b != door_to {
        path.push((middle, b));
        b += step;
    }
    while a != end + sign {
        path.push((a, door_to));
        a += sign;
    }

    for (index, (a, b)) in path.iter().enumerate() {
        for side in -1..=1 {
            for along in -1..=1 {
                let column = if along_x {
                    (a + along, b + side)
                } else {
                    (b + side, a + along)
                };
                carve_corridor_column(air, column);
            }
        }
        let center = if along_x { (*a, *b) } else { (*b, *a) };
        let index = i32::try_from(index).unwrap_or(0);
        if index % CORRIDOR_LIGHT_SPACING == CORRIDOR_LIGHT_SPACING / 2
            && air.get(&center).is_some_and(|column| column.room.is_none())
        {
            lights.insert(center);
        }
    }
}

/// Puts a grid of ceiling lights over a room.
fn light_room(
    air: &FxHashMap<(i32, i32), Column>,
    lights: &mut FxHashSet<(i32, i32)>,
    room: &Room,
) {
    let (hx, hz) = room.half;
    let (cx, cz) = room.center;
    for u in -hx..=hx {
        for v in -hz..=hz {
            let column = (cx + u, cz + v);
            if u.rem_euclid(ROOM_LIGHT_SPACING) == 0
                && v.rem_euclid(ROOM_LIGHT_SPACING) == 0
                && air.contains_key(&column)
            {
                lights.insert(column);
            }
        }
    }
}

/// Wraps the carved air in floor, walls and ceiling.
fn shell(
    seed: u64,
    air: &FxHashMap<(i32, i32), Column>,
    lights: &FxHashSet<(i32, i32)>,
) -> Vec<Placement> {
    let mut solid: FxHashMap<(i32, i32, i32), Part> = FxHashMap::default();
    let mut ore_candidates = FxHashSet::default();
    let is_air = |x: i32, y: i32, z: i32| {
        air.get(&(x, z))
            .is_some_and(|column| y >= 0 && y < i32::from(column.height))
    };
    let mut ordered: Vec<(&(i32, i32), &Column)> = air.iter().collect();
    ordered.sort_by_key(|(column, _)| **column);
    for ((x, z), column) in ordered {
        let (x, z) = (*x, *z);
        let height = i32::from(column.height);
        let floor = if hash(seed, x, -1, z).is_multiple_of(5) {
            Part::FloorAlt
        } else {
            Part::Floor
        };
        solid.insert((x, -1, z), floor);
        let ceiling = if lights.contains(&(x, z)) {
            Part::Light
        } else {
            Part::Ceiling
        };
        solid.entry((x, height, z)).or_insert(ceiling);
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let (nx, nz) = (x + dx, z + dz);
                // A solid neighbour is walled from the floor level up; an air
                // neighbour only above its own ceiling.
                let neighbour_top = air.get(&(nx, nz)).map_or(-2, |n| i32::from(n.height));
                for y in -1..=height {
                    if y <= neighbour_top || is_air(nx, y, nz) {
                        continue;
                    }
                    if column.room.is_some() && y >= 0 && y < height {
                        ore_candidates.insert((nx, y, nz));
                    }
                    let part = if hash(seed ^ 0x5A5A, nx, y, nz).is_multiple_of(4) {
                        Part::WallAlt
                    } else {
                        Part::Wall
                    };
                    solid.entry((nx, y, nz)).or_insert(part);
                }
            }
        }
    }
    // A wall column with air on all four sides stands inside a room: a pillar.
    for ((x, y, z), part) in &mut solid {
        if matches!(part, Part::Wall | Part::WallAlt)
            && *y >= 0
            && !air.contains_key(&(*x, *z))
            && DIRECTIONS
                .iter()
                .all(|(dx, dz)| air.contains_key(&(x + dx, z + dz)))
        {
            *part = Part::Pillar;
        }
    }
    let mut ore_candidates = ore_candidates
        .into_iter()
        .filter(|position| {
            solid
                .get(position)
                .is_some_and(|part| matches!(part, Part::Wall | Part::WallAlt))
        })
        .collect::<Vec<_>>();
    ore_candidates
        .sort_by_key(|(x, y, z)| (hash(seed ^ 0xC2B2_AE35_87B2_1F63, *x, *y, *z), (*x, *y, *z)));
    for position in ore_candidates
        .into_iter()
        .take(HunterRank::S.crystal_vein_count())
    {
        solid.insert(position, Part::Ore);
    }
    let mut blocks: Vec<Placement> = solid
        .into_iter()
        .map(|(offset, part)| Placement { offset, part })
        .collect();
    blocks.sort_by_key(|placement| placement.offset);
    blocks
}

/// Picks up to [`SPAWN_SPOTS`] free floor columns spread over a room.
fn spawn_spots(
    seed: u64,
    air: &FxHashMap<(i32, i32), Column>,
    index: usize,
    room: &Room,
) -> Vec<(i32, i32)> {
    let (hx, hz) = room.half;
    let (cx, cz) = room.center;
    let mut spots: Vec<(i32, i32)> = Vec::new();
    for u in -hx..=hx {
        for v in -hz..=hz {
            let column = (cx + u, cz + v);
            // A spot needs open floor around it, away from walls and pillars.
            let open = (-1..=1).all(|dx| {
                (-1..=1).all(|dz| {
                    air.get(&(column.0 + dx, column.1 + dz))
                        .is_some_and(|c| c.room == Some(index))
                })
            });
            if open {
                spots.push(column);
            }
        }
    }
    spots.sort_by_key(|(x, z)| hash(seed, *x, 3, *z));
    spots.truncate(SPAWN_SPOTS);
    spots
}

fn bounds_of(blocks: &[Placement]) -> ((i32, i32, i32), (i32, i32, i32)) {
    let mut min = (i32::MAX, i32::MAX, i32::MAX);
    let mut max = (i32::MIN, i32::MIN, i32::MIN);
    for Placement {
        offset: (x, y, z), ..
    } in blocks
    {
        min = (min.0.min(*x), min.1.min(*y), min.2.min(*z));
        max = (max.0.max(*x), max.1.max(*y), max.2.max(*z));
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use rustc_hash::FxHashSet;

    use super::{Blueprint, CELL, GRID_RADIUS, Part, RoomRole, THEMES, Theme};
    use crate::rank::HunterRank;

    fn air_columns(blueprint: &Blueprint) -> FxHashSet<(i32, i32)> {
        // The floor marks every walkable column.
        blueprint
            .blocks
            .iter()
            .filter(|block| {
                block.offset.1 == -1 && matches!(block.part, Part::Floor | Part::FloorAlt)
            })
            .map(|block| (block.offset.0, block.offset.2))
            .collect()
    }

    #[test]
    fn the_same_seed_rebuilds_the_same_dungeon() {
        assert_eq!(Blueprint::generate(42, 4), Blueprint::generate(42, 4));
        assert_ne!(
            Blueprint::generate(42, 4).blocks,
            Blueprint::generate(43, 4).blocks
        );
    }

    #[test]
    fn every_layout_has_nine_ranked_veins_and_rank_counts_select_a_stable_subset() {
        for combat_rooms in 2..=5 {
            for seed in 0..50 {
                let blueprint = Blueprint::generate(seed, combat_rooms);
                let all = blueprint.crystal_ore_positions(9);
                let s_rank = blueprint.crystal_ore_positions(HunterRank::S.crystal_vein_count());
                let d_rank = blueprint.crystal_ore_positions(HunterRank::D.crystal_vein_count());

                assert_eq!(all.len(), 9, "seed {seed}, combat rooms {combat_rooms}");
                assert_eq!(s_rank, all, "S rank must expose the whole vein set");
                assert_eq!(d_rank.len(), 2, "seed {seed}, combat rooms {combat_rooms}");
                assert_eq!(d_rank, all[..2], "lower ranks use a stable prefix");
                for rank in [
                    HunterRank::E,
                    HunterRank::D,
                    HunterRank::C,
                    HunterRank::B,
                    HunterRank::A,
                    HunterRank::S,
                ] {
                    assert_eq!(
                        blueprint
                            .crystal_ore_positions(rank.crystal_vein_count())
                            .len(),
                        rank.crystal_vein_count(),
                        "seed {seed}, combat rooms {combat_rooms}, rank {}",
                        rank.name()
                    );
                }
            }
        }
    }

    #[test]
    fn rooms_follow_the_requested_progression() {
        for combat in 2..=5 {
            let blueprint = Blueprint::generate(u64::from(combat) * 31, combat);
            assert_eq!(blueprint.entrance().role, RoomRole::Entrance);
            assert_eq!(blueprint.entrance().center, (0, 0));
            for index in 1..=combat {
                assert!(
                    blueprint.combat_room(index).is_some(),
                    "combat room {index}"
                );
            }
            assert!(blueprint.combat_room(combat + 1).is_none());
            assert_eq!(blueprint.boss_room().role, RoomRole::Boss);
        }
    }

    #[test]
    fn every_room_is_reachable_from_the_entrance() {
        for seed in 0..200 {
            let blueprint = Blueprint::generate(seed, 5);
            let walkable = air_columns(&blueprint);
            let mut seen = FxHashSet::default();
            let mut queue = VecDeque::from([(0, 0)]);
            seen.insert((0, 0));
            while let Some((x, z)) = queue.pop_front() {
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let next = (x + dx, z + dz);
                    if walkable.contains(&next) && seen.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
            for room in &blueprint.rooms {
                assert!(
                    seen.contains(&room.center)
                        || room.spawns.iter().any(|spot| seen.contains(spot)),
                    "seed {seed}: room {:?} at {:?} is sealed off",
                    room.role,
                    room.center
                );
            }
        }
    }

    #[test]
    fn the_layout_turns_and_stays_inside_its_region() {
        let mut turned = 0;
        for seed in 0..100 {
            let blueprint = Blueprint::generate(seed, 5);
            let (min, max) = blueprint.bounds;
            let limit = GRID_RADIUS * CELL + 12;
            assert!(min.0 >= -limit && min.2 >= -limit && max.0 <= limit && max.2 <= limit);
            let path: Vec<_> = blueprint
                .rooms
                .iter()
                .filter(|room| room.role != RoomRole::Chamber)
                .map(|room| room.center)
                .collect();
            let turns = path
                .windows(3)
                .any(|w| (w[1].0 - w[0].0, w[1].1 - w[0].1) != (w[2].0 - w[1].0, w[2].1 - w[1].1));
            if turns {
                turned += 1;
            }
        }
        assert!(turned > 80, "only {turned} of 100 layouts ever turn");
    }

    #[test]
    fn rooms_are_lit_and_combat_rooms_have_spawn_spots() {
        for seed in 0..50 {
            let blueprint = Blueprint::generate(seed, 3);
            let lights = blueprint
                .blocks
                .iter()
                .filter(|block| block.part == Part::Light)
                .count();
            assert!(
                lights >= blueprint.rooms.len() * 4,
                "seed {seed}: {lights} lights"
            );
            for room in &blueprint.rooms {
                if matches!(room.role, RoomRole::Combat | RoomRole::Boss) {
                    assert!(room.spawns.len() >= 8, "seed {seed}: {:?}", room.role);
                }
            }
        }
    }

    #[test]
    fn every_theme_appears() {
        let themes: FxHashSet<_> = (0..200)
            .map(|seed| format!("{:?}", Blueprint::generate(seed, 2).theme))
            .collect();
        assert_eq!(themes.len(), THEMES.len());
    }

    #[test]
    fn red_gates_always_generate_an_ice_theme() {
        for seed in 0..50 {
            let layout = crate::dungeon::DungeonLayout::for_gate(seed, true);
            assert_eq!(layout.blueprint.theme, Theme::Ice);
        }
    }
}
