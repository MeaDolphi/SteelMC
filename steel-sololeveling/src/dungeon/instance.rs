//! Block and entity fixtures for gates and dungeon/penalty instances.
use std::sync::Arc;

use glam::DVec3;
use steel_core::entity::{SharedEntity, spawn_mob};
use steel_core::gameplay_hooks::BlockMutationCause;
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

use super::boss::{BossKind, DungeonBoss};
use super::generator::{Blueprint, Part, Room, Theme};
use super::mobs::{MobClass, MobLabel, MobRarity};
use super::{
    DungeonLayout, DungeonWave, Footprint, Gate, INSTANCE_FLOOR_Y, InstanceRegion, boss_for_gate,
};
use crate::rank::HunterRank;

/// Distance between room origins of the fixed layout used before generation.
const LEGACY_ROOM_STRIDE: i32 = 20;
/// Room size, height and count of the fixed layout used before generation.
const LEGACY_ROOM: (i32, i32, i32) = (13, 5, 7);
/// Interior width and depth of the penalty arena.
const PENALTY_ARENA_SIZE: i32 = 31;
/// Interior height of the penalty arena.
const PENALTY_ARENA_HEIGHT: i32 = 7;

/// The block crystal ore is built from.
///
/// A full cube keeps a wall airtight; the amethyst appearance is what tells a
/// hunter the block is worth mining, since the item it drops is themed anyway.
pub const CRYSTAL_ORE: BlockRef = &vanilla_blocks::AMETHYST_BLOCK;

