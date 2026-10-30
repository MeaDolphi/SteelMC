//! Artifact catalog, rarity rules, and the item stacks that carry them.
//!
//! Every artifact is a vanilla item plus attribute modifiers and a Solo Leveling
//! tag. Names come from the Solo Leveling wiki; the numbers and abilities are a
//! server adaptation for Minecraft balance, not a claim about canonical values.
//!
//! Rarity decides two things: abilities start at Epic, and a transferable Epic
//! artifact needs hunter rank C while a transferable Legendary one needs rank A.
//! Personal (bound) artifacts have no rank restriction, on both sides of the
//! market.
//!
//! Gloves deliberately use the chestplate slot: the vanilla client has no glove
//! slot, so an artifact glove and a chestplate cannot be worn together. This is
//! a documented adaptation, not a hidden extra equipment slot.

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use steel_registry::attribute::AttributeRef;
use steel_registry::data_components::components::DyedItemColor;
use steel_registry::data_components::components::{
    ItemAttributeModifierDisplay, ItemAttributeModifierEntry, ItemAttributeModifiers,
};
use steel_registry::data_components::vanilla_components::{
    ATTRIBUTE_MODIFIERS, DYED_COLOR, UNBREAKABLE,
};
use steel_registry::equipment::EquipmentSlotGroup;
use steel_registry::item_stack::ItemStack;

use steel_core::entity::attribute::AttributeModifierOperation;
use steel_registry::items::Item;
use steel_registry::{vanilla_attributes, vanilla_items};
use steel_utils::Identifier;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::binding;
use super::enchantment::{self, Effect, Enchantment, ItemKind};
use super::{named, read_kind, record, tag_string, with_lore, write_tag};
use crate::class::HunterClass;
use crate::gear::GearKind;
use crate::identifier::from_sololeveling;
use crate::rank::HunterRank;
use crate::spell::SpellGear;

/// Kind stored in the record of an artifact item.
const KIND: &str = "artifact";
/// Tag field holding the artifact id.
const ID: &str = "id";
/// Tag field holding the comma-separated enchantment list.
const ENCHANTMENTS: &str = "enchantments";

/// Artifact rarity, from the cheapest transferable to the strongest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rarity {
    /// Unused by the catalog; kept so transferable and personal rules stay
    /// readable as one ordered scale.
    Common,
    /// Distinct stats, no ability.
    Rare,
    /// Abilities start here.
    Epic,
    /// Strongest abilities and the strictest transferable rank gate.
    Legendary,
}

impl Rarity {
    /// Returns the Russian display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Common => "Обычный",
            Self::Rare => "Редкий",
            Self::Epic => "Эпический",
            Self::Legendary => "Легендарный",
        }
    }

    /// Returns the tooltip color of the rarity.
    #[must_use]
    pub const fn color(self) -> Color {
        match self {
            Self::Common => Color::Gray,
            Self::Rare => Color::Aqua,
            Self::Epic => Color::LightPurple,
            Self::Legendary => Color::Gold,
        }
    }

    /// Returns the stored name of the rarity.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Rare => "rare",
            Self::Epic => "epic",
            Self::Legendary => "legendary",
        }
    }

    /// Parses a stored rarity name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Common, Self::Rare, Self::Epic, Self::Legendary]
            .into_iter()
            .find(|rarity| rarity.name() == name)
    }

    /// Returns the lowest hunter rank that may use a transferable artifact of
    /// this rarity, or `None` when every rank may.
    #[must_use]
    pub const fn transferable_minimum_rank(self) -> Option<HunterRank> {
        match self {
            Self::Common | Self::Rare => None,
            Self::Epic => Some(HunterRank::C),
            Self::Legendary => Some(HunterRank::A),
        }
    }
}

/// One catalog artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ArtifactId {
    /// Rare dagger: flat attack damage.
    KasakasVenomFang,
    /// Epic sword: bonus damage against armored targets.
    KnightsKiller,
    /// Epic dagger: short speed burst after a hit.
    BarukasDagger,
    /// Legendary daggers: damage scaling with Strength.
    DemonKingsDaggers,
    /// Legendary sword: a small storm around the hit target.
    DemonKingsLongsword,
    /// Rare glove: armor in the chestplate slot.
    ArchersGloves,
    /// Rare boots: armor and movement speed.
    AssassinsShoes,
    /// Epic chestplate: damage reduction at low health.
    HighKnightsChestplate,
    /// Epic glove: a shield on every fifth hit.
    HighKnightsGauntlet,
    /// Legendary helmet: survival of a lethal hit.
    RedKnightsHelmet,
    /// Epic pickaxe: extra crystals from dungeon ore.
    CrystalPickaxe,
    /// Rare dagger from the D-rank Steel-Fanged Raikan.
    RaikansFang,
    /// Rare ring: extra maximum health.
    RingOfEndurance,
    /// Rare amulet: movement speed.
    WindAmulet,
    /// Rare helmet: armor and toughness.
    DungeonWardensHelmet,
    /// Rare leggings: armor and speed.
    TrackersLeggings,
    /// Epic amulet: speed and a burst after a hit.
    GatekeepersNecklace,
    /// Epic ring: stronger magic.
    HighMagiciansRing,
    /// Epic staff of the high orc shaman: stronger magic.
    KargalgansStaff,
    /// Epic leggings: damage reduction at low health.
    HighKnightsGreaves,
    /// Legendary daggers from the dragon Kamish: strength-scaling damage.
    KamishsWrath,
    /// Legendary orb: much stronger magic.
    OrbOfAvarice,
    /// Legendary chestplate: health and damage reduction.
    BlackHeart,
    /// Legendary ring: health and strength-scaling damage.
    DemonMonarchsRing,
    /// Rare Mage staff: magic damage and cheaper destruction.
    AshStaff,
    /// Rare Mage ring: maximum mana.
    ManaRing,
    /// Rare Mage earrings: mana regeneration.
    ClearMindEarrings,
    /// Epic Mage robe: armor, mana and magic damage.
    ArchmageMantle,
    /// Epic Mage amulet: stronger healing and Barrier.
    HighPriestsBreviary,
    /// Epic Mage amulet: every fifth spell is free.
    ElementalOrb,
    /// Legendary Mage staff: destruction spells may strike twice.
    DemonKingsStaff,
    /// Legendary Mage crown: deep mana and free Meditation.
    CrownOfBottomlessMana,
    /// Common Mage staff sold by the city merchant.
    ApprenticeStaff,
    /// Rare ring: attack speed.
    AgileRing,
    /// Rare ring: attack damage.
    StrengthRing,
    /// Rare amulet: armor and knockback resistance.
    WardenAmulet,
    /// Rare amulet: health and speed.
    HuntersAmulet,
    /// Epic ring: power while badly hurt.
    BerserkerRing,
    /// Epic ring: damage against gate bosses.
    BossHunterRing,
    /// Epic amulet: melee lifesteal and health.
    BloodAmulet,
    /// Epic amulet: crystals and system points.
    ProspectorsNecklace,
    /// Legendary ring of the island's Ant King: double strikes.
    AntKingRing,
    /// Legendary amulet: cheats death with invulnerability.
    ShadowGuardAmulet,
    /// Common starter sword: steady all-round damage.
    HunterSword,
    /// Common starter dagger: fast, light hits.
    HunterDagger,
    /// Common starter axe: slow, heavy hits.
    HunterAxe,
    /// Common starter shield.
    HunterShield,
    /// Common starter helmet.
    HunterHelmet,
    /// Common starter chestplate.
    HunterChestplate,
    /// Common starter leggings.
    HunterLeggings,
    /// Common starter boots.
    HunterBoots,
    /// Common starter pickaxe for dungeon crystals.
    HunterPickaxe,
    /// Light specialization helmet.
    LightHelmet,
    /// Light specialization chestplate.
    LightChestplate,
    /// Light specialization leggings.
    LightLeggings,
    /// Light specialization boots.
    LightBoots,
    /// Reinforced specialization helmet.
    ReinforcedHelmet,
    /// Reinforced specialization chestplate.
    ReinforcedChestplate,
    /// Reinforced specialization leggings.
    ReinforcedLeggings,
    /// Reinforced specialization boots.
    ReinforcedBoots,
    /// Initial heavy specialization helmet.
    HeavyHelmet,
    /// Initial heavy specialization chestplate.
    HeavyChestplate,
    /// Initial heavy specialization leggings.
    HeavyLeggings,
    /// Initial heavy specialization boots.
    HeavyBoots,
    /// Mage clothing specialization helmet.
    RobeHelmet,
    /// Mage clothing specialization chestplate.
    RobeChestplate,
    /// Mage clothing specialization leggings.
    RobeLeggings,
    /// Mage clothing specialization boots.
    RobeBoots,
    /// Improved light specialization helmet.
    ImprovedLightHelmet,
    /// Improved light specialization chestplate.
    ImprovedLightChestplate,
    /// Improved light specialization leggings.
    ImprovedLightLeggings,
    /// Improved light specialization boots.
    ImprovedLightBoots,
    /// Improved reinforced specialization helmet.
    ImprovedReinforcedHelmet,
    /// Improved reinforced specialization chestplate.
    ImprovedReinforcedChestplate,
    /// Improved reinforced specialization leggings.
    ImprovedReinforcedLeggings,
    /// Improved reinforced specialization boots.
    ImprovedReinforcedBoots,
    /// Improved heavy specialization helmet.
    ImprovedHeavyHelmet,
    /// Improved heavy specialization chestplate.
    ImprovedHeavyChestplate,
    /// Improved heavy specialization leggings.
    ImprovedHeavyLeggings,
    /// Improved heavy specialization boots.
    ImprovedHeavyBoots,
    /// Improved robe specialization helmet.
    ImprovedRobeHelmet,
    /// Improved robe specialization chestplate.
    ImprovedRobeChestplate,
    /// Improved robe specialization leggings.
    ImprovedRobeLeggings,
    /// Improved robe specialization boots.
    ImprovedRobeBoots,
    /// Improved starter-line weapon.
    ImprovedHunterSword,
    /// Improved starter-line weapon.
    ImprovedHunterDagger,
    /// Improved starter-line weapon.
    ImprovedHunterAxe,
}

