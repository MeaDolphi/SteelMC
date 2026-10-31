//! Shared scenario-instance builder for one-off scripted arena fights.
//!
//! The Cartenon Temple and the Second Awakening both run in the same shared
//! instance world as dungeons, but in their own slot range so they never
//! collide with the dungeon allocator. Each run builds a small arena, spawns
//! one boss, and returns the hunter when the boss falls.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use rustc_hash::FxHashMap;
use steel_core::chunk::chunk_request::{ChunkRequest, ChunkTicketKind};
use steel_core::chunk::status::ChunkStatus;
use steel_core::entity::LivingEntity;
use steel_core::entity::spawn_mob;
use steel_core::gameplay_hooks::BlockMutationCause;
use steel_core::player::Player;
use steel_core::server::Server;
use steel_core::server::jobs::{ChunkRequestJob, ServerJobContext};
use steel_core::world::World;
use steel_registry::{vanilla_blocks, vanilla_entities};
use steel_utils::BlockPos;
use steel_utils::ChunkPos;
use steel_utils::locks::SyncMutex;
use steel_utils::types::{BlockStateId, UpdateFlags};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::combat_reward::HunterReward;
use crate::dungeon::{INSTANCE_FLOOR_Y, INSTANCE_WORLD_PATH, InstanceRegion, boss::DungeonBoss};
use crate::item::artifact::{self, ArtifactId};
use crate::quest_book::Goal;
use crate::rank::HunterRank;
use crate::state::SoloLevelingState;

/// Slot base for scenario instances, far above the dungeon allocator.
const SCENARIO_SLOT_BASE: u64 = 1_000_000;
/// Seconds a scenario run may last before the hunter is sent home.
const SCENARIO_TIME_LIMIT_SECONDS: u64 = 10 * 60;
/// Ticks per second.
const TICKS_PER_SECOND: u64 = 20;

/// Temple arena: wide ceremonial hall.
const TEMPLE_WIDTH: i32 = 25;
const TEMPLE_DEPTH: i32 = 15;
const TEMPLE_HEIGHT: i32 = 6;
/// Job-change arena: compact knightly circle/square.
const JOB_CHANGE_WIDTH: i32 = 21;
const JOB_CHANGE_DEPTH: i32 = 21;
const JOB_CHANGE_HEIGHT: i32 = 5;

static NEXT_SLOT: AtomicU64 = AtomicU64::new(SCENARIO_SLOT_BASE);
static RUNS: LazyLock<SyncMutex<FxHashMap<Uuid, ScenarioRun>>> =
    LazyLock::new(|| SyncMutex::new(FxHashMap::default()));

/// Which scripted scenario is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioKind {
    /// Cartenon Temple: the Smiling God.
    Temple,
    /// Second Awakening: Knight Igris.
    JobChange,
}

impl ScenarioKind {
    fn arena_size(self) -> (i32, i32, i32) {
        match self {
            Self::Temple => (TEMPLE_WIDTH, TEMPLE_DEPTH, TEMPLE_HEIGHT),
            Self::JobChange => (JOB_CHANGE_WIDTH, JOB_CHANGE_DEPTH, JOB_CHANGE_HEIGHT),
        }
    }

    fn floor_block(self) -> BlockStateId {
        match self {
            Self::Temple => vanilla_blocks::POLISHED_DEEPSLATE.default_state(),
            Self::JobChange => vanilla_blocks::POLISHED_BLACKSTONE.default_state(),
        }
    }

    fn wall_block(self) -> BlockStateId {
        match self {
            Self::Temple => vanilla_blocks::DEEPSLATE_BRICKS.default_state(),
            Self::JobChange => vanilla_blocks::BLACKSTONE.default_state(),
        }
    }
}

/// Live state of one scenario run.
struct ScenarioRun {
    kind: ScenarioKind,
    region: InstanceRegion,
    instance_world: Arc<World>,
    source_world: Arc<World>,
    rank: HunterRank,
    boss: DungeonBoss,
    boss_name: TextComponent,
    boss_id: i32,
    boss_max_health: f32,
    phase: u8,
    started_tick: u64,
    placed: bool,
    completed: bool,
}

