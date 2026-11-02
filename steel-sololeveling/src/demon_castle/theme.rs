//! The look of a castle floor: one theme per ten floors, cycling forever.

use steel_registry::RegistryEntry as _;
use steel_registry::biome::BiomeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::{vanilla_biomes, vanilla_blocks};

/// Floors that share one theme; the tenth of them holds a giga boss.
pub const FLOORS_PER_THEME: u32 = 10;

/// The look of ten consecutive floors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastleTheme {
    /// Blackstone fortress.
    Fortress,
    /// Nether-brick inferno.
    Inferno,
    /// Ice and quartz citadel.
    FrozenCitadel,
    /// Deepslate city of the demons.
    DemonCity,
    /// Tuff and bone catacombs.
    Catacombs,
}

/// Every theme, in climb order.
const THEMES: [CastleTheme; 5] = [
    CastleTheme::Fortress,
    CastleTheme::Inferno,
    CastleTheme::FrozenCitadel,
    CastleTheme::DemonCity,
    CastleTheme::Catacombs,
];

/// Blocks one theme builds with.
#[derive(Debug, Clone, Copy)]
pub struct CastlePalette {
    /// Main walking surface.
    pub floor: BlockRef,
    /// Floor variant mixed into patterns.
    pub floor_alt: BlockRef,
    /// Borders, rings and lines drawn into the floor.
    pub floor_trim: BlockRef,
    /// Light source set flush into the floor.
    pub floor_light: BlockRef,
    /// Walls.
    pub wall: BlockRef,
    /// Wall variant mixed into the walls.
    pub wall_alt: BlockRef,
    /// Light source set flush into the walls.
    pub wall_light: BlockRef,
    /// Door frames, pedestals and rims.
    pub trim: BlockRef,
    /// Column shafts.
    pub pillar: BlockRef,
    /// Glowing column capitals and braziers.
    pub capital: BlockRef,
    /// Ceiling slab.
    pub ceiling: BlockRef,
    /// Light source set flush into the ceiling.
    pub ceiling_light: BlockRef,
    /// Half slab, always placed in its bottom state.
    pub slab: BlockRef,
    /// Stairs of the gallery flights.
    pub stairs: BlockRef,
    /// Railing posts along ledges and bridges; always a wall block, whose
    /// post is wide enough that a hunter never slips between two of them.
    pub rail: BlockRef,
    /// Carpet of the runners and the throne aisle.
    pub carpet: BlockRef,
    /// Lantern hanging from chains.
    pub lantern: BlockRef,
    /// Statues.
    pub statue: BlockRef,
    /// Soil of the courtyard planters.
    pub soil: BlockRef,
    /// Plant growing in the planters, if the soil carries one.
    pub plant: Option<BlockRef>,
    /// Hidden foundation and chasm rock.
    pub foundation: BlockRef,
}

impl CastleTheme {
    /// Returns the theme of `floor`.
    #[must_use]
    pub const fn of_floor(floor: u32) -> Self {
        let band = floor.saturating_sub(1) / FLOORS_PER_THEME;
        THEMES[(band % THEMES.len() as u32) as usize]
    }

    /// Returns how many full theme cycles lie below `floor`; later cycles mix
    /// their variant blocks in more often.
    #[must_use]
    pub const fn cycle(floor: u32) -> u32 {
        floor.saturating_sub(1) / (FLOORS_PER_THEME * THEMES.len() as u32)
    }

