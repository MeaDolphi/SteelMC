//! Furniture inside the generated city buildings.
//!
//! Interiors are decided column by column like the rest of the city, so every
//! piece is a pure function of the lot-local position, the height above the
//! storey floor, and the district the building belongs to. Blocks with block
//! entities (beds, chests, barrels, lecterns, banners) are avoided: worldgen
//! writes plain block states, and those blocks would be broken without their
//! block entity.

use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction, SlabType};
use steel_registry::vanilla_blocks;
use steel_utils::BlockStateId;

use crate::city::District;

/// First interior column of a building, just inside its west/north wall.
pub const INTERIOR_MIN: i32 = 15;
/// Last interior column of a building, just inside its east/south wall.
pub const INTERIOR_MAX: i32 = 49;
/// Column of the ladder shaft that connects every storey.
pub const LADDER_COLUMN: (i32, i32) = (31, 49);
/// Spot on the ground floor of the Association lobby kept free for the
/// Organization agent.
pub const AGENT_SPOT: (i32, i32) = (31, 25);

/// One storey of a building and the height above its floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Storey {
    /// Zero for the ground floor.
    pub level: i32,
    /// Blocks above the storey's floor, starting at zero.
    pub rel: i32,
}

impl Storey {
    /// Returns the storey of an interior block at `y` in a building whose
    /// ground floor is `ground_y`; storey floors sit every six blocks above
    /// the first one at `ground_y + 7`.
    #[must_use]
    pub const fn at(y: i32, ground_y: i32) -> Self {
        let first_floor = ground_y + 7;
        if y < first_floor {
            return Self {
                level: 0,
                rel: y - ground_y - 1,
            };
        }
        let level = (y - first_floor) / 6 + 1;
        Self {
            level,
            rel: y - (first_floor + 6 * (level - 1)) - 1,
        }
    }
}

fn block(block: BlockRef) -> BlockStateId {
    block.default_state()
}

fn facing(block: BlockRef, direction: Direction) -> BlockStateId {
    block
        .default_state()
        .set_value(&BlockStateProperties::HORIZONTAL_FACING, direction)
}

fn top_slab(block: BlockRef) -> BlockStateId {
    block
        .default_state()
        .set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Top)
}

/// Returns the ladder block of the shaft, hooked onto the south wall.
#[must_use]
pub fn ladder() -> BlockStateId {
    facing(&vanilla_blocks::LADDER, Direction::North)
}

/// Returns the furniture at an interior position, or `None` for open air.
///
/// `variant` is a per-building roll so neighbouring buildings of one district
/// do not look identical.
#[must_use]
pub fn interior_state(
    district: District,
    x: i32,
    z: i32,
    storey: Storey,
    variant: u64,
) -> Option<BlockStateId> {
    if (x, z) == LADDER_COLUMN {
        return Some(ladder());
    }
    // Keep the ladder reachable from the corridor in front of it.
    if (x - LADDER_COLUMN.0).abs() <= 1 && z >= LADDER_COLUMN.1 - 2 {
        return None;
    }
    match district {
        District::Association => association(x, z, storey),
        District::Guild => guild(x, z, storey),
        District::Market => market(x, z, storey, variant),
        District::Civic => civic(x, z, storey),
        District::Residential => residential(x, z, storey, variant),
        District::Plaza | District::GateSquare => None,
    }
}

