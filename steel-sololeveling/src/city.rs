//! Persistent, procedurally generated city world.

use std::path::Path;
use std::sync::Arc;

use glam::{DVec3, IVec3};
use rand::RngExt as _;
use steel_core::chunk::Chunk;
use steel_core::entity::entities::ItemFrameEntity;
use steel_core::entity::{SharedEntity, next_entity_id};
use steel_core::world::World;
use steel_core::worldgen::generator::{
    ChunkGenerator, FlatChunkGenerator, GenerationChunk, TerrainPhase,
};
use steel_core::worldgen::{
    Beardifier, ChunkGeneratorType, CustomWorldGeneratorConfig, DynamicChunkGenerator,
    GeneratorOutput, WorldGenRegion, WorldGeneratorRegistry,
};
use steel_registry::biome::BiomeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_dimension_types::OVERWORLD;
use steel_registry::{vanilla_blocks, vanilla_entities, vanilla_items};
use steel_utils::random::RandomSource;
use steel_utils::{BlockPos, ChunkPos, Direction, Identifier};

use crate::city_interior::{self, AXIS, CLERK_SPOT, COUNTER_SPOT, LADDER_COLUMN, Storey};
use crate::item::model;
/// World name used for the shared city and as the default-world entry.
pub const WORLD_NAME: &str = "sololeveling_city";
/// Lot of the Hunter Association: south of the plaza, door facing it.
const ASSOCIATION_LOT: (i32, i32) = (0, 1);
/// Lot of the city merchant, east of the Association.
const MARKET_LOT: (i32, i32) = (1, 1);
/// Lot of the guild hall, west of the Association.
const GUILD_LOT: (i32, i32) = (-1, 1);
/// Where the Organization agent stands: behind the Association counter.
pub const ORGANIZATION_AGENT_POS: DVec3 = lot_point(ASSOCIATION_LOT, COUNTER_SPOT);
/// Where the Association clerk hands out starter kits.
pub const ASSOCIATION_CLERK_POS: DVec3 = lot_point(ASSOCIATION_LOT, CLERK_SPOT);
/// Where the city merchant stands: behind the shop counter.
pub const MERCHANT_POS: DVec3 = lot_point(MARKET_LOT, COUNTER_SPOT);
/// Where the island dispatcher stands on the plaza, facing the key buildings.
pub const ISLAND_DISPATCHER_POS: DVec3 = DVec3::new(6.5, (GROUND_Y + 1) as f64, -6.5);
/// Yaw of a character facing north, towards its building's door.
pub const FACING_DOOR_YAW: f32 = 180.0;
/// Yaw of a character facing east.
pub const FACING_EAST_YAW: f32 = -90.0;
/// Gate plaza centers on the edge of the city, relative to the city spawn.
pub const GATE_SITE_OFFSETS: [(i32, i32); 4] = [
    (GATE_TILE * LOT_SIZE, 0),
    (-GATE_TILE * LOT_SIZE, 0),
    (0, GATE_TILE * LOT_SIZE),
    (0, -GATE_TILE * LOT_SIZE),
];

/// What a facade sign announces; the pack draws one board per kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignKind {
    /// Hunter Association: contracts and the starter kit.
    Association,
    /// City merchant: gear and crafting orders.
    Merchant,
    /// Guild hall.
    Guild,
}

impl SignKind {
    /// Returns the pack model of the board.
    #[must_use]
    pub const fn model(self) -> &'static str {
        match self {
            Self::Association => "sign/association",
            Self::Merchant => "sign/merchant",
            Self::Guild => "sign/guild",
        }
    }
}

/// Facade signs: the kind and the lot it hangs on, above the door.
pub const SIGNS: [(SignKind, (i32, i32)); 3] = [
    (SignKind::Association, ASSOCIATION_LOT),
    (SignKind::Merchant, MARKET_LOT),
    (SignKind::Guild, GUILD_LOT),
];

/// Returns the block a facade sign hangs in: in front of the north wall of
/// `lot`, centered above the door.
#[must_use]
pub const fn sign_block(lot: (i32, i32)) -> BlockPos {
    BlockPos::new(
        lot.0 * LOT_SIZE + AXIS - HALF_LOT,
        GROUND_Y + 7,
        lot.1 * LOT_SIZE + LOT_MIN - 1 - HALF_LOT,
    )
}

/// Builds the board item a facade sign frame holds.
#[must_use]
pub fn sign_item(kind: SignKind) -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::PAPER);
    model::apply(&mut stack, kind.model());
    stack
}

/// Returns the world position of the center of lot-local column `local`
/// in `lot`, at street level.
const fn lot_point(lot: (i32, i32), local: (i32, i32)) -> DVec3 {
    DVec3::new(
        (lot.0 * LOT_SIZE + local.0 - HALF_LOT) as f64 + 0.5,
        (GROUND_Y + 1) as f64,
        (lot.1 * LOT_SIZE + local.1 - HALF_LOT) as f64 + 0.5,
    )
}

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

