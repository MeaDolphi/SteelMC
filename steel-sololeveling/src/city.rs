//! Persistent, procedurally generated city world.

use std::path::Path;

use glam::{DVec3, IVec3};
use rand::RngExt as _;
use steel_core::chunk::Chunk;
use steel_core::world::World;
use steel_core::worldgen::generator::{
    ChunkGenerator, FlatChunkGenerator, GenerationChunk, TerrainPhase,
};
use steel_core::worldgen::{
    Beardifier, ChunkGeneratorType, CustomWorldGeneratorConfig, DynamicChunkGenerator,
    GeneratorOutput, WorldGenRegion, WorldGeneratorRegistry,
};
use steel_registry::biome::BiomeRef;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_dimension_types::OVERWORLD;
use steel_utils::random::RandomSource;
use steel_utils::{BlockPos, ChunkPos, Identifier};

use crate::city_interior::{self, AGENT_SPOT, LADDER_COLUMN, Storey};
/// World name used for the shared city and as the default-world entry.
pub const WORLD_NAME: &str = "sololeveling_city";
/// Where the Organization agent stands: behind the reception of the
/// Association lobby, facing the entrance.
pub const ORGANIZATION_AGENT_POS: DVec3 = DVec3::new(
    (LOT_SIZE + AGENT_SPOT.0) as f64 + 0.5,
    (GROUND_Y + 1) as f64,
    AGENT_SPOT.1 as f64 + 0.5,
);
/// Yaw of the Organization agent: facing north, towards the lobby door.
pub const ORGANIZATION_AGENT_YAW: f32 = 180.0;
/// Gate plaza centers around the urban core, relative to the city spawn.
pub const GATE_SITE_OFFSETS: [(i32, i32); 4] = [(288, 32), (-288, 32), (32, 288), (32, -288)];

/// Returns whether `world` is the shared city world.
#[must_use]
pub fn is_city_world(world: &World) -> bool {
    world.key.path.as_ref() == WORLD_NAME
}

/// Picks a gate position inside one of the generated perimeter gate plazas.
#[must_use]
pub fn gate_candidate(center: BlockPos) -> BlockPos {
    let mut rng = rand::rng();
    let (site_x, site_z) = GATE_SITE_OFFSETS[rng.random_range(0..GATE_SITE_OFFSETS.len())];
    BlockPos::new(
        center.x() + site_x + rng.random_range(-10..=10),
        0,
        center.z() + site_z + rng.random_range(-10..=10),
    )
}

const CITY_RADIUS: i32 = 384;
const LOT_SIZE: i32 = 64;
const GROUND_Y: i32 = 3;
const LOT_MIN: i32 = 14;
const LOT_MAX: i32 = 50;
/// Facade columns around the entrance kept free of windows.
const ENTRANCE_FRAME: (i32, i32) = (28, 35);

/// What a city lot is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum District {
    Plaza,
    Association,
    Guild,
    Market,
    GateSquare,
    Civic,
    Residential,
}

#[derive(Clone, Copy)]
struct CityPalette {
    plaza: steel_utils::BlockStateId,
    road: steel_utils::BlockStateId,
    sidewalk: steel_utils::BlockStateId,
    lamp: steel_utils::BlockStateId,
    ceiling_light: steel_utils::BlockStateId,
    association_wall: steel_utils::BlockStateId,
    guild_wall: steel_utils::BlockStateId,
    market_wall: steel_utils::BlockStateId,
    civic_wall: steel_utils::BlockStateId,
    residential_wall: steel_utils::BlockStateId,
    glass: steel_utils::BlockStateId,
    roof: steel_utils::BlockStateId,
    floor: steel_utils::BlockStateId,
    accent: steel_utils::BlockStateId,
}