/// Hunter Association: a lobby with the Organization reception, and offices
/// above it.
fn association(x: i32, z: i32, storey: Storey) -> Option<BlockStateId> {
    if storey.level > 0 {
        return office(x, z, storey);
    }
    let rel = storey.rel;
    // Pillars of the hall.
    if matches!(x, 20 | 42) && matches!(z, 20 | 40) {
        return Some(block(&vanilla_blocks::QUARTZ_PILLAR));
    }
    if (x, z) == AGENT_SPOT || (x, z) == (AGENT_SPOT.0, AGENT_SPOT.1 + 1) {
        return None;
    }
    // Carpet runner from the door to the reception.
    if rel == 0 && (30..=32).contains(&x) && (15..=22).contains(&z) {
        return Some(block(&vanilla_blocks::BLUE_CARPET));
    }
    // U-shaped reception counter around the agent.
    let counter =
        (z == 23 && (26..=36).contains(&x)) || (matches!(x, 26 | 36) && (23..=27).contains(&z));
    if counter {
        return match rel {
            0 => Some(block(&vanilla_blocks::SMOOTH_QUARTZ)),
            1 => Some(block(&vanilla_blocks::QUARTZ_SLAB)),
            _ => None,
        };
    }
    // Rank boards behind the agent.
    if z == 29 && (27..=35).contains(&x) && (0..=2).contains(&rel) {
        return Some(if rel == 1 && x % 2 == 1 {
            block(&vanilla_blocks::CYAN_STAINED_GLASS)
        } else {
            block(&vanilla_blocks::POLISHED_BLACKSTONE)
        });
    }
    if rel != 0 {
        return bookshelf_walls(x, z, rel, 30);
    }
    // Waiting benches facing the reception.
    if matches!(z, 17 | 20) && ((18..=25).contains(&x) || (37..=44).contains(&x)) {
        return Some(facing(&vanilla_blocks::QUARTZ_STAIRS, Direction::North));
    }
    // Plants in the corners of the hall.
    if matches!(x, INTERIOR_MIN | INTERIOR_MAX) && matches!(z, INTERIOR_MIN | 28) {
        return Some(block(&vanilla_blocks::POTTED_FERN));
    }
    // Briefing tables with maps in the back of the hall.
    if matches!(z, 36 | 37) && matches!(x, 20 | 21 | 41 | 42) {
        return Some(block(&vanilla_blocks::CARTOGRAPHY_TABLE));
    }
    bookshelf_walls(x, z, rel, 30)
}

/// Hunter guild: a mess hall with a smithy, and barracks above it.
fn guild(x: i32, z: i32, storey: Storey) -> Option<BlockStateId> {
    let rel = storey.rel;
    if storey.level > 0 {
        return barracks(x, z, rel);
    }
    // Long tables with benches along both sides.
    for table_x in [22, 40] {
        if (20..=40).contains(&z) {
            if x == table_x && rel == 0 {
                return Some(top_slab(&vanilla_blocks::DARK_OAK_SLAB));
            }
            if x == table_x - 1 && rel == 0 && z % 2 == 0 {
                return Some(facing(&vanilla_blocks::DARK_OAK_STAIRS, Direction::West));
            }
            if x == table_x + 1 && rel == 0 && z % 2 == 0 {
                return Some(facing(&vanilla_blocks::DARK_OAK_STAIRS, Direction::East));
            }
        }
    }
    // Smithy along the east wall.
    if x == INTERIOR_MAX && (18..=44).contains(&z) && rel == 0 {
        return Some(match z % 4 {
            0 => block(&vanilla_blocks::ANVIL),
            1 => block(&vanilla_blocks::SMITHING_TABLE),
            2 => block(&vanilla_blocks::GRINDSTONE),
            _ => block(&vanilla_blocks::STONECUTTER),
        });
    }
    // Training dummies along the west wall.
    if x == 17 && matches!(z, 22 | 28 | 34 | 40) {
        return match rel {
            0 | 1 => Some(block(&vanilla_blocks::HAY_BLOCK)),
            2 => Some(facing(&vanilla_blocks::CARVED_PUMPKIN, Direction::East)),
            _ => None,
        };
    }
    // Weapon racks: iron bars on the north wall beside the door.
    if z == INTERIOR_MIN && !(29..=33).contains(&x) && (1..=2).contains(&rel) && x % 3 == 0 {
        return Some(block(&vanilla_blocks::IRON_BARS));
    }
    None
}

