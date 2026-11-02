//! The world generator of the Demon Castle.
//!
//! Each chunk asks which floor its columns belong to and writes that floor's
//! blocks straight from the plan; columns between halls stay void. Plans are
//! cheap but not free, so the last few are kept for neighbouring chunks.

use std::path::Path;
use std::sync::Arc;

use glam::IVec3;
use steel_core::chunk::Chunk;
use steel_core::worldgen::generator::{
    ChunkGenerator, EmptyChunkGenerator, GenerationChunk, TerrainPhase,
};
use steel_core::worldgen::{
    Beardifier, ChunkGeneratorType, CustomWorldGeneratorConfig, DynamicChunkGenerator,
    GeneratorOutput, WorldGenRegion, WorldGeneratorRegistry,
};
use steel_registry::biome::BiomeRef;
use steel_registry::vanilla_dimension_types::OVERWORLD;
use steel_utils::locks::SyncMutex;
use steel_utils::random::RandomSource;
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Identifier};

use super::districts::{MAX_LAYER, MIN_LAYER};
use super::plan::FloorPlan;
use super::theme::CastleTheme;
use super::{GROUND_Y, WORLD_NAME, antechamber_block, locate};

/// Plans kept for the chunks of the floors being generated.
const PLAN_CACHE: usize = 16;

/// Adds `sololeveling:demon_castle` to Steel's world generator registry.
pub fn register_world_generator(registry: &mut WorldGeneratorRegistry) -> Result<(), String> {
    registry.register_custom(
        generator_id(),
        |config| {
            if config.as_table().is_some_and(|table| !table.is_empty()) {
                return Err("demon castle generator does not accept custom config".to_owned());
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
                    CastleChunkGenerator::new(seed as u64),
                )),
                is_flat: true,
                sea_level: GROUND_Y,
            })
        },
    )
}

/// Returns the id the castle world's generator is registered under.
#[must_use]
pub fn generator_id() -> Identifier {
    Identifier::new(
        "sololeveling",
        WORLD_NAME.trim_start_matches("sololeveling_"),
    )
}

/// Builds castle floors from their plans.
struct CastleChunkGenerator {
    /// Supplies the region randomness; the castle places no features.
    empty: EmptyChunkGenerator,
    seed: u64,
    plans: SyncMutex<Vec<Arc<FloorPlan>>>,
}

impl CastleChunkGenerator {
    fn new(seed: u64) -> Self {
        Self {
            empty: EmptyChunkGenerator::new(),
            seed,
            plans: SyncMutex::new(Vec::with_capacity(PLAN_CACHE)),
        }
    }

    /// Returns the plan of `floor`, planning it on a cache miss.
    fn plan(&self, floor: u32) -> Arc<FloorPlan> {
        if let Some(plan) = self.plans.lock().iter().find(|plan| plan.floor == floor) {
            return Arc::clone(plan);
        }
        let plan = Arc::new(FloorPlan::generate(self.seed, floor));
        let mut plans = self.plans.lock();
        if plans.len() >= PLAN_CACHE {
            plans.remove(0);
        }
        plans.push(Arc::clone(&plan));
        plan
    }

    /// Writes the antechamber's share of a chunk between the halls.
    fn build_antechamber(chunk: GenerationChunk<'_, TerrainPhase>) {
        let pos = chunk.pos();
        let Ok(relative_y) = usize::try_from(GROUND_Y - OVERWORLD.min_y) else {
            return;
        };
        let blocks: Vec<(usize, usize, usize, BlockStateId)> = (0..16_usize)
            .flat_map(|x| (0..16_usize).map(move |z| (x, z)))
            .filter_map(|(x, z)| {
                let block = antechamber_block(pos.0.x * 16 + x as i32, pos.0.y * 16 + z as i32, 0)?;
                Some((x, relative_y, z, block))
            })
            .collect();
        chunk.write_block_batch(&blocks);
    }