impl ArtifactId {
    /// Returns the stable tag value of the artifact.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::KasakasVenomFang => "kasakas_venom_fang",
            Self::KnightsKiller => "knights_killer",
            Self::BarukasDagger => "barukas_dagger",
            Self::DemonKingsDaggers => "demon_kings_daggers",
            Self::DemonKingsLongsword => "demon_kings_longsword",
            Self::ArchersGloves => "archers_gloves",
            Self::AssassinsShoes => "assassins_shoes",
            Self::HighKnightsChestplate => "high_knights_chestplate",
            Self::HighKnightsGauntlet => "high_knights_gauntlet",
            Self::RedKnightsHelmet => "red_knights_helmet",
            Self::CrystalPickaxe => "crystal_pickaxe",
            Self::RaikansFang => "raikans_fang",
            Self::RingOfEndurance => "ring_of_endurance",
            Self::WindAmulet => "wind_amulet",
            Self::DungeonWardensHelmet => "dungeon_wardens_helmet",
            Self::TrackersLeggings => "trackers_leggings",
            Self::GatekeepersNecklace => "gatekeepers_necklace",
            Self::HighMagiciansRing => "high_magicians_ring",
            Self::KargalgansStaff => "kargalgans_staff",
            Self::HighKnightsGreaves => "high_knights_greaves",
            Self::KamishsWrath => "kamishs_wrath",
            Self::OrbOfAvarice => "orb_of_avarice",
            Self::BlackHeart => "black_heart",
            Self::DemonMonarchsRing => "demon_monarchs_ring",
            Self::AshStaff => "ash_staff",
            Self::ManaRing => "mana_ring",
            Self::ClearMindEarrings => "clear_mind_earrings",
            Self::ArchmageMantle => "archmage_mantle",
            Self::HighPriestsBreviary => "high_priests_breviary",
            Self::ElementalOrb => "elemental_orb",
            Self::DemonKingsStaff => "demon_kings_staff",
            Self::CrownOfBottomlessMana => "crown_of_bottomless_mana",
            Self::ApprenticeStaff => "apprentice_staff",
            Self::AgileRing => "agile_ring",
            Self::StrengthRing => "strength_ring",
            Self::WardenAmulet => "warden_amulet",
            Self::HuntersAmulet => "hunters_amulet",
            Self::BerserkerRing => "berserker_ring",
            Self::BossHunterRing => "boss_hunter_ring",
            Self::BloodAmulet => "blood_amulet",
            Self::ProspectorsNecklace => "prospectors_necklace",
            Self::AntKingRing => "ant_king_ring",
            Self::ShadowGuardAmulet => "shadow_guard_amulet",
            Self::HunterSword => "hunter_sword",
            Self::HunterDagger => "hunter_dagger",
            Self::HunterAxe => "hunter_axe",
            Self::HunterShield => "hunter_shield",
            Self::HunterHelmet => "hunter_helmet",
            Self::HunterChestplate => "hunter_chestplate",
            Self::HunterLeggings => "hunter_leggings",
            Self::HunterBoots => "hunter_boots",
            Self::HunterPickaxe => "hunter_pickaxe",
            Self::LightHelmet => "light_helmet",
            Self::LightChestplate => "light_chestplate",
            Self::LightLeggings => "light_leggings",
            Self::LightBoots => "light_boots",
            Self::ReinforcedHelmet => "reinforced_helmet",
            Self::ReinforcedChestplate => "reinforced_chestplate",
            Self::ReinforcedLeggings => "reinforced_leggings",
            Self::ReinforcedBoots => "reinforced_boots",
            Self::HeavyHelmet => "heavy_helmet",
            Self::HeavyChestplate => "heavy_chestplate",
            Self::HeavyLeggings => "heavy_leggings",
            Self::HeavyBoots => "heavy_boots",
            Self::RobeHelmet => "robe_helmet",
            Self::RobeChestplate => "robe_chestplate",
            Self::RobeLeggings => "robe_leggings",
            Self::RobeBoots => "robe_boots",
            Self::ImprovedLightHelmet => "improved_light_helmet",
            Self::ImprovedLightChestplate => "improved_light_chestplate",
            Self::ImprovedLightLeggings => "improved_light_leggings",
            Self::ImprovedLightBoots => "improved_light_boots",
            Self::ImprovedReinforcedHelmet => "improved_reinforced_helmet",
            Self::ImprovedReinforcedChestplate => "improved_reinforced_chestplate",
            Self::ImprovedReinforcedLeggings => "improved_reinforced_leggings",
            Self::ImprovedReinforcedBoots => "improved_reinforced_boots",
            Self::ImprovedHeavyHelmet => "improved_heavy_helmet",
            Self::ImprovedHeavyChestplate => "improved_heavy_chestplate",
            Self::ImprovedHeavyLeggings => "improved_heavy_leggings",
            Self::ImprovedHeavyBoots => "improved_heavy_boots",
            Self::ImprovedRobeHelmet => "improved_robe_helmet",
            Self::ImprovedRobeChestplate => "improved_robe_chestplate",
            Self::ImprovedRobeLeggings => "improved_robe_leggings",
            Self::ImprovedRobeBoots => "improved_robe_boots",
            Self::ImprovedHunterSword => "improved_hunter_sword",
            Self::ImprovedHunterDagger => "improved_hunter_dagger",
            Self::ImprovedHunterAxe => "improved_hunter_axe",
        }
    }

    /// Parses the stable tag value.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        ALL.into_iter()
            .chain(STARTER)
            .chain(SPECIALIZATION)
            .chain(IMPROVED)
            .chain(MAGE_SHOP)
            .find(|artifact| artifact.name() == name)
    }

    /// Returns the System equipment slot this artifact goes into, or `None`
    /// for weapons and tools, which are held instead.
    #[must_use]
    pub const fn gear_kind(self) -> Option<GearKind> {
        match self {
            Self::RedKnightsHelmet
            | Self::HunterHelmet
            | Self::LightHelmet
            | Self::ReinforcedHelmet
            | Self::HeavyHelmet
            | Self::RobeHelmet
            | Self::ImprovedLightHelmet
            | Self::ImprovedReinforcedHelmet
            | Self::ImprovedHeavyHelmet
            | Self::ImprovedRobeHelmet
            | Self::DungeonWardensHelmet
            | Self::CrownOfBottomlessMana => Some(GearKind::Head),
            Self::HighKnightsChestplate
            | Self::HunterChestplate
            | Self::LightChestplate
            | Self::ReinforcedChestplate
            | Self::HeavyChestplate
            | Self::RobeChestplate
            | Self::ImprovedLightChestplate
            | Self::ImprovedReinforcedChestplate
            | Self::ImprovedHeavyChestplate
            | Self::ImprovedRobeChestplate
            | Self::BlackHeart
            | Self::ArchmageMantle => Some(GearKind::Chest),
            Self::HunterLeggings
            | Self::LightLeggings
            | Self::ReinforcedLeggings
            | Self::HeavyLeggings
            | Self::RobeLeggings
            | Self::ImprovedLightLeggings
            | Self::ImprovedReinforcedLeggings
            | Self::ImprovedHeavyLeggings
            | Self::ImprovedRobeLeggings
            | Self::TrackersLeggings
            | Self::HighKnightsGreaves => Some(GearKind::Legs),
            Self::AssassinsShoes
            | Self::HunterBoots
            | Self::LightBoots
            | Self::ReinforcedBoots
            | Self::HeavyBoots
            | Self::RobeBoots
            | Self::ImprovedLightBoots
            | Self::ImprovedReinforcedBoots
            | Self::ImprovedHeavyBoots
            | Self::ImprovedRobeBoots => Some(GearKind::Feet),
            Self::ArchersGloves | Self::HighKnightsGauntlet => Some(GearKind::Gloves),
            Self::RingOfEndurance
            | Self::HighMagiciansRing
            | Self::DemonMonarchsRing
            | Self::ManaRing
            | Self::AgileRing
            | Self::StrengthRing
            | Self::BerserkerRing
            | Self::BossHunterRing
            | Self::AntKingRing => Some(GearKind::Ring),
            Self::WindAmulet
            | Self::GatekeepersNecklace
            | Self::OrbOfAvarice
            | Self::ClearMindEarrings
            | Self::HighPriestsBreviary
            | Self::ElementalOrb
            | Self::WardenAmulet
            | Self::HuntersAmulet
            | Self::BloodAmulet
            | Self::ProspectorsNecklace
            | Self::ShadowGuardAmulet => Some(GearKind::Amulet),
            Self::KasakasVenomFang
            | Self::RaikansFang
            | Self::KargalgansStaff
            | Self::AshStaff
            | Self::DemonKingsStaff
            | Self::ApprenticeStaff
            | Self::KamishsWrath
            | Self::KnightsKiller
            | Self::BarukasDagger
            | Self::DemonKingsDaggers
            | Self::DemonKingsLongsword
            | Self::CrystalPickaxe
            | Self::HunterSword
            | Self::HunterDagger
            | Self::HunterAxe
            | Self::HunterShield
            | Self::HunterPickaxe
            | Self::ImprovedHunterSword
            | Self::ImprovedHunterDagger
            | Self::ImprovedHunterAxe => None,
        }
    }

    /// Returns the catalog entry of this artifact.
    #[must_use]
    pub fn artifact(self) -> &'static Artifact {
        match self {
            Self::KasakasVenomFang => &KASAKAS_VENOM_FANG,
            Self::KnightsKiller => &KNIGHTS_KILLER,
            Self::BarukasDagger => &BARUKAS_DAGGER,
            Self::DemonKingsDaggers => &DEMON_KINGS_DAGGERS,
            Self::DemonKingsLongsword => &DEMON_KINGS_LONGSWORD,
            Self::ArchersGloves => &ARCHERS_GLOVES,
            Self::AssassinsShoes => &ASSASSINS_SHOES,
            Self::HighKnightsChestplate => &HIGH_KNIGHTS_CHESTPLATE,
            Self::HighKnightsGauntlet => &HIGH_KNIGHTS_GAUNTLET,
            Self::RedKnightsHelmet => &RED_KNIGHTS_HELMET,
            Self::CrystalPickaxe => &CRYSTAL_PICKAXE,
            Self::RaikansFang => &RAIKANS_FANG,
            Self::RingOfEndurance => &RING_OF_ENDURANCE,
            Self::WindAmulet => &WIND_AMULET,
            Self::DungeonWardensHelmet => &DUNGEON_WARDENS_HELMET,
            Self::TrackersLeggings => &TRACKERS_LEGGINGS,
            Self::GatekeepersNecklace => &GATEKEEPERS_NECKLACE,
            Self::HighMagiciansRing => &HIGH_MAGICIANS_RING,
            Self::KargalgansStaff => &KARGALGANS_STAFF,
            Self::HighKnightsGreaves => &HIGH_KNIGHTS_GREAVES,
            Self::KamishsWrath => &KAMISHS_WRATH,
            Self::OrbOfAvarice => &ORB_OF_AVARICE,
            Self::BlackHeart => &BLACK_HEART,
            Self::DemonMonarchsRing => &DEMON_MONARCHS_RING,
            Self::AshStaff => &ASH_STAFF,
            Self::ManaRing => &MANA_RING,
            Self::ClearMindEarrings => &CLEAR_MIND_EARRINGS,
            Self::ArchmageMantle => &ARCHMAGE_MANTLE,
            Self::HighPriestsBreviary => &HIGH_PRIESTS_BREVIARY,
            Self::ElementalOrb => &ELEMENTAL_ORB,
            Self::DemonKingsStaff => &DEMON_KINGS_STAFF,
            Self::CrownOfBottomlessMana => &CROWN_OF_BOTTOMLESS_MANA,
            Self::ApprenticeStaff => &APPRENTICE_STAFF,
            Self::AgileRing => &AGILE_RING,
            Self::StrengthRing => &STRENGTH_RING,
            Self::WardenAmulet => &WARDEN_AMULET,
            Self::HuntersAmulet => &HUNTERS_AMULET,
            Self::BerserkerRing => &BERSERKER_RING,
            Self::BossHunterRing => &BOSS_HUNTER_RING,
            Self::BloodAmulet => &BLOOD_AMULET,
            Self::ProspectorsNecklace => &PROSPECTORS_NECKLACE,
            Self::AntKingRing => &ANT_KING_RING,
            Self::ShadowGuardAmulet => &SHADOW_GUARD_AMULET,
            Self::HunterSword => &HUNTER_SWORD,
            Self::HunterDagger => &HUNTER_DAGGER,
            Self::HunterAxe => &HUNTER_AXE,
            Self::HunterShield => &HUNTER_SHIELD,
            Self::HunterHelmet => &HUNTER_HELMET,
            Self::HunterChestplate => &HUNTER_CHESTPLATE,
            Self::HunterLeggings => &HUNTER_LEGGINGS,
            Self::HunterBoots => &HUNTER_BOOTS,
            Self::HunterPickaxe => &HUNTER_PICKAXE,
            Self::LightHelmet => &LIGHTHELMET,
            Self::LightChestplate => &LIGHTCHESTPLATE,
            Self::LightLeggings => &LIGHTLEGGINGS,
            Self::LightBoots => &LIGHTBOOTS,
            Self::ReinforcedHelmet => &REINFORCEDHELMET,
            Self::ReinforcedChestplate => &REINFORCEDCHESTPLATE,
            Self::ReinforcedLeggings => &REINFORCEDLEGGINGS,
            Self::ReinforcedBoots => &REINFORCEDBOOTS,
            Self::HeavyHelmet => &HEAVYHELMET,
            Self::HeavyChestplate => &HEAVYCHESTPLATE,
            Self::HeavyLeggings => &HEAVYLEGGINGS,
            Self::HeavyBoots => &HEAVYBOOTS,
            Self::RobeHelmet => &ROBEHELMET,
            Self::RobeChestplate => &ROBECHESTPLATE,
            Self::RobeLeggings => &ROBELEGGINGS,
            Self::RobeBoots => &ROBEBOOTS,
            Self::ImprovedLightHelmet => &IMPROVED_LIGHT_HELMET,
            Self::ImprovedLightChestplate => &IMPROVED_LIGHT_CHESTPLATE,
            Self::ImprovedLightLeggings => &IMPROVED_LIGHT_LEGGINGS,
            Self::ImprovedLightBoots => &IMPROVED_LIGHT_BOOTS,
            Self::ImprovedReinforcedHelmet => &IMPROVED_REINFORCED_HELMET,
            Self::ImprovedReinforcedChestplate => &IMPROVED_REINFORCED_CHESTPLATE,
            Self::ImprovedReinforcedLeggings => &IMPROVED_REINFORCED_LEGGINGS,
            Self::ImprovedReinforcedBoots => &IMPROVED_REINFORCED_BOOTS,
            Self::ImprovedHeavyHelmet => &IMPROVED_HEAVY_HELMET,
            Self::ImprovedHeavyChestplate => &IMPROVED_HEAVY_CHESTPLATE,
            Self::ImprovedHeavyLeggings => &IMPROVED_HEAVY_LEGGINGS,
            Self::ImprovedHeavyBoots => &IMPROVED_HEAVY_BOOTS,
            Self::ImprovedRobeHelmet => &IMPROVED_ROBE_HELMET,
            Self::ImprovedRobeChestplate => &IMPROVED_ROBE_CHESTPLATE,
            Self::ImprovedRobeLeggings => &IMPROVED_ROBE_LEGGINGS,
            Self::ImprovedRobeBoots => &IMPROVED_ROBE_BOOTS,
            Self::ImprovedHunterSword => &IMPROVED_HUNTER_SWORD,
            Self::ImprovedHunterDagger => &IMPROVED_HUNTER_DAGGER,
            Self::ImprovedHunterAxe => &IMPROVED_HUNTER_AXE,
        }
    }

    /// Returns whether this is a piece of the Association starter kit.
    #[must_use]
    pub fn is_starter(self) -> bool {
        STARTER.contains(&self)
    }

    /// Returns the specialization armor line of this piece, if any.
    ///
    /// [`SPECIALIZATION`] and [`IMPROVED`] list each line's four pieces in
    /// [`ArmorLine::ALL`] order, so the position decides the line.
    #[must_use]
    pub fn line(self) -> Option<ArmorLine> {
        let index = SPECIALIZATION
            .iter()
            .position(|id| *id == self)
            .or_else(|| IMPROVED.iter().position(|id| *id == self))?;
        ArmorLine::ALL.get(index / 4).copied()
    }

    /// Returns the classes that may wear this piece; `None` means everyone.
    #[must_use]
    pub fn allowed_classes(self) -> Option<&'static [HunterClass]> {
        if self.spell_gear().is_some() {
            return Some(&[HunterClass::Mage]);
        }
        self.line().map(ArmorLine::classes)
    }

    /// Returns the type of item the item maker enchants this artifact as;
    /// `None` for the shield, which takes no enchantments.
    #[must_use]
    pub fn enchant_kind(self) -> Option<ItemKind> {
        let kind = match self.gear_kind() {
            Some(GearKind::Head) => ItemKind::Helmet,
            Some(GearKind::Chest) => ItemKind::Chest,
            Some(GearKind::Legs) => ItemKind::Legs,
            Some(GearKind::Feet) => ItemKind::Boots,
            Some(GearKind::Gloves) => ItemKind::Gloves,
            Some(GearKind::Ring | GearKind::Amulet) => ItemKind::Jewelry,
            None => match self {
                Self::HunterShield => return None,
                Self::CrystalPickaxe | Self::HunterPickaxe => ItemKind::Pickaxe,
                _ if self.spell_gear().is_some() => ItemKind::Staff,
                _ => ItemKind::Melee,
            },
        };
        Some(kind)
    }

    /// Returns the spellcasting bonuses of a Mage artifact; `None` for
    /// everything a non-Mage may use.
    #[must_use]
    pub fn spell_gear(self) -> Option<SpellGear> {
        let none = SpellGear::default();
        let gear = match self {
            Self::ApprenticeStaff => SpellGear {
                magic: 0.10,
                ..none
            },
            Self::AshStaff => SpellGear {
                magic: 0.20,
                destruction_cost: -0.10,
                ..none
            },
            Self::ManaRing => SpellGear { mana: 40.0, ..none },
            Self::ClearMindEarrings => SpellGear {
                regen: 0.30,
                ..none
            },
            // Its +30% magic damage is the Magic Focus ability.
            Self::KargalgansStaff => SpellGear {
                burning: true,
                ..none
            },
            Self::HighMagiciansRing => SpellGear {
                mana: 60.0,
                cooldown: 0.10,
                ..none
            },
            Self::ArchmageMantle => SpellGear {
                magic: 0.10,
                mana: 60.0,
                ..none
            },
            Self::HighPriestsBreviary => SpellGear {
                healing: 0.30,
                barrier: 4.0,
                ..none
            },
            Self::ElementalOrb => SpellGear {
                free_every: 5,
                ..none
            },
            Self::OrbOfAvarice => SpellGear { cost: 0.20, ..none },
            Self::DemonKingsStaff => SpellGear {
                magic: 0.25,
                echo_percent: 20,
                ..none
            },
            Self::CrownOfBottomlessMana => SpellGear {
                mana: 100.0,
                regen: 0.50,
                free_meditation: true,
                ..none
            },
            _ => return None,
        };
        Some(gear)
    }

    /// Returns the bonus to outgoing magic damage this piece grants, as a
    /// fraction.
    #[must_use]
    pub fn magic_bonus(self) -> f32 {
        match self.line() {
            Some(ArmorLine::Robe) if self.is_improved() => IMPROVED_ROBE_MAGIC_BONUS_PER_PIECE,
            Some(ArmorLine::Robe) => ROBE_MAGIC_BONUS_PER_PIECE,
            _ => 0.0,
        }
    }

    /// Returns whether this is an improved serial piece, made to order.
    #[must_use]
    pub fn is_improved(self) -> bool {
        IMPROVED.contains(&self)
    }

    /// Returns the Strength this piece needs before its bonuses apply.
    #[must_use]
    pub fn required_strength(self) -> u32 {
        if self.is_improved() && self.line() == Some(ArmorLine::Heavy) {
            IMPROVED_HEAVY_STRENGTH
        } else {
            0
        }
    }

    /// Returns the leather dye of this piece, if it is dyed.
    fn dye(self) -> Option<i32> {
        (self.line() == Some(ArmorLine::Robe)).then_some(0x5B_2C_8F)
    }
}