impl CityPalette {
    fn new() -> Self {
        Self {
            plaza: vanilla_blocks::SMOOTH_QUARTZ.default_state(),
            road: vanilla_blocks::GRAY_CONCRETE.default_state(),
            sidewalk: vanilla_blocks::POLISHED_ANDESITE.default_state(),
            lamp: vanilla_blocks::SEA_LANTERN.default_state(),
            ceiling_light: vanilla_blocks::OCHRE_FROGLIGHT.default_state(),
            association_wall: vanilla_blocks::SMOOTH_QUARTZ.default_state(),
            guild_wall: vanilla_blocks::DARK_PRISMARINE.default_state(),
            market_wall: vanilla_blocks::BRICKS.default_state(),
            civic_wall: vanilla_blocks::STONE_BRICKS.default_state(),
            residential_wall: vanilla_blocks::LIGHT_GRAY_CONCRETE.default_state(),
            glass: vanilla_blocks::LIGHT_BLUE_STAINED_GLASS.default_state(),
            roof: vanilla_blocks::DEEPSLATE_TILES.default_state(),
            floor: vanilla_blocks::POLISHED_ANDESITE.default_state(),
            accent: vanilla_blocks::CYAN_CONCRETE.default_state(),
        }
    }

    const fn wall(self, district: District) -> steel_utils::BlockStateId {
        match district {
            District::Association => self.association_wall,
            District::Guild => self.guild_wall,
            District::Market => self.market_wall,
            District::Civic => self.civic_wall,
            District::Residential => self.residential_wall,
            District::Plaza | District::GateSquare => self.plaza,
        }
    }
}

/// Adds `sololeveling:city` to Steel's world generator registry.
pub fn register_world_generator(registry: &mut WorldGeneratorRegistry) -> Result<(), String> {
    registry.register_custom(
        Identifier::new("sololeveling", "city"),
        validate_city_config,
        |path, config, seed, thread_pool| {
            Ok(create_city_generator(path, config, seed, thread_pool))
        },
    )
}

fn validate_city_config(config: &toml::Value) -> Result<CustomWorldGeneratorConfig, String> {
    let Some(table) = config.as_table() else {
        return Err("city generator config must be a table".to_owned());
    };
    if !table.is_empty() {
        return Err("city generator does not accept custom config".to_owned());
    }
    Ok(CustomWorldGeneratorConfig {
        data: config.clone(),
        dimension_type: Identifier::vanilla_static("overworld"),
    })
}

fn create_city_generator(
    _world_path: Option<&Path>,
    config: &CustomWorldGeneratorConfig,
    seed: i64,
    _thread_pool: &rayon::ThreadPool,
) -> GeneratorOutput {
    let flat = FlatChunkGenerator::new_layers(city_layers());
    GeneratorOutput {
        dimension_type: &OVERWORLD,
        config: config.data.clone(),
        generator: ChunkGeneratorType::Custom(DynamicChunkGenerator::new(CityChunkGenerator {
            flat,
            seed: seed as u64,
            palette: CityPalette::new(),
        })),
        is_flat: true,
        sea_level: GROUND_Y,
    }
}

/// Returns the base layers from the bottom of the overworld up to
/// [`GROUND_Y`].
///
/// Flat layers start at the dimension's minimum height, so a short stack would
/// leave the street level floating far above the terrain with nothing below
/// any column the city does not pave.
fn city_layers() -> Vec<steel_utils::BlockStateId> {
    let depth = usize::try_from(GROUND_Y - OVERWORLD.min_y + 1).unwrap_or(0);
    let mut layers = vec![vanilla_blocks::STONE.default_state(); depth];
    if let Some(bottom) = layers.first_mut() {
        *bottom = vanilla_blocks::BEDROCK.default_state();
    }
    let len = layers.len();
    for layer in layers.iter_mut().skip(len.saturating_sub(4)) {
        *layer = vanilla_blocks::DIRT.default_state();
    }
    if let Some(top) = layers.last_mut() {
        *top = vanilla_blocks::GRASS_BLOCK.default_state();
    }
    layers
}

struct CityChunkGenerator {
    flat: FlatChunkGenerator,
    seed: u64,
    palette: CityPalette,
}