/// Market hall: stalls with produce, and storage above.
fn market(x: i32, z: i32, storey: Storey, variant: u64) -> Option<BlockStateId> {
    let rel = storey.rel;
    if storey.level > 0 {
        return storage(x, z, rel, variant);
    }
    // Two rows of stalls left and right of the central aisle.
    let stall_x = (18..=27).contains(&x) || (35..=46).contains(&x);
    let stall_z = (z - 19).rem_euclid(9);
    if !stall_x || !(19..=46).contains(&z) {
        return None;
    }
    match (stall_z, rel) {
        // Counter facing the aisle.
        (0, 0) => Some(top_slab(&vanilla_blocks::SPRUCE_SLAB)),
        // Goods behind the counter.
        (2, 0) => Some(produce(x, z, variant)),
        (2, 1) if x % 3 == 0 => Some(produce(x + 1, z, variant)),
        // Awning posts and roof.
        (0 | 2, 1 | 2) if matches!(x, 18 | 27 | 35 | 46) => {
            Some(block(&vanilla_blocks::SPRUCE_FENCE))
        }
        (0..=2, 3) => Some(if (x + z) % 2 == 0 {
            block(&vanilla_blocks::RED_WOOL)
        } else {
            block(&vanilla_blocks::WHITE_WOOL)
        }),
        _ => None,
    }
}

fn produce(x: i32, z: i32, variant: u64) -> BlockStateId {
    let goods = [
        &vanilla_blocks::MELON,
        &vanilla_blocks::PUMPKIN,
        &vanilla_blocks::HAY_BLOCK,
        &vanilla_blocks::COMPOSTER,
        &vanilla_blocks::CAULDRON,
    ];
    let index = (x as u64)
        .wrapping_mul(31)
        .wrapping_add(z as u64)
        .wrapping_add(variant)
        % goods.len() as u64;
    block(goods[index as usize])
}

/// Civic building: a library with reading tables on every storey.
fn civic(x: i32, z: i32, storey: Storey) -> Option<BlockStateId> {
    let rel = storey.rel;
    // Rows of shelves in the west half.
    if (17..=27).contains(&x) && (18..=46).contains(&z) && z % 4 == 2 && rel <= 2 {
        return Some(block(&vanilla_blocks::BOOKSHELF));
    }
    // Reading tables with chairs in the east half.
    if (36..=46).contains(&x) && (19..=45).contains(&z) && rel == 0 {
        let cell_x = (x - 36).rem_euclid(5);
        let cell_z = (z - 19).rem_euclid(6);
        return match (cell_x, cell_z) {
            (1 | 2, 1) => Some(top_slab(&vanilla_blocks::SPRUCE_SLAB)),
            (1 | 2, 0) => Some(facing(&vanilla_blocks::SPRUCE_STAIRS, Direction::North)),
            (1 | 2, 2) => Some(facing(&vanilla_blocks::SPRUCE_STAIRS, Direction::South)),
            _ => None,
        };
    }
    if rel == 0 && matches!(x, INTERIOR_MIN | INTERIOR_MAX) && z % 8 == 0 {
        return Some(block(&vanilla_blocks::POTTED_AZALEA_BUSH));
    }
    None
}