/// Bonus to outgoing magic damage of one mage-clothing piece.
pub const ROBE_MAGIC_BONUS_PER_PIECE: f32 = 0.03;
/// Bonus to outgoing magic damage of one improved mage-clothing piece.
pub const IMPROVED_ROBE_MAGIC_BONUS_PER_PIECE: f32 = 0.04;
/// Strength improved heavy armor needs before its bonuses apply.
pub const IMPROVED_HEAVY_STRENGTH: u32 = 20;

/// A specialization armor line sold by the city merchant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmorLine {
    /// Light armor: little protection, extra speed.
    Light,
    /// Reinforced armor: starter-level protection plus toughness.
    Reinforced,
    /// Initial heavy armor: most protection, slower movement.
    Heavy,
    /// Mage clothing: weakest protection, stronger magic.
    Robe,
}

impl ArmorLine {
    /// Every line, in catalog order.
    pub const ALL: [Self; 4] = [Self::Light, Self::Reinforced, Self::Heavy, Self::Robe];

    /// Returns the classes that may wear the line.
    #[must_use]
    pub const fn classes(self) -> &'static [HunterClass] {
        match self {
            Self::Light => &[HunterClass::Assassin, HunterClass::Ranger],
            Self::Reinforced => &[HunterClass::Fighter, HunterClass::Tanker],
            Self::Heavy => &[HunterClass::Tanker],
            Self::Robe => &[HunterClass::Mage],
        }
    }
}

/// Improved serial gear made to order from dungeon materials: four pieces
/// per armor line in [`ArmorLine::ALL`] order, then the weapons.
pub const IMPROVED: [ArtifactId; 19] = [
    ArtifactId::ImprovedLightHelmet,
    ArtifactId::ImprovedLightChestplate,
    ArtifactId::ImprovedLightLeggings,
    ArtifactId::ImprovedLightBoots,
    ArtifactId::ImprovedReinforcedHelmet,
    ArtifactId::ImprovedReinforcedChestplate,
    ArtifactId::ImprovedReinforcedLeggings,
    ArtifactId::ImprovedReinforcedBoots,
    ArtifactId::ImprovedHeavyHelmet,
    ArtifactId::ImprovedHeavyChestplate,
    ArtifactId::ImprovedHeavyLeggings,
    ArtifactId::ImprovedHeavyBoots,
    ArtifactId::ImprovedRobeHelmet,
    ArtifactId::ImprovedRobeChestplate,
    ArtifactId::ImprovedRobeLeggings,
    ArtifactId::ImprovedRobeBoots,
    ArtifactId::ImprovedHunterSword,
    ArtifactId::ImprovedHunterDagger,
    ArtifactId::ImprovedHunterAxe,
];

/// Specialization armor sold by the city merchant, four pieces per line.
pub const SPECIALIZATION: [ArtifactId; 16] = [
    ArtifactId::LightHelmet,
    ArtifactId::LightChestplate,
    ArtifactId::LightLeggings,
    ArtifactId::LightBoots,
    ArtifactId::ReinforcedHelmet,
    ArtifactId::ReinforcedChestplate,
    ArtifactId::ReinforcedLeggings,
    ArtifactId::ReinforcedBoots,
    ArtifactId::HeavyHelmet,
    ArtifactId::HeavyChestplate,
    ArtifactId::HeavyLeggings,
    ArtifactId::HeavyBoots,
    ArtifactId::RobeHelmet,
    ArtifactId::RobeChestplate,
    ArtifactId::RobeLeggings,
    ArtifactId::RobeBoots,
];

/// The Association's Common starter equipment.
///
/// Kept apart from [`ALL`]: those are the named rewards that chests and the
/// System shop roll and that have resource-pack models, while starter pieces
/// are serial hunter gear shown as their vanilla holder.
pub const STARTER: [ArtifactId; 9] = [
    ArtifactId::HunterSword,
    ArtifactId::HunterDagger,
    ArtifactId::HunterAxe,
    ArtifactId::HunterShield,
    ArtifactId::HunterHelmet,
    ArtifactId::HunterChestplate,
    ArtifactId::HunterLeggings,
    ArtifactId::HunterBoots,
    ArtifactId::HunterPickaxe,
];

/// Starter weapons a hunter picks one of.
pub const STARTER_WEAPONS: [ArtifactId; 3] = [
    ArtifactId::HunterSword,
    ArtifactId::HunterDagger,
    ArtifactId::HunterAxe,
];

/// Starter pieces every hunter receives besides the chosen weapon.
pub const STARTER_OUTFIT: [ArtifactId; 6] = [
    ArtifactId::HunterShield,
    ArtifactId::HunterHelmet,
    ArtifactId::HunterChestplate,
    ArtifactId::HunterLeggings,
    ArtifactId::HunterBoots,
    ArtifactId::HunterPickaxe,
];

/// Every catalog artifact.
pub const ALL: [ArtifactId; 42] = [
    ArtifactId::KasakasVenomFang,
    ArtifactId::KnightsKiller,
    ArtifactId::BarukasDagger,
    ArtifactId::DemonKingsDaggers,
    ArtifactId::DemonKingsLongsword,
    ArtifactId::ArchersGloves,
    ArtifactId::AssassinsShoes,
    ArtifactId::HighKnightsChestplate,
    ArtifactId::HighKnightsGauntlet,
    ArtifactId::RedKnightsHelmet,
    ArtifactId::CrystalPickaxe,
    ArtifactId::RaikansFang,
    ArtifactId::RingOfEndurance,
    ArtifactId::WindAmulet,
    ArtifactId::DungeonWardensHelmet,
    ArtifactId::TrackersLeggings,
    ArtifactId::GatekeepersNecklace,
    ArtifactId::HighMagiciansRing,
    ArtifactId::KargalgansStaff,
    ArtifactId::HighKnightsGreaves,
    ArtifactId::KamishsWrath,
    ArtifactId::OrbOfAvarice,
    ArtifactId::BlackHeart,
    ArtifactId::DemonMonarchsRing,
    ArtifactId::AshStaff,
    ArtifactId::ManaRing,
    ArtifactId::ClearMindEarrings,
    ArtifactId::ArchmageMantle,
    ArtifactId::HighPriestsBreviary,
    ArtifactId::ElementalOrb,
    ArtifactId::DemonKingsStaff,
    ArtifactId::CrownOfBottomlessMana,
    ArtifactId::AgileRing,
    ArtifactId::StrengthRing,
    ArtifactId::WardenAmulet,
    ArtifactId::HuntersAmulet,
    ArtifactId::BerserkerRing,
    ArtifactId::BossHunterRing,
    ArtifactId::BloodAmulet,
    ArtifactId::ProspectorsNecklace,
    ArtifactId::AntKingRing,
    ArtifactId::ShadowGuardAmulet,
];

/// Mage artifacts sold by the city merchant instead of rolled as rewards.
pub const MAGE_SHOP: [ArtifactId; 1] = [ArtifactId::ApprenticeStaff];

/// Which catalog list an artifact belongs to, for chest and shop rolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Held in the main hand.
    Weapon,
    /// Worn in an armor slot.
    Armor,
}

/// One attribute bonus an artifact grants while worn.
#[derive(Debug, Clone, Copy)]
pub struct Modifier {
    /// Vanilla attribute the bonus applies to.
    pub attribute: AttributeRef,
    /// Bonus amount, interpreted by `operation`.
    pub amount: f64,
    /// How `amount` combines with the attribute's other modifiers.
    pub operation: AttributeModifierOperation,
    /// Equipment slots the bonus is active in.
    pub slot: EquipmentSlotGroup,
}

