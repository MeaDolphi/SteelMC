//! Block and entity fixtures for gates and dungeon/penalty instances.
use std::sync::Arc;

use glam::DVec3;
use steel_core::entity::spawn_mob;
use steel_core::world::World;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::{vanilla_attributes, vanilla_blocks, vanilla_entities};
use steel_utils::BlockPos;
use steel_utils::ChunkPos;
use steel_utils::types::{BlockStateId, UpdateFlags};
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;

use super::{
    BossAbilitySet, DungeonLayout, DungeonWave, Gate, INSTANCE_FLOOR_Y, InstanceRegion,
    boss_for_gate,
};
use crate::rank::HunterRank;

/// Interior width and depth of one dungeon room.
const ROOM_SIZE: i32 = 13;
/// Distance between dungeon room origins.
const ROOM_STRIDE: i32 = 20;
/// Interior height of one dungeon room.
const ROOM_HEIGHT: i32 = 5;
/// Interior width and depth of the penalty arena.
const PENALTY_ARENA_SIZE: i32 = 31;
/// Interior height of the penalty arena.
const PENALTY_ARENA_HEIGHT: i32 = 7;
/// Zombies per arena wave are laid out five across.
const PENALTY_WAVE_COLUMNS: u8 = 5;
/// Gap between zombies of the same arena wave.
const PENALTY_WAVE_SPACING: i32 = 6;

/// The block crystal ore is built from.
///
/// A full cube keeps a wall airtight; the amethyst appearance is what tells a
/// hunter the block is worth mining, since the item it drops is themed anyway.
pub const CRYSTAL_ORE: BlockRef = &vanilla_blocks::AMETHYST_BLOCK;

/// Roughly one wall block in this many is crystal ore.
const ORE_CHANCE: u64 = 23;

/// Returns whether `state` is the crystal ore dungeon walls are built from.
#[must_use]
pub fn is_crystal_ore(state: BlockStateId) -> bool {
    state == CRYSTAL_ORE.default_state()
}

/// Returns whether the wall block at this offset of a room is ore.
///
/// The pattern is hashed from the room corner and the offset instead of drawn
/// from an RNG, so a reused instance region always offers the same veins and a
/// dungeon cannot be rerolled by leaving and entering again.
const fn ore_at(corner: BlockPos, x: i32, y: i32, z: i32) -> bool {
    let hash = (corner.x() as u64).wrapping_mul(73_856_093)
        ^ (x as u64).wrapping_mul(19_349_663)
        ^ (y as u64).wrapping_mul(83_492_791)
        ^ (z as u64).wrapping_mul(2_654_435_761);
    hash.is_multiple_of(ORE_CHANCE)
}

/// Returns the quartz blocks that make up one gate entrance frame.
#[must_use]
pub fn gate_frame_positions(entrance: BlockPos) -> Vec<BlockPos> {
    let mut positions = Vec::new();
    for x in -1..=2 {
        positions.push(BlockPos::new(entrance.x() + x, entrance.y(), entrance.z()));
        positions.push(BlockPos::new(
            entrance.x() + x,
            entrance.y() + 4,
            entrance.z(),
        ));
    }
    for y in 1..4 {
        positions.push(BlockPos::new(
            entrance.x() - 1,
            entrance.y() + y,
            entrance.z(),
        ));
        positions.push(BlockPos::new(
            entrance.x() + 2,
            entrance.y() + y,
            entrance.z(),
        ));
    }
    positions
}

/// Returns whether `position` stands inside the opening of a quartz frame based
/// at `entrance`: two blocks wide, three blocks high, one block deep.
#[must_use]
pub fn opening_contains(entrance: BlockPos, position: DVec3) -> bool {
    position.x >= f64::from(entrance.x())
        && position.x < f64::from(entrance.x() + 2)
        && position.y >= f64::from(entrance.y() + 1)
        && position.y < f64::from(entrance.y() + 4)
        && position.z >= f64::from(entrance.z())
        && position.z < f64::from(entrance.z() + 1)
}

/// Returns the chunk columns the frame of a gate at `entrance` occupies.
///
/// The frame is four blocks wide along a single z, so it can straddle a chunk
/// border; the far column then has to be loaded as well, because
/// [`World::set_block`] silently fails for an unloaded chunk.
#[must_use]
pub fn gate_frame_chunks(entrance: BlockPos) -> Vec<ChunkPos> {
    let mut chunks: Vec<ChunkPos> = Vec::with_capacity(2);
    for pos in gate_frame_positions(entrance) {
        let chunk = ChunkPos::from_block_pos(pos);
        if !chunks.contains(&chunk) {
            chunks.push(chunk);
        }
    }
    chunks
}

