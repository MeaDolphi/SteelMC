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
//! corridors airtight wherever they meet. A decor pass then dresses the
//! interior — floor mosaics, doorway arches, hanging lights, walkable
//! platforms and one themed feature per combat and boss room — only ever
//! recolouring shell blocks or filling carved air, so it can never open a hole
//! or seal a path.

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, SlabType};
use steel_registry::vanilla_blocks;
use steel_utils::BlockStateId;

use crate::rank::HunterRank;
use crate::seeded::{SplitMix, hash};

/// Grid cells from the entrance cell to the grid edge.
const GRID_RADIUS: i32 = 3;
/// Blocks between the centres of two neighbouring grid cells.
const CELL: i32 = 24;
/// Largest half extent a room may roll; two neighbours in adjacent cells must
/// keep a wall column between them (`CELL / 2` would let the air touch).
const MAX_ROOM_HALF: i32 = 11;
/// Air height of a corridor.
const CORRIDOR_HEIGHT: u8 = 3;
/// Most spawn spots remembered per room.
const SPAWN_SPOTS: usize = 24;

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
    /// The snow valley of the elves behind a red gate; never rolled, a red
    /// gate always leads there.
    SnowElves,
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
    /// Half-height platform block; always a slab placed with its `bottom` state.
    slab: BlockRef,
    /// Accent block for themed decor: sarcophagi, icicles, obelisks, basin rims.
    accent: BlockRef,
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
            Self::SnowElves => "Снежная долина",
        }
    }

    /// Returns the blocks between two ceiling lights in a room and in a
    /// corridor.
    ///
    /// The nether fortress is built from bricks so dark that the usual grid
    /// leaves it gloomy, so its lights hang closer together.
    #[must_use]
    pub const fn light_spacing(self) -> (i32, i32) {
        match self {
            Self::Nether => (3, 3),
            _ => (4, 5),
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
                slab: &vanilla_blocks::STONE_BRICK_SLAB,
                accent: &vanilla_blocks::CHISELED_STONE_BRICKS,
            },
            Self::Ice => Palette {
                floor: &vanilla_blocks::PACKED_ICE,
                floor_alt: &vanilla_blocks::SNOW_BLOCK,
                wall: &vanilla_blocks::PACKED_ICE,
                wall_alt: &vanilla_blocks::BLUE_ICE,
                pillar: &vanilla_blocks::BLUE_ICE,
                ceiling: &vanilla_blocks::SNOW_BLOCK,
                light: &vanilla_blocks::SEA_LANTERN,
                slab: &vanilla_blocks::QUARTZ_SLAB,
                accent: &vanilla_blocks::BLUE_ICE,
            },
            Self::Nether => Palette {
                floor: &vanilla_blocks::NETHER_BRICKS,
                floor_alt: &vanilla_blocks::BLACKSTONE,
                wall: &vanilla_blocks::NETHER_BRICKS,
                wall_alt: &vanilla_blocks::RED_NETHER_BRICKS,
                pillar: &vanilla_blocks::POLISHED_BLACKSTONE_BRICKS,
                ceiling: &vanilla_blocks::NETHER_BRICKS,
                light: &vanilla_blocks::SHROOMLIGHT,
                slab: &vanilla_blocks::NETHER_BRICK_SLAB,
                accent: &vanilla_blocks::BLACKSTONE,
            },
            Self::Desert => Palette {
                floor: &vanilla_blocks::SMOOTH_SANDSTONE,
                floor_alt: &vanilla_blocks::CUT_SANDSTONE,
                wall: &vanilla_blocks::SANDSTONE,
                wall_alt: &vanilla_blocks::CUT_SANDSTONE,
                pillar: &vanilla_blocks::CHISELED_SANDSTONE,
                ceiling: &vanilla_blocks::SMOOTH_SANDSTONE,
                light: &vanilla_blocks::OCHRE_FROGLIGHT,
                slab: &vanilla_blocks::SMOOTH_SANDSTONE_SLAB,
                accent: &vanilla_blocks::CHISELED_SANDSTONE,
            },
            Self::Jungle => Palette {
                floor: &vanilla_blocks::MOSS_BLOCK,
                floor_alt: &vanilla_blocks::COBBLESTONE,
                wall: &vanilla_blocks::MOSSY_COBBLESTONE,
                wall_alt: &vanilla_blocks::COBBLESTONE,
                pillar: &vanilla_blocks::MOSSY_STONE_BRICKS,
                ceiling: &vanilla_blocks::MOSSY_COBBLESTONE,
                light: &vanilla_blocks::VERDANT_FROGLIGHT,
                slab: &vanilla_blocks::MOSSY_COBBLESTONE_SLAB,
                accent: &vanilla_blocks::MOSSY_STONE_BRICKS,
            },
            Self::DeepDark => Palette {
                floor: &vanilla_blocks::DEEPSLATE_TILES,
                floor_alt: &vanilla_blocks::SCULK,
                wall: &vanilla_blocks::DEEPSLATE_BRICKS,
                wall_alt: &vanilla_blocks::CRACKED_DEEPSLATE_TILES,
                pillar: &vanilla_blocks::POLISHED_DEEPSLATE,
                ceiling: &vanilla_blocks::DEEPSLATE_TILES,
                light: &vanilla_blocks::SEA_LANTERN,
                slab: &vanilla_blocks::DEEPSLATE_BRICK_SLAB,
                accent: &vanilla_blocks::SCULK,
            },
            Self::SnowElves => Palette {
                floor: &vanilla_blocks::SNOW_BLOCK,
                floor_alt: &vanilla_blocks::PODZOL,
                wall: &vanilla_blocks::PACKED_ICE,
                wall_alt: &vanilla_blocks::BLUE_ICE,
                pillar: &vanilla_blocks::SPRUCE_LOG,
                ceiling: &vanilla_blocks::SNOW_BLOCK,
                light: &vanilla_blocks::LANTERN,
                slab: &vanilla_blocks::SPRUCE_SLAB,
                accent: &vanilla_blocks::BLUE_ICE,
            },
            Self::End => Palette {
                floor: &vanilla_blocks::END_STONE_BRICKS,
                floor_alt: &vanilla_blocks::PURPUR_BLOCK,
                wall: &vanilla_blocks::END_STONE_BRICKS,
                wall_alt: &vanilla_blocks::PURPUR_BLOCK,
                pillar: &vanilla_blocks::PURPUR_PILLAR,
                ceiling: &vanilla_blocks::PURPUR_BLOCK,
                light: &vanilla_blocks::PEARLESCENT_FROGLIGHT,
                slab: &vanilla_blocks::END_STONE_BRICK_SLAB,
                accent: &vanilla_blocks::PURPUR_BLOCK,
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
    /// A light source set into the ceiling or hanging from it.
    Light,
    /// Mineable crystal ore in a wall.
    Ore,
    /// A half-height platform; built as the theme slab in its bottom state.
    Platform,
    /// Dressed framing (jambs and lintel) around a doorway.
    Arch,
    /// A themed accent block: sarcophagi, icicles, obelisks, basin rims.
    Decor,
    /// A chain suspending a hanging light.
    Chain,
    /// Still lava inside a rimmed basin; never exposed.
    Lava,
    /// A block the layout picks itself instead of the theme: the trees, huts
    /// and furnishings of an open valley need more kinds than a palette has.
    Block(BlockStateId),
}

impl Palette {
    /// Returns the block a `part` is built from; ore is not themed.
    #[must_use]
    pub fn block(&self, part: Part, ore: BlockRef) -> BlockRef {
        match part {
            Part::Floor => self.floor,
            Part::FloorAlt => self.floor_alt,
            Part::Wall => self.wall,
            Part::WallAlt => self.wall_alt,
            Part::Pillar | Part::Arch => self.pillar,
            Part::Ceiling => self.ceiling,
            Part::Light => self.light,
            Part::Ore => ore,
            Part::Platform => self.slab,
            Part::Decor => self.accent,
            Part::Chain => &vanilla_blocks::IRON_CHAIN,
            Part::Lava => &vanilla_blocks::LAVA,
            Part::Block(state) => state.get_block(),
        }
    }

    /// Returns the block state a `part` is built as.
    ///
    /// Platforms are half-height terraces: their slab sits in the bottom state
    /// so they stay walkable without jumping.
    #[must_use]
    pub fn state(&self, part: Part, ore: BlockRef) -> BlockStateId {
        match part {
            Part::Block(state) => state,
            Part::Platform => self
                .slab
                .default_state()
                .set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Bottom),
            _ => self.block(part, ore).default_state(),
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
    /// Every block of the built dungeon: the shell plus interior decor.
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
        let mut rng = SplitMix::new(seed);
        let theme = THEMES[usize::try_from(rng.next() % THEMES.len() as u64).unwrap_or(0)];
        Self::generate_with_theme(seed, combat_rooms, theme)
    }

    /// Generates the dungeon of `seed` with a fixed `theme`.
    #[must_use]
    pub fn generate_with_theme(seed: u64, combat_rooms: u8, theme: Theme) -> Self {
        let mut rng = SplitMix::new(seed);
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
        let mut doorways: Vec<Doorway> = Vec::new();
        let (room_spacing, corridor_spacing) = theme.light_spacing();
        for (from, to) in &links {
            carve_corridor(
                &mut rng,
                &mut air,
                &mut lights,
                corridor_spacing,
                &rooms[*from],
                *from,
                &rooms[*to],
                *to,
                &mut doorways,
            );
        }
        for room in &rooms {
            light_room(&air, &mut lights, room, room_spacing);
        }

        let mut solid = shell(seed, &air, &lights);
        let blocked_ground = decorate(seed, theme, &rooms, &air, &doorways, &mut solid);
        let blocks: Vec<Placement> = solid
            .into_iter()
            .map(|(offset, part)| Placement { offset, part })
            .collect();
        let mut blocks = blocks;
        blocks.sort_by_key(|placement| placement.offset);
        let mut rooms = rooms;
        for (index, room) in rooms.iter_mut().enumerate() {
            room.spawns = spawn_spots(seed, &air, index, room, &blocked_ground);
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
        RoomRole::Entrance => (Shape::Colonnade, (10, 10), 8),
        // The boss room must fit the boss, the exit frame and the fight itself.
        RoomRole::Boss => (
            if rng.chance(50) {
                Shape::Rotunda
            } else {
                Shape::Colonnade
            },
            (rng.range(9, MAX_ROOM_HALF), rng.range(9, MAX_ROOM_HALF)),
            u8::try_from(rng.range(8, 10)).unwrap_or(8),
        ),
        RoomRole::Combat => (
            SHAPES[usize::try_from(rng.next()).unwrap_or(0) % SHAPES.len()],
            (rng.range(7, MAX_ROOM_HALF), rng.range(7, MAX_ROOM_HALF)),
            u8::try_from(rng.range(6, 9)).unwrap_or(6),
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

/// Where one corridor crosses a room's wall. The boundary is the first column
/// along the corridor that is not carved air of the room, so it hugs the real
/// (possibly jittered) wall; decor uses it to frame the doorway and to keep
/// the approach lane clear.
#[derive(Debug, Clone, Copy)]
struct Doorway {
    /// Room the door belongs to.
    room: usize,
    /// Whether the corridor runs along x.
    along_x: bool,
    /// World x or z of the wall column just outside the room.
    boundary: i32,
    /// World z or x of the door centre.
    door: i32,
    /// Direction from the room centre through the door, along the corridor.
    side: i32,
}

/// Joins two rooms in neighbouring cells with a three-wide corridor.
///
/// Each door sits at its own offset along its wall, so the corridor runs out
/// of one room, steps sideways halfway, and enters the other room. The
/// corridor also reaches a few blocks into both rooms, so an uneven wall or a
/// pillar can never seal a doorway.
#[expect(
    clippy::too_many_arguments,
    reason = "a corridor is fully described by its rng, the maps it carves and lights, both linked rooms and the doorway list"
)]
fn carve_corridor(
    rng: &mut SplitMix,
    air: &mut FxHashMap<(i32, i32), Column>,
    lights: &mut FxHashSet<(i32, i32)>,
    light_spacing: i32,
    from: &Room,
    from_index: usize,
    to: &Room,
    to_index: usize,
    doorways: &mut Vec<Doorway>,
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

    // Record where the corridor leaves each room before carving: a previous
    // corridor may already have punched room-air columns, so membership of
    // this specific room decides the boundary.
    let column_at = |&(a, b): &(i32, i32)| if along_x { (a, b) } else { (b, a) };
    let boundary_of = |room_index: usize, reversed: bool| {
        let mut iter: Box<dyn Iterator<Item = (i32, i32)>> = if reversed {
            Box::new(path.iter().rev().map(column_at))
        } else {
            Box::new(path.iter().map(column_at))
        };
        iter.find(|column| air.get(column).is_none_or(|c| c.room != Some(room_index)))
    };
    if let Some((x, z)) = boundary_of(from_index, false) {
        doorways.push(Doorway {
            room: from_index,
            along_x,
            boundary: if along_x { x } else { z },
            door: if along_x { z } else { x },
            side: sign,
        });
    }
    if let Some((x, z)) = boundary_of(to_index, true) {
        doorways.push(Doorway {
            room: to_index,
            along_x,
            boundary: if along_x { x } else { z },
            door: if along_x { z } else { x },
            side: -sign,
        });
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
        if index % light_spacing == light_spacing / 2
            && air.get(&center).is_some_and(|column| column.room.is_none())
        {
            lights.insert(center);
        }
    }
}

/// Puts a grid of ceiling lights `spacing` blocks apart over a room.
fn light_room(
    air: &FxHashMap<(i32, i32), Column>,
    lights: &mut FxHashSet<(i32, i32)>,
    room: &Room,
    spacing: i32,
) {
    let (hx, hz) = room.half;
    let (cx, cz) = room.center;
    for u in -hx..=hx {
        for v in -hz..=hz {
            let column = (cx + u, cz + v);
            if u.rem_euclid(spacing) == 0 && v.rem_euclid(spacing) == 0 && air.contains_key(&column)
            {
                lights.insert(column);
            }
        }
    }
}

/// Wraps the carved air in floor, walls and ceiling.
///
/// Returns the solid blocks by offset so a later pass can add decor; the
/// conversion to sorted placements happens after decorating.
fn shell(
    seed: u64,
    air: &FxHashMap<(i32, i32), Column>,
    lights: &FxHashSet<(i32, i32)>,
) -> FxHashMap<(i32, i32, i32), Part> {
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
    // Walls are two blocks thick. Every wall block that borders carved air
    // must have solid ground on all its other horizontal faces: mining a
    // crystal ore out of the inner layer would otherwise expose the void
    // behind or beside the wall. Carved columns on the far side seal
    // themselves, so only truly solid ground receives a backing block.
    let inner_walls: Vec<(i32, i32, i32)> = solid
        .iter()
        .filter(|(_, part)| matches!(part, Part::Wall | Part::WallAlt))
        .map(|((x, y, z), _)| (*x, *y, *z))
        .collect();
    for (x, y, z) in inner_walls {
        // Ore also sits in reentrant corners that only touch room air
        // diagonally, so the air check spans all eight surrounding columns.
        let faces_air = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (dx, dz)))
            .filter(|(dx, dz)| *dx != 0 || *dz != 0)
            .any(|(dx, dz)| is_air(x + dx, y, z + dz));
        if !faces_air {
            continue;
        }
        for (dx, dz) in DIRECTIONS {
            let (bx, bz) = (x - dx, z - dz);
            // A carved column only rules out its own air; above its ceiling
            // or below its floor the position is outside shell and must be
            // backed like any other void (floors and ceilings of carved
            // columns are already solid by now, so `or_insert` keeps them).
            if is_air(bx, y, bz) {
                continue;
            }
            let backing = if hash(seed ^ 0x5A5A, bx, y, bz).is_multiple_of(4) {
                Part::WallAlt
            } else {
                Part::Wall
            };
            solid.entry((bx, y, bz)).or_insert(backing);
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
    // Cavern outlines can carve cells that touch the rest of the dungeon only
    // diagonally; sealed behind walls they would read as void holes next to
    // mined ore. Any carved position the entrance air cannot reach is filled
    // back in.
    const AROUND: [(i32, i32, i32); 6] = [
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ];
    let mut interior: FxHashSet<(i32, i32, i32)> = FxHashSet::default();
    let mut queue: VecDeque<(i32, i32, i32)> = VecDeque::from([(0, 0, 0)]);
    interior.insert((0, 0, 0));
    while let Some((x, y, z)) = queue.pop_front() {
        for (dx, dy, dz) in AROUND {
            let next = (x + dx, y + dy, z + dz);
            if is_air(next.0, next.1, next.2) && interior.insert(next) {
                queue.push_back(next);
            }
        }
    }
    for ((x, z), column) in air {
        for y in 0..i32::from(column.height) {
            if !interior.contains(&(*x, y, *z)) {
                let part = if hash(seed ^ 0x5A5A, *x, y, *z).is_multiple_of(4) {
                    Part::WallAlt
                } else {
                    Part::Wall
                };
                solid.insert((*x, y, *z), part);
            }
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
    solid
}

/// Carved columns a walker can reach from the entrance at floor level; sealed
/// pockets (filled back in by [`shell`]) stay undecorated.
fn reachable_columns(air: &FxHashMap<(i32, i32), Column>) -> FxHashSet<(i32, i32)> {
    let mut reachable: FxHashSet<(i32, i32)> = FxHashSet::default();
    let mut queue = VecDeque::from([(0, 0)]);
    reachable.insert((0, 0));
    while let Some((x, z)) = queue.pop_front() {
        for (dx, dz) in DIRECTIONS {
            let next = (x + dx, z + dz);
            if air.contains_key(&next) && reachable.insert(next) {
                queue.push_back(next);
            }
        }
    }
    reachable
}

/// Recolours one floor block into the mosaic variant, if there is a floor.
fn recolour_floor(solid: &mut FxHashMap<(i32, i32, i32), Part>, x: i32, z: i32) {
    if let Some(part @ Part::Floor) = solid.get_mut(&(x, -1, z)) {
        *part = Part::FloorAlt;
    }
}

/// Keeps decor out of the lane between a room's centre and a doorway. The
/// corridor opening is three wide, so blocking anything closer than one block
/// to its edge would narrow the walk through the door.
fn near_door(doors: &[Doorway], room: &Room, u: i32, v: i32) -> bool {
    doors.iter().any(|door| {
        let (along, across, half, door_centre) = if door.along_x {
            (u, v, room.half.0, door.door - room.center.1)
        } else {
            (v, u, room.half.1, door.door - room.center.0)
        };
        let outward = along * door.side;
        (half - 4..=half + 2).contains(&outward) && (across - door_centre).abs() <= 1
    })
}

/// The strip of a boss room where the boss waits (far +z side); the exit
/// frame stands at the centre. Decor stays clear so the fight stays open.
fn boss_strip(room: &Room, u: i32, v: i32) -> bool {
    room.role == RoomRole::Boss && u.abs() <= 2 && v >= room.half.1 - 5
}

/// Maps a wall-relative position to a world column: `along` slides along the
/// wall and `depth` grows from the wall line (`half - 3`) toward the centre,
/// so walls are 0..=+z, 1=-z, 2=+x, 3=-x.
const fn wall_cell(room: &Room, wall: usize, along: i32, depth: i32) -> (i32, i32) {
    let (cx, cz) = room.center;
    match wall {
        0 => (cx + along, cz + room.half.1 - 3 - depth),
        1 => (cx + along, cz - room.half.1 + 3 + depth),
        2 => (cx + room.half.0 - 3 - depth, cz + along),
        _ => (cx - room.half.0 + 3 + depth, cz + along),
    }
}

/// (wall, offset) candidates in a hashed order for one room.
fn wall_candidates(rng: &mut SplitMix, room: &Room, count: usize) -> Vec<(usize, i32)> {
    let limit = (room.half.0.min(room.half.1) - 4).max(0);
    let mut candidates = Vec::with_capacity(count * 4);
    for _ in 0..count.max(1) * 4 {
        let wall = usize::try_from(rng.next() % 4).unwrap_or(0);
        let offset = if limit == 0 {
            0
        } else {
            rng.range(-limit, limit)
        };
        candidates.push((wall, offset));
    }
    candidates
}

/// Checks every ground cell of a feature: the cell and, for ground features,
/// one step toward the room centre must be carved and reachable, and the cell
/// must be free of other decor, door lanes and the boss strip.
fn fits(
    cells: &[(i32, i32)],
    room: &Room,
    air: &FxHashMap<(i32, i32), Column>,
    reachable: &FxHashSet<(i32, i32)>,
    doors: &[Doorway],
    taken: &FxHashSet<(i32, i32)>,
    open_inward: bool,
) -> bool {
    let (cx, cz) = room.center;
    cells.iter().all(|&(x, z)| {
        let column = (x, z);
        let u = x - cx;
        let v = z - cz;
        if !air.contains_key(&column)
            || !reachable.contains(&column)
            || taken.contains(&column)
            || near_door(doors, room, u, v)
            || boss_strip(room, u, v)
        {
            return false;
        }
        if !open_inward {
            return true;
        }
        let inward = if u.abs() >= v.abs() {
            (x - u.signum(), z)
        } else {
            (x, z - v.signum())
        };
        air.contains_key(&inward)
    })
}

/// Recolours floor blocks into a mosaic: a centre medallion, a runner from
/// the medallion to every doorway and, in the largest halls, a ring at the walls.
fn floor_pattern(room: &Room, doors: &[Doorway], solid: &mut FxHashMap<(i32, i32, i32), Part>) {
    let (cx, cz) = room.center;
    let (hx, hz) = room.half;
    for du in -1..=1 {
        for dv in -1..=1 {
            recolour_floor(solid, cx + du, cz + dv);
        }
    }
    for door in doors {
        let centre = if door.along_x { cx } else { cz };
        let mut a = centre;
        loop {
            let (x, z) = if door.along_x {
                (a, door.door)
            } else {
                (door.door, a)
            };
            recolour_floor(solid, x, z);
            if a == door.boundary - door.side {
                break;
            }
            a += door.side;
        }
    }
    if matches!(room.role, RoomRole::Entrance | RoomRole::Boss) {
        for u in -hx..=hx {
            recolour_floor(solid, cx + u, cz + hz - 1);
            recolour_floor(solid, cx + u, cz - hz + 1);
        }
        for v in -hz..=hz {
            recolour_floor(solid, cx + hx - 1, cz + v);
            recolour_floor(solid, cx - hx + 1, cz + v);
        }
    }
}

/// Dressed doorways: the wall columns flanking the opening and the row above
/// it take the pillar block. The frame anchors on the visible wall face, the
/// first wall block walking from the room centre out along the corridor;
/// carved cells (room air, pillars, the corridor itself) are skipped. Jambs
/// then scan a couple of blocks either side of the face for wavy walls and
/// corridor bends. Only existing shell blocks change, so a doorway can never
/// be narrowed.
fn frame_doorways(room: &Room, doors: &[Doorway], solid: &mut FxHashMap<(i32, i32, i32), Part>) {
    for door in doors {
        let (centre, reach) = if door.along_x {
            (room.center.0, room.half.0)
        } else {
            (room.center.1, room.half.1)
        };
        // Scan along the opening's flank line: the door centre line passes
        // through the carved opening itself, so the first wall block there is
        // the face of the doorway.
        let mut face = None;
        for flank in [door.door + 2, door.door - 2] {
            for step in 2..=reach + 3 {
                let along = centre + door.side * step;
                let position = if door.along_x {
                    (along, 0, flank)
                } else {
                    (flank, 0, along)
                };
                if matches!(solid.get(&position), Some(Part::Wall | Part::WallAlt)) {
                    face = Some(along);
                    break;
                }
            }
            if face.is_some() {
                break;
            }
        }
        let Some(face) = face else {
            continue;
        };
        for flank in [door.door - 2, door.door + 2] {
            for y in 0..=3 {
                for step in [0, 1, 2, -1, -2] {
                    let along = face + step * door.side;
                    let position = if door.along_x {
                        (along, y, flank)
                    } else {
                        (flank, y, along)
                    };
                    if matches!(solid.get(&position), Some(Part::Wall | Part::WallAlt)) {
                        solid.insert(position, Part::Arch);
                        break;
                    }
                }
            }
        }
        for across in door.door - 1..=door.door + 1 {
            let position = if door.along_x {
                (face, 3, across)
            } else {
                (across, 3, face)
            };
            if matches!(solid.get(&position), Some(Part::Ceiling | Part::Wall)) {
                solid.insert(position, Part::Arch);
            }
        }
    }
}

/// Hanging lights on chains: every tall enough room gets one in the centre,
/// wide furnished rooms get two more so big floors stay lit.
fn hang_lanterns(
    room: &Room,
    air: &FxHashMap<(i32, i32), Column>,
    reachable: &FxHashSet<(i32, i32)>,
    solid: &mut FxHashMap<(i32, i32, i32), Part>,
) {
    let height = i32::from(room.height);
    if height < 5 {
        return;
    }
    let (cx, cz) = room.center;
    let mut lanterns = vec![(cx, cz)];
    if matches!(
        room.role,
        RoomRole::Entrance | RoomRole::Combat | RoomRole::Boss
    ) && room.half.0 >= 9
    {
        let spread = room.half.0 / 2;
        lanterns.extend([(cx + spread, cz), (cx - spread, cz)]);
    }
    for (x, z) in lanterns {
        if !reachable.contains(&(x, z)) || !air.contains_key(&(x, z)) {
            continue;
        }
        solid.insert((x, height - 1, z), Part::Chain);
        solid.insert((x, height - 2, z), Part::Light);
    }
}

/// A wall-hugging terrace of half-height platforms (bottom slabs): walkable
/// without jumping and big enough to circle behind.
#[expect(
    clippy::too_many_arguments,
    reason = "a placement needs the room, the carved and reachable maps, its doors, the occupancy sets and the solid map"
)]
fn place_platforms(
    rng: &mut SplitMix,
    room: &Room,
    air: &FxHashMap<(i32, i32), Column>,
    reachable: &FxHashSet<(i32, i32)>,
    doors: &[Doorway],
    taken: &mut FxHashSet<(i32, i32)>,
    solid: &mut FxHashMap<(i32, i32, i32), Part>,
    blocked_ground: &mut FxHashSet<(i32, i32)>,
) {
    let candidates = wall_candidates(rng, room, 6);
    for width in [3, 2] {
        for &(wall, along) in &candidates {
            let cells: Vec<(i32, i32)> = (0..width)
                .flat_map(|du| (0..=1).map(move |depth| wall_cell(room, wall, along + du, depth)))
                .collect();
            if !fits(&cells, room, air, reachable, doors, taken, true) {
                continue;
            }
            for column in cells {
                taken.insert(column);
                blocked_ground.insert(column);
                solid.insert((column.0, 0, column.1), Part::Platform);
            }
            return;
        }
    }
}

/// Waist-high pillar stubs against the walls: cover to circle behind, too
/// low to block a lane. Colonnades and crosses already carry their own.
#[expect(
    clippy::too_many_arguments,
    reason = "a placement needs the room, the carved and reachable maps, its doors, the occupancy sets and the solid map"
)]
fn place_pilasters(
    rng: &mut SplitMix,
    room: &Room,
    air: &FxHashMap<(i32, i32), Column>,
    reachable: &FxHashSet<(i32, i32)>,
    doors: &[Doorway],
    taken: &mut FxHashSet<(i32, i32)>,
    solid: &mut FxHashMap<(i32, i32, i32), Part>,
    blocked_ground: &mut FxHashSet<(i32, i32)>,
) {
    if matches!(room.shape, Shape::Colonnade | Shape::Cross) {
        return;
    }
    let wanted = 1 + i32::from(rng.chance(50));
    let mut placed = 0;
    for (wall, along) in wall_candidates(rng, room, 6) {
        if placed >= wanted {
            return;
        }
        let cells = [wall_cell(room, wall, along, 0)];
        if !fits(&cells, room, air, reachable, doors, taken, true) {
            continue;
        }
        let height = 1 + i32::from(rng.chance(50));
        let column = cells[0];
        taken.insert(column);
        blocked_ground.insert(column);
        for y in 0..height {
            solid.insert((column.0, y, column.1), Part::Pillar);
        }
        placed += 1;
    }
}

/// One themed feature per furnished room, against the first wall where it
/// fits: crypt sarcophagi, ice icicles, nether lava basins, desert
/// obelisks, jungle ruin mounds, deep-dark sculk spikes, end chorus stalks.
#[expect(
    clippy::too_many_arguments,
    reason = "a feature needs the theme, the room, the carved and reachable maps, its doors and the occupancy sets"
)]
fn place_feature(
    rng: &mut SplitMix,
    theme: Theme,
    room: &Room,
    air: &FxHashMap<(i32, i32), Column>,
    reachable: &FxHashSet<(i32, i32)>,
    doors: &[Doorway],
    taken: &mut FxHashSet<(i32, i32)>,
    solid: &mut FxHashMap<(i32, i32, i32), Part>,
    blocked_ground: &mut FxHashSet<(i32, i32)>,
) {
    let height = i32::from(room.height);
    for (wall, along) in wall_candidates(rng, room, 12) {
        let cells = feature_cells(theme, room, wall, along, height, rng);
        if place_cells(
            &cells,
            room,
            air,
            reachable,
            doors,
            taken,
            solid,
            blocked_ground,
        ) {
            return;
        }
    }
    // A crowded room (doors on every wall, pillars everywhere) can reject
    // every full feature; a single accent column still fits almost anywhere.
    for (wall, along) in wall_candidates(rng, room, 8) {
        let cells = vec![(wall_cell(room, wall, along, 0), 0, Part::Decor)];
        if place_cells(
            &cells,
            room,
            air,
            reachable,
            doors,
            taken,
            solid,
            blocked_ground,
        ) {
            return;
        }
    }
}

