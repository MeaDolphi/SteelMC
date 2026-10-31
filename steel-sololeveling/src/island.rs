//! The infested island: a shared world every hunter of rank C or higher can
//! travel to and help clear, after the ant island of Solo Leveling.
//!
//! The island is one round world in the sea, split into rings from the shore
//! to the centre: shore (C), jungle (B), nest fields (A) and the royal clutch
//! (S). Every ring holds a fixed number of nests. A nest is a huge, unmoving
//! ant that keeps hatching smaller ants around it until a hunter kills it.
//! When every nest is gone, the Ant King wakes in the centre; his death calms
//! the island for half an hour, after which the infestation starts over.
//!
//! The progress is shared by everyone on the island and lives in memory: a
//! restart starts a fresh infestation.

use std::f64::consts::TAU;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use glam::{DVec3, IVec3};
use rustc_hash::FxHashSet;
use steel_core::chunk::Chunk;
use steel_core::entity::{Entity as _, LivingEntity, RemovalReason, SharedEntity, spawn_mob};
use steel_core::player::Player;
use steel_core::server::Server;
use steel_core::world::World;
use steel_core::worldgen::generator::{
    ChunkGenerator, FlatChunkGenerator, GenerationChunk, TerrainPhase,
};
use steel_core::worldgen::{
    Beardifier, ChunkGeneratorType, CustomWorldGeneratorConfig, DynamicChunkGenerator,
    GeneratorOutput, WorldGenRegion, WorldGeneratorRegistry,
};
use steel_registry::biome::BiomeRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_dimension_types::OVERWORLD;
use steel_registry::{vanilla_attributes, vanilla_blocks, vanilla_entities};
use steel_utils::random::RandomSource;
use steel_utils::types::BlockStateId;
use steel_utils::{BlockPos, ChunkPos, Identifier, WorldAabb};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::dungeon::wave_for_rank;
use crate::item::chest::ChestKind;
use crate::item::key;
use crate::item::material::{self, Material, Quality};
use crate::rank::HunterRank;

/// World name of the shared island.
pub const WORLD_NAME: &str = "sololeveling_island";
/// Lowest rank allowed on the island.
pub const ACCESS_RANK: HunterRank = HunterRank::C;
/// Street level of the island, matching the city.
const GROUND_Y: i32 = 3;
/// Radius of the land; the sea starts beyond it.
const ISLAND_RADIUS: i32 = 150;
/// Where hunters land: on the south shore, facing the island.
pub const LANDING: BlockPos = BlockPos::new(0, GROUND_Y + 1, 132);
/// Where the evacuation officer stands on the landing beach.
pub const EVACUATION_POS: DVec3 = DVec3::new(3.5, (GROUND_Y + 1) as f64, 134.5);
/// Seconds the island stays calm after the Ant King dies.
const CALM_SECONDS: u64 = 30 * 60;
/// Ants one nest keeps alive around it.
const ANTS_PER_NEST: usize = 5;
/// Seconds between two hatchings of one nest.
const HATCH_SECONDS: u64 = 15;
/// How close a hunter has to be for a nest to be awake.
const AWAKE_RADIUS: f64 = 56.0;
/// How far from its nest an ant hatches.
const HATCH_RADIUS: i32 = 5;

/// One ring of the island.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Zone {
    /// The outer beach.
    Shore,
    /// The jungle belt.
    Jungle,
    /// The nest fields.
    Nests,
    /// The royal clutch in the centre.
    Royal,
}

/// Every zone from the shore inwards, with its outer radius and nest count.
const ZONES: [(Zone, i32, usize); 4] = [
    (Zone::Shore, ISLAND_RADIUS, 6),
    (Zone::Jungle, 105, 5),
    (Zone::Nests, 70, 4),
    (Zone::Royal, 35, 3),
];

impl Zone {
    /// Returns the rank the zone's ants fight and pay at.
    #[must_use]
    pub const fn rank(self) -> HunterRank {
        match self {
            Self::Shore => HunterRank::C,
            Self::Jungle => HunterRank::B,
            Self::Nests => HunterRank::A,
            Self::Royal => HunterRank::S,
        }
    }