/// One artifact ability. Abilities exist only from [`Rarity::Epic`] upward.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ability {
    /// Extra damage against targets wearing any armor.
    ArmorPiercing {
        /// Damage bonus as a fraction of the hit.
        percent: f32,
    },
    /// A short movement-speed burst after a successful hit.
    SpeedOnHit {
        /// Speed bonus as a fraction of the base attribute.
        percent: f64,
        /// Duration in seconds.
        seconds: u64,
        /// Cooldown in seconds.
        cooldown: u64,
    },
    /// Flat attack damage that grows with the holder's Strength.
    StrengthScaling {
        /// Damage per Strength point.
        per_point: f64,
        /// Highest total bonus.
        maximum: f64,
    },
    /// Reduced incoming damage while the holder is badly hurt.
    LowHealthReduction {
        /// Health fraction below which the ability is active.
        threshold: f32,
        /// Fraction of incoming damage removed.
        reduction: f32,
        /// Duration in seconds, refreshed on each qualifying hit.
        seconds: u64,
        /// Cooldown in seconds.
        cooldown: u64,
    },
    /// A shield after every Nth successful hit.
    HitCounterShield {
        /// Successful hits needed for one shield.
        hits: u8,
        /// Absorption health granted, in half-hearts.
        shield_health: f32,
        /// Duration in seconds.
        seconds: u64,
        /// Cooldown in seconds.
        cooldown: u64,
    },
    /// A short storm around the struck target.
    Storm {
        /// Radius in blocks.
        radius: f64,
        /// Duration in seconds.
        seconds: u64,
        /// Damage per second to every other hostile entity inside.
        damage_per_second: f32,
        /// Cooldown in seconds.
        cooldown: u64,
    },
    /// Survives one otherwise lethal hit.
    LethalSurvival {
        /// Health left after the survived hit.
        health: f32,
        /// Shield granted together with the survival.
        shield_health: f32,
        /// Shield duration in seconds.
        seconds: u64,
        /// Cooldown in seconds.
        cooldown: u64,
    },
    /// Stronger outgoing magic damage while worn or held.
    MagicFocus {
        /// Bonus as a fraction of the magic hit.
        percent: f32,
    },
    /// Extra magic crystals from dungeon ore.
    CrystalLuck {
        /// Percent chance of one additional crystal.
        percent: u32,
    },
    /// More damage and attack speed while badly hurt.
    Berserk {
        /// Health fraction below which the ability is active.
        threshold: f32,
        /// Damage bonus as a fraction of the hit.
        percent: f32,
    },
    /// Extra damage against gate bosses.
    BossSlayer {
        /// Damage bonus as a fraction of the hit.
        percent: f32,
    },
    /// Melee hits heal the holder.
    Bloodthirst {
        /// Healing as a fraction of the damage dealt.
        percent: f32,
    },
    /// More system points from kills.
    Greed {
        /// Bonus as a fraction of the reward.
        percent: f32,
    },
    /// Every Nth melee hit strikes the target again.
    DoubleStrike {
        /// Hits per extra strike.
        every: u32,
        /// Damage of the extra strike as a fraction of the hit.
        percent: f32,
    },
    /// Survives a lethal hit and turns briefly invulnerable.
    ShadowGuard {
        /// Seconds of invulnerability.
        seconds: u64,
        /// Cooldown in seconds.
        cooldown: u64,
    },
}

impl Ability {
    /// Returns the persisted cooldown key of the ability.
    #[must_use]
    pub const fn cooldown_key(self) -> &'static str {
        match self {
            Self::ArmorPiercing { .. } => "armor_piercing",
            Self::SpeedOnHit { .. } => "speed_on_hit",
            Self::StrengthScaling { .. } => "strength_scaling",
            Self::LowHealthReduction { .. } => "low_health_reduction",
            Self::HitCounterShield { .. } => "hit_counter_shield",
            Self::Storm { .. } => "storm",
            Self::LethalSurvival { .. } => "lethal_survival",
            Self::CrystalLuck { .. } => "crystal_luck",
            Self::MagicFocus { .. } => "magic_focus",
            Self::Berserk { .. } => "berserk",
            Self::BossSlayer { .. } => "boss_slayer",
            Self::Bloodthirst { .. } => "bloodthirst",
            Self::Greed { .. } => "greed",
            Self::DoubleStrike { .. } => "double_strike",
            Self::ShadowGuard { .. } => "shadow_guard",
        }
    }

    /// Returns the cooldown in seconds, or zero when the ability has none.
    #[must_use]
    pub const fn cooldown_seconds(self) -> u64 {
        match self {
            Self::SpeedOnHit { cooldown, .. }
            | Self::LowHealthReduction { cooldown, .. }
            | Self::HitCounterShield { cooldown, .. }
            | Self::Storm { cooldown, .. }
            | Self::LethalSurvival { cooldown, .. }
            | Self::ShadowGuard { cooldown, .. } => cooldown,
            Self::ArmorPiercing { .. }
            | Self::StrengthScaling { .. }
            | Self::CrystalLuck { .. }
            | Self::MagicFocus { .. }
            | Self::Berserk { .. }
            | Self::BossSlayer { .. }
            | Self::Bloodthirst { .. }
            | Self::Greed { .. }
            | Self::DoubleStrike { .. } => 0,
        }
    }
}

/// One catalog entry.
#[derive(Debug)]
pub struct Artifact {
    /// Catalog identity.
    pub id: ArtifactId,
    /// Russian display name.
    pub name: &'static str,
    /// Rarity, which also decides the transferable rank gate.
    pub rarity: Rarity,
    /// Which roll list the artifact belongs to.
    pub category: Category,
    /// Vanilla item the artifact is built from.
    pub holder: &'static LazyLock<Item>,
    /// Attribute bonuses granted while worn.
    pub modifiers: &'static [Modifier],
    /// Abilities; empty below Epic.
    pub abilities: &'static [Ability],
}

const ATTACK: EquipmentSlotGroup = EquipmentSlotGroup::MainHand;
const HELMET: EquipmentSlotGroup = EquipmentSlotGroup::Head;
const CHEST: EquipmentSlotGroup = EquipmentSlotGroup::Chest;
const FEET: EquipmentSlotGroup = EquipmentSlotGroup::Feet;

static KASAKAS_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ATTACK_DAMAGE,
    amount: 2.0,
    operation: AttributeModifierOperation::AddValue,
    slot: ATTACK,
}];
static KNIGHT_KILLER_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ATTACK_DAMAGE,
    amount: 4.0,
    operation: AttributeModifierOperation::AddValue,
    slot: ATTACK,
}];
static BARUKA_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ATTACK_DAMAGE,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: ATTACK,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.08,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: EquipmentSlotGroup::Hand,
    },
];
static DEMON_DAGGERS_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ATTACK_DAMAGE,
    amount: 6.0,
    operation: AttributeModifierOperation::AddValue,
    slot: ATTACK,
}];
static DEMON_LONGSWORD_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ATTACK_DAMAGE,
    amount: 7.0,
    operation: AttributeModifierOperation::AddValue,
    slot: ATTACK,
}];
static ARCHER_GLOVES_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 1.0,
    operation: AttributeModifierOperation::AddValue,
    slot: CHEST,
}];
static ASSASSIN_SHOES_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.05,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: FEET,
    },
];
static HIGH_KNIGHT_CHEST_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 3.0,
    operation: AttributeModifierOperation::AddValue,
    slot: CHEST,
}];
static HIGH_KNIGHT_GAUNTLET_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 2.0,
    operation: AttributeModifierOperation::AddValue,
    slot: CHEST,
}];
static RED_KNIGHT_HELMET_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
    Modifier {
        attribute: vanilla_attributes::MAX_HEALTH,
        amount: 4.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
];
static CRYSTAL_PICKAXE_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::MINING_EFFICIENCY,
    amount: 0.15,
    operation: AttributeModifierOperation::AddMultipliedBase,
    slot: EquipmentSlotGroup::Hand,
}];

static KASAKAS_VENOM_FANG: Artifact = Artifact {
    id: ArtifactId::KasakasVenomFang,
    name: "Клык Касаки",
    rarity: Rarity::Rare,
    category: Category::Weapon,
    holder: &vanilla_items::IRON_SWORD,
    modifiers: &KASAKAS_MODIFIERS,
    abilities: &[],
};
static KNIGHTS_KILLER: Artifact = Artifact {
    id: ArtifactId::KnightsKiller,
    name: "Убийца рыцарей",
    rarity: Rarity::Epic,
    category: Category::Weapon,
    holder: &vanilla_items::IRON_SWORD,
    modifiers: &KNIGHT_KILLER_MODIFIERS,
    abilities: &[Ability::ArmorPiercing { percent: 0.15 }],
};
static BARUKAS_DAGGER: Artifact = Artifact {
    id: ArtifactId::BarukasDagger,
    name: "Кинжал Баруки",
    rarity: Rarity::Epic,
    category: Category::Weapon,
    holder: &vanilla_items::GOLDEN_SWORD,
    modifiers: &BARUKA_MODIFIERS,
    abilities: &[Ability::SpeedOnHit {
        percent: 0.15,
        seconds: 3,
        cooldown: 10,
    }],
};
static DEMON_KINGS_DAGGERS: Artifact = Artifact {
    id: ArtifactId::DemonKingsDaggers,
    name: "Кинжалы короля демонов",
    rarity: Rarity::Legendary,
    category: Category::Weapon,
    holder: &vanilla_items::DIAMOND_SWORD,
    modifiers: &DEMON_DAGGERS_MODIFIERS,
    abilities: &[Ability::StrengthScaling {
        per_point: 0.05,
        maximum: 5.0,
    }],
};
static DEMON_KINGS_LONGSWORD: Artifact = Artifact {
    id: ArtifactId::DemonKingsLongsword,
    name: "Длинный меч короля демонов",
    rarity: Rarity::Legendary,
    category: Category::Weapon,
    holder: &vanilla_items::NETHERITE_SWORD,
    modifiers: &DEMON_LONGSWORD_MODIFIERS,
    abilities: &[Ability::Storm {
        radius: 3.0,
        seconds: 3,
        damage_per_second: 2.0,
        cooldown: 20,
    }],
};
static ARCHERS_GLOVES: Artifact = Artifact {
    id: ArtifactId::ArchersGloves,
    name: "Перчатки лучника",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &ARCHER_GLOVES_MODIFIERS,
    abilities: &[],
};
static ASSASSINS_SHOES: Artifact = Artifact {
    id: ArtifactId::AssassinsShoes,
    name: "Обувь убийцы",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &ASSASSIN_SHOES_MODIFIERS,
    abilities: &[],
};
static HIGH_KNIGHTS_CHESTPLATE: Artifact = Artifact {
    id: ArtifactId::HighKnightsChestplate,
    name: "Нагрудник высшего рыцаря",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::IRON_CHESTPLATE,
    modifiers: &HIGH_KNIGHT_CHEST_MODIFIERS,
    abilities: &[Ability::LowHealthReduction {
        threshold: 0.3,
        reduction: 0.15,
        seconds: 4,
        cooldown: 30,
    }],
};
static HIGH_KNIGHTS_GAUNTLET: Artifact = Artifact {
    id: ArtifactId::HighKnightsGauntlet,
    name: "Перчатки высшего рыцаря",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &HIGH_KNIGHT_GAUNTLET_MODIFIERS,
    abilities: &[Ability::HitCounterShield {
        hits: 5,
        shield_health: 4.0,
        seconds: 5,
        cooldown: 15,
    }],
};
static RED_KNIGHTS_HELMET: Artifact = Artifact {
    id: ArtifactId::RedKnightsHelmet,
    name: "Шлем красного рыцаря",
    rarity: Rarity::Legendary,
    category: Category::Armor,
    holder: &vanilla_items::NETHERITE_HELMET,
    modifiers: &RED_KNIGHT_HELMET_MODIFIERS,
    abilities: &[Ability::LethalSurvival {
        health: 1.0,
        shield_health: 6.0,
        seconds: 3,
        cooldown: 120,
    }],
};
static CRYSTAL_PICKAXE: Artifact = Artifact {
    id: ArtifactId::CrystalPickaxe,
    name: "Кирка магических кристаллов",
    rarity: Rarity::Epic,
    category: Category::Weapon,
    holder: &vanilla_items::NETHERITE_PICKAXE,
    modifiers: &CRYSTAL_PICKAXE_MODIFIERS,
    abilities: &[Ability::CrystalLuck { percent: 50 }],
};

/// A held starter weapon: flat damage plus the vanilla attack-speed offset.
///
/// The item's own modifier component replaces the holder's vanilla one, so the
/// speed offset has to be restated: base attack speed is 4 per second.
const fn weapon_modifiers(damage: f64, speed: f64) -> [Modifier; 2] {
    [
        Modifier {
            attribute: vanilla_attributes::ATTACK_DAMAGE,
            amount: damage,
            operation: AttributeModifierOperation::AddValue,
            slot: ATTACK,
        },
        Modifier {
            attribute: vanilla_attributes::ATTACK_SPEED,
            amount: speed - 4.0,
            operation: AttributeModifierOperation::AddValue,
            slot: ATTACK,
        },
    ]
}