/// Lots from the plaza to the edge of the city, in every direction.
const CITY_TILES: i32 = 6;
/// Lot of the gate plazas on the city edge, counted from the plaza.
const GATE_TILE: i32 = 6;
const LOT_SIZE: i32 = 32;
/// Lots are centered on the spawn: lot (0, 0) spans -16 to 15.
const HALF_LOT: i32 = LOT_SIZE / 2;
const GROUND_Y: i32 = 3;
/// Width of the road on the north and west edge of every lot.
const ROAD_WIDTH: i32 = 5;
const LOT_MIN: i32 = 7;
const LOT_MAX: i32 = 27;
/// Every building has three storeys of six blocks.
const BUILDING_HEIGHT: i32 = GROUND_Y + 18;
/// Facade columns around the entrance kept free of windows.
const ENTRANCE_FRAME: (i32, i32) = (AXIS - 4, AXIS + 4);
/// Top of the window-free band around the entrance, which carries the sign.
const ENTRANCE_TOP: i32 = GROUND_Y + 9;

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
        // Facade signs are item frames holding a pack board; worldgen places
        // each one once, with the chunk that contains it.
        let center = region.center();
        for (kind, lot) in SIGNS {
            let pos = sign_block(lot);
            if ChunkPos::from_block_pos(pos) != center {
                continue;
            }
            let frame = ItemFrameEntity::new_attached(
                &vanilla_entities::ITEM_FRAME,
                next_entity_id(),
                pos,
                Direction::North,
                region.weak_world(),
            );
            frame.set_item(sign_item(kind));
            let entity: SharedEntity = Arc::new(frame);
            let _ = region.add_fresh_entity(entity);
        }
    }
}