/// Starts a scenario for `player` if they are not already in one.
///
/// The arena is built asynchronously; the boss spawns once the player arrives
/// in the instance world on a later tick.
pub fn start(
    state: Arc<SoloLevelingState>,
    player: &Player,
    kind: ScenarioKind,
    rank: HunterRank,
    boss: DungeonBoss,
    boss_name: TextComponent,
) -> Result<(), String> {
    let uuid = player.gameprofile.id();
    if RUNS.lock().contains_key(&uuid) {
        return Err("Вы уже участвуете в особом испытании.".to_owned());
    }
    let source_world = player.get_world();
    if source_world.key.path.as_ref() == INSTANCE_WORLD_PATH {
        return Err("Испытание нельзя начать внутри подземелья.".to_owned());
    }
    let Some(server) = state.server() else {
        return Err("Сервер не готов к запуску испытания.".to_owned());
    };
    let Some(instance_world) = instance_world_of(&server, &source_world) else {
        return Err("Мир испытаний не загружен.".to_owned());
    };
    let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
    let region = InstanceRegion::from_slot(slot);
    let (width, depth, height) = kind.arena_size();
    let chunks = arena_chunks(region, width, depth, height);
    let request = instance_world.chunk_map.request_chunks(ChunkRequest {
        status: ChunkStatus::Full,
        positions: chunks,
        ticket_kind: ChunkTicketKind::Command,
    });
    state.notify(
        uuid,
        "Подготавлиется арена испытания; вход займёт несколько секунд.",
    );
    let state_for_job = Arc::clone(&state);
    let instance_for_job = Arc::clone(&instance_world);
    let source_for_job = Arc::clone(&source_world);
    server.jobs.spawn(ChunkRequestJob::new(
        request,
        move |context: &mut ServerJobContext, _ready| {
            let Some(server) = context.server() else {
                return;
            };
            let Some(player) = find_player(&server, uuid) else {
                RUNS.lock().remove(&uuid);
                return;
            };
            build_arena(&instance_for_job, region, kind);
            let run = ScenarioRun {
                kind,
                region,
                instance_world: instance_for_job,
                source_world: source_for_job,
                rank,
                boss,
                boss_name,
                boss_id: 0,
                boss_max_health: 0.0,
                phase: 0,
                started_tick: state_for_job.current_tick(),
                placed: false,
                completed: false,
            };
            RUNS.lock().insert(uuid, run);
            let _ = player.send_message(
                &TextComponent::plain(match kind {
                    ScenarioKind::Temple => "Картенонский храм открыт.",
                    ScenarioKind::JobChange => "Арена Рыцаря Игриса готова.",
                })
                .color(Color::DarkPurple),
            );
            if let Err(error) =
                server.queue_player_world_selection(Arc::clone(&player), &player.get_world())
            {
                log::warn!("sololeveling: could not move hunter {uuid} into scenario: {error}");
                RUNS.lock().remove(&uuid);
            }
        },
    ));
    Ok(())
}

/// Advances every active scenario run.
pub fn tick(state: &SoloLevelingState, server: &Arc<Server>, tick: u64) {
    tick_runs(state, server, tick);
}

fn tick_runs(state: &SoloLevelingState, server: &Arc<Server>, tick: u64) {
    let mut to_remove = Vec::new();
    let mut runs = RUNS.lock();
    for (uuid, run) in runs.iter_mut() {
        let Some(player) = find_player(server, *uuid) else {
            to_remove.push(*uuid);
            continue;
        };
        if player.is_dead_or_dying() {
            to_remove.push(*uuid);
            continue;
        }
        if !run.placed {
            if player.get_world().key != run.instance_world.key {
                continue;
            }
            run.placed = true;
            let _ = player.teleport(run.region.spawn, 0.0, 0.0);
            let instance_world = Arc::clone(&run.instance_world);
            match spawn_boss(run, &instance_world) {
                Ok(id) => run.boss_id = id,
                Err(error) => {
                    log::warn!("sololeveling: could not spawn scenario boss for {uuid}: {error}");
                    to_remove.push(*uuid);
                    continue;
                }
            }
            player.send_message(
                &TextComponent::plain(match run.kind {
                    ScenarioKind::Temple => "Улыбающийся бог пробудился.",
                    ScenarioKind::JobChange => "Рыцарь Игрис вступает в бой.",
                })
                .color(Color::Red)
                .bold(true),
            );
        }
        if tick.saturating_sub(run.started_tick) >= SCENARIO_TIME_LIMIT_SECONDS * TICKS_PER_SECOND {
            player
                .send_message(&TextComponent::plain("Время испытания истекло.").color(Color::Red));
            to_remove.push(*uuid);
            continue;
        }
        if run.boss_id != 0 {
            if let Some(entity) = run.instance_world.get_entity_by_id(run.boss_id) {
                if run.completed
                    || entity
                        .as_living_entity()
                        .is_some_and(LivingEntity::is_dead_or_dying)
                {
                    complete_run(state, server, run, &Arc::clone(&player));
                    to_remove.push(*uuid);
                    continue;
                }
                tick_phases(run, &entity, &player, tick);
            } else {
                // Boss disappeared without a death hook; treat as failure.
                to_remove.push(*uuid);
            }
        }
    }
    for uuid in to_remove {
        if let Some(run) = runs.remove(&uuid) {
            if let Some(player) = find_player(server, uuid) {
                return_player(server, &player, &run.source_world);
            }
        }
    }
}