const fn armor_modifier(amount: f64, slot: EquipmentSlotGroup) -> [Modifier; 1] {
    [Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount,
        operation: AttributeModifierOperation::AddValue,
        slot,
    }]
}

// Starter numbers mirror an iron set and iron weapons: 15 armor in total, a
// 6-damage sword at 1.6 attacks per second, a quicker and lighter dagger,
// and a slow, heavy axe.
static HUNTER_SWORD_MODIFIERS: [Modifier; 2] = weapon_modifiers(5.0, 1.6);
static HUNTER_DAGGER_MODIFIERS: [Modifier; 2] = weapon_modifiers(3.0, 2.4);
static HUNTER_AXE_MODIFIERS: [Modifier; 2] = weapon_modifiers(8.0, 0.9);
static HUNTER_HELMET_MODIFIERS: [Modifier; 1] = armor_modifier(2.0, HELMET);
static HUNTER_CHESTPLATE_MODIFIERS: [Modifier; 1] = armor_modifier(6.0, CHEST);
static HUNTER_LEGGINGS_MODIFIERS: [Modifier; 1] = armor_modifier(5.0, EquipmentSlotGroup::Legs);
static HUNTER_BOOTS_MODIFIERS: [Modifier; 1] = armor_modifier(2.0, FEET);

static LIGHTHELMET_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.02,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: HELMET,
    },
];
static LIGHTHELMET: Artifact = Artifact {
    id: ArtifactId::LightHelmet,
    name: "Лёгкий капюшон",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &LIGHTHELMET_MODIFIERS,
    abilities: &[],
};
static LIGHTCHESTPLATE_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 4.0,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: CHEST,
    },
];
static LIGHTCHESTPLATE: Artifact = Artifact {
    id: ArtifactId::LightChestplate,
    name: "Лёгкая куртка",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &LIGHTCHESTPLATE_MODIFIERS,
    abilities: &[],
};
static LIGHTLEGGINGS_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: EquipmentSlotGroup::Legs,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: EquipmentSlotGroup::Legs,
    },
];
static LIGHTLEGGINGS: Artifact = Artifact {
    id: ArtifactId::LightLeggings,
    name: "Лёгкие штаны",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &LIGHTLEGGINGS_MODIFIERS,
    abilities: &[],
};
static LIGHTBOOTS_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.02,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: FEET,
    },
];
static LIGHTBOOTS: Artifact = Artifact {
    id: ArtifactId::LightBoots,
    name: "Лёгкие сапоги",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &LIGHTBOOTS_MODIFIERS,
    abilities: &[],
};
static REINFORCEDHELMET_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 2.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 0.5,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
];
static REINFORCEDHELMET: Artifact = Artifact {
    id: ArtifactId::ReinforcedHelmet,
    name: "Усиленный шлем",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::IRON_HELMET,
    modifiers: &REINFORCEDHELMET_MODIFIERS,
    abilities: &[],
};
static REINFORCEDCHESTPLATE_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 6.0,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 0.5,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
];
static REINFORCEDCHESTPLATE: Artifact = Artifact {
    id: ArtifactId::ReinforcedChestplate,
    name: "Усиленный нагрудник",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::IRON_CHESTPLATE,
    modifiers: &REINFORCEDCHESTPLATE_MODIFIERS,
    abilities: &[],
};
static REINFORCEDLEGGINGS_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 5.0,
        operation: AttributeModifierOperation::AddValue,
        slot: EquipmentSlotGroup::Legs,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 0.5,
        operation: AttributeModifierOperation::AddValue,
        slot: EquipmentSlotGroup::Legs,
    },
];
static REINFORCEDLEGGINGS: Artifact = Artifact {
    id: ArtifactId::ReinforcedLeggings,
    name: "Усиленные поножи",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::IRON_LEGGINGS,
    modifiers: &REINFORCEDLEGGINGS_MODIFIERS,
    abilities: &[],
};
static REINFORCEDBOOTS_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 2.0,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 0.5,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
];
static REINFORCEDBOOTS: Artifact = Artifact {
    id: ArtifactId::ReinforcedBoots,
    name: "Усиленные сапоги",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::IRON_BOOTS,
    modifiers: &REINFORCEDBOOTS_MODIFIERS,
    abilities: &[],
};
static HEAVYHELMET_MODIFIERS: [Modifier; 3] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: -0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: HELMET,
    },
];
static HEAVYHELMET: Artifact = Artifact {
    id: ArtifactId::HeavyHelmet,
    name: "Тяжёлый шлем",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_HELMET,
    modifiers: &HEAVYHELMET_MODIFIERS,
    abilities: &[],
};
static HEAVYCHESTPLATE_MODIFIERS: [Modifier; 3] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 7.0,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: -0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: CHEST,
    },
];
static HEAVYCHESTPLATE: Artifact = Artifact {
    id: ArtifactId::HeavyChestplate,
    name: "Тяжёлый нагрудник",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_CHESTPLATE,
    modifiers: &HEAVYCHESTPLATE_MODIFIERS,
    abilities: &[],
};
static HEAVYLEGGINGS_MODIFIERS: [Modifier; 3] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 6.0,
        operation: AttributeModifierOperation::AddValue,
        slot: EquipmentSlotGroup::Legs,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: EquipmentSlotGroup::Legs,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: -0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: EquipmentSlotGroup::Legs,
    },
];
static HEAVYLEGGINGS: Artifact = Artifact {
    id: ArtifactId::HeavyLeggings,
    name: "Тяжёлые поножи",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_LEGGINGS,
    modifiers: &HEAVYLEGGINGS_MODIFIERS,
    abilities: &[],
};
static HEAVYBOOTS_MODIFIERS: [Modifier; 3] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: -0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: FEET,
    },
];
static HEAVYBOOTS: Artifact = Artifact {
    id: ArtifactId::HeavyBoots,
    name: "Тяжёлые сабатоны",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_BOOTS,
    modifiers: &HEAVYBOOTS_MODIFIERS,
    abilities: &[],
};
static ROBEHELMET_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 1.0,
    operation: AttributeModifierOperation::AddValue,
    slot: HELMET,
}];
static ROBEHELMET: Artifact = Artifact {
    id: ArtifactId::RobeHelmet,
    name: "Капюшон мага",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &ROBEHELMET_MODIFIERS,
    abilities: &[],
};
static ROBECHESTPLATE_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 3.0,
    operation: AttributeModifierOperation::AddValue,
    slot: CHEST,
}];
static ROBECHESTPLATE: Artifact = Artifact {
    id: ArtifactId::RobeChestplate,
    name: "Мантия мага",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &ROBECHESTPLATE_MODIFIERS,
    abilities: &[],
};
static ROBELEGGINGS_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 2.0,
    operation: AttributeModifierOperation::AddValue,
    slot: EquipmentSlotGroup::Legs,
}];
static ROBELEGGINGS: Artifact = Artifact {
    id: ArtifactId::RobeLeggings,
    name: "Брюки мага",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &ROBELEGGINGS_MODIFIERS,
    abilities: &[],
};
static ROBEBOOTS_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 1.0,
    operation: AttributeModifierOperation::AddValue,
    slot: FEET,
}];
static ROBEBOOTS: Artifact = Artifact {
    id: ArtifactId::RobeBoots,
    name: "Туфли мага",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &ROBEBOOTS_MODIFIERS,
    abilities: &[],
};

static IMPROVED_LIGHT_HELMET_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 2.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: HELMET,
    },
];
static IMPROVED_LIGHT_HELMET: Artifact = Artifact {
    id: ArtifactId::ImprovedLightHelmet,
    name: "Лёгкий капюшон II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &IMPROVED_LIGHT_HELMET_MODIFIERS,
    abilities: &[],
};
static IMPROVED_LIGHT_CHESTPLATE_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 5.0,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.04,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: CHEST,
    },
];
static IMPROVED_LIGHT_CHESTPLATE: Artifact = Artifact {
    id: ArtifactId::ImprovedLightChestplate,
    name: "Лёгкая куртка II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &IMPROVED_LIGHT_CHESTPLATE_MODIFIERS,
    abilities: &[],
};
static IMPROVED_LIGHT_LEGGINGS_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 4.0,
        operation: AttributeModifierOperation::AddValue,
        slot: EquipmentSlotGroup::Legs,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.04,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: EquipmentSlotGroup::Legs,
    },
];
static IMPROVED_LIGHT_LEGGINGS: Artifact = Artifact {
    id: ArtifactId::ImprovedLightLeggings,
    name: "Лёгкие штаны II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &IMPROVED_LIGHT_LEGGINGS_MODIFIERS,
    abilities: &[],
};
static IMPROVED_LIGHT_BOOTS_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: FEET,
    },
];
static IMPROVED_LIGHT_BOOTS: Artifact = Artifact {
    id: ArtifactId::ImprovedLightBoots,
    name: "Лёгкие сапоги II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &IMPROVED_LIGHT_BOOTS_MODIFIERS,
    abilities: &[],
};
static IMPROVED_REINFORCED_HELMET_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
];
static IMPROVED_REINFORCED_HELMET: Artifact = Artifact {
    id: ArtifactId::ImprovedReinforcedHelmet,
    name: "Усиленный шлем II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::IRON_HELMET,
    modifiers: &IMPROVED_REINFORCED_HELMET_MODIFIERS,
    abilities: &[],
};
static IMPROVED_REINFORCED_CHESTPLATE_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 7.0,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
];
static IMPROVED_REINFORCED_CHESTPLATE: Artifact = Artifact {
    id: ArtifactId::ImprovedReinforcedChestplate,
    name: "Усиленный нагрудник II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::IRON_CHESTPLATE,
    modifiers: &IMPROVED_REINFORCED_CHESTPLATE_MODIFIERS,
    abilities: &[],
};
static IMPROVED_REINFORCED_LEGGINGS_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 6.0,
        operation: AttributeModifierOperation::AddValue,
        slot: EquipmentSlotGroup::Legs,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: EquipmentSlotGroup::Legs,
    },
];
static IMPROVED_REINFORCED_LEGGINGS: Artifact = Artifact {
    id: ArtifactId::ImprovedReinforcedLeggings,
    name: "Усиленные поножи II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::IRON_LEGGINGS,
    modifiers: &IMPROVED_REINFORCED_LEGGINGS_MODIFIERS,
    abilities: &[],
};
static IMPROVED_REINFORCED_BOOTS_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 2.0,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
];
static IMPROVED_REINFORCED_BOOTS: Artifact = Artifact {
    id: ArtifactId::ImprovedReinforcedBoots,
    name: "Усиленные сапоги II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::IRON_BOOTS,
    modifiers: &IMPROVED_REINFORCED_BOOTS_MODIFIERS,
    abilities: &[],
};
static IMPROVED_HEAVY_HELMET_MODIFIERS: [Modifier; 3] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.5,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: -0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: HELMET,
    },
];
static IMPROVED_HEAVY_HELMET: Artifact = Artifact {
    id: ArtifactId::ImprovedHeavyHelmet,
    name: "Тяжёлый шлем II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_HELMET,
    modifiers: &IMPROVED_HEAVY_HELMET_MODIFIERS,
    abilities: &[],
};
static IMPROVED_HEAVY_CHESTPLATE_MODIFIERS: [Modifier; 3] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 8.0,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.5,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: -0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: CHEST,
    },
];
static IMPROVED_HEAVY_CHESTPLATE: Artifact = Artifact {
    id: ArtifactId::ImprovedHeavyChestplate,
    name: "Тяжёлый нагрудник II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_CHESTPLATE,
    modifiers: &IMPROVED_HEAVY_CHESTPLATE_MODIFIERS,
    abilities: &[],
};
static IMPROVED_HEAVY_LEGGINGS_MODIFIERS: [Modifier; 3] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 6.0,
        operation: AttributeModifierOperation::AddValue,
        slot: EquipmentSlotGroup::Legs,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.5,
        operation: AttributeModifierOperation::AddValue,
        slot: EquipmentSlotGroup::Legs,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: -0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: EquipmentSlotGroup::Legs,
    },
];
static IMPROVED_HEAVY_LEGGINGS: Artifact = Artifact {
    id: ArtifactId::ImprovedHeavyLeggings,
    name: "Тяжёлые поножи II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_LEGGINGS,
    modifiers: &IMPROVED_HEAVY_LEGGINGS_MODIFIERS,
    abilities: &[],
};
static IMPROVED_HEAVY_BOOTS_MODIFIERS: [Modifier; 3] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.5,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: -0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: FEET,
    },
];
static IMPROVED_HEAVY_BOOTS: Artifact = Artifact {
    id: ArtifactId::ImprovedHeavyBoots,
    name: "Тяжёлые сабатоны II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_BOOTS,
    modifiers: &IMPROVED_HEAVY_BOOTS_MODIFIERS,
    abilities: &[],
};
static IMPROVED_ROBE_HELMET_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 1.0,
    operation: AttributeModifierOperation::AddValue,
    slot: HELMET,
}];
static IMPROVED_ROBE_HELMET: Artifact = Artifact {
    id: ArtifactId::ImprovedRobeHelmet,
    name: "Капюшон мага II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &IMPROVED_ROBE_HELMET_MODIFIERS,
    abilities: &[],
};
static IMPROVED_ROBE_CHESTPLATE_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 4.0,
    operation: AttributeModifierOperation::AddValue,
    slot: CHEST,
}];
static IMPROVED_ROBE_CHESTPLATE: Artifact = Artifact {
    id: ArtifactId::ImprovedRobeChestplate,
    name: "Мантия мага II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &IMPROVED_ROBE_CHESTPLATE_MODIFIERS,
    abilities: &[],
};
static IMPROVED_ROBE_LEGGINGS_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 3.0,
    operation: AttributeModifierOperation::AddValue,
    slot: EquipmentSlotGroup::Legs,
}];
static IMPROVED_ROBE_LEGGINGS: Artifact = Artifact {
    id: ArtifactId::ImprovedRobeLeggings,
    name: "Брюки мага II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &IMPROVED_ROBE_LEGGINGS_MODIFIERS,
    abilities: &[],
};
static IMPROVED_ROBE_BOOTS_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 1.0,
    operation: AttributeModifierOperation::AddValue,
    slot: FEET,
}];
static IMPROVED_ROBE_BOOTS: Artifact = Artifact {
    id: ArtifactId::ImprovedRobeBoots,
    name: "Туфли мага II",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &IMPROVED_ROBE_BOOTS_MODIFIERS,
    abilities: &[],
};
static IMPROVED_HUNTER_SWORD_MODIFIERS: [Modifier; 2] = weapon_modifiers(6.0, 1.6);
static IMPROVED_HUNTER_SWORD: Artifact = Artifact {
    id: ArtifactId::ImprovedHunterSword,
    name: "Стальной меч охотника II",
    rarity: Rarity::Common,
    category: Category::Weapon,
    holder: &vanilla_items::IRON_SWORD,
    modifiers: &IMPROVED_HUNTER_SWORD_MODIFIERS,
    abilities: &[],
};
static IMPROVED_HUNTER_DAGGER_MODIFIERS: [Modifier; 2] = weapon_modifiers(4.0, 2.4);
static IMPROVED_HUNTER_DAGGER: Artifact = Artifact {
    id: ArtifactId::ImprovedHunterDagger,
    name: "Охотничий кинжал II",
    rarity: Rarity::Common,
    category: Category::Weapon,
    holder: &vanilla_items::STONE_SWORD,
    modifiers: &IMPROVED_HUNTER_DAGGER_MODIFIERS,
    abilities: &[],
};
static IMPROVED_HUNTER_AXE_MODIFIERS: [Modifier; 2] = weapon_modifiers(9.0, 0.9);
static IMPROVED_HUNTER_AXE: Artifact = Artifact {
    id: ArtifactId::ImprovedHunterAxe,
    name: "Тяжёлый топор охотника II",
    rarity: Rarity::Common,
    category: Category::Weapon,
    holder: &vanilla_items::IRON_AXE,
    modifiers: &IMPROVED_HUNTER_AXE_MODIFIERS,
    abilities: &[],
};