/// Places one feature attempt if every ground cell fits; returns whether it
/// did. Ground cells become unavailable to later decor and to spawn spots.
#[expect(
    clippy::too_many_arguments,
    reason = "placing cells needs the room, the carved and reachable maps, its doors and the occupancy sets"
)]
fn place_cells(
    cells: &[((i32, i32), i32, Part)],
    room: &Room,
    air: &FxHashMap<(i32, i32), Column>,
    reachable: &FxHashSet<(i32, i32)>,
    doors: &[Doorway],
    taken: &mut FxHashSet<(i32, i32)>,
    solid: &mut FxHashMap<(i32, i32, i32), Part>,
    blocked_ground: &mut FxHashSet<(i32, i32)>,
) -> bool {
    let ground: Vec<(i32, i32)> = cells.iter().map(|&(column, _, _)| column).collect();
    if !fits(&ground, room, air, reachable, doors, taken, true) {
        return false;
    }
    for &(column, y, part) in cells {
        if y == 0 {
            blocked_ground.insert(column);
        }
        solid.insert((column.0, y, column.1), part);
    }
    taken.extend(ground);
    true
}

/// The cells of one themed feature attempt, anchored like the other wall
/// decor: `(column, y, part)` in world offsets.
fn feature_cells(
    theme: Theme,
    room: &Room,
    wall: usize,
    along: i32,
    height: i32,
    rng: &mut SplitMix,
) -> Vec<((i32, i32), i32, Part)> {
    let mut cells = Vec::new();
    let mut push = |du: i32, depth: i32, y: i32, part: Part| {
        cells.push((wall_cell(room, wall, along + du, depth), y, part));
    };
    match theme {
        Theme::Crypt => {
            // A stone sarcophagus: two long, one deep, two tall.
            for du in 0..=1 {
                for y in 0..=1 {
                    push(du, 0, y, Part::Decor);
                }
            }
        }
        // The snow valley is built by its own generator; were a cave ever
        // dressed in it, it would take the ice cave's icicles.
        Theme::Ice | Theme::SnowElves => {
            // Icicles under the ceiling: a three-long tip row and a centre
            // spike hanging lower.
            for du in -1..=1 {
                push(du, 0, height - 1, Part::Decor);
            }
            for y in height - 3..height - 1 {
                push(0, 0, y, Part::Decor);
            }
        }
        Theme::Nether => {
            // A lava basin behind a full rim: the lava sits one block below
            // the rim, so it can neither flow out nor be stumbled into.
            for du in -1..=1 {
                for depth in 0..=2 {
                    let part = if du == 0 && depth == 1 {
                        Part::Lava
                    } else {
                        Part::Decor
                    };
                    push(du, depth, 0, part);
                }
            }
        }
        Theme::Desert => {
            // A chiseled obelisk, three or four blocks tall.
            let top = 2 + i32::from(rng.chance(50));
            for y in 0..=top {
                push(0, 0, y, Part::Decor);
            }
        }
        Theme::Jungle => {
            // A sunken ruin corner: a slab platform with one idol block.
            for du in 0..=1 {
                for depth in 0..=1 {
                    push(du, depth, 0, Part::Platform);
                }
            }
            push(0, 0, 1, Part::Decor);
        }
        Theme::DeepDark => {
            // Two sculk spikes of different heights.
            let tall = 1 + i32::from(rng.chance(50));
            for y in 0..=tall {
                push(0, 0, y, Part::Decor);
            }
            for y in 0..=1 {
                push(1, 0, y, Part::Decor);
            }
        }
        Theme::End => {
            // A chorus-like stalk with a purpur cap.
            for y in 0..=2 {
                push(0, 0, y, Part::Decor);
            }
        }
    }
    cells
}