/// Marks a scenario run complete if `entity_id` is its boss.
pub fn on_boss_death(world: &World, entity_id: i32) {
    let mut runs = RUNS.lock();
    for run in runs.values_mut() {
        if run.instance_world.key == world.key && run.boss_id == entity_id {
            run.completed = true;
            return;
        }
    }
}

/// Drops a scenario run when its player dies.
pub fn on_player_death(uuid: Uuid) {
    RUNS.lock().remove(&uuid);
}

/// Drops a scenario run when its player disconnects.
pub fn on_disconnect(uuid: Uuid) {
    RUNS.lock().remove(&uuid);
}

fn complete_run(
    state: &SoloLevelingState,
    server: &Arc<Server>,
    run: &ScenarioRun,
    player: &Arc<Player>,
) {
    let uuid = player.gameprofile.id();
    match run.kind {
        ScenarioKind::Temple => {
            state.record_goal(uuid, Goal::TempleCleared, 1);
            let reward = artifact::stack(ArtifactId::SmilingGodsIdol, Some(uuid));
            state.queue_system_loot(uuid, reward);
            state.notify(
                uuid,
                "Картенонский храм пройден: Идол улыбающегося бога ваш.",
            );
            grant_scenario_experience(state, uuid, run.rank);
        }
        ScenarioKind::JobChange => {
            state.record_goal(uuid, Goal::IgrisDefeated, 1);
            state.with_session_mut(uuid, |session| session.class.evolved = true);
            state.apply_class(player);
            let _ = state.save_session(uuid);
            state.notify(
                uuid,
                "Второе пробуждение завершено: ваш класс эволюционировал.",
            );
            grant_scenario_experience(state, uuid, run.rank);
        }
    }
    return_player(server, player, &run.source_world);
}

fn grant_scenario_experience(state: &SoloLevelingState, uuid: Uuid, rank: HunterRank) {
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "the multiplier is a small positive constant of the rank"
    )]
    let experience = (1_000.0 * rank.rank_multiplier()) as u64;
    if let Some((stats, level_up)) = state.grant_experience(uuid, experience) {
        state.notify_progress(
            uuid,
            HunterReward {
                experience,
                system_points: 0,
            },
            level_up,
            stats.level,
        );
    }
}

fn return_player(server: &Arc<Server>, player: &Arc<Player>, source_world: &Arc<World>) {
    let _ = server.queue_player_world_selection(Arc::clone(player), source_world);
}

fn tick_phases(
    run: &mut ScenarioRun,
    entity: &Arc<dyn steel_core::entity::Entity>,
    player: &Player,
    tick: u64,
) {
    if run.kind != ScenarioKind::Temple {
        return;
    }
    let Some(living) = entity.as_living_entity() else {
        return;
    };
    let health_fraction = living.get_health() / run.boss_max_health.max(1.0);
    if run.phase == 0 && health_fraction < 0.66 {
        run.phase = 1;
        spawn_adds(run, player, 2, tick);
        player.send_message(&TextComponent::plain("Статуи храма ожили!").color(Color::Red));
    }
    if run.phase == 1 && health_fraction < 0.33 {
        run.phase = 2;
        spawn_adds(run, player, 4, tick);
        player.send_message(
            &TextComponent::plain("Храм гневается: ещё больше статуй!").color(Color::DarkRed),
        );
    }
}