static HUNTER_SWORD: Artifact = Artifact {
    id: ArtifactId::HunterSword,
    name: "Стальной меч охотника",
    rarity: Rarity::Common,
    category: Category::Weapon,
    holder: &vanilla_items::IRON_SWORD,
    modifiers: &HUNTER_SWORD_MODIFIERS,
    abilities: &[],
};
static HUNTER_DAGGER: Artifact = Artifact {
    id: ArtifactId::HunterDagger,
    name: "Охотничий кинжал",
    rarity: Rarity::Common,
    category: Category::Weapon,
    holder: &vanilla_items::STONE_SWORD,
    modifiers: &HUNTER_DAGGER_MODIFIERS,
    abilities: &[],
};
static HUNTER_AXE: Artifact = Artifact {
    id: ArtifactId::HunterAxe,
    name: "Тяжёлый топор охотника",
    rarity: Rarity::Common,
    category: Category::Weapon,
    holder: &vanilla_items::IRON_AXE,
    modifiers: &HUNTER_AXE_MODIFIERS,
    abilities: &[],
};
static HUNTER_SHIELD: Artifact = Artifact {
    id: ArtifactId::HunterShield,
    name: "Щит охотника",
    rarity: Rarity::Common,
    category: Category::Weapon,
    holder: &vanilla_items::SHIELD,
    modifiers: &[],
    abilities: &[],
};
static HUNTER_HELMET: Artifact = Artifact {
    id: ArtifactId::HunterHelmet,
    name: "Шлем охотника",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::CHAINMAIL_HELMET,
    modifiers: &HUNTER_HELMET_MODIFIERS,
    abilities: &[],
};
static HUNTER_CHESTPLATE: Artifact = Artifact {
    id: ArtifactId::HunterChestplate,
    name: "Нагрудник охотника",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::CHAINMAIL_CHESTPLATE,
    modifiers: &HUNTER_CHESTPLATE_MODIFIERS,
    abilities: &[],
};
static HUNTER_LEGGINGS: Artifact = Artifact {
    id: ArtifactId::HunterLeggings,
    name: "Поножи охотника",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::CHAINMAIL_LEGGINGS,
    modifiers: &HUNTER_LEGGINGS_MODIFIERS,
    abilities: &[],
};
static HUNTER_BOOTS: Artifact = Artifact {
    id: ArtifactId::HunterBoots,
    name: "Ботинки охотника",
    rarity: Rarity::Common,
    category: Category::Armor,
    holder: &vanilla_items::CHAINMAIL_BOOTS,
    modifiers: &HUNTER_BOOTS_MODIFIERS,
    abilities: &[],
};
/// Keeps the holder's vanilla attack and mining stats.
static HUNTER_PICKAXE: Artifact = Artifact {
    id: ArtifactId::HunterPickaxe,
    name: "Кирка охотника",
    rarity: Rarity::Common,
    category: Category::Weapon,
    holder: &vanilla_items::IRON_PICKAXE,
    modifiers: &[],
    abilities: &[],
};

static RAIKANS_FANG_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ATTACK_DAMAGE,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: ATTACK,
    },
    Modifier {
        attribute: vanilla_attributes::ATTACK_SPEED,
        amount: -2.0,
        operation: AttributeModifierOperation::AddValue,
        slot: ATTACK,
    },
];
static RAIKANS_FANG: Artifact = Artifact {
    id: ArtifactId::RaikansFang,
    name: "Клык стального райкана",
    rarity: Rarity::Rare,
    category: Category::Weapon,
    holder: &vanilla_items::IRON_SWORD,
    modifiers: &RAIKANS_FANG_MODIFIERS,
    abilities: &[],
};
static RING_OF_ENDURANCE_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::MAX_HEALTH,
    amount: 2.0,
    operation: AttributeModifierOperation::AddValue,
    slot: EquipmentSlotGroup::Body,
}];
static RING_OF_ENDURANCE: Artifact = Artifact {
    id: ArtifactId::RingOfEndurance,
    name: "Кольцо стойкости",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::GOLD_NUGGET,
    modifiers: &RING_OF_ENDURANCE_MODIFIERS,
    abilities: &[],
};
static WIND_AMULET_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::MOVEMENT_SPEED,
    amount: 0.05,
    operation: AttributeModifierOperation::AddMultipliedBase,
    slot: EquipmentSlotGroup::Body,
}];
static WIND_AMULET: Artifact = Artifact {
    id: ArtifactId::WindAmulet,
    name: "Амулет ветра",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::HEART_OF_THE_SEA,
    modifiers: &WIND_AMULET_MODIFIERS,
    abilities: &[],
};
static DUNGEON_WARDENS_HELMET_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 2.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
    Modifier {
        attribute: vanilla_attributes::ARMOR_TOUGHNESS,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
];
static DUNGEON_WARDENS_HELMET: Artifact = Artifact {
    id: ArtifactId::DungeonWardensHelmet,
    name: "Шлем стража подземелья",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::IRON_HELMET,
    modifiers: &DUNGEON_WARDENS_HELMET_MODIFIERS,
    abilities: &[],
};
static TRACKERS_LEGGINGS_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: EquipmentSlotGroup::Legs,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.03,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: EquipmentSlotGroup::Legs,
    },
];
static TRACKERS_LEGGINGS: Artifact = Artifact {
    id: ArtifactId::TrackersLeggings,
    name: "Поножи следопыта",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &TRACKERS_LEGGINGS_MODIFIERS,
    abilities: &[],
};
static GATEKEEPERS_NECKLACE_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::MOVEMENT_SPEED,
    amount: 0.06,
    operation: AttributeModifierOperation::AddMultipliedBase,
    slot: EquipmentSlotGroup::Body,
}];
static GATEKEEPERS_NECKLACE: Artifact = Artifact {
    id: ArtifactId::GatekeepersNecklace,
    name: "Ожерелье привратника",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::HEART_OF_THE_SEA,
    modifiers: &GATEKEEPERS_NECKLACE_MODIFIERS,
    abilities: &[Ability::SpeedOnHit {
        percent: 0.2,
        seconds: 3,
        cooldown: 12,
    }],
};
static HIGH_MAGICIANS_RING: Artifact = Artifact {
    id: ArtifactId::HighMagiciansRing,
    name: "Кольцо высшего мага",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::GOLD_NUGGET,
    modifiers: &[],
    abilities: &[Ability::MagicFocus { percent: 0.15 }],
};
static KARGALGANS_STAFF_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ATTACK_DAMAGE,
        amount: 2.0,
        operation: AttributeModifierOperation::AddValue,
        slot: ATTACK,
    },
    Modifier {
        attribute: vanilla_attributes::ATTACK_SPEED,
        amount: -2.4,
        operation: AttributeModifierOperation::AddValue,
        slot: ATTACK,
    },
];
static KARGALGANS_STAFF: Artifact = Artifact {
    id: ArtifactId::KargalgansStaff,
    name: "Посох Каргалгана",
    rarity: Rarity::Epic,
    category: Category::Weapon,
    holder: &vanilla_items::BLAZE_ROD,
    modifiers: &KARGALGANS_STAFF_MODIFIERS,
    abilities: &[Ability::MagicFocus { percent: 0.3 }],
};
static HIGH_KNIGHTS_GREAVES_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 4.0,
    operation: AttributeModifierOperation::AddValue,
    slot: EquipmentSlotGroup::Legs,
}];
static HIGH_KNIGHTS_GREAVES: Artifact = Artifact {
    id: ArtifactId::HighKnightsGreaves,
    name: "Поножи высшего рыцаря",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::IRON_LEGGINGS,
    modifiers: &HIGH_KNIGHTS_GREAVES_MODIFIERS,
    abilities: &[Ability::LowHealthReduction {
        threshold: 0.35,
        reduction: 0.1,
        seconds: 4,
        cooldown: 30,
    }],
};
static KAMISHS_WRATH_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ATTACK_DAMAGE,
        amount: 8.0,
        operation: AttributeModifierOperation::AddValue,
        slot: ATTACK,
    },
    Modifier {
        attribute: vanilla_attributes::ATTACK_SPEED,
        amount: -2.0,
        operation: AttributeModifierOperation::AddValue,
        slot: ATTACK,
    },
];
static KAMISHS_WRATH: Artifact = Artifact {
    id: ArtifactId::KamishsWrath,
    name: "Гнев Камиша",
    rarity: Rarity::Legendary,
    category: Category::Weapon,
    holder: &vanilla_items::NETHERITE_SWORD,
    modifiers: &KAMISHS_WRATH_MODIFIERS,
    abilities: &[Ability::StrengthScaling {
        per_point: 0.08,
        maximum: 8.0,
    }],
};
static ORB_OF_AVARICE: Artifact = Artifact {
    id: ArtifactId::OrbOfAvarice,
    name: "Сфера алчности",
    rarity: Rarity::Legendary,
    category: Category::Armor,
    holder: &vanilla_items::ENDER_EYE,
    modifiers: &[],
    abilities: &[Ability::MagicFocus { percent: 0.4 }],
};
static BLACK_HEART_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 4.0,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
    Modifier {
        attribute: vanilla_attributes::MAX_HEALTH,
        amount: 6.0,
        operation: AttributeModifierOperation::AddValue,
        slot: CHEST,
    },
];
static BLACK_HEART: Artifact = Artifact {
    id: ArtifactId::BlackHeart,
    name: "Чёрное сердце",
    rarity: Rarity::Legendary,
    category: Category::Armor,
    holder: &vanilla_items::NETHERITE_CHESTPLATE,
    modifiers: &BLACK_HEART_MODIFIERS,
    abilities: &[Ability::LowHealthReduction {
        threshold: 0.3,
        reduction: 0.25,
        seconds: 5,
        cooldown: 45,
    }],
};
static DEMON_MONARCHS_RING_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::MAX_HEALTH,
    amount: 4.0,
    operation: AttributeModifierOperation::AddValue,
    slot: EquipmentSlotGroup::Body,
}];
static DEMON_MONARCHS_RING: Artifact = Artifact {
    id: ArtifactId::DemonMonarchsRing,
    name: "Кольцо Монарха демонов",
    rarity: Rarity::Legendary,
    category: Category::Armor,
    holder: &vanilla_items::GOLD_NUGGET,
    modifiers: &DEMON_MONARCHS_RING_MODIFIERS,
    abilities: &[Ability::StrengthScaling {
        per_point: 0.03,
        maximum: 3.0,
    }],
};