    /// Returns the Russian name of the zone.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Shore => "Берег",
            Self::Jungle => "Джунгли",
            Self::Nests => "Поля гнёзд",
            Self::Royal => "Королевская кладка",
        }
    }

    /// Returns the zone of the column at horizontal distance `radius`.
    #[must_use]
    pub fn at_radius(radius: f64) -> Option<Self> {
        ZONES
            .iter()
            .rev()
            .find(|(_, outer, _)| radius <= f64::from(*outer))
            .map(|(zone, _, _)| *zone)
    }

    /// Returns the inner radius of the zone.
    fn inner(self) -> i32 {
        let index = ZONES
            .iter()
            .position(|(zone, _, _)| *zone == self)
            .unwrap_or(0);
        ZONES.get(index + 1).map_or(0, |(_, outer, _)| *outer)
    }
}

/// Returns every nest of the island: its zone and the column it stands on.
///
/// Nests sit evenly around the middle of their ring, each ring turned a
/// little against the next, so they never line up with the landing path.
#[must_use]
pub fn nests() -> Vec<(Zone, BlockPos)> {
    let mut nests = Vec::new();
    for (ring, (zone, outer, count)) in ZONES.iter().enumerate() {
        let middle = f64::from(zone.inner() + outer) / 2.0;
        for index in 0..*count {
            let angle = TAU * (index as f64 + 0.5 * ring as f64) / *count as f64 + 0.3;
            nests.push((
                *zone,
                BlockPos::new(
                    (middle * angle.cos()).round() as i32,
                    GROUND_Y + 1,
                    (middle * angle.sin()).round() as i32,
                ),
            ));
        }
    }
    nests
}

/// Returns whether `world` is the shared island.
#[must_use]
pub fn is_island_world(world: &World) -> bool {
    world.key.path.as_ref() == WORLD_NAME
}

// ---------------------------------------------------------------------------
// World generation
// ---------------------------------------------------------------------------

/// Adds `sololeveling:island` to Steel's world generator registry.
pub fn register_world_generator(registry: &mut WorldGeneratorRegistry) -> Result<(), String> {
    registry.register_custom(
        Identifier::new("sololeveling", "island"),
        |config| {
            if config.as_table().is_some_and(|table| !table.is_empty()) {
                return Err("island generator does not accept custom config".to_owned());
            }
            Ok(CustomWorldGeneratorConfig {
                data: config.clone(),
                dimension_type: Identifier::vanilla_static("overworld"),
            })
        },
        |_path: Option<&Path>, config, seed, _pool| {
            Ok(GeneratorOutput {
                dimension_type: &OVERWORLD,
                config: config.data.clone(),
                generator: ChunkGeneratorType::Custom(DynamicChunkGenerator::new(
                    IslandChunkGenerator {
                        flat: FlatChunkGenerator::new_layers(base_layers()),
                        seed: seed as u64,
                        nests: nests(),
                    },
                )),
                is_flat: true,
                sea_level: GROUND_Y,
            })
        },
    )
}

fn base_layers() -> Vec<BlockStateId> {
    let depth = usize::try_from(GROUND_Y - OVERWORLD.min_y + 1).unwrap_or(0);
    let mut layers = vec![vanilla_blocks::STONE.default_state(); depth];
    if let Some(bottom) = layers.first_mut() {
        *bottom = vanilla_blocks::BEDROCK.default_state();
    }
    let len = layers.len();
    for layer in layers.iter_mut().skip(len.saturating_sub(3)) {
        *layer = vanilla_blocks::SAND.default_state();
    }
    layers
}

struct IslandChunkGenerator {
    flat: FlatChunkGenerator,
    seed: u64,
    nests: Vec<(Zone, BlockPos)>,
}

impl ChunkGenerator for IslandChunkGenerator {
    fn min_y(&self) -> i32 {
        self.flat.min_y()
    }

    fn gen_depth(&self) -> i32 {
        self.flat.gen_depth()
    }