impl ChunkGenerator for CityChunkGenerator {
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
        BlockPos::new(0, GROUND_Y + 1, 0)
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
        let chunk_pos = chunk.pos();
        let min_x = chunk_pos.0.x * 16;
        let min_z = chunk_pos.0.y * 16;
        for local_x in 0..16 {
            for local_z in 0..16 {
                let x = min_x + local_x;
                let z = min_z + local_z;
                self.place_city_column(chunk, x, z);
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

impl CityChunkGenerator {
    fn place_city_column(&self, chunk: GenerationChunk<'_, TerrainPhase>, x: i32, z: i32) {
        if x.abs() > CITY_RADIUS || z.abs() > CITY_RADIUS {
            return;
        }

        let tile_x = x.div_euclid(LOT_SIZE);
        let tile_z = z.div_euclid(LOT_SIZE);
        let local_x = x.rem_euclid(LOT_SIZE);
        let local_z = z.rem_euclid(LOT_SIZE);
        let district = district(tile_x, tile_z);
        let height = self.building_height(tile_x, tile_z, district);
        let in_building = height > GROUND_Y
            && (LOT_MIN..=LOT_MAX).contains(&local_x)
            && (LOT_MIN..=LOT_MAX).contains(&local_z);
        let ground = if district == District::Plaza {
            Some(self.plaza_state(x, z))
        } else if district == District::GateSquare {
            self.gate_square_state(x, z, local_x, local_z)
        } else if is_road(local_x, local_z) {
            Some(self.palette.road)
        } else if is_sidewalk(local_x, local_z) {
            Some(self.palette.sidewalk)
        } else if in_building {
            Some(self.palette.floor)
        } else {
            None
        };

        if let Some(state) = ground {
            chunk.set_block_state(BlockPos::new(x, GROUND_Y, z), state);
        }

        if in_building {
            let variant = city_hash(self.seed ^ 0x5eed, tile_x, tile_z);
            for y in GROUND_Y + 1..=height + 1 {
                if let Some(state) = building_state(
                    district,
                    local_x,
                    local_z,
                    y,
                    height,
                    variant,
                    &self.palette,
                ) {
                    chunk.set_block_state(BlockPos::new(x, y, z), state);
                }
            }
        }

        let lamp = district != District::Plaza
            && (is_street_lamp(local_x, local_z)
                || (district == District::GateSquare && is_gate_square_lamp(local_x, local_z)));
        if !in_building && !lamp && is_ground_light(district, x, z) {
            chunk.set_block_state(
                BlockPos::new(x, GROUND_Y + 1, z),
                vanilla_blocks::LIGHT.default_state(),
            );
        }
        if lamp {
            for y in GROUND_Y + 1..=GROUND_Y + 5 {
                let state = if y == GROUND_Y + 5 {
                    self.palette.lamp
                } else {
                    vanilla_blocks::IRON_BLOCK.default_state()
                };
                chunk.set_block_state(BlockPos::new(x, y, z), state);
            }
        }
    }

    const fn building_height(&self, tile_x: i32, tile_z: i32, district: District) -> i32 {
        let base = match district {
            District::Association => 30,
            District::Guild => 24,
            District::Market => 18,
            District::Civic => 20,
            District::Residential => 12,
            District::Plaza | District::GateSquare => return GROUND_Y,
        };
        base + (city_hash(self.seed, tile_x, tile_z) % 8) as i32
    }

    const fn plaza_state(&self, x: i32, z: i32) -> steel_utils::BlockStateId {
        if x.rem_euclid(16) < 2 || z.rem_euclid(16) < 2 {
            return self.palette.accent;
        }
        if x.abs() <= 5 && z.abs() <= 5 {
            return self.palette.lamp;
        }
        self.palette.plaza
    }

    const fn gate_square_state(
        &self,
        x: i32,
        z: i32,
        local_x: i32,
        local_z: i32,
    ) -> Option<steel_utils::BlockStateId> {
        if (local_x - 32).abs() <= 14 && (local_z - 32).abs() <= 14 {
            if (local_x - 32).abs() == 14 || (local_z - 32).abs() == 14 {
                return Some(self.palette.accent);
            }
            return Some(self.palette.plaza);
        }
        if x.abs() % 16 < 2 || z.abs() % 16 < 2 {
            return Some(self.palette.sidewalk);
        }
        None
    }
}

const fn district(tile_x: i32, tile_z: i32) -> District {
    match (tile_x, tile_z) {
        (0, 0) => District::Plaza,
        (1, 0) => District::Association,
        (-1, 0) => District::Guild,
        (0, 1) => District::Market,
        (0, -1 | 4 | -5) | (4 | -5, 0) => District::GateSquare,
        (x, z) if x.abs() <= 2 && z.abs() <= 2 => District::Civic,
        _ => District::Residential,
    }
}

const fn is_road(local_x: i32, local_z: i32) -> bool {
    local_x < 8 || local_z < 8
}

const fn is_sidewalk(local_x: i32, local_z: i32) -> bool {
    (8 <= local_x && local_x < 14) || (8 <= local_z && local_z < 14)
}

/// Returns whether a lamp post stands here: one on every crossing and corner,
/// and one every sixteen blocks along the sidewalks in between.
const fn is_street_lamp(local_x: i32, local_z: i32) -> bool {
    let crossing = (local_x == 4 || local_x == 60) && (local_z == 4 || local_z == 60);
    let along = matches!(local_z, 22 | 38 | 54);
    let across = matches!(local_x, 22 | 38 | 54);
    crossing || (local_x == 10 && along) || (local_z == 10 && across)
}

/// Returns whether a lamp post marks a corner of a gate plaza.
const fn is_gate_square_lamp(local_x: i32, local_z: i32) -> bool {
    matches!(local_x, 17 | 47) && matches!(local_z, 17 | 47)
}

/// Returns whether an invisible light sits on this open ground column.
///
/// Lamps alone leave long dark stretches between them; a sparse grid of light
/// blocks keeps streets, lawns and plazas lit at night without anything to
/// see. Gate plazas are left out, because a gate frame needs empty air.
const fn is_ground_light(district: District, x: i32, z: i32) -> bool {
    !matches!(district, District::GateSquare) && x.rem_euclid(8) == 4 && z.rem_euclid(8) == 4
}

fn building_state(
    district: District,
    local_x: i32,
    local_z: i32,
    y: i32,
    height: i32,
    variant: u64,
    palette: &CityPalette,
) -> Option<steel_utils::BlockStateId> {
    let edge_x = local_x == LOT_MIN || local_x == LOT_MAX;
    let edge_z = local_z == LOT_MIN || local_z == LOT_MAX;
    let on_wall = edge_x || edge_z;
    let lit = !on_wall && local_x % 8 == 3 && local_z % 8 == 3;
    if y == height + 1 {
        return Some(if lit {
            palette.ceiling_light
        } else {
            palette.roof
        });
    }
    if y > GROUND_Y + 1 && (y - GROUND_Y - 1).rem_euclid(6) == 0 {
        if (local_x, local_z) == LADDER_COLUMN {
            return Some(city_interior::ladder());
        }
        return Some(if lit {
            palette.ceiling_light
        } else {
            palette.floor
        });
    }
    if !on_wall {
        return city_interior::interior_state(
            district,
            local_x,
            local_z,
            Storey::at(y, GROUND_Y),
            variant,
        );
    }
    // The entrance gets a frame of its own, so the doorway never cuts into a
    // window band.
    if edge_z
        && local_z == LOT_MIN
        && (ENTRANCE_FRAME.0..=ENTRANCE_FRAME.1).contains(&local_x)
        && y <= GROUND_Y + 5
    {
        let doorway = (local_x - 31).abs() <= 1 && y <= GROUND_Y + 3;
        if doorway {
            return None;
        }
        if local_x == 31 && y == GROUND_Y + 5 {
            return Some(palette.lamp);
        }
        let frame = (local_x - 31).abs() == 2 || (y == GROUND_Y + 4 && (local_x - 31).abs() <= 2);
        return Some(if frame {
            palette.accent
        } else {
            palette.wall(district)
        });
    }
    let facade_position = if edge_x { local_z } else { local_x };
    let window_row = (y - GROUND_Y - 2).rem_euclid(6);
    let window = window_row < 3
        && (facade_position - LOT_MIN).rem_euclid(8) >= 2
        && (facade_position - LOT_MIN).rem_euclid(8) <= 5;
    Some(if window {
        palette.glass
    } else if (local_x + local_z).rem_euclid(11) == 0 && district != District::Residential {
        palette.accent
    } else {
        palette.wall(district)
    })
}

const fn city_hash(seed: u64, x: i32, z: i32) -> u64 {
    let mut value =
        seed ^ (x as u64).wrapping_mul(73_856_093) ^ (z as u64).wrapping_mul(2_654_435_761);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^ (value >> 27)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_core::worldgen::{ChunkGenerator, WorldGeneratorRegistry};
    use steel_registry::{init_vanilla_registry, vanilla_dimension_types};
    use steel_utils::BlockPos;
    use steel_utils::Identifier;
    use toml::{Value, map::Map};

    use steel_registry::vanilla_blocks;

    use super::{
        CityPalette, District, ENTRANCE_FRAME, GATE_SITE_OFFSETS, GROUND_Y, LOT_MIN, LOT_SIZE,
        WORLD_NAME, building_state, city_layers, district, gate_candidate, is_ground_light,
        register_world_generator,
    };

    #[test]
    fn the_entrance_frame_keeps_glass_away_from_the_doorway() {
        init_vanilla_registry();
        let palette = CityPalette::new();
        for x in ENTRANCE_FRAME.0..=ENTRANCE_FRAME.1 {
            for y in GROUND_Y + 1..=GROUND_Y + 5 {
                let state = building_state(District::Civic, x, LOT_MIN, y, 30, 0, &palette);
                assert_ne!(state, Some(palette.glass), "glass at x {x}, y {y}");
                let doorway = (x - 31).abs() <= 1 && y <= GROUND_Y + 3;
                assert_eq!(state.is_none(), doorway, "doorway at x {x}, y {y}");
            }
        }
    }

    #[test]
    fn ground_lights_spare_gate_plazas_and_the_spawn() {
        assert!(!is_ground_light(District::Plaza, 0, 0));
        assert!(is_ground_light(District::Plaza, 4, 12));
        assert!(!is_ground_light(District::GateSquare, 4, 12));
    }

    #[test]
    fn base_layers_reach_street_level_without_gaps() {
        init_vanilla_registry();
        let layers = city_layers();
        let min_y = vanilla_dimension_types::OVERWORLD.min_y;
        assert_eq!(min_y + layers.len() as i32 - 1, GROUND_Y);
        assert_eq!(layers[0], vanilla_blocks::BEDROCK.default_state());
        assert_eq!(
            layers.last().copied(),
            Some(vanilla_blocks::GRASS_BLOCK.default_state())
        );
        assert!(
            layers
                .iter()
                .all(|state| *state != vanilla_blocks::AIR.default_state())
        );
    }

    #[test]
    fn city_generator_registers_as_flat_overworld_and_spawns_at_street_level() {
        init_vanilla_registry();
        let mut registry =
            WorldGeneratorRegistry::new_with_builtins().expect("built-ins should initialize");
        register_world_generator(&mut registry).expect("city generator should register");

        let config = registry
            .validate_config(
                &Identifier::new("sololeveling", "city"),
                &Value::Table(Map::new()),
            )
            .expect("city config should validate");
        assert_eq!(config.dimension_type(), &vanilla_dimension_types::OVERWORLD);
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .expect("generation pool"),
        );
        let output = registry
            .create(None, &config, 1, pool)
            .expect("city generator should instantiate");
        assert!(output.is_flat);
        assert_eq!(output.generator.spawn_height(0, 384), 4);
        assert_eq!(WORLD_NAME, "sololeveling_city");
    }

    #[test]
    fn automatic_gate_sites_land_inside_the_generated_gate_squares() {
        for (x, z) in GATE_SITE_OFFSETS {
            assert_eq!(
                district(x.div_euclid(LOT_SIZE), z.div_euclid(LOT_SIZE)),
                District::GateSquare
            );
        }
        for _ in 0..100 {
            let candidate = gate_candidate(BlockPos::new(0, 0, 0));
            assert_eq!(
                district(
                    candidate.x().div_euclid(LOT_SIZE),
                    candidate.z().div_euclid(LOT_SIZE)
                ),
                District::GateSquare
            );
        }
    }
}