static ASH_STAFF_MODIFIERS: [Modifier; 2] = weapon_modifiers(2.0, 1.2);
static ASH_STAFF: Artifact = Artifact {
    id: ArtifactId::AshStaff,
    name: "Ясеневый посох",
    rarity: Rarity::Rare,
    category: Category::Weapon,
    holder: &vanilla_items::STICK,
    modifiers: &ASH_STAFF_MODIFIERS,
    abilities: &[],
};
static APPRENTICE_STAFF_MODIFIERS: [Modifier; 2] = weapon_modifiers(1.0, 1.2);
static APPRENTICE_STAFF: Artifact = Artifact {
    id: ArtifactId::ApprenticeStaff,
    name: "Посох ученика",
    rarity: Rarity::Common,
    category: Category::Weapon,
    holder: &vanilla_items::STICK,
    modifiers: &APPRENTICE_STAFF_MODIFIERS,
    abilities: &[],
};
static MANA_RING: Artifact = Artifact {
    id: ArtifactId::ManaRing,
    name: "Кольцо маны",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::GOLD_NUGGET,
    modifiers: &[],
    abilities: &[],
};
static CLEAR_MIND_EARRINGS: Artifact = Artifact {
    id: ArtifactId::ClearMindEarrings,
    name: "Серьги ясного разума",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::HEART_OF_THE_SEA,
    modifiers: &[],
    abilities: &[],
};
static ARCHMAGE_MANTLE_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 4.0,
    operation: AttributeModifierOperation::AddValue,
    slot: EquipmentSlotGroup::Chest,
}];
static ARCHMAGE_MANTLE: Artifact = Artifact {
    id: ArtifactId::ArchmageMantle,
    name: "Мантия архимага",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &ARCHMAGE_MANTLE_MODIFIERS,
    abilities: &[],
};
static HIGH_PRIESTS_BREVIARY: Artifact = Artifact {
    id: ArtifactId::HighPriestsBreviary,
    name: "Требник верховного жреца",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::HEART_OF_THE_SEA,
    modifiers: &[],
    abilities: &[],
};
static ELEMENTAL_ORB: Artifact = Artifact {
    id: ArtifactId::ElementalOrb,
    name: "Сфера стихий",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::ENDER_EYE,
    modifiers: &[],
    abilities: &[],
};
static DEMON_KINGS_STAFF_MODIFIERS: [Modifier; 2] = weapon_modifiers(4.0, 1.2);
static DEMON_KINGS_STAFF: Artifact = Artifact {
    id: ArtifactId::DemonKingsStaff,
    name: "Посох короля демонов",
    rarity: Rarity::Legendary,
    category: Category::Weapon,
    holder: &vanilla_items::BLAZE_ROD,
    modifiers: &DEMON_KINGS_STAFF_MODIFIERS,
    abilities: &[],
};
static CROWN_OF_BOTTOMLESS_MANA_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 2.0,
    operation: AttributeModifierOperation::AddValue,
    slot: EquipmentSlotGroup::Head,
}];
static CROWN_OF_BOTTOMLESS_MANA: Artifact = Artifact {
    id: ArtifactId::CrownOfBottomlessMana,
    name: "Венец бездонной маны",
    rarity: Rarity::Legendary,
    category: Category::Armor,
    holder: &vanilla_items::GOLDEN_HELMET,
    modifiers: &CROWN_OF_BOTTOMLESS_MANA_MODIFIERS,
    abilities: &[],
};

const fn body(attribute: AttributeRef, amount: f64, multiplied: bool) -> Modifier {
    Modifier {
        attribute,
        amount,
        operation: if multiplied {
            AttributeModifierOperation::AddMultipliedBase
        } else {
            AttributeModifierOperation::AddValue
        },
        slot: EquipmentSlotGroup::Body,
    }
}

static AGILE_RING_MODIFIERS: [Modifier; 1] = [body(vanilla_attributes::ATTACK_SPEED, 0.08, true)];
static AGILE_RING: Artifact = Artifact {
    id: ArtifactId::AgileRing,
    name: "Кольцо ловкача",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::GOLD_NUGGET,
    modifiers: &AGILE_RING_MODIFIERS,
    abilities: &[],
};
static STRENGTH_RING_MODIFIERS: [Modifier; 1] =
    [body(vanilla_attributes::ATTACK_DAMAGE, 1.0, false)];
static STRENGTH_RING: Artifact = Artifact {
    id: ArtifactId::StrengthRing,
    name: "Кольцо силы",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::GOLD_NUGGET,
    modifiers: &STRENGTH_RING_MODIFIERS,
    abilities: &[],
};
static WARDEN_AMULET_MODIFIERS: [Modifier; 2] = [
    body(vanilla_attributes::ARMOR, 2.0, false),
    body(vanilla_attributes::KNOCKBACK_RESISTANCE, 0.1, false),
];
static WARDEN_AMULET: Artifact = Artifact {
    id: ArtifactId::WardenAmulet,
    name: "Амулет стража",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::HEART_OF_THE_SEA,
    modifiers: &WARDEN_AMULET_MODIFIERS,
    abilities: &[],
};
static HUNTERS_AMULET_MODIFIERS: [Modifier; 2] = [
    body(vanilla_attributes::MAX_HEALTH, 2.0, false),
    body(vanilla_attributes::MOVEMENT_SPEED, 0.05, true),
];
static HUNTERS_AMULET: Artifact = Artifact {
    id: ArtifactId::HuntersAmulet,
    name: "Амулет охотника",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::HEART_OF_THE_SEA,
    modifiers: &HUNTERS_AMULET_MODIFIERS,
    abilities: &[],
};
static BERSERKER_RING: Artifact = Artifact {
    id: ArtifactId::BerserkerRing,
    name: "Кольцо берсерка",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::GOLD_NUGGET,
    modifiers: &[],
    abilities: &[Ability::Berserk {
        threshold: 0.4,
        percent: 0.25,
    }],
};
static BOSS_HUNTER_RING: Artifact = Artifact {
    id: ArtifactId::BossHunterRing,
    name: "Кольцо охотника на боссов",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::GOLD_NUGGET,
    modifiers: &[],
    abilities: &[Ability::BossSlayer { percent: 0.2 }],
};
static BLOOD_AMULET_MODIFIERS: [Modifier; 1] = [body(vanilla_attributes::MAX_HEALTH, 2.0, false)];
static BLOOD_AMULET: Artifact = Artifact {
    id: ArtifactId::BloodAmulet,
    name: "Амулет крови",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::HEART_OF_THE_SEA,
    modifiers: &BLOOD_AMULET_MODIFIERS,
    abilities: &[Ability::Bloodthirst { percent: 0.06 }],
};
static PROSPECTORS_NECKLACE: Artifact = Artifact {
    id: ArtifactId::ProspectorsNecklace,
    name: "Ожерелье старателя",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::HEART_OF_THE_SEA,
    modifiers: &[],
    abilities: &[
        Ability::CrystalLuck { percent: 20 },
        Ability::Greed { percent: 0.1 },
    ],
};
static ANT_KING_RING_MODIFIERS: [Modifier; 1] = [body(vanilla_attributes::MAX_HEALTH, 4.0, false)];
static ANT_KING_RING: Artifact = Artifact {
    id: ArtifactId::AntKingRing,
    name: "Кольцо Короля муравьёв",
    rarity: Rarity::Legendary,
    category: Category::Armor,
    holder: &vanilla_items::GOLD_NUGGET,
    modifiers: &ANT_KING_RING_MODIFIERS,
    abilities: &[Ability::DoubleStrike {
        every: 4,
        percent: 0.5,
    }],
};
static SHADOW_GUARD_AMULET: Artifact = Artifact {
    id: ArtifactId::ShadowGuardAmulet,
    name: "Амулет теневого стража",
    rarity: Rarity::Legendary,
    category: Category::Armor,
    holder: &vanilla_items::HEART_OF_THE_SEA,
    modifiers: &[],
    abilities: &[Ability::ShadowGuard {
        seconds: 3,
        cooldown: 90,
    }],
};

/// Returns every weapon artifact of `rarity`.
#[must_use]
pub fn weapons_of(rarity: Rarity) -> Vec<ArtifactId> {
    ALL.into_iter()
        .filter(|id| {
            let artifact = id.artifact();
            artifact.category == Category::Weapon && artifact.rarity == rarity
        })
        .collect()
}

/// Returns every armor artifact of `rarity`.
#[must_use]
pub fn armors_of(rarity: Rarity) -> Vec<ArtifactId> {
    ALL.into_iter()
        .filter(|id| {
            let artifact = id.artifact();
            artifact.category == Category::Armor && artifact.rarity == rarity
        })
        .collect()
}

/// Builds the item stack of one artifact.
///
/// A personal stack carries its owner's binding; `owner` is `None` for a
/// transferable instance.
#[must_use]
pub fn stack(id: ArtifactId, owner: Option<Uuid>) -> ItemStack {
    enchanted_stack(id, owner, &[])
}

/// Builds the item stack of one artifact carrying `enchantments`.
///
/// Enchanting rebuilds the stack from the catalog, so the binding is passed
/// back in and survives.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "the lore is one chain of every kind of line an artifact can show"
)]
pub fn enchanted_stack(
    id: ArtifactId,
    owner: Option<Uuid>,
    enchantments: &[Enchantment],
) -> ItemStack {
    let artifact = id.artifact();
    let lines = artifact
        .modifiers
        .iter()
        .map(|modifier| {
            TextComponent::plain(format!(
                "{} {}",
                describe_attribute(modifier.attribute),
                describe_amount(modifier)
            ))
            .color(Color::Gray)
        })
        .chain(artifact.abilities.iter().map(|ability| {
            TextComponent::plain(describe_ability(*ability)).color(Color::LightPurple)
        }))
        .chain(enchantments.iter().map(|enchantment| {
            TextComponent::plain(format!(
                "{}: {}",
                enchantment.display_name(),
                enchantment.describe()
            ))
            .color(if enchantment.is_side_effect() {
                Color::Red
            } else {
                Color::Blue
            })
        }))
        .chain(
            id.spell_gear()
                .into_iter()
                .flat_map(SpellGear::describe)
                .map(|line| TextComponent::plain(line).color(Color::LightPurple)),
        )
        .chain((id.magic_bonus() > 0.0).then(|| {
            TextComponent::plain(format!("Магический урон {:+.0}%", id.magic_bonus() * 100.0))
                .color(Color::Gray)
        }))
        .chain(id.allowed_classes().map(|classes| {
            let names = classes
                .iter()
                .map(|class| class.display_name())
                .collect::<Vec<_>>()
                .join(", ");
            TextComponent::plain(format!("Только для класса: {names}")).color(Color::Yellow)
        }))
        .chain((id.required_strength() > 0).then(|| {
            TextComponent::plain(format!("Требуется сила: {}", id.required_strength()))
                .color(Color::Yellow)
        }))
        .chain([
            TextComponent::plain(format!("Редкость: {}", artifact.rarity.display_name()))
                .color(artifact.rarity.color()),
        ])
        .chain(
            owner
                .is_none()
                .then(|| artifact.rarity.transferable_minimum_rank())
                .flatten()
                .map(|rank| {
                    TextComponent::plain(format!("Без привязки работает с ранга {}", rank.name()))
                        .color(Color::Yellow)
                }),
        )
        .chain((!enchantments.is_empty()).then(|| {
            TextComponent::plain(format!(
                "Очки зачарования: {} / {}",
                enchantment::points_used(enchantments),
                enchantment::budget(artifact.rarity)
            ))
            .color(Color::DarkAqua)
        }))
        .collect::<Vec<_>>();

    let mut stack = named(
        ItemStack::new(artifact.holder),
        TextComponent::plain(artifact.name.to_owned()).color(artifact.rarity.color()),
    );
    stack = with_lore(stack, lines);
    let modifiers = modifiers_component(artifact, enchantments);
    if !modifiers.modifiers.is_empty() {
        stack.set(ATTRIBUTE_MODIFIERS, modifiers);
    }
    if id.is_starter() || id.line().is_some() || id.is_improved() {
        // Serial gear keeps its vanilla look; only the free, bound starter
        // kit never breaks, merchant copies wear like any tool.
        if id.is_starter() && owner.is_some() {
            stack.set(UNBREAKABLE, ());
        }
        if let Some(color) = id.dye() {
            stack.set(DYED_COLOR, DyedItemColor::new(color));
        }
    } else {
        super::model::apply(&mut stack, &format!("artifact/{}", id.name()));
        if artifact.category == Category::Armor {
            super::model::apply_equipment(&mut stack, id.name());
        }
    }
    let mut fields = record(KIND);
    fields.insert(ID, id.name().to_owned());
    fields.insert("rarity", artifact.rarity.name().to_owned());
    if !enchantments.is_empty() {
        fields.insert(ENCHANTMENTS, enchantment::encode(enchantments));
    }
    write_tag(&mut stack, fields);
    if let Some(owner) = owner {
        binding::bind(&mut stack, owner);
    }
    stack
}

