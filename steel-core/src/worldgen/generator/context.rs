//! This module contains the `WorldGenContext` struct, which is used to provide context for chunk generation.

use std::sync::{Arc, Weak};

use enum_dispatch::enum_dispatch;
use glam::IVec3;
use steel_registry::biome::BiomeRef;
use steel_utils::random::RandomSource;
use steel_utils::{BlockPos, ChunkPos};
use steel_worldgen::density_functions::{
    end::EndNoises, nether::NetherNoises, overworld::OverworldNoises,
};
use steel_worldgen::noise::Beardifier;

use crate::chunk::Chunk;
use crate::world::World;
use crate::worldgen::generator::{
    ChunkGenerator, EmptyChunkGenerator, FlatChunkGenerator, GenerationChunk, TerrainPhase,
    VanillaGenerator,
};
use crate::worldgen::region::WorldGenRegion;
use crate::worldgen::structure::StructureGenerator;

/// Type alias for overworld generator.
pub type OverworldGenerator = VanillaGenerator<OverworldNoises>;

/// Type alias for nether generator.
pub type NetherGenerator = VanillaGenerator<NetherNoises>;

/// Type alias for end generator.
pub type EndGenerator = VanillaGenerator<EndNoises>;

/// Owned trait-object adapter used by custom world generators.
pub struct DynamicChunkGenerator(Box<dyn ChunkGenerator>);

impl DynamicChunkGenerator {
    /// Wraps a custom generator for use in [`ChunkGeneratorType`].
    #[must_use]
    pub fn new(generator: impl ChunkGenerator + 'static) -> Self {
        Self(Box::new(generator))
    }
}

impl ChunkGenerator for DynamicChunkGenerator {
    fn min_y(&self) -> i32 {
        self.0.min_y()
    }

    fn gen_depth(&self) -> i32 {
        self.0.gen_depth()
    }

    fn noise_biome(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> BiomeRef {
        self.0.noise_biome(quart_x, quart_y, quart_z)
    }

    fn initial_spawn_search_origin(&self) -> BlockPos {
        self.0.initial_spawn_search_origin()
    }

    fn spawn_height(&self, min_y: i32, height: i32) -> i32 {
        self.0.spawn_height(min_y, height)
    }

    fn structure_generator(&self) -> Option<&StructureGenerator> {
        self.0.structure_generator()
    }

    fn create_structures(&self, chunk: &Chunk) {
        self.0.create_structures(chunk);
    }

    fn create_biomes(&self, chunk: &Chunk) {
        self.0.create_biomes(chunk);
    }

    fn build_terrain(
        &self,
        chunk: GenerationChunk<'_, TerrainPhase>,
        beardifier: Option<&Beardifier>,
        neighbor_biomes: &dyn Fn(IVec3) -> u16,
    ) {
        self.0.build_terrain(chunk, beardifier, neighbor_biomes);
    }

    fn fill_from_noise(
        &self,
        chunk: GenerationChunk<'_, TerrainPhase>,
        beardifier: Option<&Beardifier>,
    ) {
        self.0.fill_from_noise(chunk, beardifier);
    }

    fn build_surface(
        &self,
        chunk: GenerationChunk<'_, TerrainPhase>,
        neighbor_biomes: &dyn Fn(IVec3) -> u16,
    ) {
        self.0.build_surface(chunk, neighbor_biomes);
    }

    fn apply_carvers(&self, chunk: GenerationChunk<'_, TerrainPhase>) {
        self.0.apply_carvers(chunk);
    }

    fn create_worldgen_region_random(&self, world_seed: i64, center: ChunkPos) -> RandomSource {
        self.0.create_worldgen_region_random(world_seed, center)
    }

    fn apply_biome_decorations(&self, region: &mut WorldGenRegion<'_>) {
        self.0.apply_biome_decorations(region);
    }
}

#[expect(
    missing_docs,
    reason = "variants are named after their dimension; self-explanatory"
)]
#[enum_dispatch(ChunkGenerator)]
pub enum ChunkGeneratorType {
    Flat(FlatChunkGenerator),
    Empty(EmptyChunkGenerator),
    Overworld(OverworldGenerator),
    Nether(NetherGenerator),
    End(EndGenerator),
    Custom(DynamicChunkGenerator),
}

/// Context for world generation.
pub struct WorldGenContext {
    /// The chunk generator to use.
    pub generator: Arc<ChunkGeneratorType>,
    /// Weak reference to the world (to avoid circular Arc reference).
    /// Use `world()` to get a strong reference when needed.
    world: Weak<World>,
    /// Cached dimension minimum build Y. Immutable for the world's lifetime;
    /// cached here so per-block queries avoid a `Weak<World>::upgrade` (a
    /// cross-thread atomic refcount round-trip) on every call.
    min_y: i32,
    /// Cached dimension build height. See [`Self::min_y`].
    height: i32,
    /// Cached dimension sea level. Immutable for the world's lifetime. Read
    /// per-column by the `freeze_top_layer` feature (snow/ice placement);
    /// cached here so it avoids a `Weak<World>::upgrade` (cross-thread atomic
    /// on the shared `Arc<World>`) on every column. See [`Self::min_y`].
    sea_level: i32,
}

impl WorldGenContext {
    /// Creates a new `WorldGenContext`.
    ///
    /// `min_y`/`height` are the dimension's build bounds (`DimensionType::min_y`
    /// / `::height`); they are cached rather than read from the world per call.
    #[must_use]
    pub const fn new(
        generator: Arc<ChunkGeneratorType>,
        world: Weak<World>,
        min_y: i32,
        height: i32,
        sea_level: i32,
    ) -> Self {
        Self {
            generator,
            world,
            min_y,
            height,
            sea_level,
        }
    }

    /// Returns the dimension's sea level (cached; see [`Self::min_y`]).
    #[must_use]
    pub const fn sea_level(&self) -> i32 {
        self.sea_level
    }

    /// Gets a strong reference to the world.
    ///
    /// # Panics
    /// Panics if the world has been dropped.
    #[must_use]
    pub fn world(&self) -> Arc<World> {
        self.world.upgrade().expect("World has been dropped")
    }

    /// Gets a weak reference to the world.
    ///
    /// This is useful for passing to chunks without creating a strong reference cycle.
    #[must_use]
    pub fn weak_world(&self) -> Weak<World> {
        Weak::clone(&self.world)
    }

    /// Returns the minimum Y coordinate of the world.
    #[must_use]
    pub const fn min_y(&self) -> i32 {
        self.min_y
    }

    /// Returns the total height of the world in blocks.
    #[must_use]
    pub const fn height(&self) -> i32 {
        self.height
    }

    /// Returns the minimum Y coordinate used by `WorldGenerationContext`.
    #[must_use]
    pub fn generation_min_y(&self) -> i32 {
        self.min_y.max(self.generator.min_y())
    }

    /// Returns the height used by `WorldGenerationContext`.
    #[must_use]
    pub fn generation_height(&self) -> i32 {
        self.height.min(self.generator.gen_depth())
    }

    #[must_use]
    /// How many sections this dimension has
    pub const fn section_count(&self) -> usize {
        (self.height / 16) as usize
    }
}