/// Apartment block: a lobby on the ground floor and four flats on every
/// storey above it, off a central corridor.
fn residential(x: i32, z: i32, storey: Storey, variant: u64) -> Option<BlockStateId> {
    let rel = storey.rel;
    if storey.level == 0 {
        if rel == 0 && (30..=32).contains(&x) {
            return Some(block(&vanilla_blocks::GRAY_CARPET));
        }
        if rel == 0 && matches!(x, 17 | 45) && z % 6 == 0 {
            return Some(block(&vanilla_blocks::POTTED_FERN));
        }
        // Letter boxes on the walls of the lobby.
        if matches!(x, INTERIOR_MIN | INTERIOR_MAX) && (20..=30).contains(&z) && rel == 1 {
            return Some(facing(
                &vanilla_blocks::SPRUCE_TRAPDOOR,
                if x == INTERIOR_MIN {
                    Direction::East
                } else {
                    Direction::West
                },
            ));
        }
        return None;
    }
    // Corridor walls with a door gap into every flat.
    if matches!(x, 29 | 33) {
        let door = matches!(z, 23 | 41) && rel <= 1;
        return (!door).then(|| block(&vanilla_blocks::WHITE_CONCRETE));
    }
    if (30..=32).contains(&x) {
        return (rel == 0).then(|| block(&vanilla_blocks::GRAY_CARPET));
    }
    // Wall between the north and south flat on each side.
    if z == 32 {
        return Some(block(&vanilla_blocks::WHITE_CONCRETE));
    }
    // Flat-local coordinates, mirrored so every flat opens on the corridor.
    let east = x > 32;
    let south = z > 32;
    let fx = if east { x - 34 } else { 28 - x };
    let fz = if south { z - 33 } else { 31 - z };
    flat(
        fx,
        fz,
        rel,
        variant ^ u64::from(east) ^ (u64::from(south) << 1),
    )
}

/// Furnishes one flat; `fx` grows away from the corridor and `fz` away from
/// the wall between flats.
fn flat(fx: i32, fz: i32, rel: i32, variant: u64) -> Option<BlockStateId> {
    if rel > 0 {
        // Shelf above the kitchen.
        return (rel == 2 && fz == 15 && (2..=6).contains(&fx))
            .then(|| block(&vanilla_blocks::BOOKSHELF));
    }
    let rug = [
        &vanilla_blocks::RED_CARPET,
        &vanilla_blocks::LIGHT_BLUE_CARPET,
        &vanilla_blocks::GREEN_CARPET,
        &vanilla_blocks::YELLOW_CARPET,
    ][(variant % 4) as usize];
    match (fx, fz) {
        // Bed in the far corner: foot and pillow.
        (11 | 12, 2) => Some(block(&vanilla_blocks::RED_WOOL)),
        (11 | 12, 1) => Some(block(&vanilla_blocks::WHITE_WOOL)),
        (13, 1) => Some(block(&vanilla_blocks::POTTED_OAK_SAPLING)),
        // Wardrobe.
        (13, 5 | 6) => Some(block(&vanilla_blocks::SPRUCE_PLANKS)),
        // Kitchen along the outer wall: counter, sink and stove.
        (2..=6, 15) => Some(match fx {
            2 => block(&vanilla_blocks::WATER_CAULDRON),
            4 => block(&vanilla_blocks::SMOOTH_STONE),
            _ => top_slab(&vanilla_blocks::SMOOTH_STONE_SLAB),
        }),
        // Dining table with two chairs.
        (5, 10) => Some(block(&vanilla_blocks::OAK_FENCE)),
        (4, 10) => Some(facing(&vanilla_blocks::OAK_STAIRS, Direction::West)),
        (6, 10) => Some(facing(&vanilla_blocks::OAK_STAIRS, Direction::East)),
        // Rug in the middle of the room.
        (8..=10, 7..=9) => Some(block(rug)),
        _ => None,
    }
}

/// Offices: desks with chairs in a grid and shelves on the walls.
fn office(x: i32, z: i32, storey: Storey) -> Option<BlockStateId> {
    let rel = storey.rel;
    if (19..=45).contains(&x) && (18..=44).contains(&z) && rel == 0 {
        let cell_x = (x - 19).rem_euclid(7);
        let cell_z = (z - 18).rem_euclid(7);
        return match (cell_x, cell_z) {
            (1 | 2, 1) => Some(top_slab(&vanilla_blocks::DARK_OAK_SLAB)),
            (1, 2) => Some(facing(&vanilla_blocks::DARK_OAK_STAIRS, Direction::South)),
            (3, 1) => Some(block(&vanilla_blocks::POTTED_FERN)),
            _ => None,
        };
    }
    bookshelf_walls(x, z, rel, INTERIOR_MIN)
}