    fn noise_biome(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> BiomeRef {
        self.flat.noise_biome(quart_x, quart_y, quart_z)
    }

    fn initial_spawn_search_origin(&self) -> BlockPos {
        LANDING
    }

    fn spawn_height(&self, _min_y: i32, _height: i32) -> i32 {
        GROUND_Y + 1
    }

    fn create_structures(&self, chunk: &Chunk) {
        self.flat.create_structures(chunk);
    }

    fn create_biomes(&self, chunk: &Chunk) {
        self.flat.create_biomes(chunk);
    }

    fn build_terrain(
        &self,
        chunk: GenerationChunk<'_, TerrainPhase>,
        beardifier: Option<&Beardifier>,
        neighbor_biomes: &dyn Fn(IVec3) -> u16,
    ) {
        self.flat.build_terrain(chunk, beardifier, neighbor_biomes);
        let pos = chunk.pos();
        for local_x in 0..16 {
            for local_z in 0..16 {
                self.place_column(chunk, pos.0.x * 16 + local_x, pos.0.y * 16 + local_z);
            }
        }
    }

    fn fill_from_noise(
        &self,
        chunk: GenerationChunk<'_, TerrainPhase>,
        beardifier: Option<&Beardifier>,
    ) {
        self.flat.fill_from_noise(chunk, beardifier);
    }

    fn build_surface(
        &self,
        chunk: GenerationChunk<'_, TerrainPhase>,
        neighbor_biomes: &dyn Fn(IVec3) -> u16,
    ) {
        self.flat.build_surface(chunk, neighbor_biomes);
    }

    fn apply_carvers(&self, chunk: GenerationChunk<'_, TerrainPhase>) {
        self.flat.apply_carvers(chunk);
    }

    fn create_worldgen_region_random(&self, world_seed: i64, center: ChunkPos) -> RandomSource {
        self.flat.create_worldgen_region_random(world_seed, center)
    }

    fn apply_biome_decorations(&self, region: &mut WorldGenRegion<'_>) {
        self.flat.apply_biome_decorations(region);
    }
}

impl IslandChunkGenerator {
    fn place_column(&self, chunk: GenerationChunk<'_, TerrainPhase>, x: i32, z: i32) {
        let set = |y: i32, state: BlockStateId| {
            chunk.set_block_state(BlockPos::new(x, y, z), state);
        };
        let radius = f64::from(x).hypot(f64::from(z));
        let Some(zone) = Zone::at_radius(radius) else {
            // Open sea around the island.
            set(GROUND_Y, vanilla_blocks::WATER.default_state());
            set(GROUND_Y - 1, vanilla_blocks::WATER.default_state());
            return;
        };
        let hash = column_hash(self.seed, x, z);
        let ground = match zone {
            Zone::Shore if radius > f64::from(ISLAND_RADIUS - 12) => {
                vanilla_blocks::SAND.default_state()
            }
            Zone::Shore | Zone::Jungle => vanilla_blocks::GRASS_BLOCK.default_state(),
            Zone::Nests if hash.is_multiple_of(3) => vanilla_blocks::PODZOL.default_state(),
            Zone::Nests => vanilla_blocks::COARSE_DIRT.default_state(),
            Zone::Royal if hash.is_multiple_of(4) => vanilla_blocks::MUD_BRICKS.default_state(),
            Zone::Royal => vanilla_blocks::PACKED_MUD.default_state(),
        };
        set(GROUND_Y, ground);

        // Nest mounds: a broken ring of mud around every nest.
        for (_, nest) in &self.nests {
            let distance = f64::from(x - nest.x()).hypot(f64::from(z - nest.z()));
            if (3.5..=4.5).contains(&distance) && !hash.is_multiple_of(5) {
                set(GROUND_Y + 1, vanilla_blocks::PACKED_MUD.default_state());
                if hash.is_multiple_of(2) {
                    set(GROUND_Y + 2, vanilla_blocks::MUD_BRICKS.default_state());
                }
                return;
            }
            if distance < 3.5 {
                set(GROUND_Y, vanilla_blocks::MUD.default_state());
                return;
            }
        }
        // The landing beach and the royal clutch stay open ground.
        let landing = f64::from(x - LANDING.x()).hypot(f64::from(z - LANDING.z())) < 12.0;
        if landing || zone == Zone::Royal {
            return;
        }
        if zone == Zone::Jungle || (zone == Zone::Shore && radius < f64::from(ISLAND_RADIUS - 12)) {
            self.place_tree(chunk, x, z, zone == Zone::Jungle);
        } else if zone == Zone::Nests && hash.is_multiple_of(23) {
            set(GROUND_Y + 1, vanilla_blocks::DEAD_BUSH.default_state());
        }
    }

