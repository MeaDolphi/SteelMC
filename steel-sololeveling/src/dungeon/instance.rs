//! Block and entity fixtures for gates and dungeon/penalty instances.
use std::f64::consts::TAU;
use std::sync::Arc;

use glam::DVec3;
use rand::RngExt as _;
use steel_core::entity::entities::{DisplayBillboard, TextDisplayEntity};
use steel_core::entity::{Entity as _, SharedEntity, next_entity_id, spawn_mob};
use steel_core::gameplay_hooks::BlockMutationCause;
use steel_core::inventory::equipment::EquipmentSlot;
use steel_core::world::World;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::{vanilla_attributes, vanilla_blocks, vanilla_entities, vanilla_items};
use steel_utils::BlockPos;
use steel_utils::ChunkPos;
use steel_utils::types::{BlockStateId, UpdateFlags};
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;

use super::boss::{BossKind, DungeonBoss};
use super::generator::{Blueprint, Part, Room, Theme};
use super::mobs::{MobClass, MobLabel, MobRarity, wear_theme_skin};
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
        Some(Footprint::Dungeon { seed, red }) => {
            blueprint_chunks(region, &DungeonLayout::for_gate(seed, red).blueprint)
        }
        Some(Footprint::Arena) => penalty_arena_chunks(region),
        Some(Footprint::Tower) => super::tower::tower_chunks(region),
        Some(Footprint::Castle { .. }) => Vec::new(),
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

/// Returns the columns a gate based at `entrance` needs on level ground: the
/// four frame columns, and the opening's columns one step before and behind
/// it, so the gate can be walked into from either side.
#[must_use]
pub const fn gate_footprint(entrance: BlockPos) -> [(i32, i32); 8] {
    let (x, z) = (entrance.x(), entrance.z());
    [
        (x - 1, z),
        (x, z),
        (x + 1, z),
        (x + 2, z),
        (x, z - 1),
        (x + 1, z - 1),
        (x, z + 1),
        (x + 1, z + 1),
    ]
}

/// Returns whether a gate can stand at `entrance` on level, dry ground.
///
/// [`World::safe_surface_position`] returns the highest standing surface of a
/// column and skips water, so every footprint column must report exactly the
/// entrance height: a column that reaches higher is a roof, a lamp or another
/// frame above the site, and one that is lower or wet leaves the frame
/// hanging over a drop or a shore.
pub fn gate_ground_is_level(world: &World, entrance: BlockPos) -> bool {
    gate_is_level_at(
        entrance,
        |x, z| world.safe_surface_position(x, z).map(|surface| surface.y()),
        |pos| world.get_block_state(pos),
    )
}

fn gate_is_level_at(
    entrance: BlockPos,
    surface_y: impl Fn(i32, i32) -> Option<i32>,
    state_at: impl Fn(BlockPos) -> BlockStateId,
) -> bool {
    let level = gate_footprint(entrance)
        .into_iter()
        .all(|(x, z)| surface_y(x, z) == Some(entrance.y()));
    let opening_is_dry = (1..4).all(|height| {
        (0..2).all(|offset| {
            let pos = BlockPos::new(entrance.x() + offset, entrance.y() + height, entrance.z());
            !state_at(pos).has_fluid()
        })
    });
    level && opening_is_dry
}

/// The look of one gate entrance frame.
///
/// The block is derived from the gate instead of being stored, so a gate can
/// never disagree with the frame standing in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStyle {
    /// An ordinary gate: quartz blocks.
    Normal,
    /// A red gate: red stained glass.
    Red,
    /// A Demon Castle portal: polished blackstone bricks with crying obsidian
    /// corners, so the castle gate never masquerades as an ordinary gate.
    Castle,
}

impl FrameStyle {
    /// Returns the frame style a gate is built with.
    #[must_use]
    pub const fn from_gate(gate: &Gate) -> Self {
        if gate.floor.is_some() {
            Self::Castle
        } else if gate.red {
            Self::Red
        } else {
            Self::Normal
        }
    }
}

/// Returns the frame block of `style` at one frame position of the frame
/// based at `entrance`.
fn frame_block(style: FrameStyle, entrance: BlockPos, pos: BlockPos) -> BlockStateId {
    match style {
        FrameStyle::Red => vanilla_blocks::RED_STAINED_GLASS.default_state(),
        FrameStyle::Normal => vanilla_blocks::QUARTZ_BLOCK.default_state(),
        FrameStyle::Castle => {
            let on_side_column = pos.x() == entrance.x() - 1 || pos.x() == entrance.x() + 2;
            let on_end_row = pos.y() == entrance.y() || pos.y() == entrance.y() + 4;
            if on_side_column && on_end_row {
                vanilla_blocks::CRYING_OBSIDIAN.default_state()
            } else {
                vanilla_blocks::POLISHED_BLACKSTONE_BRICKS.default_state()
            }
        }
    }
}