/// Barracks: rows of bunks with lockers between them.
fn barracks(x: i32, z: i32, rel: i32) -> Option<BlockStateId> {
    if !(18..=46).contains(&z) || rel > 0 {
        return None;
    }
    let along_west = (17..=18).contains(&x);
    let along_east = (44..=45).contains(&x);
    if !along_west && !along_east {
        return None;
    }
    let pillow = if along_west { x == 17 } else { x == 45 };
    Some(match (z - 18).rem_euclid(4) {
        0 | 1 if pillow => block(&vanilla_blocks::WHITE_WOOL),
        0 | 1 => block(&vanilla_blocks::BLUE_WOOL),
        2 => block(&vanilla_blocks::SPRUCE_PLANKS),
        _ => return None,
    })
}

/// Storage: stacks of crates and sacks along the walls.
fn storage(x: i32, z: i32, rel: i32, variant: u64) -> Option<BlockStateId> {
    let near_wall = x <= 18 || x >= 46 || z <= 18;
    if !near_wall || rel > 1 {
        return None;
    }
    let pick = (x as u64)
        .wrapping_mul(7)
        .wrapping_add((z as u64).wrapping_mul(13))
        .wrapping_add(variant)
        % 5;
    match pick {
        0 => None,
        1 | 2 => Some(block(&vanilla_blocks::SPRUCE_PLANKS)),
        3 => Some(block(&vanilla_blocks::HAY_BLOCK)),
        _ => (rel == 0).then(|| block(&vanilla_blocks::BROWN_WOOL)),
    }
}

/// Bookshelves lining the side walls, from `from_z` to the back wall.
fn bookshelf_walls(x: i32, z: i32, rel: i32, from_z: i32) -> Option<BlockStateId> {
    (matches!(x, INTERIOR_MIN | INTERIOR_MAX) && z >= from_z && rel <= 2)
        .then(|| block(&vanilla_blocks::BOOKSHELF))
}

#[cfg(test)]
mod tests {
    use super::{AGENT_SPOT, LADDER_COLUMN, Storey, interior_state, ladder};
    use crate::city::District;
    use steel_registry::init_vanilla_registry;

    #[test]
    fn storeys_follow_the_six_block_floor_spacing() {
        assert_eq!(Storey::at(4, 3), Storey { level: 0, rel: 0 });
        assert_eq!(Storey::at(9, 3), Storey { level: 0, rel: 5 });
        assert_eq!(Storey::at(11, 3), Storey { level: 1, rel: 0 });
        assert_eq!(Storey::at(17, 3), Storey { level: 2, rel: 0 });
    }

    #[test]
    fn every_building_keeps_its_ladder_and_the_agent_spot_free() {
        init_vanilla_registry();
        for district in [
            District::Association,
            District::Guild,
            District::Market,
            District::Civic,
            District::Residential,
        ] {
            for level in 0..3 {
                let storey = Storey { level, rel: 0 };
                assert_eq!(
                    interior_state(district, LADDER_COLUMN.0, LADDER_COLUMN.1, storey, 0),
                    Some(ladder())
                );
            }
        }
        for rel in 0..4 {
            let storey = Storey { level: 0, rel };
            assert_eq!(
                interior_state(District::Association, AGENT_SPOT.0, AGENT_SPOT.1, storey, 0),
                None
            );
        }
    }

    #[test]
    fn the_ground_floor_door_opens_onto_free_space() {
        init_vanilla_registry();
        for district in [
            District::Association,
            District::Guild,
            District::Market,
            District::Civic,
            District::Residential,
        ] {
            for x in 30..=32 {
                for rel in 1..=2 {
                    let storey = Storey { level: 0, rel };
                    assert_eq!(interior_state(district, x, 15, storey, 0), None);
                }
            }
        }
    }
}