    /// Places the part of a jungle tree that falls into column `(x, z)`.
    ///
    /// Trees stand on a 9-block grid with a hashed offset; every column asks
    /// the nearest grid tree whether it is a trunk or under its canopy.
    fn place_tree(&self, chunk: GenerationChunk<'_, TerrainPhase>, x: i32, z: i32, dense: bool) {
        let cell_x = x.div_euclid(9);
        let cell_z = z.div_euclid(9);
        let roll = column_hash(self.seed ^ 0x7ee5, cell_x, cell_z);
        if !dense && !roll.is_multiple_of(3) {
            return;
        }
        let tree_x = cell_x * 9 + 2 + (roll % 5) as i32;
        let tree_z = cell_z * 9 + 2 + ((roll >> 8) % 5) as i32;
        let tree_radius = f64::from(tree_x).hypot(f64::from(tree_z));
        // Keep nests and their mounds clear of trunks.
        if self
            .nests
            .iter()
            .any(|(_, nest)| f64::from(tree_x - nest.x()).hypot(f64::from(tree_z - nest.z())) < 8.0)
            || Zone::at_radius(tree_radius).is_none()
        {
            return;
        }
        let height = 5 + (roll >> 16) as i32 % 3;
        let dx = (x - tree_x).abs();
        let dz = (z - tree_z).abs();
        let leaves = vanilla_blocks::JUNGLE_LEAVES
            .default_state()
            .set_value(&BlockStateProperties::PERSISTENT, true);
        if dx == 0 && dz == 0 {
            for y in GROUND_Y + 1..GROUND_Y + 1 + height {
                chunk.set_block_state(
                    BlockPos::new(x, y, z),
                    vanilla_blocks::JUNGLE_LOG.default_state(),
                );
            }
            chunk.set_block_state(BlockPos::new(x, GROUND_Y + 1 + height, z), leaves);
        } else if dx <= 2 && dz <= 2 && dx + dz < 4 {
            let top = GROUND_Y + height;
            chunk.set_block_state(BlockPos::new(x, top, z), leaves);
            if dx <= 1 && dz <= 1 {
                chunk.set_block_state(BlockPos::new(x, top + 1, z), leaves);
            }
        }
    }
}

const fn column_hash(seed: u64, x: i32, z: i32) -> u64 {
    let mut value =
        seed ^ (x as u64).wrapping_mul(73_856_093) ^ (z as u64).wrapping_mul(2_654_435_761);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^ (value >> 27)
}

// ---------------------------------------------------------------------------
// The shared infestation
// ---------------------------------------------------------------------------

/// Live state of one nest.
#[derive(Debug)]
struct Nest {
    zone: Zone,
    pos: BlockPos,
    /// The nest body while it lives.
    core: Option<i32>,
    /// Ants it hatched that are still alive.
    ants: Vec<i32>,
    destroyed: bool,
    /// Unix second at which the next ant may hatch; zero means as soon as possible.
    next_hatch: u64,
    /// Whether leftovers of an earlier run were swept from around the nest.
    swept: bool,
    /// Ants that should be respawned after a restore before normal hatching resumes.
    restore_ants: usize,
}

/// What died on the island, and who is paid for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IslandKill {
    /// An ant of the given rank.
    Ant(HunterRank),
    /// A nest of a zone; `zone_cleared` when it was the zone's last one.
    Nest {
        /// Zone of the nest.
        zone: Zone,
        /// Nests still standing on the whole island.
        left: usize,
        /// Whether this was the last nest of its zone.
        zone_cleared: bool,
    },
    /// The Ant King; everyone who fought on the island is paid.
    King {
        /// Hunters credited with the kill.
        hunters: Vec<Uuid>,
    },
}

/// The shared island infestation.
#[derive(Debug)]
pub struct IslandRuntime {
    nests: Vec<Nest>,
    king: Option<i32>,
    /// Unix second the calm after the King ends; zero while infested.
    calm_until: u64,
    /// Hunters who hit something on the island since the last King.
    fighters: FxHashSet<Uuid>,
}