    /// Returns the floor of a chunk; a chunk never spans two floors, since
    /// halls are far narrower than the stride between them.
    fn floor_of(pos: ChunkPos) -> Option<u32> {
        (0..16)
            .find_map(|dx| (0..16).find_map(|dz| locate(pos.0.x * 16 + dx, pos.0.y * 16 + dz)))
            .map(|(floor, _, _)| floor)
    }
}

impl ChunkGenerator for CastleChunkGenerator {
    fn min_y(&self) -> i32 {
        OVERWORLD.min_y
    }

    fn gen_depth(&self) -> i32 {
        OVERWORLD.height
    }

    fn noise_biome(&self, _quart_x: i32, _quart_y: i32, quart_z: i32) -> BiomeRef {
        locate(0, quart_z * 4)
            .map_or(CastleTheme::Fortress, |(floor, _, _)| {
                CastleTheme::of_floor(floor)
            })
            .biome()
    }

    fn initial_spawn_search_origin(&self) -> BlockPos {
        BlockPos::new(0, GROUND_Y + 1, 0)
    }

    fn spawn_height(&self, _min_y: i32, _height: i32) -> i32 {
        GROUND_Y + 1
    }

    fn create_structures(&self, _chunk: &Chunk) {}

    fn create_biomes(&self, chunk: &Chunk) {
        let theme =
            Self::floor_of(chunk.pos()).map_or(CastleTheme::Fortress, CastleTheme::of_floor);
        let biome = theme.biome_id();
        for section in &chunk.sections().sections {
            let mut section = section.write();
            for x in 0..4 {
                for y in 0..4 {
                    for z in 0..4 {
                        section.biomes.set(x, y, z, biome);
                    }
                }
            }
        }
        chunk.mark_dirty();
    }

    fn build_terrain(
        &self,
        chunk: GenerationChunk<'_, TerrainPhase>,
        _beardifier: Option<&Beardifier>,
        _neighbor_biomes: &dyn Fn(IVec3) -> u16,
    ) {
        let pos = chunk.pos();
        let Some(floor) = Self::floor_of(pos) else {
            Self::build_antechamber(chunk);
            return;
        };
        let plan = self.plan(floor);
        let columns: Vec<(usize, usize, i32, i32)> = (0..16_usize)
            .flat_map(|x| (0..16_usize).map(move |z| (x, z)))
            .filter_map(|(x, z)| {
                let wx = pos.0.x * 16 + x as i32;
                let wz = pos.0.y * 16 + z as i32;
                let (column_floor, lx, lz) = locate(wx, wz)?;
                (column_floor == floor).then_some((x, z, lx, lz))
            })
            .collect();
        if columns.is_empty() {
            return;
        }
        let min_y = OVERWORLD.min_y;
        let mut blocks: Vec<(usize, usize, usize, BlockStateId)> = Vec::new();
        for layer in MIN_LAYER..=MAX_LAYER + 2 {
            let Ok(relative_y) = usize::try_from(GROUND_Y + layer - min_y) else {
                continue;
            };
            for &(x, z, lx, lz) in &columns {
                if let Some(block) = plan.block(lx, lz, layer) {
                    blocks.push((x, relative_y, z, block));
                }
            }
        }
        chunk.write_block_batch(&blocks);
    }

    fn fill_from_noise(
        &self,
        _chunk: GenerationChunk<'_, TerrainPhase>,
        _beardifier: Option<&Beardifier>,
    ) {
    }

    fn build_surface(
        &self,
        _chunk: GenerationChunk<'_, TerrainPhase>,
        _neighbor_biomes: &dyn Fn(IVec3) -> u16,
    ) {
    }

    fn apply_carvers(&self, _chunk: GenerationChunk<'_, TerrainPhase>) {}

    fn create_worldgen_region_random(&self, world_seed: i64, center: ChunkPos) -> RandomSource {
        self.empty.create_worldgen_region_random(world_seed, center)
    }

    fn apply_biome_decorations(&self, _region: &mut WorldGenRegion<'_>) {}
}