    /// Returns the Russian name shown on arrival.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Fortress => "Чёрная крепость",
            Self::Inferno => "Пылающая преисподняя",
            Self::FrozenCitadel => "Ледяная цитадель",
            Self::DemonCity => "Город демонов",
            Self::Catacombs => "Костяные катакомбы",
        }
    }

    /// Returns the biome whose fog, particles and music fill the floor. Only
    /// biomes without rain or snow, since the castle roof is open to the sky.
    #[must_use]
    pub fn biome(self) -> BiomeRef {
        match self {
            Self::Fortress => &vanilla_biomes::BASALT_DELTAS,
            Self::Inferno => &vanilla_biomes::CRIMSON_FOREST,
            Self::FrozenCitadel => &vanilla_biomes::WARPED_FOREST,
            Self::DemonCity => &vanilla_biomes::SOUL_SAND_VALLEY,
            Self::Catacombs => &vanilla_biomes::NETHER_WASTES,
        }
    }

    /// Returns the registry id of [`Self::biome`].
    #[must_use]
    pub fn biome_id(self) -> u16 {
        self.biome()
            .try_id()
            .and_then(|id| u16::try_from(id).ok())
            .unwrap_or(0)
    }

    /// Returns the blocks of this theme.
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "every theme's palette reads best as one table"
    )]
    pub fn palette(self) -> CastlePalette {
        match self {
            Self::Fortress => CastlePalette {
                floor: &vanilla_blocks::POLISHED_BLACKSTONE,
                floor_alt: &vanilla_blocks::BLACKSTONE,
                floor_trim: &vanilla_blocks::POLISHED_BLACKSTONE_BRICKS,
                floor_light: &vanilla_blocks::SHROOMLIGHT,
                wall: &vanilla_blocks::POLISHED_BLACKSTONE_BRICKS,
                wall_alt: &vanilla_blocks::CRACKED_POLISHED_BLACKSTONE_BRICKS,
                wall_light: &vanilla_blocks::SHROOMLIGHT,
                trim: &vanilla_blocks::CHISELED_POLISHED_BLACKSTONE,
                pillar: &vanilla_blocks::POLISHED_BASALT,
                capital: &vanilla_blocks::GLOWSTONE,
                ceiling: &vanilla_blocks::POLISHED_BLACKSTONE_BRICKS,
                ceiling_light: &vanilla_blocks::SHROOMLIGHT,
                slab: &vanilla_blocks::POLISHED_BLACKSTONE_BRICK_SLAB,
                stairs: &vanilla_blocks::POLISHED_BLACKSTONE_BRICK_STAIRS,
                rail: &vanilla_blocks::POLISHED_BLACKSTONE_BRICK_WALL,
                carpet: &vanilla_blocks::RED_CARPET,
                lantern: &vanilla_blocks::SOUL_LANTERN,
                statue: &vanilla_blocks::GILDED_BLACKSTONE,
                soil: &vanilla_blocks::CRIMSON_NYLIUM,
                plant: Some(&vanilla_blocks::CRIMSON_FUNGUS),
                foundation: &vanilla_blocks::BLACKSTONE,
            },
            Self::Inferno => CastlePalette {
                floor: &vanilla_blocks::NETHER_BRICKS,
                floor_alt: &vanilla_blocks::RED_NETHER_BRICKS,
                floor_trim: &vanilla_blocks::CHISELED_NETHER_BRICKS,
                floor_light: &vanilla_blocks::SHROOMLIGHT,
                wall: &vanilla_blocks::NETHER_BRICKS,
                wall_alt: &vanilla_blocks::CRACKED_NETHER_BRICKS,
                wall_light: &vanilla_blocks::SHROOMLIGHT,
                trim: &vanilla_blocks::CHISELED_NETHER_BRICKS,
                pillar: &vanilla_blocks::RED_NETHER_BRICKS,
                capital: &vanilla_blocks::SHROOMLIGHT,
                ceiling: &vanilla_blocks::NETHER_BRICKS,
                ceiling_light: &vanilla_blocks::GLOWSTONE,
                slab: &vanilla_blocks::NETHER_BRICK_SLAB,
                stairs: &vanilla_blocks::NETHER_BRICK_STAIRS,
                rail: &vanilla_blocks::NETHER_BRICK_WALL,
                carpet: &vanilla_blocks::BLACK_CARPET,
                lantern: &vanilla_blocks::LANTERN,
                statue: &vanilla_blocks::CRYING_OBSIDIAN,
                soil: &vanilla_blocks::CRIMSON_NYLIUM,
                plant: Some(&vanilla_blocks::CRIMSON_ROOTS),
                foundation: &vanilla_blocks::BLACKSTONE,
            },
            Self::FrozenCitadel => CastlePalette {
                floor: &vanilla_blocks::POLISHED_DIORITE,
                floor_alt: &vanilla_blocks::CALCITE,
                floor_trim: &vanilla_blocks::PACKED_ICE,
                floor_light: &vanilla_blocks::SEA_LANTERN,
                wall: &vanilla_blocks::PACKED_ICE,
                wall_alt: &vanilla_blocks::BLUE_ICE,
                wall_light: &vanilla_blocks::SEA_LANTERN,
                trim: &vanilla_blocks::CHISELED_QUARTZ_BLOCK,
                pillar: &vanilla_blocks::QUARTZ_PILLAR,
                capital: &vanilla_blocks::SEA_LANTERN,
                ceiling: &vanilla_blocks::SNOW_BLOCK,
                ceiling_light: &vanilla_blocks::SEA_LANTERN,
                slab: &vanilla_blocks::QUARTZ_SLAB,
                stairs: &vanilla_blocks::QUARTZ_STAIRS,
                rail: &vanilla_blocks::DIORITE_WALL,
                carpet: &vanilla_blocks::LIGHT_BLUE_CARPET,
                lantern: &vanilla_blocks::SOUL_LANTERN,
                statue: &vanilla_blocks::BLUE_ICE,
                soil: &vanilla_blocks::SNOW_BLOCK,
                plant: None,
                foundation: &vanilla_blocks::PACKED_ICE,
            },
            Self::DemonCity => CastlePalette {
                floor: &vanilla_blocks::DEEPSLATE_TILES,
                floor_alt: &vanilla_blocks::POLISHED_DEEPSLATE,
                floor_trim: &vanilla_blocks::CHISELED_DEEPSLATE,
                floor_light: &vanilla_blocks::OCHRE_FROGLIGHT,
                wall: &vanilla_blocks::DEEPSLATE_BRICKS,
                wall_alt: &vanilla_blocks::CRACKED_DEEPSLATE_BRICKS,
                wall_light: &vanilla_blocks::OCHRE_FROGLIGHT,
                trim: &vanilla_blocks::CHISELED_DEEPSLATE,
                pillar: &vanilla_blocks::POLISHED_DEEPSLATE,
                capital: &vanilla_blocks::OCHRE_FROGLIGHT,
                ceiling: &vanilla_blocks::DEEPSLATE_TILES,
                ceiling_light: &vanilla_blocks::OCHRE_FROGLIGHT,
                slab: &vanilla_blocks::DEEPSLATE_TILE_SLAB,
                stairs: &vanilla_blocks::DEEPSLATE_TILE_STAIRS,
                rail: &vanilla_blocks::DEEPSLATE_TILE_WALL,
                carpet: &vanilla_blocks::PURPLE_CARPET,
                lantern: &vanilla_blocks::SOUL_LANTERN,
                statue: &vanilla_blocks::CRYING_OBSIDIAN,
                soil: &vanilla_blocks::SOUL_SOIL,
                plant: None,
                foundation: &vanilla_blocks::COBBLED_DEEPSLATE,
            },
            Self::Catacombs => CastlePalette {
                floor: &vanilla_blocks::TUFF_BRICKS,
                floor_alt: &vanilla_blocks::POLISHED_TUFF,
                floor_trim: &vanilla_blocks::CHISELED_TUFF,
                floor_light: &vanilla_blocks::VERDANT_FROGLIGHT,
                wall: &vanilla_blocks::TUFF_BRICKS,
                wall_alt: &vanilla_blocks::CHISELED_TUFF_BRICKS,
                wall_light: &vanilla_blocks::VERDANT_FROGLIGHT,
                trim: &vanilla_blocks::BONE_BLOCK,
                pillar: &vanilla_blocks::BONE_BLOCK,
                capital: &vanilla_blocks::VERDANT_FROGLIGHT,
                ceiling: &vanilla_blocks::TUFF,
                ceiling_light: &vanilla_blocks::VERDANT_FROGLIGHT,
                slab: &vanilla_blocks::TUFF_BRICK_SLAB,
                stairs: &vanilla_blocks::TUFF_BRICK_STAIRS,
                rail: &vanilla_blocks::TUFF_BRICK_WALL,
                carpet: &vanilla_blocks::GRAY_CARPET,
                lantern: &vanilla_blocks::SOUL_LANTERN,
                statue: &vanilla_blocks::CHISELED_TUFF_BRICKS,
                soil: &vanilla_blocks::COARSE_DIRT,
                plant: Some(&vanilla_blocks::DEAD_BUSH),
                foundation: &vanilla_blocks::TUFF,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CastleTheme;

    #[test]
    fn themes_change_every_ten_floors_and_cycle_forever() {
        assert_eq!(CastleTheme::of_floor(1), CastleTheme::Fortress);
        // The giga floor closes its band instead of opening the next one.
        assert_eq!(CastleTheme::of_floor(10), CastleTheme::Fortress);
        assert_eq!(CastleTheme::of_floor(11), CastleTheme::Inferno);
        assert_eq!(CastleTheme::of_floor(51), CastleTheme::Fortress);
        assert_eq!(CastleTheme::cycle(50), 0);
        assert_eq!(CastleTheme::cycle(51), 1);
        // Far floors still resolve.
        let _ = CastleTheme::of_floor(50_000).palette();
    }
}