/// Dresses room interiors on top of the finished shell.
///
/// Everything here either recolours an existing shell block (floor mosaic,
/// doorway arches) or fills carved, reachable air (lights, platforms,
/// pilasters, themed features), so decor can never open a wall hole or seal
/// a path. Returns the ground columns occupied by decor, which spawn spots
/// must stay clear of.
fn decorate(
    seed: u64,
    theme: Theme,
    rooms: &[Room],
    air: &FxHashMap<(i32, i32), Column>,
    doorways: &[Doorway],
    solid: &mut FxHashMap<(i32, i32, i32), Part>,
) -> FxHashSet<(i32, i32)> {
    let reachable = reachable_columns(air);
    let mut rng = SplitMix::new(seed ^ 0xDEC0_F173_9B2A_7C15);
    let mut blocked_ground: FxHashSet<(i32, i32)> = FxHashSet::default();
    for (index, room) in rooms.iter().enumerate() {
        let doors: Vec<Doorway> = doorways
            .iter()
            .copied()
            .filter(|door| door.room == index)
            .collect();
        floor_pattern(room, &doors, solid);
        frame_doorways(room, &doors, solid);
        hang_lanterns(room, air, &reachable, solid);
        if !matches!(
            room.role,
            RoomRole::Entrance | RoomRole::Combat | RoomRole::Boss
        ) {
            continue;
        }
        let mut taken: FxHashSet<(i32, i32)> = FxHashSet::default();
        place_platforms(
            &mut rng,
            room,
            air,
            &reachable,
            &doors,
            &mut taken,
            solid,
            &mut blocked_ground,
        );
        place_pilasters(
            &mut rng,
            room,
            air,
            &reachable,
            &doors,
            &mut taken,
            solid,
            &mut blocked_ground,
        );
        place_feature(
            &mut rng,
            theme,
            room,
            air,
            &reachable,
            &doors,
            &mut taken,
            solid,
            &mut blocked_ground,
        );
    }
    blocked_ground
}