/// Returns the artifact a stack holds, or `None` for an ordinary item.
#[must_use]
pub fn of(stack: &ItemStack) -> Option<ArtifactId> {
    let tag = read_kind(stack, KIND)?;
    ArtifactId::from_name(&tag_string(&tag, ID)?)
}

/// Returns the enchantments laid on an artifact stack.
#[must_use]
pub fn enchantments_of(stack: &ItemStack) -> Vec<Enchantment> {
    read_kind(stack, KIND)
        .and_then(|tag| tag_string(&tag, ENCHANTMENTS))
        .map_or_else(Vec::new, |stored| enchantment::decode(&stored))
}

/// Returns the rarity of an artifact stack, falling back to the catalog.
#[must_use]
pub fn rarity_of(stack: &ItemStack) -> Option<Rarity> {
    let id = of(stack)?;
    let tag = read_kind(stack, KIND)?;
    tag_string(&tag, "rarity")
        .and_then(|name| Rarity::from_name(&name))
        .or(Some(id.artifact().rarity))
}

/// Returns the rank a hunter needs to use `stack`, or `None` when anyone can.
#[must_use]
pub fn required_rank(stack: &ItemStack) -> Option<HunterRank> {
    let id = of(stack)?;
    if binding::bound_owner(stack).is_some() {
        return None;
    }
    id.artifact().rarity.transferable_minimum_rank()
}

/// Returns whether a hunter of `rank` may use `stack`.
///
/// The check runs on use and on equip, not only in a shop menu, so a
/// transferable artifact cannot be handed to a lower rank after purchase.
#[must_use]
pub fn usable_by(stack: &ItemStack, rank: HunterRank) -> bool {
    let Some(id) = of(stack) else {
        return true;
    };
    let artifact = id.artifact();
    if binding::bound_owner(stack).is_some() {
        return true;
    }
    artifact
        .rarity
        .transferable_minimum_rank()
        .is_none_or(|minimum| rank >= minimum)
}

/// Returns the artifact a player holds in a given hand, if any.
#[must_use]
pub fn held(stack: &ItemStack) -> Option<&'static Artifact> {
    of(stack).map(ArtifactId::artifact)
}

/// Returns whether `ability` is available on the artifact a stack holds.
#[must_use]
pub fn has_ability(stack: &ItemStack, want: &Ability) -> bool {
    of(stack).is_some_and(|id| {
        id.artifact()
            .abilities
            .iter()
            .any(|ability| ability.cooldown_key() == want.cooldown_key())
    })
}

/// Returns the first ability of `stack` with the given cooldown key.
#[must_use]
pub fn ability_of(stack: &ItemStack, key: &str) -> Option<Ability> {
    let id = of(stack)?;
    id.artifact()
        .abilities
        .iter()
        .copied()
        .find(|ability| ability.cooldown_key() == key)
}

fn modifiers_component(
    artifact: &'static Artifact,
    enchantments: &[Enchantment],
) -> ItemAttributeModifiers {
    let slot = enchantment_slot(artifact);
    let enchanted = enchantments.iter().filter_map(|enchantment| {
        let Effect::Attribute {
            attribute,
            amount,
            operation,
        } = enchantment.effect()
        else {
            return None;
        };
        Some(ItemAttributeModifierEntry {
            attribute,
            id: from_sololeveling(format!(
                "artifact/{}/enchantment/{}",
                artifact.id.name(),
                enchantment.name()
            )),
            amount,
            operation,
            slot,
            display: ItemAttributeModifierDisplay::Default,
        })
    });
    ItemAttributeModifiers {
        modifiers: artifact
            .modifiers
            .iter()
            .map(|modifier| ItemAttributeModifierEntry {
                attribute: modifier.attribute,
                id: attribute_modifier_id(artifact.id, modifier.attribute),
                amount: modifier.amount,
                operation: modifier.operation,
                slot: modifier.slot,
                display: ItemAttributeModifierDisplay::Default,
            })
            .chain(enchanted)
            .collect(),
    }
}

/// Weapons enchant the main hand; armor enchants the slot it is worn in.
fn enchantment_slot(artifact: &Artifact) -> EquipmentSlotGroup {
    match artifact.category {
        Category::Weapon => ATTACK,
        Category::Armor => artifact
            .modifiers
            .iter()
            .map(|modifier| modifier.slot)
            .next()
            .unwrap_or(EquipmentSlotGroup::Armor),
    }
}

fn attribute_modifier_id(artifact: ArtifactId, attribute: AttributeRef) -> Identifier {
    from_sololeveling(format!(
        "artifact/{}/{}",
        artifact.name(),
        attribute.key.path
    ))
}

fn describe_attribute(attribute: AttributeRef) -> &'static str {
    match attribute.key.path.as_ref() {
        "attack_damage" => "Урон атаки",
        "attack_speed" => "Скорость атаки",
        "armor_toughness" => "Прочность брони",
        "armor" => "Броня",
        "movement_speed" => "Скорость",
        "max_health" => "Максимальное здоровье",
        "mining_efficiency" => "Скорость добычи",
        _ => "Характеристика",
    }
}

fn describe_amount(modifier: &Modifier) -> String {
    if modifier.attribute.key == vanilla_attributes::ATTACK_SPEED.key
        && modifier.operation == AttributeModifierOperation::AddValue
    {
        // The offset is relative to the base of 4; hunters read the result.
        return format!(
            "{:.1} в секунду",
            vanilla_attributes::ATTACK_SPEED.default_value + modifier.amount
        );
    }
    match modifier.operation {
        AttributeModifierOperation::AddMultipliedBase
        | AttributeModifierOperation::AddMultipliedTotal => {
            format!("{:+.0}%", modifier.amount * 100.0)
        }
        AttributeModifierOperation::AddValue => format!("{:+.1}", modifier.amount),
    }
}

/// Returns a one-line Russian summary of an ability for tooltips and messages.
#[must_use]
pub fn describe_ability(ability: Ability) -> String {
    match ability {
        Ability::ArmorPiercing { percent } => {
            format!("+{:.0}% урона по цели в броне", percent * 100.0)
        }
        Ability::SpeedOnHit {
            percent,
            seconds,
            cooldown,
        } => format!(
            "Попадание: +{:.0}% скорости на {seconds} с (перезарядка {cooldown} с)",
            percent * 100.0
        ),
        Ability::StrengthScaling { per_point, maximum } => {
            format!("+{per_point:.2} урона за очко силы, максимум +{maximum:.1}")
        }
        Ability::LowHealthReduction {
            threshold,
            reduction,
            seconds,
            cooldown,
        } => format!(
            "Ниже {:.0}% здоровья: -{:.0}% входящего урона {seconds} с (перезарядка {cooldown} с)",
            threshold * 100.0,
            reduction * 100.0
        ),
        Ability::HitCounterShield {
            hits,
            shield_health,
            seconds,
            cooldown,
        } => format!(
            "Каждое {hits}-е попадание: щит {shield_health:.0} HP на {seconds} с (перезарядка {cooldown} с)"
        ),
        Ability::Storm {
            radius,
            seconds,
            damage_per_second,
            cooldown,
        } => format!(
            "Попадание: буря радиусом {radius:.0} блока на {seconds} с, {damage_per_second:.0} урона в секунду (перезарядка {cooldown} с)"
        ),
        Ability::LethalSurvival {
            health,
            shield_health,
            seconds,
            cooldown,
        } => format!(
            "Смертельный удар оставляет {health:.0} HP и даёт щит {shield_health:.0} HP на {seconds} с (перезарядка {cooldown} с)"
        ),
        Ability::CrystalLuck { percent } => {
            format!("+{percent}% шанс дополнительного кристалла")
        }
        Ability::MagicFocus { percent } => {
            format!("+{:.0}% магического урона", percent * 100.0)
        }
        Ability::Berserk { threshold, percent } => format!(
            "Ниже {:.0}% здоровья: +{:.0}% урона и +10% скорости атаки",
            threshold * 100.0,
            percent * 100.0
        ),
        Ability::BossSlayer { percent } => {
            format!("+{:.0}% урона по боссам врат", percent * 100.0)
        }
        Ability::Bloodthirst { percent } => {
            format!("Удар лечит на {:.0}% урона", percent * 100.0)
        }
        Ability::Greed { percent } => {
            format!("+{:.0}% очков Системы за убийства", percent * 100.0)
        }
        Ability::DoubleStrike { every, percent } => format!(
            "Каждое {every}-е попадание бьёт ещё раз на {:.0}% урона",
            percent * 100.0
        ),
        Ability::ShadowGuard { seconds, cooldown } => format!(
            "Смертельный удар оставляет 1 HP и даёт {seconds} с неуязвимости (перезарядка {cooldown} с)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steel_registry::data_components::vanilla_components::LORE;

    #[test]
    fn only_epic_and_above_artifacts_have_abilities() {
        for id in ALL {
            let artifact = id.artifact();
            // Mage artifacts carry their power as spellcasting bonuses.
            if artifact.rarity == Rarity::Rare {
                assert!(artifact.abilities.is_empty(), "{:?}", artifact.id);
            } else {
                assert!(
                    !artifact.abilities.is_empty() || id.spell_gear().is_some(),
                    "{:?}",
                    artifact.id
                );
            }
        }
    }

    #[test]
    fn transferable_rank_gates_start_at_c_and_a() {
        assert_eq!(Rarity::Rare.transferable_minimum_rank(), None);
        assert_eq!(
            Rarity::Epic.transferable_minimum_rank(),
            Some(HunterRank::C)
        );
        assert_eq!(
            Rarity::Legendary.transferable_minimum_rank(),
            Some(HunterRank::A)
        );
    }

    #[test]
    fn a_personal_artifact_ignores_the_transferable_rank_gate() {
        let owner = Uuid::new_v4();
        let personal = stack(ArtifactId::RedKnightsHelmet, Some(owner));
        let transferable = stack(ArtifactId::RedKnightsHelmet, None);
        assert!(usable_by(&personal, HunterRank::E));
        assert!(!usable_by(&transferable, HunterRank::E));
        assert!(usable_by(&transferable, HunterRank::A));
        assert_eq!(required_rank(&transferable), Some(HunterRank::A));
        assert_eq!(required_rank(&personal), None);
        let lore = format!("{:?}", transferable.get(LORE));
        assert!(lore.contains("Без привязки работает с ранга A"), "{lore}");
        let lore = format!("{:?}", personal.get(LORE));
        assert!(!lore.contains("Без привязки"), "{lore}");
    }

    #[test]
    fn enchanting_keeps_the_binding_and_adds_its_modifiers() {
        let owner = Uuid::new_v4();
        let applied = [Enchantment::Protection, Enchantment::Frail];
        let stack = enchanted_stack(ArtifactId::AssassinsShoes, Some(owner), &applied);
        assert_eq!(of(&stack), Some(ArtifactId::AssassinsShoes));
        assert_eq!(binding::bound_owner(&stack), Some(owner));
        assert_eq!(enchantments_of(&stack), applied);
        let modifiers = stack.get(ATTRIBUTE_MODIFIERS).expect("modifiers");
        assert_eq!(
            modifiers.modifiers.len(),
            ASSASSIN_SHOES_MODIFIERS.len() + applied.len()
        );
        assert!(
            modifiers
                .modifiers
                .iter()
                .all(|modifier| modifier.slot == FEET)
        );
    }

    #[test]
    fn stacks_round_trip_through_their_tag() {
        for id in ALL {
            let stack = stack(id, None);
            assert_eq!(of(&stack), Some(id));
            assert_eq!(rarity_of(&stack), Some(id.artifact().rarity));
        }
    }
}