impl Default for IslandRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl IslandRuntime {
    /// Starts a fresh infestation.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nests: nests()
                .into_iter()
                .map(|(zone, pos)| Nest {
                    zone,
                    pos,
                    core: None,
                    ants: Vec::new(),
                    destroyed: false,
                    next_hatch: 0,
                    swept: false,
                    restore_ants: 0,
                })
                .collect(),
            king: None,
            calm_until: 0,
            fighters: FxHashSet::default(),
        }
    }

    /// Returns how many nests still stand.
    #[must_use]
    pub fn nests_left(&self) -> usize {
        self.nests.iter().filter(|nest| !nest.destroyed).count()
    }

    /// Returns whether the island is in the calm period after the King died.
    #[must_use]
    pub const fn is_calm(&self) -> bool {
        self.calm_until > 0
    }

    /// Remembers that `hunter` fought on the island.
    pub fn record_fighter(&mut self, hunter: Uuid) {
        self.fighters.insert(hunter);
    }

    /// Advances the island once per second: wakes nests near hunters, hatches
    /// ants, raises the King, and ends the calm.
    pub fn tick(&mut self, server: &Server, tick: u64) {
        if !tick.is_multiple_of(20) {
            return;
        }
        let Some(world) = server.worlds.values().find(|world| is_island_world(world)) else {
            return;
        };
        let now = unix_now();
        if self.calm_until > 0 {
            if now < self.calm_until {
                show_status(
                    world,
                    &format!(
                        "Остров очищен. Новое заражение через {} мин.",
                        (self.calm_until - now).div_ceil(60)
                    ),
                );
                return;
            }
            *self = Self::new();
            broadcast(
                world,
                "Остров снова заражён: гнёзда проснулись.",
                Color::Red,
            );
        }
        let hunters = hunter_positions(world);
        for nest in &mut self.nests {
            if nest.destroyed {
                continue;
            }
            let center = DVec3::new(
                f64::from(nest.pos.x()) + 0.5,
                f64::from(nest.pos.y()),
                f64::from(nest.pos.z()) + 0.5,
            );
            if !hunters
                .iter()
                .any(|hunter| hunter.distance(center) <= AWAKE_RADIUS)
            {
                continue;
            }
            if !nest.swept {
                sweep_leftovers(world, nest.pos);
                nest.swept = true;
            }
            nest.ants.retain(|id| alive(world, *id));
            if nest.core.is_none_or(|id| !alive(world, id)) {
                nest.core = spawn_nest(world, nest.zone, nest.pos);
            }
            let restore_target = nest.ants.len().max(nest.restore_ants).min(ANTS_PER_NEST);
            while nest.ants.len() < restore_target {
                let offset =
                    (nest.ants.len() as i32).rem_euclid(2 * HATCH_RADIUS + 1) - HATCH_RADIUS;
                let spot =
                    BlockPos::new(nest.pos.x() + offset, nest.pos.y(), nest.pos.z() - offset);
                if let Some(id) = spawn_ant(world, nest.zone, spot) {
                    nest.ants.push(id);
                } else {
                    break;
                }
            }
            nest.restore_ants = 0;
            if nest.ants.len() < ANTS_PER_NEST && now >= nest.next_hatch {
                nest.next_hatch = now + HATCH_SECONDS;
                let offset = (tick as i32).rem_euclid(2 * HATCH_RADIUS + 1) - HATCH_RADIUS;
                let spot =
                    BlockPos::new(nest.pos.x() + offset, nest.pos.y(), nest.pos.z() - offset);
                if let Some(id) = spawn_ant(world, nest.zone, spot) {
                    nest.ants.push(id);
                }
            }
        }
        if self.nests_left() == 0 {
            let center = DVec3::new(0.5, f64::from(GROUND_Y + 1), 0.5);
            let king_alive = self.king.is_some_and(|id| alive(world, id));
            if !king_alive
                && hunters
                    .iter()
                    .any(|hunter| hunter.distance(center) <= AWAKE_RADIUS)
            {
                self.king = spawn_king(world);
                if self.king.is_some() {
                    broadcast(
                        world,
                        "Король муравьёв пробудился в центре острова!",
                        Color::DarkRed,
                    );
                }
            }
            show_status(world, "Все гнёзда уничтожены — убейте Короля муравьёв!");
        } else {
            show_status(
                world,
                &format!(
                    "Заражение острова: осталось гнёзд {} из {}",
                    self.nests_left(),
                    self.nests.len()
                ),
            );
        }
    }

    /// Settles a death on the island; `None` when the entity was not an
    /// island mob.
    pub fn on_death(&mut self, world: &World, entity_id: i32) -> Option<IslandKill> {
        if self.king == Some(entity_id) {
            self.king = None;
            self.calm_until = unix_now() + CALM_SECONDS;
            for nest in &mut self.nests {
                for id in nest.ants.drain(..) {
                    if let Some(entity) = world.get_entity_by_id(id) {
                        entity.set_removed(RemovalReason::Discarded);
                    }
                }
            }
            broadcast(
                world,
                "Король муравьёв повержен! Остров очищен.",
                Color::Gold,
            );
            return Some(IslandKill::King {
                hunters: self.fighters.drain().collect(),
            });
        }
        if let Some(index) = self
            .nests
            .iter()
            .position(|nest| nest.core == Some(entity_id))
        {
            let zone = self.nests[index].zone;
            self.nests[index].core = None;
            self.nests[index].destroyed = true;
            let zone_cleared = self
                .nests
                .iter()
                .all(|nest| nest.zone != zone || nest.destroyed);
            let left = self.nests_left();
            broadcast(
                world,
                &format!(
                    "Гнездо уничтожено ({}). Осталось: {left}.",
                    zone.display_name()
                ),
                Color::Green,
            );
            if zone_cleared {
                broadcast(
                    world,
                    &format!("Зона «{}» очищена!", zone.display_name()),
                    Color::Aqua,
                );
            }
            return Some(IslandKill::Nest {
                zone,
                left,
                zone_cleared,
            });
        }
        let nest = self
            .nests
            .iter_mut()
            .find(|nest| nest.ants.contains(&entity_id))?;
        nest.ants.retain(|id| *id != entity_id);
        Some(IslandKill::Ant(nest.zone.rank()))
    }
}

