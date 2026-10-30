//! Furniture inside the generated city buildings.
//!
//! Interiors are decided column by column like the rest of the city, so every
//! piece is a pure function of the lot-local position, the height above the
//! storey floor, and the district the building belongs to. Blocks with block
//! entities (beds, chests, barrels, lecterns, banners) are avoided: worldgen
//! writes plain block states, and those blocks would be broken without their
//! block entity.
//!
//! A building fills lot columns 7 to 27; its door is in the middle of the
//! north wall, so x 17 is the axis of every hall and z grows away from the
//! street.

use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction, SlabType};
use steel_registry::vanilla_blocks;
use steel_utils::BlockStateId;

use crate::city::District;

/// First interior column of a building, just inside its west/north wall.
pub const INTERIOR_MIN: i32 = 8;
/// Last interior column of a building, just inside its east/south wall.
pub const INTERIOR_MAX: i32 = 26;
/// Column of the door axis and of the halls built around it.
pub const AXIS: i32 = 17;
/// Column of the ladder shaft that connects every storey.
pub const LADDER_COLUMN: (i32, i32) = (AXIS, INTERIOR_MAX);
/// Spot behind the main counter kept free for the building's character.
pub const COUNTER_SPOT: (i32, i32) = (AXIS, 14);
/// Spot at the side desk of the Association lobby, for the starter clerk.
pub const CLERK_SPOT: (i32, i32) = (10, 11);

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
    // Keep the ladder and the doorway reachable.
    if (x - AXIS).abs() <= 1 && (z >= INTERIOR_MAX - 2 || (storey.level == 0 && z <= 9)) {
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

/// Returns whether `(x, z)` is on the main counter in front of
/// [`COUNTER_SPOT`]: a U open to the back.
const fn on_counter(x: i32, z: i32) -> bool {
    let front = z == COUNTER_SPOT.1 - 1 && x >= AXIS - 3 && x <= AXIS + 3;
    let side = (x - AXIS).abs() == 3 && z >= COUNTER_SPOT.1 - 1 && z <= COUNTER_SPOT.1 + 1;
    front || side
}

/// Returns the counter block at `rel`: the body, then a slab top.
fn counter(rel: i32, top: BlockRef, slab: BlockRef) -> Option<BlockStateId> {
    match rel {
        0 => Some(block(top)),
        1 => Some(block(slab)),
        _ => None,
    }
}

/// Hunter Association: a lobby with the Organization counter and the starter
/// clerk, offices above it.
fn association(x: i32, z: i32, storey: Storey) -> Option<BlockStateId> {
    if storey.level > 0 {
        return office(x, z, storey.rel);
    }
    let rel = storey.rel;
    if (x, z) == COUNTER_SPOT || (x, z) == CLERK_SPOT {
        return None;
    }
    if on_counter(x, z) {
        return counter(
            rel,
            &vanilla_blocks::SMOOTH_QUARTZ,
            &vanilla_blocks::QUARTZ_SLAB,
        );
    }
    // Rank boards behind the agent.
    if z == COUNTER_SPOT.1 + 3 && (AXIS - 3..=AXIS + 3).contains(&x) && (0..=2).contains(&rel) {
        return Some(if rel == 1 && x % 2 == 1 {
            block(&vanilla_blocks::CYAN_STAINED_GLASS)
        } else {
            block(&vanilla_blocks::POLISHED_BLACKSTONE)
        });
    }
    // The clerk's desk, facing the door side.
    if x == CLERK_SPOT.0 + 1 && (CLERK_SPOT.1 - 1..=CLERK_SPOT.1 + 1).contains(&z) && rel == 0 {
        return Some(top_slab(&vanilla_blocks::BIRCH_SLAB));
    }
    if rel != 0 {
        return pillars(x, z, rel, &vanilla_blocks::QUARTZ_PILLAR);
    }
    // Carpet runner from the door to the counter.
    if (x - AXIS).abs() <= 1 && (10..COUNTER_SPOT.1 - 1).contains(&z) {
        return Some(block(&vanilla_blocks::BLUE_CARPET));
    }
    // Waiting benches in the back of the hall.
    if z == 21 && ((9..=13).contains(&x) || (21..=25).contains(&x)) {
        return Some(facing(&vanilla_blocks::QUARTZ_STAIRS, Direction::South));
    }
    if matches!(x, INTERIOR_MIN | INTERIOR_MAX) && matches!(z, INTERIOR_MIN | 18) {
        return Some(block(&vanilla_blocks::POTTED_FERN));
    }
    pillars(x, z, rel, &vanilla_blocks::QUARTZ_PILLAR)
}

/// Hunter guild: a mess hall with a smithy, and barracks above it.
fn guild(x: i32, z: i32, storey: Storey) -> Option<BlockStateId> {
    let rel = storey.rel;
    if storey.level > 0 {
        return barracks(x, z, rel);
    }
    // Two long tables with benches along both sides.
    for table_x in [12, 22] {
        if (12..=22).contains(&z) && rel == 0 {
            if x == table_x {
                return Some(top_slab(&vanilla_blocks::DARK_OAK_SLAB));
            }
            if (x - table_x).abs() == 1 && z % 2 == 0 {
                let side = if x < table_x {
                    Direction::West
                } else {
                    Direction::East
                };
                return Some(facing(&vanilla_blocks::DARK_OAK_STAIRS, side));
            }
        }
    }
    // Smithy along the east wall.
    if x == INTERIOR_MAX && (10..=22).contains(&z) && rel == 0 {
        return Some(match z % 4 {
            0 => block(&vanilla_blocks::ANVIL),
            1 => block(&vanilla_blocks::SMITHING_TABLE),
            2 => block(&vanilla_blocks::GRINDSTONE),
            _ => block(&vanilla_blocks::STONECUTTER),
        });
    }
    // Training dummies along the west wall.
    if x == INTERIOR_MIN && matches!(z, 12 | 17 | 22) {
        return match rel {
            0 | 1 => Some(block(&vanilla_blocks::HAY_BLOCK)),
            2 => Some(facing(&vanilla_blocks::CARVED_PUMPKIN, Direction::East)),
            _ => None,
        };
    }
    // Weapon racks on the north wall beside the door.
    if z == INTERIOR_MIN && (x - AXIS).abs() > 2 && (1..=2).contains(&rel) && x % 3 == 0 {
        return Some(block(&vanilla_blocks::IRON_BARS));
    }
    None
}

/// City merchant: a shop counter, stalls of goods along the walls, and
/// storage above.
fn market(x: i32, z: i32, storey: Storey, variant: u64) -> Option<BlockStateId> {
    let rel = storey.rel;
    if storey.level > 0 {
        return storage(x, z, rel, variant);
    }
    if (x, z) == COUNTER_SPOT {
        return None;
    }
    if on_counter(x, z) {
        return counter(
            rel,
            &vanilla_blocks::STRIPPED_SPRUCE_WOOD,
            &vanilla_blocks::SPRUCE_SLAB,
        );
    }
    // Workshop wall behind the merchant: where orders are made.
    if z == COUNTER_SPOT.1 + 3 && (AXIS - 3..=AXIS + 3).contains(&x) && rel == 0 {
        return Some(match x.rem_euclid(4) {
            0 => block(&vanilla_blocks::SMITHING_TABLE),
            1 => block(&vanilla_blocks::LOOM),
            2 => block(&vanilla_blocks::FLETCHING_TABLE),
            _ => block(&vanilla_blocks::CRAFTING_TABLE),
        });
    }
    // Stalls along the side walls, under striped awnings.
    if matches!(x, INTERIOR_MIN | INTERIOR_MAX) && (10..=23).contains(&z) {
        return match rel {
            0 => Some(produce(x, z, variant)),
            1 if z % 3 == 0 => Some(produce(x + 1, z, variant)),
            3 => Some(if z % 2 == 0 {
                block(&vanilla_blocks::RED_WOOL)
            } else {
                block(&vanilla_blocks::WHITE_WOOL)
            }),
            _ => None,
        };
    }
    // Display stands of armor and weapons in the front of the shop.
    if matches!(x, 11 | 23) && matches!(z, 10 | 12) && rel <= 1 {
        return Some(if rel == 0 {
            block(&vanilla_blocks::POLISHED_ANDESITE)
        } else {
            block(&vanilla_blocks::IRON_BARS)
        });
    }
    None
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
    if (10..=24).contains(&x) && (x - AXIS).abs() > 2 && (12..=22).contains(&z) && z % 4 == 0 {
        return (rel <= 2).then(|| block(&vanilla_blocks::BOOKSHELF));
    }
    if rel == 0 && x == AXIS && matches!(z, 14 | 18) {
        return Some(top_slab(&vanilla_blocks::OAK_SLAB));
    }
    if rel == 0 && (x - AXIS).abs() == 1 && matches!(z, 14 | 18) {
        return Some(facing(
            &vanilla_blocks::OAK_STAIRS,
            if x < AXIS {
                Direction::West
            } else {
                Direction::East
            },
        ));
    }
    if matches!(x, INTERIOR_MIN | INTERIOR_MAX) && (9..=24).contains(&z) && rel <= 3 {
        return Some(block(&vanilla_blocks::BOOKSHELF));
    }
    None
}

/// Residential building: two flats per storey.
fn residential(x: i32, z: i32, storey: Storey, variant: u64) -> Option<BlockStateId> {
    let rel = storey.rel;
    // A wall splits each storey into an east and a west flat, with a doorway
    // off the central corridor.
    if (x - AXIS).abs() == 2 && z >= 12 {
        let doorway = matches!(z, 15 | 16) && rel <= 1;
        return (!doorway).then(|| block(&vanilla_blocks::SPRUCE_PLANKS));
    }
    if (x - AXIS).abs() < 2 {
        return None;
    }
    let fx = if x < AXIS {
        x - INTERIOR_MIN
    } else {
        INTERIOR_MAX - x
    };
    flat(fx, z, rel, variant)
}

/// One flat, mirrored so `fx` counts from its outer wall.
fn flat(fx: i32, z: i32, rel: i32, variant: u64) -> Option<BlockStateId> {
    match (fx, z, rel) {
        // Kitchen along the outer wall.
        (0, 10..=13, 0) => Some(if z % 2 == 0 {
            block(&vanilla_blocks::SMOKER)
        } else {
            block(&vanilla_blocks::CRAFTING_TABLE)
        }),
        // Dining table with two chairs.
        (3, 11, 0) => Some(top_slab(&vanilla_blocks::OAK_SLAB)),
        (3, 10 | 12, 0) => Some(facing(&vanilla_blocks::OAK_STAIRS, Direction::North)),
        // Bed made of a wool mattress.
        (0..=1, 22..=24, 0) => Some(if variant.is_multiple_of(2) {
            block(&vanilla_blocks::LIGHT_BLUE_WOOL)
        } else {
            block(&vanilla_blocks::RED_WOOL)
        }),
        // Wardrobe and a plant.
        (0, 18..=19, 0..=1) => Some(block(&vanilla_blocks::SPRUCE_PLANKS)),
        (5, 24, 0) => Some(block(&vanilla_blocks::POTTED_OXEYE_DAISY)),
        _ => None,
    }
}

/// Upper office storey with desks in rows.
fn office(x: i32, z: i32, rel: i32) -> Option<BlockStateId> {
    if rel != 0 || (x - AXIS).abs() <= 1 {
        return None;
    }
    let column = (x - INTERIOR_MIN).rem_euclid(5) == 1;
    let row = (z - INTERIOR_MIN).rem_euclid(5);
    if column && row == 2 {
        return Some(top_slab(&vanilla_blocks::BIRCH_SLAB));
    }
    if column && row == 3 {
        return Some(facing(&vanilla_blocks::BIRCH_STAIRS, Direction::North));
    }
    None
}

/// Guild barracks: rows of bunks.
fn barracks(x: i32, z: i32, rel: i32) -> Option<BlockStateId> {
    if (x - AXIS).abs() <= 1 || !(10..=22).contains(&z) || z % 3 != 1 {
        return None;
    }
    if !matches!(x, INTERIOR_MIN..=10 | 24..=INTERIOR_MAX) {
        return None;
    }
    match rel {
        0 | 2 => Some(block(&vanilla_blocks::GRAY_WOOL)),
        1 => Some(block(&vanilla_blocks::DARK_OAK_FENCE)),
        _ => None,
    }
}

/// Storage floor: stacks of goods along the walls.
fn storage(x: i32, z: i32, rel: i32, variant: u64) -> Option<BlockStateId> {
    let wall = matches!(x, INTERIOR_MIN | INTERIOR_MAX) || z == INTERIOR_MIN;
    if !wall || rel > 1 || (x - AXIS).abs() <= 2 {
        return None;
    }
    Some(produce(x, z + rel, variant))
}

/// Pillars in the four corners of a hall.
fn pillars(x: i32, z: i32, rel: i32, pillar: BlockRef) -> Option<BlockStateId> {
    (matches!(x, 9 | 25) && matches!(z, 9 | 24) && rel <= 4).then(|| block(pillar))
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::{
        AXIS, CLERK_SPOT, COUNTER_SPOT, INTERIOR_MAX, INTERIOR_MIN, LADDER_COLUMN, Storey,
        interior_state, ladder,
    };
    use crate::city::District;

    #[test]
    fn storeys_are_six_blocks_tall() {
        assert_eq!(Storey::at(4, 3), Storey { level: 0, rel: 0 });
        assert_eq!(Storey::at(11, 3), Storey { level: 1, rel: 0 });
        assert_eq!(Storey::at(17, 3), Storey { level: 2, rel: 0 });
    }

    #[test]
    fn characters_doors_and_ladder_stay_free() {
        init_vanilla_registry();
        let ground = Storey { level: 0, rel: 0 };
        for district in [
            District::Association,
            District::Guild,
            District::Market,
            District::Civic,
            District::Residential,
        ] {
            for rel in 0..=1 {
                let storey = Storey { level: 0, rel };
                for z in INTERIOR_MIN..=9 {
                    for x in AXIS - 1..=AXIS + 1 {
                        assert_eq!(interior_state(district, x, z, storey, 0), None);
                    }
                }
            }
            assert_eq!(
                interior_state(district, LADDER_COLUMN.0, LADDER_COLUMN.1, ground, 0),
                Some(ladder())
            );
            assert_eq!(
                interior_state(district, AXIS, INTERIOR_MAX - 1, ground, 0),
                None
            );
        }
        for spot in [COUNTER_SPOT, CLERK_SPOT] {
            for rel in 0..=1 {
                let storey = Storey { level: 0, rel };
                assert_eq!(
                    interior_state(District::Association, spot.0, spot.1, storey, 0),
                    None
                );
            }
        }
        assert_eq!(
            interior_state(District::Market, COUNTER_SPOT.0, COUNTER_SPOT.1, ground, 0),
            None
        );
    }
}