/// Places a gate frame without replacing blocks built after its site was chosen.
///
/// A frame of another style already standing at the site is restyled, so a
/// gate that turns red, or a red frame left by a run that did not survive a
/// restart, takes the colour the gate has now.
pub fn place_gate_frame(world: &Arc<World>, entrance: BlockPos, style: FrameStyle) -> bool {
    let positions = gate_frame_positions(entrance);
    if positions.iter().any(|pos| {
        let state = world.get_block_state(*pos);
        !state.is_air() && !is_frame_block(entrance, *pos, state)
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
        set_fixture(world, pos, frame_block(style, entrance, pos), &mut failed);
    }
    warn_unwritten("gate frame", entrance, failed);
    failed == 0
}

/// Places a player-facing label above a gate and returns its entity id.
pub fn spawn_gate_label(
    world: &Arc<World>,
    entrance: BlockPos,
    text: TextComponent,
) -> Option<i32> {
    let position = DVec3::new(
        f64::from(entrance.x()) + 0.5,
        f64::from(entrance.y()) + 5.5,
        f64::from(entrance.z()) + 0.5,
    );
    let display = TextDisplayEntity::new(
        &vanilla_entities::TEXT_DISPLAY,
        next_entity_id(),
        position,
        Arc::downgrade(world),
    );
    display.set_text(text);
    display.set_billboard(DisplayBillboard::Center);
    let id = display.base().id();
    let entity: SharedEntity = Arc::new(display);
    world.try_add_entity(entity).ok().map(|()| id)
}

/// Returns the style of the complete frame standing at `entrance`, or `None`
/// when no whole frame stands there (or its chunk is not loaded).
#[must_use]
pub fn standing_frame_style(world: &World, entrance: BlockPos) -> Option<FrameStyle> {
    let positions = gate_frame_positions(entrance);
    [FrameStyle::Normal, FrameStyle::Red, FrameStyle::Castle]
        .into_iter()
        .find(|style| {
            positions
                .iter()
                .all(|pos| world.get_block_state(*pos) == frame_block(*style, entrance, *pos))
        })
}

/// Returns whether `state` at `pos` is a block of any frame style of the gate
/// at `entrance`.
fn is_frame_block(entrance: BlockPos, pos: BlockPos, state: BlockStateId) -> bool {
    [FrameStyle::Normal, FrameStyle::Red, FrameStyle::Castle]
        .into_iter()
        .any(|style| state == frame_block(style, entrance, pos))
}

/// Restores a gate entrance to air, leaving any other block untouched.
///
/// Removal accepts every known frame style, not only the style the registry
/// expects: wave explosions and legacy records both leave frames whose blocks
/// no longer match the expected style exactly.
pub fn remove_gate_frame(world: &Arc<World>, gate: &Gate) {
    for pos in gate_frame_positions(gate.entrance) {
        if is_frame_block(gate.entrance, pos, world.get_block_state(pos)) {
            let _ = world.set_block_with_cause(
                pos,
                vanilla_blocks::AIR.default_state(),
                UpdateFlags::UPDATE_ALL,
                BlockMutationCause::System,
            );
        }
    }
}

/// Returns whether a legacy gate frame exactly matches its expected pattern.
#[must_use]
pub fn gate_frame_matches(world: &Arc<World>, gate: &Gate) -> bool {
    let style = FrameStyle::from_gate(gate);
    gate_frame_positions(gate.entrance)
        .into_iter()
        .all(|pos| world.get_block_state(pos) == frame_block(style, gate.entrance, pos))
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
        let state = palette.state(part, CRYSTAL_ORE);
        set_fixture(world, at(region, placement.offset), state, &mut failed);
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
        Some(Footprint::Dungeon { seed, red }) => {
            let layout = DungeonLayout::for_gate(seed, red);
            for placement in &layout.blueprint.blocks {
                set_fixture(world, at(region, placement.offset), air, &mut failed);
            }
            for pos in gate_frame_positions(exit_entrance(region, &layout)) {
                set_fixture(world, pos, air, &mut failed);
            }
        }
        Some(Footprint::Arena) => clear_box_of(world, arena_box(region), &mut failed),
        Some(Footprint::Tower) => super::tower::erase_tower(world, region, &mut failed),
        Some(Footprint::Castle { .. }) => {}
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
    place_gate_frame(world, entrance, FrameStyle::Normal);
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
        let roll = roll ^ (roll >> 29);
        let mob = if theme == Theme::SnowElves {
            spawn_elf(world, position, roll, wave, rank)
        } else {
            spawn_class_mob(
                world,
                theme,
                position,
                MobClass::from_roll(roll),
                wave,
                rank,
            )
        };
        if let Some(mob) = mob {
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

/// Movement speed a snow elf has over the dead of other gates.
const ELF_SPEED: f64 = 1.15;

/// Spawns one snow elf of a roll out of sixteen: five swordsmen, three
/// berserkers, two shield bearers, four archers and two frost witches. Elves
/// keep no bombers or broodmothers.
fn spawn_elf(
    world: &Arc<World>,
    position: BlockPos,
    roll: u64,
    wave: DungeonWave,
    rank: HunterRank,
) -> Option<SpawnedMob> {
    let class = match roll % 16 {
        0..=4 => MobClass::Warrior,
        5..=7 => MobClass::Berserker,
        8 | 9 => MobClass::Guard,
        10..=13 => {
            let label = MobLabel {
                title: "Лучник".to_owned(),
                color: Color::Aqua,
                ..MobLabel::mob(Theme::SnowElves, MobClass::Warrior)
            };
            return spawn_archer(world, Theme::SnowElves, position, wave, label);
        }
        _ => MobClass::Shaman,
    };
    let mob = spawn_class_mob(world, Theme::SnowElves, position, class, wave, rank)?;
    if let Some(living) = world
        .get_entity_by_id(mob.id)
        .as_ref()
        .and_then(|entity| entity.as_living_entity())
    {
        let speed = living
            .attributes()
            .lock()
            .required_value(vanilla_attributes::MOVEMENT_SPEED);
        living.set_base_attribute_value(vanilla_attributes::MOVEMENT_SPEED, speed * ELF_SPEED);
    }
    Some(mob)
}

/// Spawns a skeleton archer with a bow, dressed in the skin of `theme`, under
/// `label`.
pub(crate) fn spawn_archer(
    world: &Arc<World>,
    theme: Theme,
    position: BlockPos,
    wave: DungeonWave,
    label: MobLabel,
) -> Option<SpawnedMob> {
    let entity = spawn_scaled_mob(
        world,
        &vanilla_entities::SKELETON,
        position,
        wave.health_multiplier * 0.8,
        wave.damage_multiplier,
    )?;
    let living = entity.as_living_entity()?;
    living.with_equipment_slot_mut(EquipmentSlot::MainHand, &mut |stack| {
        *stack = ItemStack::new(&vanilla_items::BOW);
    });
    wear_theme_skin(living, theme);
    label.show(world, living);
    Some(SpawnedMob {
        id: entity.id(),
        class: MobClass::Warrior,
        label,
    })
}

/// Spawns one zombie-bodied mob of `class`, scaled by `wave` and geared for
/// `rank`, with its nameplate.
pub(crate) fn spawn_class_mob(
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

/// Spawns one mob for a gate-break wave near the gate frame.
///
/// The mob appears on dry ground within [`GATE_BREAK_SPAWN_RADIUS`] of the
/// entrance, using the gate's own rank scaling.
#[expect(
    clippy::cast_possible_truncation,
    reason = "spawn radius is small and fits i32"
)]
pub fn spawn_city_break_mob(
    world: &Arc<World>,
    gate_entrance: BlockPos,
    wave: DungeonWave,
    rank: HunterRank,
    theme: Theme,
) -> Option<SpawnedMob> {
    let mut rng = rand::rng();
    let angle = rng.random_range(0.0..TAU);
    let distance = rng.random_range(2.0..=super::GATE_BREAK_SPAWN_RADIUS as f64);
    let x = (f64::from(gate_entrance.x()) + 0.5 + distance * angle.cos()).round() as i32;
    let z = (f64::from(gate_entrance.z()) + 0.5 + distance * angle.sin()).round() as i32;
    let surface = world.safe_surface_position(x, z)?;
    let roll = rng.random::<u64>();
    let class = MobClass::from_roll(roll ^ (roll >> 29));
    spawn_class_mob(world, theme, surface, class, wave, rank)
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
    let kind = if red {
        BossKind::for_red_gate()
    } else {
        BossKind::from_seed(layout.seed)
    };
    spawn_boss_body(
        world,
        position,
        boss_for_gate(kind, rank, red),
        boss_label(kind, rank, red),
    )
}

/// Spawns the body of `boss` at `position` under the nameplate `label`.
pub(crate) fn spawn_boss_body(
    world: &Arc<World>,
    position: BlockPos,
    boss: DungeonBoss,
    label: MobLabel,
) -> Option<SpawnedBoss> {
    let entity_type = if boss.kind == BossKind::Archer {
        &vanilla_entities::SKELETON
    } else {
        &vanilla_entities::ZOMBIE
    };
    let entity = spawn_mob(world, entity_type, position).ok()?;
    let living = entity.as_living_entity()?;
    boss.apply_to(living);
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
    // The chieftain has a name of its own and needs no red-gate prefix.
    let name = if red && kind != BossKind::ElfChieftain {
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
pub(crate) fn spawn_scaled_mob(
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
pub(crate) fn set_fixture(
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
    use steel_registry::blocks::block_state_ext::BlockStateExt as _;
    use steel_registry::blocks::properties::{BlockStateProperties, SlabType};
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::types::BlockStateId;
    use steel_utils::{BlockPos, ChunkPos};

    use super::{
        CRYSTAL_ORE, FrameStyle, INSTANCE_FLOOR_Y, at, dungeon_chunks, frame_block, gate_footprint,
        gate_frame_chunks, gate_is_level_at, opening_contains, penalty_arena_chunks,
    };
    use crate::dungeon::generator::{Part, THEMES};
    use crate::dungeon::{DungeonLayout, InstanceRegion};

    #[test]
    fn every_theme_builds_platforms_from_a_real_slab() {
        init_vanilla_registry();
        for theme in THEMES {
            let palette = theme.palette();
            let state = palette.block(Part::Platform, CRYSTAL_ORE).default_state();
            let bottom = state.set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Bottom);
            let top = state.set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Top);
            assert_ne!(
                bottom,
                top,
                "{theme:?} platform block {:?} has no slab type states",
                palette.block(Part::Platform, CRYSTAL_ORE).key,
            );
        }
    }

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
    fn a_gate_needs_level_ground_under_its_whole_footprint() {
        init_vanilla_registry();
        let entrance = BlockPos::new(10, 64, 0);
        let flat = |_: i32, _: i32| Some(entrance.y());
        assert!(gate_is_level_at(entrance, flat, dry_shore(entrance)));

        for (x, z) in gate_footprint(entrance) {
            // A roof or lamp above one column, a drop or water under it: the
            // frame would stand on top of something or hang over the edge.
            for surface in [Some(entrance.y() + 1), Some(entrance.y() - 1), None] {
                assert!(
                    !gate_is_level_at(
                        entrance,
                        |cx, cz| if (cx, cz) == (x, z) {
                            surface
                        } else {
                            flat(cx, cz)
                        },
                        dry_shore(entrance)
                    ),
                    "column ({x}, {z}) at {surface:?}"
                );
            }
        }
    }

    #[test]
    fn a_gate_rejects_a_flooded_opening() {
        init_vanilla_registry();
        let entrance = BlockPos::new(10, 64, 0);
        let flooded = BlockPos::new(entrance.x() + 1, entrance.y() + 2, 0);
        assert!(!gate_is_level_at(
            entrance,
            |_, _| Some(entrance.y()),
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
    fn the_castle_portal_never_masquerades_as_a_gate() {
        init_vanilla_registry();
        let entrance = BlockPos::new(10, 64, 0);
        let corner = BlockPos::new(entrance.x() - 1, entrance.y(), entrance.z());
        let body = BlockPos::new(entrance.x(), entrance.y(), entrance.z());

        let quartz = frame_block(FrameStyle::Normal, entrance, body);
        let red = frame_block(FrameStyle::Red, entrance, body);
        let castle = frame_block(FrameStyle::Castle, entrance, body);
        let castle_corner = frame_block(FrameStyle::Castle, entrance, corner);

        assert_eq!(quartz, vanilla_blocks::QUARTZ_BLOCK.default_state());
        assert_eq!(red, vanilla_blocks::RED_STAINED_GLASS.default_state());
        assert_eq!(
            castle,
            vanilla_blocks::POLISHED_BLACKSTONE_BRICKS.default_state()
        );
        assert_eq!(
            castle_corner,
            vanilla_blocks::CRYING_OBSIDIAN.default_state()
        );
        assert_ne!(castle, quartz);
        assert_ne!(castle, red);
        assert_ne!(castle_corner, castle);
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
        init_vanilla_registry();
        let region = InstanceRegion::from_slot(3);
        for seed in [0, 7, 3 << 56, u64::MAX] {
            let layout = DungeonLayout::from_seed(seed);
            let previous = DungeonLayout::for_gate(seed ^ 0xABCD, true);
            let chunks = dungeon_chunks(region, &layout, Some(previous.footprint()));
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