/// Serialized state of one nest, independent of transient entity ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NestSnapshot {
    /// Nest column, matched against the deterministic island layout on restore.
    pub pos: [i32; 3],
    /// Whether the nest body has been destroyed.
    pub destroyed: bool,
    /// Whether leftovers from an earlier run were already swept.
    pub swept: bool,
    /// How many ants were alive around the nest when the snapshot was taken.
    pub ant_count: usize,
    /// Unix second at which the next ant may hatch.
    pub next_hatch_at: u64,
}

/// Serialized shared island infestation, safe to publish as a gameplay document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct IslandSnapshot {
    /// Unix second the calm after the King ends; zero while infested.
    #[serde(default)]
    pub calm_until: u64,
    /// Per-nest progress, in no guaranteed order.
    #[serde(default)]
    pub nests: Vec<NestSnapshot>,
    /// Hunters who hit something on the island since the last King.
    #[serde(default)]
    pub fighters: Vec<Uuid>,
}

impl NestSnapshot {
    fn from_nest(nest: &Nest) -> Self {
        Self {
            pos: [nest.pos.x(), nest.pos.y(), nest.pos.z()],
            destroyed: nest.destroyed,
            swept: nest.swept,
            ant_count: nest.ants.len(),
            next_hatch_at: nest.next_hatch,
        }
    }
}

impl IslandRuntime {
    /// Captures the persistent parts of the current infestation.
    #[must_use]
    pub fn snapshot(&self) -> IslandSnapshot {
        IslandSnapshot {
            calm_until: self.calm_until,
            nests: self.nests.iter().map(NestSnapshot::from_nest).collect(),
            fighters: self.fighters.iter().copied().collect(),
        }
    }

    /// Replaces live state with a previously published snapshot.
    ///
    /// Live entity ids are intentionally discarded; ants and the King are
    /// respawned by the normal tick rules after the world is loaded.
    pub fn restore(&mut self, snapshot: IslandSnapshot) {
        let expected = nests();
        let mut restored = Vec::with_capacity(expected.len());
        for (zone, pos) in expected {
            let saved = snapshot
                .nests
                .iter()
                .find(|nest| nest.pos == [pos.x(), pos.y(), pos.z()]);
            restored.push(Nest {
                zone,
                pos,
                core: None,
                ants: Vec::new(),
                destroyed: saved.map_or(false, |nest| nest.destroyed),
                next_hatch: saved.map_or(0, |nest| nest.next_hatch_at),
                swept: saved.map_or(false, |nest| nest.swept),
                restore_ants: saved.map_or(0, |nest| nest.ant_count),
            });
        }
        *self = Self {
            nests: restored,
            king: None,
            calm_until: snapshot.calm_until,
            fighters: snapshot.fighters.iter().copied().collect(),
        };
    }
}

/// Returns the rewards of one island kill for the hunter who made it.
#[must_use]
pub fn kill_loot(kill: &IslandKill, roll: &mut impl FnMut() -> u32) -> Vec<ItemStack> {
    match kill {
        IslandKill::Ant(rank) => {
            let quality = Quality::for_rank(*rank);
            let mut drops = Vec::new();
            if roll() < 50 {
                drops.push(material::stack(Material::ChitinPlate, quality, 1));
            }
            if roll() < 20 {
                drops.push(material::stack(Material::ToughHide, quality, 1));
            }
            if roll() < material::ESSENCE_PERCENT {
                drops.push(material::stack(Material::StableEssence, quality, 1));
            }
            drops
        }
        IslandKill::Nest { zone, .. } => {
            let quality = Quality::for_rank(zone.rank());
            let mut drops = vec![
                material::stack(Material::ChitinPlate, quality, 3),
                material::stack(Material::ToughHide, quality, 1),
            ];
            if roll() < 30 {
                drops.push(material::stack(Material::StableEssence, quality, 1));
            }
            drops
        }
        IslandKill::King { .. } => Vec::new(),
    }
}