fn spawn_adds(run: &ScenarioRun, player: &Player, count: usize, tick: u64) {
    let center = BlockPos::from(run.region.spawn);
    for i in 0..count {
        let offset_x = if i % 2 == 0 { 3 } else { -3 };
        let offset_z = if i < 2 { 3 } else { -3 };
        let pos = center.offset(offset_x, 0, offset_z);
        if let Ok(entity) = spawn_mob(&run.instance_world, &vanilla_entities::ZOMBIE, pos) {
            if let Some(living) = entity.as_living_entity() {
                let health = 20.0 * f64::from(run.rank.rank_multiplier());
                let damage = 3.0 * f64::from(run.rank.rank_multiplier());
                living.set_base_attribute_value(
                    steel_registry::vanilla_attributes::MAX_HEALTH,
                    health,
                );
                living.set_base_attribute_value(
                    steel_registry::vanilla_attributes::ATTACK_DAMAGE,
                    damage,
                );
                living.set_health(health as f32);
            }
            entity.set_custom_name(Some(
                TextComponent::plain("Ожившая статуя").color(Color::Gray),
            ));
            entity.set_custom_name_visible(true);
            entity.refresh_dimensions();
        }
    }
    let _ = tick;
    let _ = player;
}

fn spawn_boss(run: &mut ScenarioRun, world: &Arc<World>) -> Result<i32, String> {
    let pos = BlockPos::from(run.region.spawn).offset(0, 0, -4);
    let entity = spawn_mob(world, &vanilla_entities::ZOMBIE, pos).map_err(|e| format!("{e:?}"))?;
    let living = entity
        .as_living_entity()
        .ok_or("spawned boss has no living entity")?;
    run.boss.apply_to(living);
    run.boss_max_health = living.get_max_health();
    entity.set_custom_name(Some(run.boss_name.clone()));
    entity.set_custom_name_visible(true);
    entity.refresh_dimensions();
    Ok(entity.id())
}

fn build_arena(world: &Arc<World>, region: InstanceRegion, kind: ScenarioKind) {
    let (width, depth, height) = kind.arena_size();
    let floor = kind.floor_block();
    let wall = kind.wall_block();
    let air = vanilla_blocks::AIR.default_state();
    let origin = BlockPos::from(region.spawn);
    let floor_y = INSTANCE_FLOOR_Y - 1;
    let half_w = width / 2;
    let half_d = depth / 2;
    for x in -half_w..=half_w {
        for z in -half_d..=half_d {
            set_block(world, origin.offset(x, floor_y - origin.y(), z), floor);
        }
    }
    for y in 0..height {
        for x in -half_w..=half_w {
            set_block(world, origin.offset(x, y, -half_d), wall);
            set_block(world, origin.offset(x, y, half_d), wall);
        }
        for z in -half_d..=half_d {
            set_block(world, origin.offset(-half_w, y, z), wall);
            set_block(world, origin.offset(half_w, y, z), wall);
        }
    }
    for y in 0..height {
        for x in -(half_w - 1)..=(half_w - 1) {
            for z in -(half_d - 1)..=(half_d - 1) {
                set_block(world, origin.offset(x, y, z), air);
            }
        }
    }
}

fn set_block(world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
    let _ = world.set_block_with_cause(
        pos,
        state,
        UpdateFlags::UPDATE_ALL,
        BlockMutationCause::System,
    );
}

fn arena_chunks(region: InstanceRegion, width: i32, depth: i32, height: i32) -> Vec<ChunkPos> {
    let origin = BlockPos::from(region.spawn);
    let floor_y = INSTANCE_FLOOR_Y - 1;
    let half_w = width / 2;
    let half_d = depth / 2;
    let min = origin.offset(-half_w, floor_y - origin.y(), -half_d);
    let max = origin.offset(half_w, height, half_d);
    chunks_covering(min, max)
}

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

fn instance_world_of(server: &Arc<Server>, source_world: &Arc<World>) -> Option<Arc<World>> {
    server
        .worlds
        .values()
        .find(|candidate| {
            candidate.domain() == source_world.domain()
                && candidate.key.path.as_ref() == INSTANCE_WORLD_PATH
        })
        .cloned()
}

fn find_player(server: &Server, uuid: Uuid) -> Option<Arc<Player>> {
    server
        .get_players()
        .into_iter()
        .find(|player| player.gameprofile.id() == uuid)
}

impl SoloLevelingState {
    /// Advances every active scenario run.
    pub fn tick_scenarios(&self, server: &Arc<Server>, tick: u64) {
        tick_runs(self, server, tick);
    }
}

impl HunterRank {
    fn rank_multiplier(self) -> f32 {
        match self {
            HunterRank::E => 1.0,
            HunterRank::D => 1.25,
            HunterRank::C => 1.5,
            HunterRank::B => 2.0,
            HunterRank::A => 2.5,
            HunterRank::S => 3.0,
        }
    }
}