/// Returns the chunk columns [`build_dungeon`] writes for `layout`.
///
/// The layout is longer than the spawn chunk and its last rooms cross the
/// chunk border, so the whole built box has to be requested before any block is
/// written: [`World::set_block`] silently drops writes to unloaded chunks, which
/// would leave holes a hunter and its mobs fall through.
#[must_use]
pub fn dungeon_chunks(region: InstanceRegion, layout: &DungeonLayout) -> Vec<ChunkPos> {
    let origin = BlockPos::from(region.spawn);
    let floor_y = INSTANCE_FLOOR_Y - 1;
    let last_corner_x = origin.x() + i32::from(layout.total_room_count() - 1) * ROOM_STRIDE;
    // Corridors run between two rooms and stay inside the same box.
    chunks_covering(
        BlockPos::new(origin.x() - 1, floor_y, origin.z() - 1),
        BlockPos::new(
            last_corner_x + ROOM_SIZE,
            floor_y + ROOM_HEIGHT,
            origin.z() + ROOM_SIZE,
        ),
    )
}

/// Returns the chunk columns [`build_penalty_arena`] writes.
#[must_use]
pub fn penalty_arena_chunks(region: InstanceRegion) -> Vec<ChunkPos> {
    let origin = BlockPos::from(region.spawn);
    let corner = penalty_arena_corner(origin);
    chunks_covering(
        BlockPos::new(corner.x() - 1, corner.y(), corner.z() - 1),
        BlockPos::new(
            corner.x() + PENALTY_ARENA_SIZE,
            corner.y() + PENALTY_ARENA_HEIGHT,
            corner.z() + PENALTY_ARENA_SIZE,
        ),
    )
}

/// Returns every chunk column the block box `min`..=`max` touches.
fn chunks_covering(min: BlockPos, max: BlockPos) -> Vec<ChunkPos> {
    let min = ChunkPos::from_block_pos(min);
    let max = ChunkPos::from_block_pos(max);
    let mut chunks = Vec::new();
    for x in min.0.x..=max.0.x {
        for z in min.0.y..=max.0.y {
            chunks.push(ChunkPos::new(x, z));
        }
    }
    chunks
}

/// Returns whether every gate frame block is currently air.
pub fn gate_space_is_clear(world: &World, entrance: BlockPos) -> bool {
    gate_frame_positions(entrance)
        .into_iter()
        .all(|pos| world.get_block_state(pos) == vanilla_blocks::AIR.default_state())
}

/// Returns whether a gate can stand at `entrance` without hanging over water.
///
/// The centre column is already known to be dry, because the entrance was
/// found through [`World::safe_surface_position`], which skips water columns.
/// A frame is four blocks wide though, so on a shoreline the outer columns
/// would still reach out over the water surface. Every frame column therefore
/// needs dry land of its own, and the opening has to be free of fluid.
pub fn gate_ground_is_dry(world: &World, entrance: BlockPos) -> bool {
    gate_is_dry_at(
        entrance,
        |x, z| world.safe_surface_position(x, z).is_some(),
        |pos| world.get_block_state(pos),
    )
}

fn gate_is_dry_at(
    entrance: BlockPos,
    column_has_land: impl Fn(i32, i32) -> bool,
    state_at: impl Fn(BlockPos) -> BlockStateId,
) -> bool {
    let frames_stand_on_land =
        (-1..=2).all(|offset| column_has_land(entrance.x() + offset, entrance.z()));
    let opening_is_dry = (1..4).all(|height| {
        (0..2).all(|offset| {
            let pos = BlockPos::new(entrance.x() + offset, entrance.y() + height, entrance.z());
            !state_at(pos).has_fluid()
        })
    });
    frames_stand_on_land && opening_is_dry
}

/// Returns the frame block of a gate: quartz for a normal gate, red glass for
/// a red gate.
///
/// The block is derived from the gate instead of being stored, so a gate can
/// never disagree with the frame standing in the world.
fn gate_frame_block(red: bool) -> BlockStateId {
    if red {
        vanilla_blocks::RED_STAINED_GLASS.default_state()
    } else {
        vanilla_blocks::QUARTZ_BLOCK.default_state()
    }
}