/// Returns the rewards one hunter of `rank` receives for the Ant King.
#[must_use]
pub fn king_loot(rank: HunterRank) -> Vec<ItemStack> {
    vec![
        ChestKind::Sacred.stack(),
        material::stack(Material::StableEssence, Quality::Superior, 2),
        material::stack(Material::ChitinPlate, Quality::Superior, 4),
        key::stack(rank),
        key::castle_stack(),
    ]
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn hunter_positions(world: &World) -> Vec<DVec3> {
    let mut positions = Vec::new();
    world.players.iter_players(|_, player| {
        if !player.is_dead_or_dying() {
            positions.push(player.position());
        }
        true
    });
    positions
}

fn alive(world: &World, id: i32) -> bool {
    world.get_entity_by_id(id).is_some_and(|entity| {
        !entity.is_removed()
            && entity
                .as_living_entity()
                .is_none_or(|living| !living.is_dead_or_dying())
    })
}

fn show_status(world: &World, line: &str) {
    world.players.iter_players(|_, player| {
        player.send_action_bar(TextComponent::plain(line.to_owned()).color(Color::Gold));
        true
    });
}

fn broadcast(world: &World, line: &str, color: Color) {
    world.players.iter_players(|_, player| {
        player.send_message(&TextComponent::plain(line.to_owned()).color(color.clone()));
        true
    });
}

/// Removes ants and nests a previous server run left around `pos`; the
/// infestation lives in memory, so they are no longer tracked.
fn sweep_leftovers(world: &World, pos: BlockPos) {
    let reach = f64::from(HATCH_RADIUS + 12);
    let center = DVec3::new(f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()));
    let area = WorldAabb::new(
        center.x - reach,
        center.y - 4.0,
        center.z - reach,
        center.x + reach,
        center.y + 12.0,
        center.z + reach,
    );
    for entity in world.get_entities_in_aabb_matching(&area, |entity| {
        entity.entity_type() == &vanilla_entities::ENDERMITE && !entity.is_removed()
    }) {
        entity.set_removed(RemovalReason::Discarded);
    }
}

/// Spawns one island mob: an endermite body scaled and strengthened.
fn spawn_island_mob(
    world: &Arc<World>,
    pos: BlockPos,
    scale: f64,
    health: f64,
    damage: f64,
    speed: Option<f64>,
    name: TextComponent,
) -> Option<i32> {
    let entity: SharedEntity = spawn_mob(world, &vanilla_entities::ENDERMITE, pos).ok()?;
    let living = entity.as_living_entity()?;
    living.set_base_attribute_value(vanilla_attributes::SCALE, scale);
    living.set_base_attribute_value(vanilla_attributes::MAX_HEALTH, health);
    living.set_base_attribute_value(vanilla_attributes::ATTACK_DAMAGE, damage);
    if let Some(speed) = speed {
        living.set_base_attribute_value(vanilla_attributes::MOVEMENT_SPEED, speed);
        living.set_base_attribute_value(vanilla_attributes::KNOCKBACK_RESISTANCE, 1.0);
    }
    living.set_health(health as f32);
    entity.set_custom_name(Some(name));
    entity.set_custom_name_visible(true);
    entity.refresh_dimensions();
    Some(entity.id())
}

fn spawn_ant(world: &Arc<World>, zone: Zone, pos: BlockPos) -> Option<i32> {
    let wave = wave_for_rank(zone.rank());
    spawn_island_mob(
        world,
        pos,
        1.8,
        24.0 * wave.health_multiplier,
        4.0 * wave.damage_multiplier,
        None,
        TextComponent::plain(format!("Муравей-солдат [{}]", zone.rank().name())).color(Color::Red),
    )
}

fn spawn_nest(world: &Arc<World>, zone: Zone, pos: BlockPos) -> Option<i32> {
    let wave = wave_for_rank(zone.rank());
    spawn_island_mob(
        world,
        pos,
        4.0,
        160.0 * wave.health_multiplier,
        3.0 * wave.damage_multiplier,
        Some(0.0),
        TextComponent::plain(format!("Гнездо [{}]", zone.display_name())).color(Color::DarkPurple),
    )
}

