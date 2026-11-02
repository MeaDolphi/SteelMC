//! The Demon Castle: a world of its own, climbed floor by floor.
//!
//! Every floor is a 320×320 hall of themed districts — courtyards, great
//! halls, bridges over lava, barracks and prisons — ending in the throne room
//! of the floor's boss. Dead ends hide treasure caches guarded by elites.
//! The boss hands every surviving hunter the key of its floor; the key opens
//! the gate beside the throne onto the next floor. Every tenth floor ends in
//! a round arena with the theme's giga boss ([`giga`]). Floors never end.
//!
//! The castle is built by its world generator, not block by block: a floor is
//! a pure function of the world seed and the floor number ([`plan`]), so its
//! chunks generate in parallel wherever a hunter needs them. Floors lie side
//! by side, never stacked: the slot of a party picks the column along X, the
//! floor number the row along Z. Each party climbs its own copy, and the
//! world keeps no chunk once unloaded, so a floor resets itself the moment
//! its party has left.

pub mod districts;
pub mod encounter;
pub mod generator;
pub mod giga;
pub mod plan;
pub mod theme;

use glam::DVec3;
use steel_core::world::World;
use steel_registry::vanilla_blocks;
use steel_utils::{BlockPos, BlockStateId, ChunkPos};

/// World name of the Demon Castle.
pub const WORLD_NAME: &str = "sololeveling_demon_castle";
/// Blocks between the centres of two neighbouring floors or slots.
pub const FLOOR_STRIDE: i32 = 512;
/// Districts along one side of a floor.
pub const GRID: usize = 4;
/// Side of one district, walls included.
pub const DISTRICT_SIZE: i32 = 80;
/// Half the side of a floor; a floor spans `-HALL_HALF..HALL_HALF` around
/// its centre on both axes.
pub const HALL_HALF: i32 = GRID as i32 * DISTRICT_SIZE / 2;
/// Height of the floor blocks every hunter walks on.
pub const GROUND_Y: i32 = 64;
/// Highest floor whose hall still fits inside the world border.
pub const MAX_FLOOR: u32 = (29_999_000 / FLOOR_STRIDE - 1) as u32;

/// First and last world column of the antechamber on both axes.
///
/// The world spawn of the castle lies between the halls, where a player
/// entering the world would find no ground: the spawn search would probe
/// chunk after chunk of void before giving up. The antechamber gives it solid
/// floor at once; the climb moves its hunters on to their floor the tick
/// they arrive.
const ANTECHAMBER: (i32, i32) = (-8, 24);

/// Returns the block of the antechamber at world column `(x, z)` and
/// `layer`, or `None` outside it.
#[must_use]
pub fn antechamber_block(x: i32, z: i32, layer: i32) -> Option<BlockStateId> {
    let span = ANTECHAMBER.0..=ANTECHAMBER.1;
    if !span.contains(&x) || !span.contains(&z) || layer != 0 {
        return None;
    }
    let edge = x == ANTECHAMBER.0 || x == ANTECHAMBER.1 || z == ANTECHAMBER.0 || z == ANTECHAMBER.1;
    let block = if edge {
        &vanilla_blocks::CHISELED_POLISHED_BLACKSTONE
    } else if x % 8 == 0 && z % 8 == 0 {
        &vanilla_blocks::SHROOMLIGHT
    } else {
        &vanilla_blocks::POLISHED_BLACKSTONE_BRICKS
    };
    Some(block.default_state())
}

/// Returns whether `world` is the Demon Castle.
#[must_use]
pub fn is_castle_world(world: &World) -> bool {
    world.key.path.as_ref() == WORLD_NAME
}

/// One party's copy of the castle: the column of floors at its X offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CastleSlot(pub u64);

impl CastleSlot {
    /// Returns the world X of the floor centres of this slot.
    fn center_x(self) -> i32 {
        i32::try_from(self.0)
            .unwrap_or(i32::MAX)
            .saturating_mul(FLOOR_STRIDE)
    }

    /// Returns the centre of `floor` in this slot, at the walking surface.
    #[must_use]
    pub fn center(self, floor: u32) -> BlockPos {
        BlockPos::new(self.center_x(), GROUND_Y, floor_center_z(floor))
    }

    /// Returns the world block at hall coordinates `(lx, lz)` of `floor`.
    #[must_use]
    pub fn block(self, floor: u32, lx: i32, lz: i32, y: i32) -> BlockPos {
        let center = self.center(floor);
        BlockPos::new(center.x() + lx, y, center.z() + lz)
    }