/// Places the frame of a new gate.
pub fn place_gate_frame(world: &Arc<World>, entrance: BlockPos, red: bool) {
    let block = gate_frame_block(red);
    let mut failed = 0;
    for pos in gate_frame_positions(entrance) {
        set_fixture(world, pos, block, &mut failed);
    }
    warn_unwritten("gate frame", entrance, failed);
}

/// Restores a gate entrance to air, leaving any other block untouched.
pub fn remove_gate_frame(world: &Arc<World>, gate: &Gate) {
    let block = gate_frame_block(gate.red);
    for pos in gate_frame_positions(gate.entrance) {
        let _ = world.set_block_if_unchanged(
            pos,
            block,
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
    }
}

/// Builds the entrance, combat, and reserved boss rooms of one dungeon.
pub fn build_dungeon(world: &Arc<World>, region: InstanceRegion, layout: &DungeonLayout) {
    let origin = BlockPos::from(region.spawn);
    let floor_y = INSTANCE_FLOOR_Y - 1;
    for room in 0..layout.total_room_count() {
        let corner_x = origin.x() + i32::from(room) * ROOM_STRIDE;
        build_room(
            world,
            BlockPos::new(corner_x, floor_y, origin.z()),
            ROOM_SIZE,
            ROOM_HEIGHT,
            true,
        );
    }
    // Doorways are carved only after every room exists, otherwise the next
    // room's wall would brick up each corridor again.
    for room in 0..layout.boss_room() {
        build_corridor(
            world,
            origin.x() + i32::from(room) * ROOM_STRIDE,
            floor_y,
            origin.z() + ROOM_SIZE / 2,
        );
    }
}

/// Opens the exit inside a cleared dungeon's reserved boss room and returns its
/// entrance position.
///
/// The exit reuses the quartz gate frame, so a hunter already knows the shape
/// that moves them between worlds.
pub fn open_dungeon_exit(
    world: &Arc<World>,
    region: InstanceRegion,
    layout: &DungeonLayout,
) -> BlockPos {
    let origin = BlockPos::from(region.spawn);
    let corner_x = origin.x() + i32::from(layout.boss_room()) * ROOM_STRIDE;
    let entrance = BlockPos::new(
        corner_x + ROOM_SIZE / 2 - 1,
        INSTANCE_FLOOR_Y,
        origin.z() + ROOM_SIZE / 2,
    );
    place_gate_frame(world, entrance, false);
    entrance
}

/// Builds the single sealed arena of one penalty-zone instance.
pub fn build_penalty_arena(world: &Arc<World>, region: InstanceRegion) {
    let corner = penalty_arena_corner(BlockPos::from(region.spawn));
    build_room(
        world,
        corner,
        PENALTY_ARENA_SIZE,
        PENALTY_ARENA_HEIGHT,
        false,
    );
}

/// Returns the floor corner of the penalty arena centred on `origin`.
const fn penalty_arena_corner(origin: BlockPos) -> BlockPos {
    let half = PENALTY_ARENA_SIZE / 2;
    BlockPos::new(origin.x() - half, INSTANCE_FLOOR_Y - 1, origin.z() - half)
}

/// Builds one hollow quartz room with a solid floor, starting at the floor
/// block of the room's north-west wall corner.
///
/// The interior is emptied as well: instance regions are reused, so a room must
/// not inherit the exit frame or fixtures of the run that used it before.
///
/// Only dungeons grow crystal ore walls; a penalty arena is not a place to mine.
fn build_room(world: &Arc<World>, corner: BlockPos, size: i32, height: i32, ore: bool) {
    let mut failed = 0;
    for x in -1..=size {
        for z in -1..=size {
            for y in 0..=height {
                let boundary = x == -1 || x == size || z == -1 || z == size || y == height;
                if y != 0 && !boundary {
                    continue;
                }
                let pos = BlockPos::new(corner.x() + x, corner.y() + y, corner.z() + z);
                let wall = y != 0 && y != height && (x == -1 || x == size || z == -1 || z == size);
                let state = if ore && wall && ore_at(corner, x, y, z) {
                    CRYSTAL_ORE.default_state()
                } else {
                    vanilla_blocks::QUARTZ_BLOCK.default_state()
                };
                set_fixture(world, pos, state, &mut failed);
            }
        }
    }
    for x in 0..size {
        for z in 0..size {
            for y in 1..height {
                let pos = BlockPos::new(corner.x() + x, corner.y() + y, corner.z() + z);
                if world.get_block_state(pos) != vanilla_blocks::AIR.default_state() {
                    set_fixture(world, pos, vanilla_blocks::AIR.default_state(), &mut failed);
                }
            }
        }
    }
    warn_unwritten("room", corner, failed);
}

/// Connects the room starting at `corner_x` with the room after it.
fn build_corridor(world: &Arc<World>, corner_x: i32, floor_y: i32, z: i32) {
    let mut failed = 0;
    for x in ROOM_SIZE..ROOM_STRIDE {
        for y in 0..3 {
            let pos = BlockPos::new(corner_x + x, floor_y + y, z);
            let state = if y == 0 {
                vanilla_blocks::QUARTZ_BLOCK.default_state()
            } else {
                vanilla_blocks::AIR.default_state()
            };
            set_fixture(world, pos, state, &mut failed);
        }
    }
    warn_unwritten(
        "corridor",
        BlockPos::new(corner_x + ROOM_SIZE, floor_y, z),
        failed,
    );
}
/// Spawns the fixed wave of `room` and returns the spawned entity ids.
///
/// The wave is all zombies: Steel has no arrow projectile or ranged-attack goal
/// yet, so a skeleton could only be a melee mob, which is not what vanilla does.
pub fn spawn_dungeon_wave(
    world: &Arc<World>,
    region: InstanceRegion,
    wave: DungeonWave,
    room: u8,
) -> Vec<i32> {
    let origin = BlockPos::from(region.spawn);
    let room_x = origin.x() + i32::from(room) * ROOM_STRIDE;
    let mut spawned = Vec::new();
    for index in 0..wave.mob_count {
        let position = BlockPos::new(
            room_x + 2 + i32::from(index % 4) * 2,
            INSTANCE_FLOOR_Y,
            origin.z() + 2 + i32::from(index / 4) * 2,
        );
        if let Some(id) = spawn_scaled_mob(
            world,
            &vanilla_entities::ZOMBIE,
            position,
            wave.health_multiplier,
            wave.damage_multiplier,
        ) {
            spawned.push(id);
        }
    }
    let missing = u32::from(wave.mob_count) - u32::try_from(spawned.len()).unwrap_or(u32::MAX);
    if missing > 0 {
        log::warn!(
            "sololeveling: {missing} mob(s) of the wave in room {room} at {:?} could not be spawned",
            BlockPos::new(room_x, INSTANCE_FLOOR_Y, origin.z())
        );
    }
    spawned
}

/// The boss of a dungeon, ready to be announced to the hunter who reaches it.
pub struct SpawnedBoss {
    /// Entity id of the boss body.
    pub id: i32,
    /// Name shown above the boss and on its boss bar.
    pub name: TextComponent,
    /// Abilities the fight may use.
    pub abilities: BossAbilitySet,
}

/// Spawns the boss of a dungeon in its reserved final room.
///
/// Steel has no boss-type mob yet, so the boss body is a ranked zombie: the
/// encounter rules are the plugin's, while health, damage, gear, and hitbox
/// come from a vanilla zombie plus the rank's boss parameters.
pub fn spawn_dungeon_boss(
    world: &Arc<World>,
    region: InstanceRegion,
    layout: &DungeonLayout,
    rank: HunterRank,
    red: bool,
) -> Option<SpawnedBoss> {
    let origin = BlockPos::from(region.spawn);
    let corner_x = origin.x() + i32::from(layout.boss_room()) * ROOM_STRIDE;
    let position = BlockPos::new(
        corner_x + ROOM_SIZE - 3,
        INSTANCE_FLOOR_Y,
        origin.z() + ROOM_SIZE / 2,
    );
    let boss = boss_for_gate(rank, red);
    let entity = spawn_mob(world, &vanilla_entities::ZOMBIE, position).ok()?;
    let living = entity.as_living_entity()?;
    boss.apply_to(living);
    let name = boss_name(rank, red);
    entity.set_custom_name(Some(name.clone()));
    // The boss was spawned at scale 1.0, so its hitbox has to follow the new
    // scale before the fight starts.
    entity.refresh_dimensions();
    Some(SpawnedBoss {
        id: entity.id(),
        name,
        abilities: boss.abilities,
    })
}

/// Returns the name shown above a dungeon boss.
fn boss_name(rank: HunterRank, red: bool) -> TextComponent {
    let kind = if red { "Red Gate Boss" } else { "Gate Boss" };
    TextComponent::plain(format!("{}-Rank {kind}", rank.name())).color(Color::Red)
}

/// Spawns `count` vanilla zombies spread around the penalty arena and returns
/// the spawned entity ids.
pub fn spawn_penalty_wave(world: &Arc<World>, region: InstanceRegion, count: u8) -> Vec<i32> {
    let origin = BlockPos::from(region.spawn);
    let half = PENALTY_ARENA_SIZE / 2;
    let mut spawned = Vec::new();
    for index in 0..count {
        let column = i32::from(index % PENALTY_WAVE_COLUMNS);
        let row = i32::from(index / PENALTY_WAVE_COLUMNS);
        let position = BlockPos::new(
            origin.x() - half + 3 + column * PENALTY_WAVE_SPACING,
            INSTANCE_FLOOR_Y,
            origin.z() - half + 3 + row * PENALTY_WAVE_SPACING,
        );
        if let Some(id) = spawn_scaled_mob(world, &vanilla_entities::ZOMBIE, position, 1.0, 1.0) {
            spawned.push(id);
        }
    }
    let missing = u32::from(count) - u32::try_from(spawned.len()).unwrap_or(u32::MAX);
    if missing > 0 {
        log::warn!(
            "sololeveling: {missing} zombie(s) of the penalty wave at {origin:?} could not be spawned"
        );
    }
    spawned
}

/// Spawns one mob and applies the rank multipliers to its vanilla attributes.
fn spawn_scaled_mob(
    world: &Arc<World>,
    entity_type: EntityTypeRef,
    position: BlockPos,
    health_multiplier: f64,
    damage_multiplier: f64,
) -> Option<i32> {
    let entity = match spawn_mob(world, entity_type, position) {
        Ok(entity) => entity,
        Err(error) => {
            log::debug!(
                "sololeveling: could not spawn {} at {position:?} in {}: {error:?}",
                entity_type.key,
                world.key
            );
            return None;
        }
    };
    let living = entity.as_living_entity()?;
    let max_health = living
        .attributes()
        .lock()
        .required_value(vanilla_attributes::MAX_HEALTH);
    let damage = living
        .attributes()
        .lock()
        .required_value(vanilla_attributes::ATTACK_DAMAGE);
    living.set_base_attribute_value(
        vanilla_attributes::MAX_HEALTH,
        max_health * health_multiplier,
    );
    living.set_base_attribute_value(
        vanilla_attributes::ATTACK_DAMAGE,
        damage * damage_multiplier,
    );
    living.set_health((max_health * health_multiplier) as f32);
    Some(entity.id())
}

/// Writes one fixture block and counts the positions the server could not
/// reach.
///
/// [`World::set_block`] reports an out-of-bounds position or an unloaded chunk
/// as a plain `false`; ignoring that would leave an invisible dungeon behind.
fn set_fixture(world: &Arc<World>, pos: BlockPos, state: BlockStateId, failed: &mut u32) {
    if world.get_block_state(pos) == state {
        return;
    }
    // `set_block` only reports a chunk it could not write to; an unchanged
    // write never reaches it, so a `false` here means the chunk was missing.
    if !world.set_block(pos, state, UpdateFlags::UPDATE_ALL) {
        *failed += 1;
    }
}

/// Reports fixture blocks that silently failed to write.
fn warn_unwritten(what: &str, anchor: BlockPos, failed: u32) {
    if failed > 0 {
        log::warn!(
            "sololeveling: {failed} block(s) of the {what} at {anchor:?} could not be written"
        );
    }
}

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::types::BlockStateId;
    use steel_utils::{BlockPos, ChunkPos};

    use super::{
        INSTANCE_FLOOR_Y, ROOM_SIZE, ROOM_STRIDE, dungeon_chunks, gate_frame_chunks,
        gate_is_dry_at, opening_contains, penalty_arena_chunks,
    };
    use crate::dungeon::{DungeonLayout, InstanceRegion};

    /// Builds a dry stone shore under `entrance` with air above it.
    fn dry_shore(entrance: BlockPos) -> impl Fn(BlockPos) -> BlockStateId {
        move |pos| {
            if pos.y() < entrance.y() {
                vanilla_blocks::STONE.default_state()
            } else {
                vanilla_blocks::AIR.default_state()
            }
        }
    }

    #[test]
    fn a_gate_needs_land_under_every_frame_column() {
        init_vanilla_registry();
        let entrance = BlockPos::new(10, 64, 0);
        assert!(gate_is_dry_at(entrance, |_, _| true, dry_shore(entrance)));

        // The frame hangs one column over the water surface: that column has no
        // dry surface of its own, which a centre-only check would not notice.
        for shore_edge in [-1, 2] {
            assert!(!gate_is_dry_at(
                entrance,
                |x, _| x != entrance.x() + shore_edge,
                dry_shore(entrance)
            ));
        }
    }

    #[test]
    fn a_gate_rejects_a_flooded_opening() {
        init_vanilla_registry();
        let entrance = BlockPos::new(10, 64, 0);
        let flooded = BlockPos::new(entrance.x() + 1, entrance.y() + 2, 0);
        assert!(!gate_is_dry_at(
            entrance,
            |_, _| true,
            |pos| {
                if pos == flooded {
                    vanilla_blocks::WATER.default_state()
                } else {
                    dry_shore(entrance)(pos)
                }
            }
        ));
    }

    #[test]
    fn opening_contains_the_frame_interior_only() {
        let entrance = BlockPos::new(10, 64, -3);

        assert!(opening_contains(entrance, DVec3::new(10.0, 65.0, -3.0)));
        assert!(opening_contains(entrance, DVec3::new(11.9, 67.9, -2.1)));

        // The frame's bottom and side columns, its top row, and the block behind it are solid.
        assert!(!opening_contains(entrance, DVec3::new(10.5, 64.0, -3.0)));
        assert!(!opening_contains(entrance, DVec3::new(9.9, 65.0, -3.0)));
        assert!(!opening_contains(entrance, DVec3::new(12.0, 65.0, -3.0)));
        assert!(!opening_contains(entrance, DVec3::new(10.5, 68.0, -3.0)));
        assert!(!opening_contains(entrance, DVec3::new(10.5, 65.0, -2.0)));
    }

    #[test]
    fn a_frame_straddling_a_chunk_border_requests_both_columns() {
        // The frame spans x 15 through 18 at a single z, so it touches the
        // chunks of x=15 and x=18.
        let chunks = gate_frame_chunks(BlockPos::new(16, 64, 0));
        assert_eq!(chunks.len(), 2);
        assert!(chunks.contains(&ChunkPos::from_block_pos(BlockPos::new(15, 64, 0))));
        assert!(chunks.contains(&ChunkPos::from_block_pos(BlockPos::new(18, 64, 0))));

        // A frame well inside one chunk needs a single request.
        assert_eq!(gate_frame_chunks(BlockPos::new(4, 64, 0)).len(), 1);
    }
    #[test]
    fn dungeon_chunks_cover_the_last_room_of_the_longest_layout() {
        let region = InstanceRegion::from_slot(0);
        let layout = DungeonLayout::from_roll(3);
        let chunks = dungeon_chunks(region, &layout);
        let origin = BlockPos::from(region.spawn);
        let floor_y = INSTANCE_FLOOR_Y - 1;

        // Every room, including the reserved boss room, has to be loaded before
        // the build writes its floor and walls.
        for room in 0..layout.total_room_count() {
            let corner_x = origin.x() + i32::from(room) * ROOM_STRIDE;
            for x in [corner_x - 1, corner_x + ROOM_SIZE] {
                for z in [origin.z() - 1, origin.z() + ROOM_SIZE] {
                    let chunk = ChunkPos::from_block_pos(BlockPos::new(x, floor_y, z));
                    assert!(
                        chunks.contains(&chunk),
                        "room {room} writes {chunk:?}, which the request leaves unloaded"
                    );
                }
            }
        }
        // Corridors run past the last room's far wall into the next room.
        for room in 0..layout.boss_room() {
            let corner_x = origin.x() + i32::from(room) * ROOM_STRIDE;
            let chunk = ChunkPos::from_block_pos(BlockPos::new(
                corner_x + ROOM_STRIDE - 1,
                floor_y,
                origin.z() + ROOM_SIZE / 2,
            ));
            assert!(chunks.contains(&chunk), "corridor {room} writes {chunk:?}");
        }
    }

    #[test]
    fn penalty_arena_chunks_cover_the_whole_arena() {
        let region = InstanceRegion::from_slot(0);
        let chunks = penalty_arena_chunks(region);
        let origin = BlockPos::from(region.spawn);
        let half = super::PENALTY_ARENA_SIZE / 2;
        for x in [origin.x() - half - 1, origin.x() + half] {
            for z in [origin.z() - half - 1, origin.z() + half] {
                let chunk = ChunkPos::from_block_pos(BlockPos::new(x, INSTANCE_FLOOR_Y, z));
                assert!(chunks.contains(&chunk), "arena writes {chunk:?}");
            }
        }
    }
}