fn spawn_king(world: &Arc<World>) -> Option<i32> {
    let wave = wave_for_rank(HunterRank::S);
    spawn_island_mob(
        world,
        BlockPos::new(0, GROUND_Y + 1, 0),
        6.0,
        900.0 * wave.health_multiplier,
        5.0 * wave.damage_multiplier,
        None,
        TextComponent::plain("Король муравьёв").color(Color::DarkRed),
    )
}

/// Returns whether `player` may travel to the island.
#[must_use]
pub fn may_enter(rank: HunterRank) -> bool {
    rank >= ACCESS_RANK
}

/// Sends `player` to the island or tells them why not.
pub fn travel_to(server: &Server, player: Arc<Player>, rank: HunterRank) {
    if !may_enter(rank) {
        player.send_message(
            &TextComponent::plain(format!(
                "На остров пускают охотников с ранга {}.",
                ACCESS_RANK.name()
            ))
            .color(Color::Red),
        );
        return;
    }
    let Some(world) = server.worlds.values().find(|world| is_island_world(world)) else {
        return;
    };
    if let Err(error) = server.queue_player_world_selection(player, world) {
        log::warn!("sololeveling: could not send a hunter to the island: {error}");
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use uuid::Uuid;

    use super::{ISLAND_RADIUS, IslandRuntime, IslandSnapshot, LANDING, NestSnapshot, Zone, nests};

    #[test]
    fn zones_go_from_the_shore_to_the_centre() {
        assert_eq!(Zone::at_radius(0.0), Some(Zone::Royal));
        assert_eq!(Zone::at_radius(50.0), Some(Zone::Nests));
        assert_eq!(Zone::at_radius(90.0), Some(Zone::Jungle));
        assert_eq!(Zone::at_radius(140.0), Some(Zone::Shore));
        assert_eq!(Zone::at_radius(f64::from(ISLAND_RADIUS) + 1.0), None);
    }

    #[test]
    fn nests_stand_in_their_own_zone_and_off_the_landing() {
        let nests = nests();
        assert_eq!(nests.len(), 18);
        for (zone, pos) in nests {
            let radius = f64::from(pos.x()).hypot(f64::from(pos.z()));
            assert_eq!(Zone::at_radius(radius), Some(zone), "{pos:?}");
            let landing = f64::from(pos.x() - LANDING.x()).hypot(f64::from(pos.z() - LANDING.z()));
            assert!(landing > 16.0, "nest {pos:?} sits on the landing");
        }
    }

    #[test]
    fn island_snapshot_round_trips_nest_progress_and_calm() {
        let mut island = IslandRuntime::new();
        let first_pos = [
            island.nests[0].pos.x(),
            island.nests[0].pos.y(),
            island.nests[0].pos.z(),
        ];
        island.nests[0].destroyed = true;
        island.nests[0].swept = true;
        island.nests[0].next_hatch = 123;
        island.nests[1].ants.push(1);
        island.nests[1].ants.push(2);
        island.nests[1].next_hatch = 456;
        let fighter = Uuid::new_v4();
        island.fighters.insert(fighter);
        let calm_until = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs())
            + 1_000;
        island.calm_until = calm_until;

        let bytes = serde_json::to_vec(&island.snapshot()).expect("snapshot should serialize");
        let snapshot: IslandSnapshot =
            serde_json::from_slice(&bytes).expect("snapshot should load");

        let mut restored = IslandRuntime::new();
        restored.restore(snapshot);

        assert!(restored.is_calm());
        assert_eq!(restored.calm_until, calm_until);
        assert!(restored.fighters.contains(&fighter));
        assert_eq!(restored.nests.len(), island.nests.len());
        assert!(restored.nests[0].destroyed);
        assert!(restored.nests[0].swept);
        assert_eq!(restored.nests[0].next_hatch, 123);
        assert_eq!(restored.nests[0].restore_ants, 0);
        assert_eq!(restored.nests[1].restore_ants, 2);
        assert_eq!(restored.nests[1].next_hatch, 456);

        let missing_pos = [first_pos[0] + 1, first_pos[1], first_pos[2]];
        let orphan = NestSnapshot {
            pos: missing_pos,
            destroyed: true,
            swept: true,
            ant_count: 7,
            next_hatch_at: 999,
        };
        let mut with_orphan = IslandSnapshot::default();
        with_orphan.nests.push(orphan);
        let mut restored = IslandRuntime::new();
        restored.restore(with_orphan);
        assert!(!restored.nests.iter().any(|nest| nest.destroyed));
    }
}