/// Returns whether `state` is the crystal ore dungeon walls are built from.
#[must_use]
pub fn is_crystal_ore(state: BlockStateId) -> bool {
    state == CRYSTAL_ORE.default_state()
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

/// Returns the chunk columns a dungeon build writes, including the erasure of
/// whatever `previous` left in the region.
///
/// [`World::set_block`] silently drops writes to unloaded chunks, which would
/// leave holes a hunter and its mobs fall through, so the whole box has to be
/// requested before any block is written.
#[must_use]
pub fn dungeon_chunks(
    region: InstanceRegion,
    layout: &DungeonLayout,
    previous: Option<Footprint>,
) -> Vec<ChunkPos> {
    let mut chunks = blueprint_chunks(region, &layout.blueprint);
    for chunk in footprint_chunks(region, previous) {
        if !chunks.contains(&chunk) {
            chunks.push(chunk);
        }
    }
    chunks
}

fn blueprint_chunks(region: InstanceRegion, blueprint: &Blueprint) -> Vec<ChunkPos> {
    let ((min_x, _, min_z), (max_x, _, max_z)) = blueprint.bounds;
    chunks_covering(
        at(region, (min_x, -1, min_z)),
        at(region, (max_x, -1, max_z)),
    )
}

pub(super) fn footprint_chunks(
    region: InstanceRegion,
    footprint: Option<Footprint>,
) -> Vec<ChunkPos> {
    match footprint {
        Some(Footprint::Dungeon { seed }) => {
            blueprint_chunks(region, &DungeonLayout::from_seed(seed).blueprint)
        }
        Some(Footprint::Arena) => penalty_arena_chunks(region),
        Some(Footprint::Tower) => super::tower::tower_chunks(region),
        None => {
            let (min, max) = legacy_box(region);
            let mut chunks = chunks_covering(min, max);
            for chunk in penalty_arena_chunks(region) {
                if !chunks.contains(&chunk) {
                    chunks.push(chunk);
                }
            }
            chunks
        }
    }
}

/// Returns the box the fixed pre-generator layout could occupy.
fn legacy_box(region: InstanceRegion) -> (BlockPos, BlockPos) {
    let origin = BlockPos::from(region.spawn);
    let (size, height, rooms) = LEGACY_ROOM;
    (
        BlockPos::new(origin.x() - 1, INSTANCE_FLOOR_Y - 1, origin.z() - 1),
        BlockPos::new(
            origin.x() + (rooms - 1) * LEGACY_ROOM_STRIDE + size,
            INSTANCE_FLOOR_Y - 1 + height,
            origin.z() + size,
        ),
    )
}

/// Returns the world position of a blueprint offset inside `region`.
pub(super) fn at(region: InstanceRegion, (x, y, z): (i32, i32, i32)) -> BlockPos {
    let origin = BlockPos::from(region.spawn);
    BlockPos::new(origin.x() + x, INSTANCE_FLOOR_Y + y, origin.z() + z)
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
pub(super) fn chunks_covering(min: BlockPos, max: BlockPos) -> Vec<ChunkPos> {
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

/// Places a gate frame without replacing blocks built after its site was chosen.
pub fn place_gate_frame(world: &Arc<World>, entrance: BlockPos, red: bool) -> bool {
    let block = gate_frame_block(red);
    let positions = gate_frame_positions(entrance);
    if positions.iter().any(|pos| {
        let state = world.get_block_state(*pos);
        !state.is_air() && state != block
    }) || !(1..4).all(|height| {
        (0..2).all(|offset| {
            world
                .get_block_state(BlockPos::new(
                    entrance.x() + offset,
                    entrance.y() + height,
                    entrance.z(),
                ))
                .is_air()
        })
    }) {
        return false;
    }
    let mut failed = 0;
    for pos in positions {
        set_fixture(world, pos, block, &mut failed);
    }
    warn_unwritten("gate frame", entrance, failed);
    failed == 0
}

/// Restores a gate entrance to air, leaving any other block untouched.
pub fn remove_gate_frame(world: &Arc<World>, gate: &Gate) {
    let block = gate_frame_block(gate.red);
    for pos in gate_frame_positions(gate.entrance) {
        let _ = world.set_block_if_unchanged_with_cause(
            pos,
            block,
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_ALL,
            BlockMutationCause::System,
        );
    }
}

/// Returns whether a legacy gate frame exactly matches its expected pattern.
#[must_use]
pub fn gate_frame_matches(world: &Arc<World>, gate: &Gate) -> bool {
    let block = gate_frame_block(gate.red);
    gate_frame_positions(gate.entrance)
        .into_iter()
        .all(|pos| world.get_block_state(pos) == block)
        && (1..4).all(|height| {
            (0..2).all(|offset| {
                world
                    .get_block_state(BlockPos::new(
                        gate.entrance.x() + offset,
                        gate.entrance.y() + height,
                        gate.entrance.z(),
                    ))
                    .is_air()
            })
        })
}

/// Returns whether every expected frame location is already clear.
#[must_use]
pub fn gate_frame_is_clear(world: &Arc<World>, gate: &Gate) -> bool {
    gate_frame_positions(gate.entrance)
        .into_iter()
        .all(|pos| world.get_block_state(pos).is_air())
}

/// Builds every room, corridor and light of one generated dungeon.
pub fn build_dungeon(
    world: &Arc<World>,
    region: InstanceRegion,
    layout: &DungeonLayout,
    rank: HunterRank,
) {
    let blueprint = &layout.blueprint;
    let palette = blueprint.theme.palette();
    let crystal_ores = blueprint.crystal_ore_positions(rank.crystal_vein_count());
    let mut failed = 0;
    for placement in &blueprint.blocks {
        let part = if placement.part == Part::Ore && !crystal_ores.contains(&placement.offset) {
            Part::Wall
        } else {
            placement.part
        };
        let block = palette.block(part, CRYSTAL_ORE);
        set_fixture(
            world,
            at(region, placement.offset),
            block.default_state(),
            &mut failed,
        );
    }
    warn_unwritten("dungeon", at(region, (0, -1, 0)), failed);
}

/// Removes whatever a previous run built in `region`.
///
/// A generated dungeon is rebuilt from its seed and every block of it, plus the
/// exit frame its boss room may hold, is set back to air. A region with no
/// record may still hold the fixed layout or the arena from before generation,
/// so their whole boxes are cleared instead.
pub fn erase_footprint(world: &Arc<World>, region: InstanceRegion, footprint: Option<Footprint>) {
    let air = vanilla_blocks::AIR.default_state();
    let mut failed = 0;
    match footprint {
        Some(Footprint::Dungeon { seed }) => {
            let layout = DungeonLayout::from_seed(seed);
            for placement in &layout.blueprint.blocks {
                set_fixture(world, at(region, placement.offset), air, &mut failed);
            }
            for pos in gate_frame_positions(exit_entrance(region, &layout)) {
                set_fixture(world, pos, air, &mut failed);
            }
        }
        Some(Footprint::Arena) => clear_box_of(world, arena_box(region), &mut failed),
        Some(Footprint::Tower) => super::tower::erase_tower(world, region, &mut failed),
        None => {
            clear_box_of(world, legacy_box(region), &mut failed);
            clear_box_of(world, arena_box(region), &mut failed);
        }
    }
    warn_unwritten("previous instance", BlockPos::from(region.spawn), failed);
}

pub(super) fn clear_box_of(world: &Arc<World>, (min, max): (BlockPos, BlockPos), failed: &mut u32) {
    let air = vanilla_blocks::AIR.default_state();
    for x in min.x()..=max.x() {
        for z in min.z()..=max.z() {
            for y in min.y()..=max.y() {
                set_fixture(world, BlockPos::new(x, y, z), air, failed);
            }
        }
    }
}

/// Returns the bottom of the exit frame in the boss room.
fn exit_entrance(region: InstanceRegion, layout: &DungeonLayout) -> BlockPos {
    let (x, z) = layout.blueprint.boss_room().center;
    at(region, (x - 1, 0, z))
}

/// Opens the exit inside a cleared dungeon's boss room and returns its
/// entrance position.
///
/// The exit reuses the quartz gate frame, so a hunter already knows the shape
/// that moves them between worlds.
pub fn open_dungeon_exit(
    world: &Arc<World>,
    region: InstanceRegion,
    layout: &DungeonLayout,
) -> BlockPos {
    let entrance = exit_entrance(region, layout);
    place_gate_frame(world, entrance, false);
    entrance
}

/// Returns the box the penalty arena occupies, walls included.
fn arena_box(region: InstanceRegion) -> (BlockPos, BlockPos) {
    let corner = penalty_arena_corner(BlockPos::from(region.spawn));
    (
        BlockPos::new(corner.x() - 1, corner.y(), corner.z() - 1),
        BlockPos::new(
            corner.x() + PENALTY_ARENA_SIZE,
            corner.y() + PENALTY_ARENA_HEIGHT,
            corner.z() + PENALTY_ARENA_SIZE,
        ),
    )
}

/// Returns the floor corner of the penalty arena centred on `origin`.
const fn penalty_arena_corner(origin: BlockPos) -> BlockPos {
    let half = PENALTY_ARENA_SIZE / 2;
    BlockPos::new(origin.x() - half, INSTANCE_FLOOR_Y - 1, origin.z() - half)
}

/// One mob a wave or guard group spawned.
#[derive(Debug, Clone)]
pub struct SpawnedMob {
    /// Entity id of the mob.
    pub id: i32,
    /// Its class, which decides whether it has an ability.
    pub class: MobClass,
    /// Its nameplate.
    pub label: MobLabel,
}

/// Spawns the fixed wave of combat room `room`.
///
/// The wave is all zombie bodies: Steel has no arrow projectile or
/// ranged-attack goal yet, so a skeleton could only be a melee mob, which is
/// not what vanilla does. Variety comes from the classes instead.
pub fn spawn_dungeon_wave(
    world: &Arc<World>,
    region: InstanceRegion,
    layout: &DungeonLayout,
    wave: DungeonWave,
    rank: HunterRank,
    room: u8,
) -> Vec<SpawnedMob> {
    let Some(plan) = layout.blueprint.combat_room(room) else {
        return Vec::new();
    };
    let salt = layout.seed.rotate_left(u32::from(room) * 7);
    spawn_room_mobs(
        world,
        region,
        layout.blueprint.theme,
        plan,
        wave.mob_count,
        wave,
        rank,
        salt,
    )
}

/// Spawns `count` mobs across the spawn spots of `plan`.
///
/// Each mob rolls a class from `salt` and its place in the group, and carries
/// a nameplate with its name, class and health.
#[expect(
    clippy::too_many_arguments,
    reason = "a spawn is fully described by where, how many, how strong and which roll"
)]
pub fn spawn_room_mobs(
    world: &Arc<World>,
    region: InstanceRegion,
    theme: Theme,
    plan: &Room,
    count: u8,
    wave: DungeonWave,
    rank: HunterRank,
    salt: u64,
) -> Vec<SpawnedMob> {
    let mut spawned = Vec::new();
    for index in 0..count {
        let position = spawn_position(region, plan, usize::from(index));
        let roll = salt.wrapping_add(u64::from(index).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let class = MobClass::from_roll(roll ^ (roll >> 29));
        if let Some(mob) = spawn_class_mob(world, theme, position, class, wave, rank) {
            spawned.push(mob);
        }
    }
    let missing = u32::from(count) - u32::try_from(spawned.len()).unwrap_or(u32::MAX);
    if missing > 0 {
        log::warn!(
            "sololeveling: {missing} mob(s) of a group at {:?} could not be spawned",
            at(region, (plan.center.0, 0, plan.center.1))
        );
    }
    spawned
}

/// Spawns one zombie-bodied mob of `class`, scaled by `wave` and geared for
/// `rank`, with its nameplate.
pub(super) fn spawn_class_mob(
    world: &Arc<World>,
    theme: Theme,
    position: BlockPos,
    class: MobClass,
    wave: DungeonWave,
    rank: HunterRank,
) -> Option<SpawnedMob> {
    let (health, damage, _) = class.proportions();
    let entity = spawn_scaled_mob(
        world,
        &vanilla_entities::ZOMBIE,
        position,
        wave.health_multiplier * health,
        wave.damage_multiplier * damage,
    )?;
    let living = entity.as_living_entity()?;
    class.equip(living, rank, theme);
    let label = MobLabel::mob(theme, class);
    if let Some(mob) = entity.as_mob() {
        mob.set_allied_tank(label.is_tank);
    }
    label.show(world, living);
    Some(SpawnedMob {
        id: entity.id(),
        class,
        label,
    })
}

/// Returns the `index`-th spawn spot of a room, cycling through its spots.
fn spawn_position(region: InstanceRegion, room: &Room, index: usize) -> BlockPos {
    let (x, z) = room
        .spawns
        .get(index % room.spawns.len().max(1))
        .copied()
        .unwrap_or(room.center);
    at(region, (x, 0, z))
}

/// The boss of a dungeon, ready to be announced to the hunter who reaches it.
pub struct SpawnedBoss {
    /// Entity id of the boss body.
    pub id: i32,
    /// Name shown above the boss and on its boss bar.
    pub name: TextComponent,
    /// Stats and abilities the fight uses.
    pub boss: DungeonBoss,
    /// Nameplate kept up to date with the boss's health.
    pub label: MobLabel,
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
    let room = layout.blueprint.boss_room();
    // The boss waits at the far side of its room; the exit appears at the centre.
    let position = at(region, (room.center.0, 0, room.center.1 + room.half.1 - 3));
    let kind = BossKind::from_seed(layout.seed);
    let boss = boss_for_gate(kind, rank, red);
    let entity = spawn_mob(world, &vanilla_entities::ZOMBIE, position).ok()?;
    let living = entity.as_living_entity()?;
    boss.apply_to(living);
    let label = boss_label(kind, rank, red);
    label.show(world, living);
    // The boss was spawned at scale 1.0, so its hitbox has to follow the new
    // scale before the fight starts.
    entity.refresh_dimensions();
    Some(SpawnedBoss {
        id: entity.id(),
        name: TextComponent::plain(label.name.clone()).color(Color::Red),
        boss,
        label,
    })
}

/// Returns the nameplate of a dungeon boss.
fn boss_label(kind: BossKind, rank: HunterRank, red: bool) -> MobLabel {
    let name = if red {
        format!("Кровавый {}", kind.display_name().to_lowercase())
    } else {
        kind.display_name().to_owned()
    };
    MobLabel {
        name,
        title: format!("Босс, ранг {}", rank.name()),
        color: Color::Red,
        is_tank: false,
        is_broodmother: false,
        rarity: MobRarity::Boss,
    }
}

/// Spawns one mob and applies the rank multipliers to its vanilla attributes.
fn spawn_scaled_mob(
    world: &Arc<World>,
    entity_type: EntityTypeRef,
    position: BlockPos,
    health_multiplier: f64,
    damage_multiplier: f64,
) -> Option<SharedEntity> {
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
    Some(entity)
}

/// Writes one fixture block and counts the positions the server could not
/// reach.
///
/// [`World::set_block`] reports an out-of-bounds position or an unloaded chunk
/// as a plain `false`; ignoring that would leave an invisible dungeon behind.
pub(super) fn set_fixture(
    world: &Arc<World>,
    pos: BlockPos,
    state: BlockStateId,
    failed: &mut u32,
) {
    if world.get_block_state(pos) == state {
        return;
    }
    // `set_block` only reports a chunk it could not write to; an unchanged
    // write never reaches it, so a `false` here means the chunk was missing.
    if !world.set_block_with_cause(
        pos,
        state,
        UpdateFlags::UPDATE_ALL,
        BlockMutationCause::System,
    ) {
        *failed += 1;
    }
}

/// Reports fixture blocks that silently failed to write.
pub(super) fn warn_unwritten(what: &str, anchor: BlockPos, failed: u32) {
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
        INSTANCE_FLOOR_Y, at, dungeon_chunks, gate_frame_chunks, gate_is_dry_at, opening_contains,
        penalty_arena_chunks,
    };
    use crate::dungeon::{DungeonLayout, Footprint, InstanceRegion};

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
    fn dungeon_chunks_cover_every_block_and_the_previous_footprint() {
        let region = InstanceRegion::from_slot(3);
        for seed in [0, 7, 3 << 56, u64::MAX] {
            let layout = DungeonLayout::from_seed(seed);
            let previous = DungeonLayout::from_seed(seed ^ 0xABCD);
            let chunks = dungeon_chunks(
                region,
                &layout,
                Some(Footprint::Dungeon {
                    seed: previous.seed,
                }),
            );
            // Every block the build writes, and every block the erasure of the
            // previous dungeon writes, has to be loaded first.
            for placement in layout
                .blueprint
                .blocks
                .iter()
                .chain(&previous.blueprint.blocks)
            {
                let chunk = ChunkPos::from_block_pos(at(region, placement.offset));
                assert!(
                    chunks.contains(&chunk),
                    "seed {seed}: {chunk:?} is not requested"
                );
            }
        }
        // A region without a record also clears the fixed pre-generator layout.
        let layout = DungeonLayout::from_seed(1);
        let chunks = dungeon_chunks(region, &layout, None);
        let origin = BlockPos::from(region.spawn);
        let far = BlockPos::new(origin.x() + 6 * 20 + 13, INSTANCE_FLOOR_Y, origin.z() + 13);
        assert!(chunks.contains(&ChunkPos::from_block_pos(far)));
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