/// Picks up to [`SPAWN_SPOTS`] free floor columns spread over a room.
///
/// A spot needs open floor around it, away from walls and pillars, and no
/// ground decor within one block, so a spawned mob never stands inside a
/// platform or a sarcophagus.
fn spawn_spots(
    seed: u64,
    air: &FxHashMap<(i32, i32), Column>,
    index: usize,
    room: &Room,
    blocked_ground: &FxHashSet<(i32, i32)>,
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
            let clear_of_decor = (-1..=1).all(|dx| {
                (-1..=1).all(|dz| !blocked_ground.contains(&(column.0 + dx, column.1 + dz)))
            });
            if open && clear_of_decor {
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

    use super::{
        Blueprint, CELL, GRID_RADIUS, MAX_ROOM_HALF, Part, Placement, RoomRole, THEMES, Theme,
    };
    use crate::dungeon::DungeonLayout;
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
            // Farthest room centre plus its half extent, the wall, and the
            // backing block behind the wall.
            let limit = GRID_RADIUS * CELL + MAX_ROOM_HALF + 3;
            assert!(
                min.0 >= -limit && min.2 >= -limit && max.0 <= limit && max.2 <= limit,
                "seed {seed}: bounds {min:?}..{max:?} exceed {limit}",
            );
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

    /// Columns carrying full-solid decor at foot level: walkable floor may
    /// pass everywhere else, but a mob cannot stand on these.
    fn blocked_ground_columns(blueprint: &Blueprint) -> FxHashSet<(i32, i32)> {
        blueprint
            .blocks
            .iter()
            .filter(|block| {
                block.offset.1 == 0 && matches!(block.part, Part::Decor | Part::Lava | Part::Pillar)
            })
            .map(|block| (block.offset.0, block.offset.2))
            .collect()
    }

    #[test]
    fn decor_leaves_a_walkable_way_through_every_room() {
        for seed in 0..200 {
            let blueprint = Blueprint::generate(seed, 5);
            let blocked = blocked_ground_columns(&blueprint);
            let walkable: FxHashSet<(i32, i32)> = air_columns(&blueprint)
                .difference(&blocked)
                .copied()
                .collect();
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
                    seen.contains(&room.center),
                    "seed {seed}: room {:?} at {:?} is cut off by decor",
                    room.role,
                    room.center
                );
                for spot in &room.spawns {
                    assert!(
                        seen.contains(spot),
                        "seed {seed}: spawn spot {spot:?} of {:?} is cut off",
                        room.role
                    );
                    for dx in -1..=1 {
                        for dz in -1..=1 {
                            assert!(
                                !blocked.contains(&(spot.0 + dx, spot.1 + dz)),
                                "seed {seed}: spawn spot {spot:?} of {:?} stands in decor",
                                room.role
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_room_has_a_light_inside_it() {
        for seed in 0..100 {
            let blueprint = Blueprint::generate(seed, 4);
            for room in &blueprint.rooms {
                let (hx, hz) = room.half;
                let lit = blueprint.blocks.iter().any(|block| {
                    block.part == Part::Light
                        && (block.offset.0 - room.center.0).abs() <= hx
                        && (block.offset.2 - room.center.1).abs() <= hz
                        && (0..=i32::from(room.height)).contains(&block.offset.1)
                });
                assert!(
                    lit,
                    "seed {seed}: room {:?} at {:?} has no light",
                    room.role, room.center
                );
            }
        }
    }

    #[test]
    fn furnished_rooms_are_dressed() {
        let mut min_dressed = usize::MAX;
        for seed in 0..100 {
            let blueprint = Blueprint::generate(seed, 4);
            for room in &blueprint.rooms {
                if !matches!(
                    room.role,
                    RoomRole::Entrance | RoomRole::Combat | RoomRole::Boss
                ) {
                    continue;
                }
                let (hx, hz) = room.half;
                let inside = |block: &Placement| {
                    (block.offset.0 - room.center.0).abs() <= hx
                        && (block.offset.2 - room.center.1).abs() <= hz
                };
                let chains = blueprint
                    .blocks
                    .iter()
                    .filter(|block| inside(block) && block.part == Part::Chain)
                    .count();
                let dressed = blueprint
                    .blocks
                    .iter()
                    .filter(|block| {
                        inside(block)
                            && matches!(block.part, Part::Platform | Part::Decor | Part::Lava)
                    })
                    .count();
                // Doorway arches frame the wall face, which can sit several
                // blocks from the room centre: walls are jittered and a
                // corridor may bend right at the boundary column.
                let arches = blueprint
                    .blocks
                    .iter()
                    .filter(|block| {
                        (block.offset.0 - room.center.0).abs() <= hx + 6
                            && (block.offset.2 - room.center.1).abs() <= hz + 6
                            && block.part == Part::Arch
                    })
                    .count();
                assert!(
                    chains >= 1,
                    "seed {seed}: {:?} has no hanging light",
                    room.role
                );
                min_dressed = min_dressed.min(dressed);
                assert!(
                    arches >= 3,
                    "seed {seed}: {:?} at {:?} has no doorway arch",
                    room.role,
                    room.center
                );
            }
        }
        assert!(
            min_dressed >= 2,
            "a furnished room ended up with only {min_dressed} dressed cells"
        );
    }

    #[test]
    fn large_blue_gate_entrances_are_furnished_and_keep_arrival_clear() {
        for theme in THEMES {
            for seed in 0..12 {
                let blueprint = Blueprint::generate_with_theme(seed, 4, theme);
                let entrance = blueprint.entrance();
                assert_eq!(entrance.half, (10, 10));
                assert_eq!(entrance.height, 8);
                assert_eq!(entrance.shape, super::Shape::Colonnade);

                let dressed = blueprint
                    .blocks
                    .iter()
                    .filter(|block| {
                        block.offset.0.abs() <= entrance.half.0
                            && block.offset.2.abs() <= entrance.half.1
                            && matches!(block.part, Part::Platform | Part::Decor | Part::Lava)
                    })
                    .count();
                assert!(dressed >= 2, "{theme:?} seed {seed}: {dressed}");
                let chains = blueprint
                    .blocks
                    .iter()
                    .filter(|block| {
                        block.offset.0.abs() <= entrance.half.0
                            && block.offset.2.abs() <= entrance.half.1
                            && block.part == Part::Chain
                    })
                    .count();
                assert!(chains >= 3, "{theme:?} seed {seed}: {chains}");
                for y in 0..=2 {
                    assert!(
                        !blueprint
                            .blocks
                            .iter()
                            .any(|block| block.offset == (0, y, 0)),
                        "{theme:?} seed {seed}: arrival is blocked at y={y}"
                    );
                }
            }
        }
    }

    #[test]
    fn nether_dungeons_usually_have_a_lava_basin() {
        let mut dungeons = 0;
        let mut with_lava = 0;
        for seed in 0..300 {
            let blueprint = Blueprint::generate_with_theme(seed, 4, Theme::Nether);
            dungeons += 1;
            if blueprint
                .blocks
                .iter()
                .any(|block| block.part == Part::Lava)
            {
                with_lava += 1;
            }
        }
        assert!(
            with_lava * 10 >= dungeons * 9,
            "only {with_lava} of {dungeons} nether dungeons have a lava basin"
        );
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
    fn only_red_gates_lead_into_the_snow_valley() {
        steel_registry::init_vanilla_registry();
        for seed in 0..8 {
            let red = DungeonLayout::for_gate(seed, true);
            assert_eq!(red.blueprint.theme, Theme::SnowElves);
            let blue = DungeonLayout::for_gate(seed, false);
            assert_ne!(blue.blueprint.theme, Theme::SnowElves);
        }
    }

    #[test]
    fn walls_are_two_blocks_thick_and_mining_ore_opens_no_hole_outside() {
        const AROUND: [(i32, i32, i32); 6] = [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ];
        for seed in 0..50 {
            let blueprint = Blueprint::generate(seed, 5);
            let solid: FxHashSet<(i32, i32, i32)> =
                blueprint.blocks.iter().map(|block| block.offset).collect();
            let ((min_x, min_y, min_z), (max_x, max_y, max_z)) = blueprint.bounds;

            // The carved interior is sealed: a flood fill from the entrance
            // air stays strictly inside the shell and never reaches the void.
            let mut interior: FxHashSet<(i32, i32, i32)> = FxHashSet::default();
            let mut queue = VecDeque::from([(0, 0, 0)]);
            interior.insert((0, 0, 0));
            while let Some((x, y, z)) = queue.pop_front() {
                for (dx, dy, dz) in AROUND {
                    let next = (x + dx, y + dy, z + dz);
                    if solid.contains(&next) {
                        continue;
                    }
                    assert!(
                        next.0 > min_x
                            && next.0 < max_x
                            && next.1 > min_y
                            && next.1 < max_y
                            && next.2 > min_z
                            && next.2 < max_z,
                        "seed {seed}: dungeon leaks towards {next:?}"
                    );
                    if interior.insert(next) {
                        queue.push_back(next);
                    }
                }
            }

            // Mining a crystal ore out of the inner wall layer leaves every
            // exposed neighbour solid or interior: the backing wall keeps the
            // void outside.
            for block in blueprint
                .blocks
                .iter()
                .filter(|block| block.part == Part::Ore)
            {
                let (x, y, z) = block.offset;
                for (dx, dy, dz) in AROUND {
                    let neighbour = (x + dx, y + dy, z + dz);
                    assert!(
                        solid.contains(&neighbour) || interior.contains(&neighbour),
                        "seed {seed}: mining ore at {:?} opens a hole to {neighbour:?}",
                        block.offset
                    );
                }
            }
        }
    }
}