impl CityChunkGenerator {
    fn place_city_column(&self, chunk: GenerationChunk<'_, TerrainPhase>, x: i32, z: i32) {
        let tile_x = (x + HALF_LOT).div_euclid(LOT_SIZE);
        let tile_z = (z + HALF_LOT).div_euclid(LOT_SIZE);
        if tile_x.abs() > CITY_TILES || tile_z.abs() > CITY_TILES {
            return;
        }
        let local_x = (x + HALF_LOT).rem_euclid(LOT_SIZE);
        let local_z = (z + HALF_LOT).rem_euclid(LOT_SIZE);
        let district = district(tile_x, tile_z);
        let height = building_height(district);
        let in_building = height > GROUND_Y
            && (LOT_MIN..=LOT_MAX).contains(&local_x)
            && (LOT_MIN..=LOT_MAX).contains(&local_z);
        let ground = if district == District::Plaza {
            Some(self.plaza_state(x, z))
        } else if district == District::GateSquare {
            self.gate_square_state(local_x, local_z)
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

    const fn plaza_state(&self, x: i32, z: i32) -> steel_utils::BlockStateId {
        if x.abs() <= 1 && z.abs() <= 1 {
            return self.palette.lamp;
        }
        if x.abs() == z.abs() || x == 0 || z == 0 {
            return self.palette.accent;
        }
        self.palette.plaza
    }

    const fn gate_square_state(
        &self,
        local_x: i32,
        local_z: i32,
    ) -> Option<steel_utils::BlockStateId> {
        let reach = HALF_LOT - ROAD_WIDTH + 1;
        if (local_x - HALF_LOT).abs() <= reach && (local_z - HALF_LOT).abs() <= reach {
            if (local_x - HALF_LOT).abs() == reach || (local_z - HALF_LOT).abs() == reach {
                return Some(self.palette.accent);
            }
            return Some(self.palette.plaza);
        }
        if is_road(local_x, local_z) {
            return Some(self.palette.road);
        }
        None
    }
}

pub(crate) const fn district(tile_x: i32, tile_z: i32) -> District {
    match (tile_x, tile_z) {
        (0, 0) => District::Plaza,
        (x, z) if x == ASSOCIATION_LOT.0 && z == ASSOCIATION_LOT.1 => District::Association,
        (x, z) if x == MARKET_LOT.0 && z == MARKET_LOT.1 => District::Market,
        (x, z) if x == GUILD_LOT.0 && z == GUILD_LOT.1 => District::Guild,
        (x, z) if (x == 0 && z.abs() == GATE_TILE) || (z == 0 && x.abs() == GATE_TILE) => {
            District::GateSquare
        }
        (x, z) if x.abs() <= 2 && z.abs() <= 2 => District::Civic,
        _ => District::Residential,
    }
}

/// Returns the height of the buildings of `district`.
const fn building_height(district: District) -> i32 {
    match district {
        District::Plaza | District::GateSquare => GROUND_Y,
        // Homes are a storey lower than the public buildings.
        District::Residential => BUILDING_HEIGHT - 6,
        _ => BUILDING_HEIGHT,
    }
}

const fn is_road(local_x: i32, local_z: i32) -> bool {
    local_x < ROAD_WIDTH || local_z < ROAD_WIDTH
}

const fn is_sidewalk(local_x: i32, local_z: i32) -> bool {
    local_x < LOT_MIN || local_z < LOT_MIN || local_x > LOT_MAX || local_z > LOT_MAX
}

/// Returns whether a lamp post stands here: one on every crossing and one in
/// the middle of every block side.
const fn is_street_lamp(local_x: i32, local_z: i32) -> bool {
    let crossing = local_x == ROAD_WIDTH && local_z == ROAD_WIDTH;
    let along = local_x == ROAD_WIDTH && local_z == HALF_LOT + 8;
    let across = local_z == ROAD_WIDTH && local_x == HALF_LOT + 8;
    crossing || along || across
}

/// Returns whether a lamp post marks a corner of a gate plaza.
const fn is_gate_square_lamp(local_x: i32, local_z: i32) -> bool {
    matches!(local_x, 4 | 28) && matches!(local_z, 4 | 28)
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
        && y <= ENTRANCE_TOP
    {
        let doorway = (local_x - AXIS).abs() <= 1 && y <= GROUND_Y + 3;
        if doorway {
            return None;
        }
        if local_x == AXIS && y == GROUND_Y + 5 {
            return Some(palette.lamp);
        }
        let frame =
            (local_x - AXIS).abs() == 2 || (y == GROUND_Y + 4 && (local_x - AXIS).abs() <= 2);
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
        ASSOCIATION_CLERK_POS, ASSOCIATION_LOT, AXIS, CityPalette, District, ENTRANCE_FRAME,
        ENTRANCE_TOP, GATE_SITE_OFFSETS, GROUND_Y, HALF_LOT, LOT_MAX, LOT_MIN, LOT_SIZE,
        MARKET_LOT, MERCHANT_POS, ORGANIZATION_AGENT_POS, SIGNS, WORLD_NAME, building_state,
        city_layers, district, gate_candidate, is_ground_light, register_world_generator,
        sign_block,
    };

    #[test]
    fn the_entrance_frame_keeps_glass_away_from_the_doorway() {
        init_vanilla_registry();
        let palette = CityPalette::new();
        for x in ENTRANCE_FRAME.0..=ENTRANCE_FRAME.1 {
            for y in GROUND_Y + 1..=ENTRANCE_TOP {
                let state = building_state(District::Civic, x, LOT_MIN, y, 21, 0, &palette);
                assert_ne!(state, Some(palette.glass), "glass at x {x}, y {y}");
                let doorway = (x - AXIS).abs() <= 1 && y <= GROUND_Y + 3;
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
        let lot = |v: i32| (v + HALF_LOT).div_euclid(LOT_SIZE);
        for (x, z) in GATE_SITE_OFFSETS {
            assert_eq!(district(lot(x), lot(z)), District::GateSquare);
        }
        for _ in 0..100 {
            let candidate = gate_candidate(BlockPos::new(0, 0, 0));
            // The whole four-block frame stays on the plaza.
            for dx in -1..=2 {
                assert_eq!(
                    district(lot(candidate.x() + dx), lot(candidate.z())),
                    District::GateSquare
                );
            }
        }
    }

    #[test]
    fn key_buildings_face_the_plaza_and_have_their_characters_inside() {
        assert_eq!(district(0, 0), District::Plaza);
        for (district_kind, lot, point) in [
            (
                District::Association,
                ASSOCIATION_LOT,
                ORGANIZATION_AGENT_POS,
            ),
            (
                District::Association,
                ASSOCIATION_LOT,
                ASSOCIATION_CLERK_POS,
            ),
            (District::Market, MARKET_LOT, MERCHANT_POS),
        ] {
            assert_eq!(district(lot.0, lot.1), district_kind);
            let local_x = (point.x.floor() as i32 + HALF_LOT).rem_euclid(LOT_SIZE);
            let local_z = (point.z.floor() as i32 + HALF_LOT).rem_euclid(LOT_SIZE);
            assert!((LOT_MIN + 1..LOT_MAX).contains(&local_x));
            assert!((LOT_MIN + 1..LOT_MAX).contains(&local_z));
        }
        for (_, lot) in SIGNS {
            let sign = sign_block(lot);
            // The board hangs just outside the north wall, above the door.
            assert_eq!((sign.z() + HALF_LOT).rem_euclid(LOT_SIZE), LOT_MIN - 1);
            assert!(sign.y() > GROUND_Y + 5 && sign.y() <= ENTRANCE_TOP);
        }
    }
}