    /// Returns the hall coordinates of `position` when it stands on `floor`
    /// of this slot.
    #[must_use]
    pub fn local(self, floor: u32, position: DVec3) -> Option<(i32, i32)> {
        let center = self.center(floor);
        let lx = position.x.floor() as i32 - center.x();
        let lz = position.z.floor() as i32 - center.z();
        ((-HALL_HALF..HALL_HALF).contains(&lx) && (-HALL_HALF..HALL_HALF).contains(&lz))
            .then_some((lx, lz))
    }

    /// Returns every chunk column the hall of `floor` covers.
    #[must_use]
    pub fn floor_chunks(self, floor: u32) -> Vec<ChunkPos> {
        let center = self.center(floor);
        let min = ChunkPos::from_block_pos(BlockPos::new(
            center.x() - HALL_HALF,
            GROUND_Y,
            center.z() - HALL_HALF,
        ));
        let max = ChunkPos::from_block_pos(BlockPos::new(
            center.x() + HALL_HALF - 1,
            GROUND_Y,
            center.z() + HALL_HALF - 1,
        ));
        let mut chunks = Vec::new();
        for x in min.0.x..=max.0.x {
            for z in min.0.y..=max.0.y {
                chunks.push(ChunkPos::new(x, z));
            }
        }
        chunks
    }
}

/// Returns the world Z of the centre of `floor`.
fn floor_center_z(floor: u32) -> i32 {
    i32::try_from(floor.min(MAX_FLOOR))
        .unwrap_or(i32::MAX)
        .saturating_mul(FLOOR_STRIDE)
}

/// Returns the floor and the hall coordinates of the world column `(x, z)`,
/// or `None` for the void between halls.
#[must_use]
pub fn locate(x: i32, z: i32) -> Option<(u32, i32, i32)> {
    let half = FLOOR_STRIDE / 2;
    let row = (z + half).div_euclid(FLOOR_STRIDE);
    let floor = u32::try_from(row)
        .ok()
        .filter(|floor| (1..=MAX_FLOOR).contains(floor))?;
    let lz = z - row * FLOOR_STRIDE;
    let lx = x - (x + half).div_euclid(FLOOR_STRIDE) * FLOOR_STRIDE;
    ((-HALL_HALF..HALL_HALF).contains(&lx) && (-HALL_HALF..HALL_HALF).contains(&lz))
        .then_some((floor, lx, lz))
}

/// Hands out castle slots and reuses the ones finished climbs gave back.
#[derive(Debug, Default)]
pub struct CastleSlots {
    next: u64,
    free: Vec<u64>,
}

impl CastleSlots {
    /// Allocates one slot.
    pub fn allocate(&mut self) -> CastleSlot {
        CastleSlot(self.free.pop().unwrap_or_else(|| {
            let slot = self.next;
            self.next = self.next.saturating_add(1);
            slot
        }))
    }

    /// Gives `slot` back once its climb is over.
    pub fn release(&mut self, slot: CastleSlot) {
        if !self.free.contains(&slot.0) {
            self.free.push(slot.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CastleSlot, FLOOR_STRIDE, HALL_HALF, MAX_FLOOR, antechamber_block, locate};
    use glam::DVec3;

    #[test]
    fn every_hall_column_locates_back_to_its_floor_and_slot() {
        // The generator only sees world columns; a floor must map back to
        // itself at every slot and every edge, up to the last floor.
        for slot in [0_u64, 1, 57_000] {
            for floor in [1_u32, 2, 10, 1_000, MAX_FLOOR] {
                for (lx, lz) in [
                    (-HALL_HALF, -HALL_HALF),
                    (HALL_HALF - 1, HALL_HALF - 1),
                    (0, 0),
                    (-1, HALL_HALF - 1),
                ] {
                    let pos = CastleSlot(slot).block(floor, lx, lz, 64);
                    assert_eq!(
                        locate(pos.x(), pos.z()),
                        Some((floor, lx, lz)),
                        "slot {slot} floor {floor} ({lx}, {lz})"
                    );
                    let at = DVec3::new(f64::from(pos.x()) + 0.5, 65.0, f64::from(pos.z()) + 0.5);
                    assert_eq!(CastleSlot(slot).local(floor, at), Some((lx, lz)));
                }
            }
        }
        // The void between halls and the rows before the first floor stay empty.
        assert_eq!(locate(HALL_HALF, FLOOR_STRIDE), None);
        assert_eq!(locate(0, 0), None);
        assert_eq!(locate(0, -FLOOR_STRIDE), None);
        // The antechamber under the world spawn lies in the void between halls.
        assert!(antechamber_block(8, 8, 0).is_some());
        assert_eq!(locate(8, 8), None);
        let top = CastleSlot(0).center(MAX_FLOOR);
        assert!(
            top.z() + HALL_HALF < 29_999_984,
            "the last floor leaves the world"
        );
    }
}
