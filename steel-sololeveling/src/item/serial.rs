//! Serial hunter gear: starter, specialization, improved, elite, supreme and merchant items.
//!
//! Unlike named artifacts, serial items are regular Minecraft equipment. They
//! keep their vanilla holder look, work in vanilla armor slots, and carry a
//! stable `serial` tag instead of an `artifact` tag. Their names, lore,
//! attribute modifiers, dye, binding and enchantments are preserved, but they
//! have no artifact abilities and no rank gate.

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use steel_core::entity::attribute::AttributeModifierOperation;
use steel_registry::attribute::AttributeRef;
use steel_registry::data_components::components::{
    DyedItemColor, ItemAttributeModifierDisplay, ItemAttributeModifierEntry, ItemAttributeModifiers,
};
use steel_registry::data_components::vanilla_components::{
    ATTRIBUTE_MODIFIERS, DYED_COLOR, UNBREAKABLE,
};
use steel_registry::equipment::EquipmentSlotGroup;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::Item;
use steel_registry::{vanilla_attributes, vanilla_items};
use steel_utils::Identifier;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::artifact::{ArmorLine, Category, Modifier, Rarity};
use super::binding;
use super::enchantment::{self, Effect, Enchantment, ItemKind};
use super::material::Quality;
use super::{named, read_kind, record, tag_string, with_lore, write_tag};
use crate::class::HunterClass;
use crate::gear::GearKind;
use crate::identifier::from_sololeveling;
use crate::rank::HunterRank;
use crate::spell::SpellGear;

/// Kind stored in the record of a serial item.
const KIND: &str = "serial";
/// Tag field holding the serial id.
const ID: &str = "id";
/// Tag field holding the comma-separated enchantment list.
const ENCHANTMENTS: &str = "enchantments";

const ATTACK: EquipmentSlotGroup = EquipmentSlotGroup::MainHand;
const HELMET: EquipmentSlotGroup = EquipmentSlotGroup::Head;
const CHEST: EquipmentSlotGroup = EquipmentSlotGroup::Chest;
const LEGS: EquipmentSlotGroup = EquipmentSlotGroup::Legs;
const FEET: EquipmentSlotGroup = EquipmentSlotGroup::Feet;

/// One serial catalog item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SerialId {
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
    /// Elite light specialization helmet.
    EliteLightHelmet,
    /// Elite light specialization chestplate.
    EliteLightChestplate,
    /// Elite light specialization leggings.
    EliteLightLeggings,
    /// Elite light specialization boots.
    EliteLightBoots,
    /// Elite reinforced specialization helmet.
    EliteReinforcedHelmet,
    /// Elite reinforced specialization chestplate.
    EliteReinforcedChestplate,
    /// Elite reinforced specialization leggings.
    EliteReinforcedLeggings,
    /// Elite reinforced specialization boots.
    EliteReinforcedBoots,
    /// Elite heavy specialization helmet.
    EliteHeavyHelmet,
    /// Elite heavy specialization chestplate.
    EliteHeavyChestplate,
    /// Elite heavy specialization leggings.
    EliteHeavyLeggings,
    /// Elite heavy specialization boots.
    EliteHeavyBoots,
    /// Elite robe specialization helmet.
    EliteRobeHelmet,
    /// Elite robe specialization chestplate.
    EliteRobeChestplate,
    /// Elite robe specialization leggings.
    EliteRobeLeggings,
    /// Elite robe specialization boots.
    EliteRobeBoots,
    /// Elite starter-line weapon.
    EliteHunterSword,
    /// Elite starter-line weapon.
    EliteHunterDagger,
    /// Elite starter-line weapon.
    EliteHunterAxe,
    /// Supreme light specialization helmet.
    SupremeLightHelmet,
    /// Supreme light specialization chestplate.
    SupremeLightChestplate,
    /// Supreme light specialization leggings.
    SupremeLightLeggings,
    /// Supreme light specialization boots.
    SupremeLightBoots,
    /// Supreme reinforced specialization helmet.
    SupremeReinforcedHelmet,
    /// Supreme reinforced specialization chestplate.
    SupremeReinforcedChestplate,
    /// Supreme reinforced specialization leggings.
    SupremeReinforcedLeggings,
    /// Supreme reinforced specialization boots.
    SupremeReinforcedBoots,
    /// Supreme heavy specialization helmet.
    SupremeHeavyHelmet,
    /// Supreme heavy specialization chestplate.
    SupremeHeavyChestplate,
    /// Supreme heavy specialization leggings.
    SupremeHeavyLeggings,
    /// Supreme heavy specialization boots.
    SupremeHeavyBoots,
    /// Supreme robe specialization helmet.
    SupremeRobeHelmet,
    /// Supreme robe specialization chestplate.
    SupremeRobeChestplate,
    /// Supreme robe specialization leggings.
    SupremeRobeLeggings,
    /// Supreme robe specialization boots.
    SupremeRobeBoots,
    /// Supreme starter-line weapon.
    SupremeHunterSword,
    /// Supreme starter-line weapon.
    SupremeHunterDagger,
    /// Supreme starter-line weapon.
    SupremeHunterAxe,
    /// Common Mage staff sold by the city merchant.
    ApprenticeStaff,
    /// Common starter bow sold by the city merchant.
    HunterBow,
    /// Common starter crossbow sold by the city merchant.
    HunterCrossbow,
    /// Improved starter-line bow.
    ImprovedHunterBow,
    /// Improved starter-line crossbow.
    ImprovedHunterCrossbow,
    /// Elite starter-line bow.
    EliteHunterBow,
    /// Elite starter-line crossbow.
    EliteHunterCrossbow,
    /// Supreme starter-line bow.
    SupremeHunterBow,
    /// Supreme starter-line crossbow.
    SupremeHunterCrossbow,
}

pub(crate) mod tiered;

/// One of the three crafted serial tiers made to order at the city merchant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CraftedTier {
    /// Tier II: common materials, rank D.
    Improved,
    /// Tier III: reinforced materials, rank C.
    Elite,
    /// Tier IV: superior materials, rank A.
    Supreme,
}

impl CraftedTier {
    /// Returns the lowest hunter rank that may order this tier.
    #[must_use]
    pub const fn rank(self) -> HunterRank {
        match self {
            Self::Improved => HunterRank::D,
            Self::Elite => HunterRank::C,
            Self::Supreme => HunterRank::A,
        }
    }

    /// Returns the material quality consumed by this tier.
    #[must_use]
    pub const fn quality(self) -> Quality {
        match self {
            Self::Improved => Quality::Common,
            Self::Elite => Quality::Reinforced,
            Self::Supreme => Quality::Superior,
        }
    }

    /// Returns the Russian tier name shown in the order menu.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Improved => "Улучшенная",
            Self::Elite => "Элитная",
            Self::Supreme => "Высшая",
        }
    }

    /// Multiplier applied to the improved tier's work fee.
    #[must_use]
    pub const fn work_multiplier(self) -> u64 {
        match self {
            Self::Improved => 1,
            Self::Elite => 2,
            Self::Supreme => 4,
        }
    }

    /// Multiplier applied to the improved tier's base material counts.
    #[must_use]
    pub const fn material_multiplier(self) -> f64 {
        match self {
            Self::Improved => 1.0,
            Self::Elite => 1.5,
            Self::Supreme => 2.0,
        }
    }

    /// Number of stable essences each recipe of this tier needs.
    #[must_use]
    pub const fn essence(self) -> u32 {
        match self {
            Self::Improved => 1,
            Self::Elite => 2,
            Self::Supreme => 3,
        }
    }
}

/// Bonus to outgoing magic damage of one elite mage-clothing piece.
pub const ELITE_ROBE_MAGIC_BONUS_PER_PIECE: f32 = 0.06;
/// Bonus to outgoing magic damage of one supreme mage-clothing piece.
pub const SUPREME_ROBE_MAGIC_BONUS_PER_PIECE: f32 = 0.08;
/// Strength elite heavy armor needs before its bonuses apply.
pub const ELITE_HEAVY_STRENGTH: u32 = 30;
/// Strength supreme heavy armor needs before its bonuses apply.
pub const SUPREME_HEAVY_STRENGTH: u32 = 40;

impl SerialId {
    /// Returns the stable tag value of the serial item.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
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
            Self::EliteLightHelmet => "elite_light_helmet",
            Self::EliteLightChestplate => "elite_light_chestplate",
            Self::EliteLightLeggings => "elite_light_leggings",
            Self::EliteLightBoots => "elite_light_boots",
            Self::EliteReinforcedHelmet => "elite_reinforced_helmet",
            Self::EliteReinforcedChestplate => "elite_reinforced_chestplate",
            Self::EliteReinforcedLeggings => "elite_reinforced_leggings",
            Self::EliteReinforcedBoots => "elite_reinforced_boots",
            Self::EliteHeavyHelmet => "elite_heavy_helmet",
            Self::EliteHeavyChestplate => "elite_heavy_chestplate",
            Self::EliteHeavyLeggings => "elite_heavy_leggings",
            Self::EliteHeavyBoots => "elite_heavy_boots",
            Self::EliteRobeHelmet => "elite_robe_helmet",
            Self::EliteRobeChestplate => "elite_robe_chestplate",
            Self::EliteRobeLeggings => "elite_robe_leggings",
            Self::EliteRobeBoots => "elite_robe_boots",
            Self::EliteHunterSword => "elite_hunter_sword",
            Self::EliteHunterDagger => "elite_hunter_dagger",
            Self::EliteHunterAxe => "elite_hunter_axe",
            Self::SupremeLightHelmet => "supreme_light_helmet",
            Self::SupremeLightChestplate => "supreme_light_chestplate",
            Self::SupremeLightLeggings => "supreme_light_leggings",
            Self::SupremeLightBoots => "supreme_light_boots",
            Self::SupremeReinforcedHelmet => "supreme_reinforced_helmet",
            Self::SupremeReinforcedChestplate => "supreme_reinforced_chestplate",
            Self::SupremeReinforcedLeggings => "supreme_reinforced_leggings",
            Self::SupremeReinforcedBoots => "supreme_reinforced_boots",
            Self::SupremeHeavyHelmet => "supreme_heavy_helmet",
            Self::SupremeHeavyChestplate => "supreme_heavy_chestplate",
            Self::SupremeHeavyLeggings => "supreme_heavy_leggings",
            Self::SupremeHeavyBoots => "supreme_heavy_boots",
            Self::SupremeRobeHelmet => "supreme_robe_helmet",
            Self::SupremeRobeChestplate => "supreme_robe_chestplate",
            Self::SupremeRobeLeggings => "supreme_robe_leggings",
            Self::SupremeRobeBoots => "supreme_robe_boots",
            Self::SupremeHunterSword => "supreme_hunter_sword",
            Self::SupremeHunterDagger => "supreme_hunter_dagger",
            Self::SupremeHunterAxe => "supreme_hunter_axe",
            Self::ApprenticeStaff => "apprentice_staff",
            Self::HunterBow => "hunter_bow",
            Self::HunterCrossbow => "hunter_crossbow",
            Self::ImprovedHunterBow => "improved_hunter_bow",
            Self::ImprovedHunterCrossbow => "improved_hunter_crossbow",
            Self::EliteHunterBow => "elite_hunter_bow",
            Self::EliteHunterCrossbow => "elite_hunter_crossbow",
            Self::SupremeHunterBow => "supreme_hunter_bow",
            Self::SupremeHunterCrossbow => "supreme_hunter_crossbow",
        }
    }

    /// Parses the stable tag value.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        ALL.into_iter().find(|serial| serial.name() == name)
    }

    /// Returns the catalog entry of this serial item.
    #[must_use]
    pub fn serial(self) -> &'static Serial {
        match self {
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
            Self::EliteLightHelmet => &tiered::ELITE_LIGHT_HELMET,
            Self::EliteLightChestplate => &tiered::ELITE_LIGHT_CHESTPLATE,
            Self::EliteLightLeggings => &tiered::ELITE_LIGHT_LEGGINGS,
            Self::EliteLightBoots => &tiered::ELITE_LIGHT_BOOTS,
            Self::EliteReinforcedHelmet => &tiered::ELITE_REINFORCED_HELMET,
            Self::EliteReinforcedChestplate => &tiered::ELITE_REINFORCED_CHESTPLATE,
            Self::EliteReinforcedLeggings => &tiered::ELITE_REINFORCED_LEGGINGS,
            Self::EliteReinforcedBoots => &tiered::ELITE_REINFORCED_BOOTS,
            Self::EliteHeavyHelmet => &tiered::ELITE_HEAVY_HELMET,
            Self::EliteHeavyChestplate => &tiered::ELITE_HEAVY_CHESTPLATE,
            Self::EliteHeavyLeggings => &tiered::ELITE_HEAVY_LEGGINGS,
            Self::EliteHeavyBoots => &tiered::ELITE_HEAVY_BOOTS,
            Self::EliteRobeHelmet => &tiered::ELITE_ROBE_HELMET,
            Self::EliteRobeChestplate => &tiered::ELITE_ROBE_CHESTPLATE,
            Self::EliteRobeLeggings => &tiered::ELITE_ROBE_LEGGINGS,
            Self::EliteRobeBoots => &tiered::ELITE_ROBE_BOOTS,
            Self::EliteHunterSword => &tiered::ELITE_HUNTER_SWORD,
            Self::EliteHunterDagger => &tiered::ELITE_HUNTER_DAGGER,
            Self::EliteHunterAxe => &tiered::ELITE_HUNTER_AXE,
            Self::SupremeLightHelmet => &tiered::SUPREME_LIGHT_HELMET,
            Self::SupremeLightChestplate => &tiered::SUPREME_LIGHT_CHESTPLATE,
            Self::SupremeLightLeggings => &tiered::SUPREME_LIGHT_LEGGINGS,
            Self::SupremeLightBoots => &tiered::SUPREME_LIGHT_BOOTS,
            Self::SupremeReinforcedHelmet => &tiered::SUPREME_REINFORCED_HELMET,
            Self::SupremeReinforcedChestplate => &tiered::SUPREME_REINFORCED_CHESTPLATE,
            Self::SupremeReinforcedLeggings => &tiered::SUPREME_REINFORCED_LEGGINGS,
            Self::SupremeReinforcedBoots => &tiered::SUPREME_REINFORCED_BOOTS,
            Self::SupremeHeavyHelmet => &tiered::SUPREME_HEAVY_HELMET,
            Self::SupremeHeavyChestplate => &tiered::SUPREME_HEAVY_CHESTPLATE,
            Self::SupremeHeavyLeggings => &tiered::SUPREME_HEAVY_LEGGINGS,
            Self::SupremeHeavyBoots => &tiered::SUPREME_HEAVY_BOOTS,
            Self::SupremeRobeHelmet => &tiered::SUPREME_ROBE_HELMET,
            Self::SupremeRobeChestplate => &tiered::SUPREME_ROBE_CHESTPLATE,
            Self::SupremeRobeLeggings => &tiered::SUPREME_ROBE_LEGGINGS,
            Self::SupremeRobeBoots => &tiered::SUPREME_ROBE_BOOTS,
            Self::SupremeHunterSword => &tiered::SUPREME_HUNTER_SWORD,
            Self::SupremeHunterDagger => &tiered::SUPREME_HUNTER_DAGGER,
            Self::SupremeHunterAxe => &tiered::SUPREME_HUNTER_AXE,
            Self::ApprenticeStaff => &APPRENTICE_STAFF,
            Self::HunterBow => &HUNTER_BOW,
            Self::HunterCrossbow => &HUNTER_CROSSBOW,
            Self::ImprovedHunterBow => &IMPROVED_HUNTER_BOW,
            Self::ImprovedHunterCrossbow => &IMPROVED_HUNTER_CROSSBOW,
            Self::EliteHunterBow => &tiered::ELITE_HUNTER_BOW,
            Self::EliteHunterCrossbow => &tiered::ELITE_HUNTER_CROSSBOW,
            Self::SupremeHunterBow => &tiered::SUPREME_HUNTER_BOW,
            Self::SupremeHunterCrossbow => &tiered::SUPREME_HUNTER_CROSSBOW,
        }
    }

    /// Returns the System equipment slot this item goes into, or `None` for
    /// weapons and tools, which are held instead.
    #[must_use]
    pub fn gear_kind(self) -> Option<GearKind> {
        if let Some(index) = SPECIALIZATION.iter().position(|id| *id == self) {
            return Some(match index % 4 {
                0 => GearKind::Head,
                1 => GearKind::Chest,
                2 => GearKind::Legs,
                _ => GearKind::Feet,
            });
        }
        if let Some((_, index)) = self.tier_index() {
            return (index < 16).then_some(match index % 4 {
                0 => GearKind::Head,
                1 => GearKind::Chest,
                2 => GearKind::Legs,
                _ => GearKind::Feet,
            });
        }
        match self {
            Self::HunterHelmet => Some(GearKind::Head),
            Self::HunterChestplate => Some(GearKind::Chest),
            Self::HunterLeggings => Some(GearKind::Legs),
            Self::HunterBoots => Some(GearKind::Feet),
            _ => None,
        }
    }

    /// Returns the position of this piece inside its crafted tier array, if it
    /// belongs to one.
    pub(crate) fn tier_index(self) -> Option<(CraftedTier, usize)> {
        if let Some(index) = IMPROVED.iter().position(|id| *id == self) {
            return Some((CraftedTier::Improved, index));
        }
        if let Some(index) = tiered::ELITE.iter().position(|id| *id == self) {
            return Some((CraftedTier::Elite, index));
        }
        if let Some(index) = tiered::SUPREME.iter().position(|id| *id == self) {
            return Some((CraftedTier::Supreme, index));
        }
        None
    }

    /// Returns the specialization armor line of this piece, if any.
    ///
    /// [`SPECIALIZATION`], [`IMPROVED`], [`tiered::ELITE`] and
    /// [`tiered::SUPREME`] list each line's four pieces in [`ArmorLine::ALL`]
    /// order, so the position decides the line.
    #[must_use]
    pub fn line(self) -> Option<ArmorLine> {
        let index = SPECIALIZATION
            .iter()
            .position(|id| *id == self)
            .or_else(|| self.tier_index().map(|(_, index)| index))?;
        ArmorLine::ALL.get(index / 4).copied()
    }

    /// Returns the classes that may wear this piece; `None` means everyone.
    #[must_use]
    pub fn allowed_classes(self) -> Option<&'static [HunterClass]> {
        if self.spell_gear().is_some() && matches!(self.line(), Some(ArmorLine::Robe)) {
            return Some(&[HunterClass::Mage]);
        }
        self.line().map(ArmorLine::classes)
    }

    /// Returns the type of item the item maker enchants this serial as;
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
                Self::HunterPickaxe => ItemKind::Pickaxe,
                Self::HunterBow
                | Self::HunterCrossbow
                | Self::ImprovedHunterBow
                | Self::ImprovedHunterCrossbow
                | Self::EliteHunterBow
                | Self::EliteHunterCrossbow
                | Self::SupremeHunterBow
                | Self::SupremeHunterCrossbow => ItemKind::Bow,
                _ if self.spell_gear().is_some() => ItemKind::Staff,
                _ => ItemKind::Melee,
            },
        };
        Some(kind)
    }

    /// Returns the spellcasting bonuses of a Mage serial item; `None` for
    /// everything a non-Mage may use.
    #[must_use]
    pub fn spell_gear(self) -> Option<SpellGear> {
        let none = SpellGear::default();
        let gear = match self {
            Self::ApprenticeStaff => SpellGear {
                magic: 0.10,
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
            Some(ArmorLine::Robe) => match self.crafted_tier() {
                Some(CraftedTier::Improved) => IMPROVED_ROBE_MAGIC_BONUS_PER_PIECE,
                Some(CraftedTier::Elite) => ELITE_ROBE_MAGIC_BONUS_PER_PIECE,
                Some(CraftedTier::Supreme) => SUPREME_ROBE_MAGIC_BONUS_PER_PIECE,
                None => ROBE_MAGIC_BONUS_PER_PIECE,
            },
            _ => 0.0,
        }
    }

    /// Returns whether this is an improved serial piece, made to order.
    #[must_use]
    pub fn is_improved(self) -> bool {
        IMPROVED.contains(&self)
    }

    /// Returns the crafted tier this serial belongs to, if any.
    #[must_use]
    pub fn crafted_tier(self) -> Option<CraftedTier> {
        self.tier_index().map(|(tier, _)| tier)
    }

    /// Returns whether this is a piece of the Association starter kit.
    #[must_use]
    pub fn is_starter(self) -> bool {
        STARTER.contains(&self)
    }

    /// Returns the Strength this piece needs before its bonuses apply.
    #[must_use]
    pub fn required_strength(self) -> u32 {
        if self.line() == Some(ArmorLine::Heavy) {
            match self.crafted_tier() {
                Some(CraftedTier::Improved) => IMPROVED_HEAVY_STRENGTH,
                Some(CraftedTier::Elite) => ELITE_HEAVY_STRENGTH,
                Some(CraftedTier::Supreme) => SUPREME_HEAVY_STRENGTH,
                None => 0,
            }
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

/// Improved serial gear made to order from dungeon materials: four pieces
/// per armor line in [`ArmorLine::ALL`] order, then the weapons.
pub const IMPROVED: [SerialId; 21] = [
    SerialId::ImprovedLightHelmet,
    SerialId::ImprovedLightChestplate,
    SerialId::ImprovedLightLeggings,
    SerialId::ImprovedLightBoots,
    SerialId::ImprovedReinforcedHelmet,
    SerialId::ImprovedReinforcedChestplate,
    SerialId::ImprovedReinforcedLeggings,
    SerialId::ImprovedReinforcedBoots,
    SerialId::ImprovedHeavyHelmet,
    SerialId::ImprovedHeavyChestplate,
    SerialId::ImprovedHeavyLeggings,
    SerialId::ImprovedHeavyBoots,
    SerialId::ImprovedRobeHelmet,
    SerialId::ImprovedRobeChestplate,
    SerialId::ImprovedRobeLeggings,
    SerialId::ImprovedRobeBoots,
    SerialId::ImprovedHunterSword,
    SerialId::ImprovedHunterDagger,
    SerialId::ImprovedHunterAxe,
    SerialId::ImprovedHunterBow,
    SerialId::ImprovedHunterCrossbow,
];

/// Specialization armor sold by the city merchant, four pieces per line.
pub const SPECIALIZATION: [SerialId; 16] = [
    SerialId::LightHelmet,
    SerialId::LightChestplate,
    SerialId::LightLeggings,
    SerialId::LightBoots,
    SerialId::ReinforcedHelmet,
    SerialId::ReinforcedChestplate,
    SerialId::ReinforcedLeggings,
    SerialId::ReinforcedBoots,
    SerialId::HeavyHelmet,
    SerialId::HeavyChestplate,
    SerialId::HeavyLeggings,
    SerialId::HeavyBoots,
    SerialId::RobeHelmet,
    SerialId::RobeChestplate,
    SerialId::RobeLeggings,
    SerialId::RobeBoots,
];

/// The Association's Common starter equipment.
pub const STARTER: [SerialId; 9] = [
    SerialId::HunterSword,
    SerialId::HunterDagger,
    SerialId::HunterAxe,
    SerialId::HunterShield,
    SerialId::HunterHelmet,
    SerialId::HunterChestplate,
    SerialId::HunterLeggings,
    SerialId::HunterBoots,
    SerialId::HunterPickaxe,
];

/// Starter weapons a hunter picks one of.
pub const STARTER_WEAPONS: [SerialId; 3] = [
    SerialId::HunterSword,
    SerialId::HunterDagger,
    SerialId::HunterAxe,
];

/// Starter pieces every hunter receives besides the chosen weapon.
pub const STARTER_OUTFIT: [SerialId; 6] = [
    SerialId::HunterShield,
    SerialId::HunterHelmet,
    SerialId::HunterChestplate,
    SerialId::HunterLeggings,
    SerialId::HunterBoots,
    SerialId::HunterPickaxe,
];

/// Mage serial items sold by the city merchant.
pub const MAGE_SHOP: [SerialId; 1] = [SerialId::ApprenticeStaff];

/// Ranged starter-line weapons always sold by the city merchant.
pub const WEAPON_SHOP: [SerialId; 2] = [SerialId::HunterBow, SerialId::HunterCrossbow];

/// Every serial item.
pub const ALL: [SerialId; 91] = {
    let all = [
        SerialId::HunterSword,
        SerialId::HunterDagger,
        SerialId::HunterAxe,
        SerialId::HunterShield,
        SerialId::HunterHelmet,
        SerialId::HunterChestplate,
        SerialId::HunterLeggings,
        SerialId::HunterBoots,
        SerialId::HunterPickaxe,
        SerialId::LightHelmet,
        SerialId::LightChestplate,
        SerialId::LightLeggings,
        SerialId::LightBoots,
        SerialId::ReinforcedHelmet,
        SerialId::ReinforcedChestplate,
        SerialId::ReinforcedLeggings,
        SerialId::ReinforcedBoots,
        SerialId::HeavyHelmet,
        SerialId::HeavyChestplate,
        SerialId::HeavyLeggings,
        SerialId::HeavyBoots,
        SerialId::RobeHelmet,
        SerialId::RobeChestplate,
        SerialId::RobeLeggings,
        SerialId::RobeBoots,
        SerialId::ImprovedLightHelmet,
        SerialId::ImprovedLightChestplate,
        SerialId::ImprovedLightLeggings,
        SerialId::ImprovedLightBoots,
        SerialId::ImprovedReinforcedHelmet,
        SerialId::ImprovedReinforcedChestplate,
        SerialId::ImprovedReinforcedLeggings,
        SerialId::ImprovedReinforcedBoots,
        SerialId::ImprovedHeavyHelmet,
        SerialId::ImprovedHeavyChestplate,
        SerialId::ImprovedHeavyLeggings,
        SerialId::ImprovedHeavyBoots,
        SerialId::ImprovedRobeHelmet,
        SerialId::ImprovedRobeChestplate,
        SerialId::ImprovedRobeLeggings,
        SerialId::ImprovedRobeBoots,
        SerialId::ImprovedHunterSword,
        SerialId::ImprovedHunterDagger,
        SerialId::ImprovedHunterAxe,
        SerialId::EliteLightHelmet,
        SerialId::EliteLightChestplate,
        SerialId::EliteLightLeggings,
        SerialId::EliteLightBoots,
        SerialId::EliteReinforcedHelmet,
        SerialId::EliteReinforcedChestplate,
        SerialId::EliteReinforcedLeggings,
        SerialId::EliteReinforcedBoots,
        SerialId::EliteHeavyHelmet,
        SerialId::EliteHeavyChestplate,
        SerialId::EliteHeavyLeggings,
        SerialId::EliteHeavyBoots,
        SerialId::EliteRobeHelmet,
        SerialId::EliteRobeChestplate,
        SerialId::EliteRobeLeggings,
        SerialId::EliteRobeBoots,
        SerialId::EliteHunterSword,
        SerialId::EliteHunterDagger,
        SerialId::EliteHunterAxe,
        SerialId::SupremeLightHelmet,
        SerialId::SupremeLightChestplate,
        SerialId::SupremeLightLeggings,
        SerialId::SupremeLightBoots,
        SerialId::SupremeReinforcedHelmet,
        SerialId::SupremeReinforcedChestplate,
        SerialId::SupremeReinforcedLeggings,
        SerialId::SupremeReinforcedBoots,
        SerialId::SupremeHeavyHelmet,
        SerialId::SupremeHeavyChestplate,
        SerialId::SupremeHeavyLeggings,
        SerialId::SupremeHeavyBoots,
        SerialId::SupremeRobeHelmet,
        SerialId::SupremeRobeChestplate,
        SerialId::SupremeRobeLeggings,
        SerialId::SupremeRobeBoots,
        SerialId::SupremeHunterSword,
        SerialId::SupremeHunterDagger,
        SerialId::SupremeHunterAxe,
        SerialId::ApprenticeStaff,
        SerialId::HunterBow,
        SerialId::HunterCrossbow,
        SerialId::ImprovedHunterBow,
        SerialId::ImprovedHunterCrossbow,
        SerialId::EliteHunterBow,
        SerialId::EliteHunterCrossbow,
        SerialId::SupremeHunterBow,
        SerialId::SupremeHunterCrossbow,
    ];
    // Const assertions are not stable for enum array lengths, so keep the
    // literal size in sync with the actual item count manually.
    all
};

/// One serial catalog entry.
#[derive(Debug)]
pub struct Serial {
    /// Catalog identity.
    pub id: SerialId,
    /// Russian display name.
    pub name: &'static str,
    /// Which roll list the item belongs to.
    pub category: Category,
    /// Vanilla item the serial is built from.
    pub holder: &'static LazyLock<Item>,
    /// Attribute bonuses granted while worn.
    pub modifiers: &'static [Modifier],
    /// System gear slot, if any.
    pub gear_kind: Option<GearKind>,
    /// Specialization armor line, if any.
    pub line: Option<ArmorLine>,
    /// Spellcasting bonuses for Mage items.
    pub spell_gear: Option<SpellGear>,
}

/// A held serial weapon: flat damage plus the vanilla attack-speed offset.
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

const fn light_modifiers(armor: f64, speed: f64, slot: EquipmentSlotGroup) -> [Modifier; 2] {
    [
        Modifier {
            attribute: vanilla_attributes::ARMOR,
            amount: armor,
            operation: AttributeModifierOperation::AddValue,
            slot,
        },
        Modifier {
            attribute: vanilla_attributes::MOVEMENT_SPEED,
            amount: speed,
            operation: AttributeModifierOperation::AddMultipliedBase,
            slot,
        },
    ]
}

const fn reinforced_modifiers(armor: f64, slot: EquipmentSlotGroup) -> [Modifier; 2] {
    [
        Modifier {
            attribute: vanilla_attributes::ARMOR,
            amount: armor,
            operation: AttributeModifierOperation::AddValue,
            slot,
        },
        Modifier {
            attribute: vanilla_attributes::ARMOR_TOUGHNESS,
            amount: 0.5,
            operation: AttributeModifierOperation::AddValue,
            slot,
        },
    ]
}

const fn heavy_modifiers(armor: f64, slot: EquipmentSlotGroup) -> [Modifier; 3] {
    [
        Modifier {
            attribute: vanilla_attributes::ARMOR,
            amount: armor,
            operation: AttributeModifierOperation::AddValue,
            slot,
        },
        Modifier {
            attribute: vanilla_attributes::ARMOR_TOUGHNESS,
            amount: 1.0,
            operation: AttributeModifierOperation::AddValue,
            slot,
        },
        Modifier {
            attribute: vanilla_attributes::MOVEMENT_SPEED,
            amount: -0.03,
            operation: AttributeModifierOperation::AddMultipliedBase,
            slot,
        },
    ]
}

const fn improved_reinforced_modifiers(armor: f64, slot: EquipmentSlotGroup) -> [Modifier; 2] {
    [
        Modifier {
            attribute: vanilla_attributes::ARMOR,
            amount: armor,
            operation: AttributeModifierOperation::AddValue,
            slot,
        },
        Modifier {
            attribute: vanilla_attributes::ARMOR_TOUGHNESS,
            amount: 1.0,
            operation: AttributeModifierOperation::AddValue,
            slot,
        },
    ]
}

const fn improved_heavy_modifiers(armor: f64, slot: EquipmentSlotGroup) -> [Modifier; 3] {
    [
        Modifier {
            attribute: vanilla_attributes::ARMOR,
            amount: armor,
            operation: AttributeModifierOperation::AddValue,
            slot,
        },
        Modifier {
            attribute: vanilla_attributes::ARMOR_TOUGHNESS,
            amount: 1.5,
            operation: AttributeModifierOperation::AddValue,
            slot,
        },
        Modifier {
            attribute: vanilla_attributes::MOVEMENT_SPEED,
            amount: -0.03,
            operation: AttributeModifierOperation::AddMultipliedBase,
            slot,
        },
    ]
}

// Starter numbers mirror an iron set and iron weapons: 15 armor in total, a
// 6-damage sword at 1.6 attacks per second, a quicker and lighter dagger,
// and a slow, heavy axe.
static HUNTER_SWORD_MODIFIERS: [Modifier; 2] = weapon_modifiers(5.0, 1.6);
static HUNTER_DAGGER_MODIFIERS: [Modifier; 2] = weapon_modifiers(3.0, 2.4);
static HUNTER_AXE_MODIFIERS: [Modifier; 2] = weapon_modifiers(8.0, 0.9);
static HUNTER_HELMET_MODIFIERS: [Modifier; 1] = armor_modifier(2.0, HELMET);
static HUNTER_CHESTPLATE_MODIFIERS: [Modifier; 1] = armor_modifier(6.0, CHEST);
static HUNTER_LEGGINGS_MODIFIERS: [Modifier; 1] = armor_modifier(5.0, LEGS);
static HUNTER_BOOTS_MODIFIERS: [Modifier; 1] = armor_modifier(2.0, FEET);

static LIGHTHELMET_MODIFIERS: [Modifier; 2] = light_modifiers(1.0, 0.02, HELMET);
static LIGHTCHESTPLATE_MODIFIERS: [Modifier; 2] = light_modifiers(4.0, 0.03, CHEST);
static LIGHTLEGGINGS_MODIFIERS: [Modifier; 2] = light_modifiers(3.0, 0.03, LEGS);
static LIGHTBOOTS_MODIFIERS: [Modifier; 2] = light_modifiers(1.0, 0.02, FEET);

static REINFORCEDHELMET_MODIFIERS: [Modifier; 2] = reinforced_modifiers(2.0, HELMET);
static REINFORCEDCHESTPLATE_MODIFIERS: [Modifier; 2] = reinforced_modifiers(6.0, CHEST);
static REINFORCEDLEGGINGS_MODIFIERS: [Modifier; 2] = reinforced_modifiers(5.0, LEGS);
static REINFORCEDBOOTS_MODIFIERS: [Modifier; 2] = reinforced_modifiers(2.0, FEET);

static HEAVYHELMET_MODIFIERS: [Modifier; 3] = heavy_modifiers(3.0, HELMET);
static HEAVYCHESTPLATE_MODIFIERS: [Modifier; 3] = heavy_modifiers(7.0, CHEST);
static HEAVYLEGGINGS_MODIFIERS: [Modifier; 3] = heavy_modifiers(6.0, LEGS);
static HEAVYBOOTS_MODIFIERS: [Modifier; 3] = heavy_modifiers(3.0, FEET);

static ROBEHELMET_MODIFIERS: [Modifier; 1] = armor_modifier(1.0, HELMET);
static ROBECHESTPLATE_MODIFIERS: [Modifier; 1] = armor_modifier(3.0, CHEST);
static ROBELEGGINGS_MODIFIERS: [Modifier; 1] = armor_modifier(2.0, LEGS);
static ROBEBOOTS_MODIFIERS: [Modifier; 1] = armor_modifier(1.0, FEET);

static IMPROVED_LIGHT_HELMET_MODIFIERS: [Modifier; 2] = light_modifiers(2.0, 0.03, HELMET);
static IMPROVED_LIGHT_CHESTPLATE_MODIFIERS: [Modifier; 2] = light_modifiers(5.0, 0.04, CHEST);
static IMPROVED_LIGHT_LEGGINGS_MODIFIERS: [Modifier; 2] = light_modifiers(4.0, 0.04, LEGS);
static IMPROVED_LIGHT_BOOTS_MODIFIERS: [Modifier; 2] = light_modifiers(1.0, 0.03, FEET);

static IMPROVED_REINFORCED_HELMET_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(3.0, HELMET);
static IMPROVED_REINFORCED_CHESTPLATE_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(7.0, CHEST);
static IMPROVED_REINFORCED_LEGGINGS_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(6.0, LEGS);
static IMPROVED_REINFORCED_BOOTS_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(2.0, FEET);

static IMPROVED_HEAVY_HELMET_MODIFIERS: [Modifier; 3] = improved_heavy_modifiers(3.0, HELMET);
static IMPROVED_HEAVY_CHESTPLATE_MODIFIERS: [Modifier; 3] = improved_heavy_modifiers(8.0, CHEST);
static IMPROVED_HEAVY_LEGGINGS_MODIFIERS: [Modifier; 3] = improved_heavy_modifiers(6.0, LEGS);
static IMPROVED_HEAVY_BOOTS_MODIFIERS: [Modifier; 3] = improved_heavy_modifiers(3.0, FEET);

static IMPROVED_ROBE_HELMET_MODIFIERS: [Modifier; 1] = armor_modifier(1.0, HELMET);
static IMPROVED_ROBE_CHESTPLATE_MODIFIERS: [Modifier; 1] = armor_modifier(4.0, CHEST);
static IMPROVED_ROBE_LEGGINGS_MODIFIERS: [Modifier; 1] = armor_modifier(3.0, LEGS);
static IMPROVED_ROBE_BOOTS_MODIFIERS: [Modifier; 1] = armor_modifier(1.0, FEET);

static IMPROVED_HUNTER_SWORD_MODIFIERS: [Modifier; 2] = weapon_modifiers(6.0, 1.6);
static IMPROVED_HUNTER_DAGGER_MODIFIERS: [Modifier; 2] = weapon_modifiers(4.0, 2.4);
static IMPROVED_HUNTER_AXE_MODIFIERS: [Modifier; 2] = weapon_modifiers(9.0, 0.9);
static IMPROVED_HUNTER_BOW_MODIFIERS: [Modifier; 2] = weapon_modifiers(6.0, 1.6);
static IMPROVED_HUNTER_CROSSBOW_MODIFIERS: [Modifier; 2] = weapon_modifiers(9.0, 0.9);

static APPRENTICE_STAFF_MODIFIERS: [Modifier; 2] = weapon_modifiers(1.0, 1.2);

// Ranged starter numbers mirror the melee table: the bow follows the sword
// line, the crossbow the heavy axe line.
static HUNTER_BOW_MODIFIERS: [Modifier; 2] = weapon_modifiers(5.0, 1.6);
static HUNTER_CROSSBOW_MODIFIERS: [Modifier; 2] = weapon_modifiers(8.0, 0.9);

static HUNTER_SWORD: Serial = Serial {
    id: SerialId::HunterSword,
    name: "Стальной меч охотника",
    category: Category::Weapon,
    holder: &vanilla_items::IRON_SWORD,
    modifiers: &HUNTER_SWORD_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
static HUNTER_DAGGER: Serial = Serial {
    id: SerialId::HunterDagger,
    name: "Охотничий кинжал",
    category: Category::Weapon,
    holder: &vanilla_items::STONE_SWORD,
    modifiers: &HUNTER_DAGGER_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
static HUNTER_AXE: Serial = Serial {
    id: SerialId::HunterAxe,
    name: "Тяжёлый топор охотника",
    category: Category::Weapon,
    holder: &vanilla_items::IRON_AXE,
    modifiers: &HUNTER_AXE_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
static HUNTER_SHIELD: Serial = Serial {
    id: SerialId::HunterShield,
    name: "Щит охотника",
    category: Category::Weapon,
    holder: &vanilla_items::SHIELD,
    modifiers: &[],
    gear_kind: None,
    line: None,
    spell_gear: None,
};
static HUNTER_HELMET: Serial = Serial {
    id: SerialId::HunterHelmet,
    name: "Шлем охотника",
    category: Category::Armor,
    holder: &vanilla_items::CHAINMAIL_HELMET,
    modifiers: &HUNTER_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: None,
    spell_gear: None,
};
static HUNTER_CHESTPLATE: Serial = Serial {
    id: SerialId::HunterChestplate,
    name: "Нагрудник охотника",
    category: Category::Armor,
    holder: &vanilla_items::CHAINMAIL_CHESTPLATE,
    modifiers: &HUNTER_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: None,
    spell_gear: None,
};
static HUNTER_LEGGINGS: Serial = Serial {
    id: SerialId::HunterLeggings,
    name: "Поножи охотника",
    category: Category::Armor,
    holder: &vanilla_items::CHAINMAIL_LEGGINGS,
    modifiers: &HUNTER_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: None,
    spell_gear: None,
};
static HUNTER_BOOTS: Serial = Serial {
    id: SerialId::HunterBoots,
    name: "Ботинки охотника",
    category: Category::Armor,
    holder: &vanilla_items::CHAINMAIL_BOOTS,
    modifiers: &HUNTER_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: None,
    spell_gear: None,
};
/// Keeps the holder's vanilla attack and mining stats.
static HUNTER_PICKAXE: Serial = Serial {
    id: SerialId::HunterPickaxe,
    name: "Кирка охотника",
    category: Category::Weapon,
    holder: &vanilla_items::IRON_PICKAXE,
    modifiers: &[],
    gear_kind: None,
    line: None,
    spell_gear: None,
};

static LIGHTHELMET: Serial = Serial {
    id: SerialId::LightHelmet,
    name: "Лёгкий капюшон",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &LIGHTHELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
static LIGHTCHESTPLATE: Serial = Serial {
    id: SerialId::LightChestplate,
    name: "Лёгкая куртка",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &LIGHTCHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
static LIGHTLEGGINGS: Serial = Serial {
    id: SerialId::LightLeggings,
    name: "Лёгкие штаны",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &LIGHTLEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
static LIGHTBOOTS: Serial = Serial {
    id: SerialId::LightBoots,
    name: "Лёгкие сапоги",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &LIGHTBOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};

static REINFORCEDHELMET: Serial = Serial {
    id: SerialId::ReinforcedHelmet,
    name: "Усиленный шлем",
    category: Category::Armor,
    holder: &vanilla_items::IRON_HELMET,
    modifiers: &REINFORCEDHELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
static REINFORCEDCHESTPLATE: Serial = Serial {
    id: SerialId::ReinforcedChestplate,
    name: "Усиленный нагрудник",
    category: Category::Armor,
    holder: &vanilla_items::IRON_CHESTPLATE,
    modifiers: &REINFORCEDCHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
static REINFORCEDLEGGINGS: Serial = Serial {
    id: SerialId::ReinforcedLeggings,
    name: "Усиленные поножи",
    category: Category::Armor,
    holder: &vanilla_items::IRON_LEGGINGS,
    modifiers: &REINFORCEDLEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
static REINFORCEDBOOTS: Serial = Serial {
    id: SerialId::ReinforcedBoots,
    name: "Усиленные сапоги",
    category: Category::Armor,
    holder: &vanilla_items::IRON_BOOTS,
    modifiers: &REINFORCEDBOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};

static HEAVYHELMET: Serial = Serial {
    id: SerialId::HeavyHelmet,
    name: "Тяжёлый шлем",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_HELMET,
    modifiers: &HEAVYHELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
static HEAVYCHESTPLATE: Serial = Serial {
    id: SerialId::HeavyChestplate,
    name: "Тяжёлый нагрудник",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_CHESTPLATE,
    modifiers: &HEAVYCHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
static HEAVYLEGGINGS: Serial = Serial {
    id: SerialId::HeavyLeggings,
    name: "Тяжёлые поножи",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_LEGGINGS,
    modifiers: &HEAVYLEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
static HEAVYBOOTS: Serial = Serial {
    id: SerialId::HeavyBoots,
    name: "Тяжёлые сабатоны",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_BOOTS,
    modifiers: &HEAVYBOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};

static ROBEHELMET: Serial = Serial {
    id: SerialId::RobeHelmet,
    name: "Капюшон мага",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &ROBEHELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
static ROBECHESTPLATE: Serial = Serial {
    id: SerialId::RobeChestplate,
    name: "Мантия мага",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &ROBECHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
static ROBELEGGINGS: Serial = Serial {
    id: SerialId::RobeLeggings,
    name: "Брюки мага",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &ROBELEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
static ROBEBOOTS: Serial = Serial {
    id: SerialId::RobeBoots,
    name: "Туфли мага",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &ROBEBOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};

static IMPROVED_LIGHT_HELMET: Serial = Serial {
    id: SerialId::ImprovedLightHelmet,
    name: "Лёгкий капюшон II",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &IMPROVED_LIGHT_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
static IMPROVED_LIGHT_CHESTPLATE: Serial = Serial {
    id: SerialId::ImprovedLightChestplate,
    name: "Лёгкая куртка II",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &IMPROVED_LIGHT_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
static IMPROVED_LIGHT_LEGGINGS: Serial = Serial {
    id: SerialId::ImprovedLightLeggings,
    name: "Лёгкие штаны II",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &IMPROVED_LIGHT_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
static IMPROVED_LIGHT_BOOTS: Serial = Serial {
    id: SerialId::ImprovedLightBoots,
    name: "Лёгкие сапоги II",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &IMPROVED_LIGHT_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};

static IMPROVED_REINFORCED_HELMET: Serial = Serial {
    id: SerialId::ImprovedReinforcedHelmet,
    name: "Усиленный шлем II",
    category: Category::Armor,
    holder: &vanilla_items::IRON_HELMET,
    modifiers: &IMPROVED_REINFORCED_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
static IMPROVED_REINFORCED_CHESTPLATE: Serial = Serial {
    id: SerialId::ImprovedReinforcedChestplate,
    name: "Усиленный нагрудник II",
    category: Category::Armor,
    holder: &vanilla_items::IRON_CHESTPLATE,
    modifiers: &IMPROVED_REINFORCED_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
static IMPROVED_REINFORCED_LEGGINGS: Serial = Serial {
    id: SerialId::ImprovedReinforcedLeggings,
    name: "Усиленные поножи II",
    category: Category::Armor,
    holder: &vanilla_items::IRON_LEGGINGS,
    modifiers: &IMPROVED_REINFORCED_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
static IMPROVED_REINFORCED_BOOTS: Serial = Serial {
    id: SerialId::ImprovedReinforcedBoots,
    name: "Усиленные сапоги II",
    category: Category::Armor,
    holder: &vanilla_items::IRON_BOOTS,
    modifiers: &IMPROVED_REINFORCED_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};

static IMPROVED_HEAVY_HELMET: Serial = Serial {
    id: SerialId::ImprovedHeavyHelmet,
    name: "Тяжёлый шлем II",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_HELMET,
    modifiers: &IMPROVED_HEAVY_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
static IMPROVED_HEAVY_CHESTPLATE: Serial = Serial {
    id: SerialId::ImprovedHeavyChestplate,
    name: "Тяжёлый нагрудник II",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_CHESTPLATE,
    modifiers: &IMPROVED_HEAVY_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
static IMPROVED_HEAVY_LEGGINGS: Serial = Serial {
    id: SerialId::ImprovedHeavyLeggings,
    name: "Тяжёлые поножи II",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_LEGGINGS,
    modifiers: &IMPROVED_HEAVY_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
static IMPROVED_HEAVY_BOOTS: Serial = Serial {
    id: SerialId::ImprovedHeavyBoots,
    name: "Тяжёлые сабатоны II",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_BOOTS,
    modifiers: &IMPROVED_HEAVY_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};

static IMPROVED_ROBE_HELMET: Serial = Serial {
    id: SerialId::ImprovedRobeHelmet,
    name: "Капюшон мага II",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &IMPROVED_ROBE_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
static IMPROVED_ROBE_CHESTPLATE: Serial = Serial {
    id: SerialId::ImprovedRobeChestplate,
    name: "Мантия мага II",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &IMPROVED_ROBE_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
static IMPROVED_ROBE_LEGGINGS: Serial = Serial {
    id: SerialId::ImprovedRobeLeggings,
    name: "Брюки мага II",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &IMPROVED_ROBE_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
static IMPROVED_ROBE_BOOTS: Serial = Serial {
    id: SerialId::ImprovedRobeBoots,
    name: "Туфли мага II",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &IMPROVED_ROBE_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};

static IMPROVED_HUNTER_SWORD: Serial = Serial {
    id: SerialId::ImprovedHunterSword,
    name: "Стальной меч охотника II",
    category: Category::Weapon,
    holder: &vanilla_items::IRON_SWORD,
    modifiers: &IMPROVED_HUNTER_SWORD_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
static IMPROVED_HUNTER_DAGGER: Serial = Serial {
    id: SerialId::ImprovedHunterDagger,
    name: "Охотничий кинжал II",
    category: Category::Weapon,
    holder: &vanilla_items::STONE_SWORD,
    modifiers: &IMPROVED_HUNTER_DAGGER_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
static IMPROVED_HUNTER_AXE: Serial = Serial {
    id: SerialId::ImprovedHunterAxe,
    name: "Тяжёлый топор охотника II",
    category: Category::Weapon,
    holder: &vanilla_items::IRON_AXE,
    modifiers: &IMPROVED_HUNTER_AXE_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
static IMPROVED_HUNTER_BOW: Serial = Serial {
    id: SerialId::ImprovedHunterBow,
    name: "Охотничий лук II",
    category: Category::Weapon,
    holder: &vanilla_items::BOW,
    modifiers: &IMPROVED_HUNTER_BOW_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
static IMPROVED_HUNTER_CROSSBOW: Serial = Serial {
    id: SerialId::ImprovedHunterCrossbow,
    name: "Арбалет охотника II",
    category: Category::Weapon,
    holder: &vanilla_items::CROSSBOW,
    modifiers: &IMPROVED_HUNTER_CROSSBOW_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};

static APPRENTICE_STAFF: Serial = Serial {
    id: SerialId::ApprenticeStaff,
    name: "Посох ученика",
    category: Category::Weapon,
    holder: &vanilla_items::STICK,
    modifiers: &APPRENTICE_STAFF_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: Some(SpellGear {
        magic: 0.10,
        mana: 0.0,
        regen: 0.0,
        cost: 0.0,
        destruction_cost: 0.0,
        cooldown: 0.0,
        healing: 0.0,
        barrier: 0.0,
        free_every: 0,
        echo_percent: 0,
        burning: false,
        free_meditation: false,
    }),
};

static HUNTER_BOW: Serial = Serial {
    id: SerialId::HunterBow,
    name: "Охотничий лук",
    category: Category::Weapon,
    holder: &vanilla_items::BOW,
    modifiers: &HUNTER_BOW_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
static HUNTER_CROSSBOW: Serial = Serial {
    id: SerialId::HunterCrossbow,
    name: "Арбалет охотника",
    category: Category::Weapon,
    holder: &vanilla_items::CROSSBOW,
    modifiers: &HUNTER_CROSSBOW_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};

/// Builds the item stack of one serial item.
///
/// A personal stack carries its owner's binding; `owner` is `None` for a
/// transferable instance.
#[must_use]
pub fn stack(id: SerialId, owner: Option<Uuid>) -> ItemStack {
    enchanted_stack(id, owner, &[])
}

/// Builds the item stack of one serial item carrying `enchantments`.
///
/// Enchanting rebuilds the stack from the catalog, so the binding is passed
/// back in and survives.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "the lore is one chain of every kind of line a serial item can show"
)]
pub fn enchanted_stack(
    id: SerialId,
    owner: Option<Uuid>,
    enchantments: &[Enchantment],
) -> ItemStack {
    let serial = id.serial();
    let lines = serial
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
            TextComponent::plain(format!("Редкость: {}", Rarity::Common.display_name()))
                .color(Rarity::Common.color()),
        ])
        .chain((!enchantments.is_empty()).then(|| {
            TextComponent::plain(format!(
                "Очки зачарования: {} / {}",
                enchantment::points_used(enchantments),
                enchantment::budget(Rarity::Common)
            ))
            .color(Color::DarkAqua)
        }))
        .collect::<Vec<_>>();

    let mut stack = named(
        ItemStack::new(serial.holder),
        TextComponent::plain(serial.name.to_owned()).color(Rarity::Common.color()),
    );
    stack = with_lore(stack, lines);
    let modifiers = modifiers_component(serial, enchantments);
    if !modifiers.modifiers.is_empty() {
        stack.set(ATTRIBUTE_MODIFIERS, modifiers);
    }
    // Serial gear keeps its vanilla look; only the free, bound starter kit
    // never breaks, merchant copies wear like any tool.
    if id.is_starter() && owner.is_some() {
        stack.set(UNBREAKABLE, ());
    }
    if let Some(color) = id.dye() {
        stack.set(DYED_COLOR, DyedItemColor::new(color));
    }
    let mut fields = record(KIND);
    fields.insert(ID, id.name().to_owned());
    if !enchantments.is_empty() {
        fields.insert(ENCHANTMENTS, enchantment::encode(enchantments));
    }
    write_tag(&mut stack, fields);
    if let Some(owner) = owner {
        binding::bind(&mut stack, owner);
    }
    stack
}

/// Returns the serial item a stack holds, or `None` for an ordinary item.
#[must_use]
pub fn of(stack: &ItemStack) -> Option<SerialId> {
    let tag = read_kind(stack, KIND)?;
    SerialId::from_name(&tag_string(&tag, ID)?)
}

/// Returns the enchantments laid on a serial stack.
#[must_use]
pub fn enchantments_of(stack: &ItemStack) -> Vec<Enchantment> {
    read_kind(stack, KIND)
        .and_then(|tag| tag_string(&tag, ENCHANTMENTS))
        .map_or_else(Vec::new, |stored| enchantment::decode(&stored))
}

/// Returns the serial catalog entry a stack holds, if any.
#[must_use]
pub fn held(stack: &ItemStack) -> Option<&'static Serial> {
    of(stack).map(SerialId::serial)
}

/// Returns the rarity of a serial stack, which is always Common.
#[must_use]
pub fn rarity_of(_stack: &ItemStack) -> Option<Rarity> {
    Some(Rarity::Common)
}

/// Returns whether a hunter of `rank` may use `stack`.
///
/// Serial items have no rank gate.
#[must_use]
pub fn usable_by(_stack: &ItemStack, _rank: crate::rank::HunterRank) -> bool {
    true
}

fn modifiers_component(serial: &Serial, enchantments: &[Enchantment]) -> ItemAttributeModifiers {
    let slot = enchantment_slot(serial);
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
                "serial/{}/enchantment/{}",
                serial.id.name(),
                enchantment.name()
            )),
            amount,
            operation,
            slot,
            display: ItemAttributeModifierDisplay::Default,
        })
    });
    ItemAttributeModifiers {
        modifiers: serial
            .modifiers
            .iter()
            .map(|modifier| ItemAttributeModifierEntry {
                attribute: modifier.attribute,
                id: attribute_modifier_id(serial.id, modifier.attribute),
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
fn enchantment_slot(serial: &Serial) -> EquipmentSlotGroup {
    match serial.category {
        Category::Weapon => ATTACK,
        Category::Armor => serial
            .modifiers
            .iter()
            .map(|modifier| modifier.slot)
            .next()
            .unwrap_or(EquipmentSlotGroup::Armor),
    }
}

fn attribute_modifier_id(id: SerialId, attribute: AttributeRef) -> Identifier {
    from_sololeveling(format!("serial/{}/{}", id.name(), attribute.key.path))
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

#[cfg(test)]
mod tests {
    use super::*;
    use steel_registry::data_components::vanilla_components::{LORE, UNBREAKABLE};
    use steel_registry::init_vanilla_registry;
    use uuid::Uuid;

    #[test]
    fn serial_items_are_not_artifacts() {
        init_vanilla_registry();
        for id in ALL {
            let stack = stack(id, None);
            assert!(of(&stack).is_some(), "{id:?} should decode");
            assert!(
                crate::item::artifact::of(&stack).is_none(),
                "{id:?} must not be an artifact"
            );
        }
    }

    #[test]
    fn stacks_round_trip_through_their_tag() {
        init_vanilla_registry();
        for id in ALL {
            let stack = stack(id, None);
            assert_eq!(of(&stack), Some(id));
            assert_eq!(rarity_of(&stack), Some(Rarity::Common));
        }
    }

    #[test]
    fn enchanting_keeps_the_binding_and_adds_its_modifiers() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let applied = [Enchantment::Protection, Enchantment::Frail];
        let stack = enchanted_stack(SerialId::HunterBoots, Some(owner), &applied);
        assert_eq!(of(&stack), Some(SerialId::HunterBoots));
        assert_eq!(binding::bound_owner(&stack), Some(owner));
        assert_eq!(enchantments_of(&stack), applied);
        let modifiers = stack.get(ATTRIBUTE_MODIFIERS).expect("modifiers");
        assert_eq!(
            modifiers.modifiers.len(),
            HUNTER_BOOTS_MODIFIERS.len() + applied.len()
        );
        assert!(
            modifiers
                .modifiers
                .iter()
                .all(|modifier| modifier.slot == FEET)
        );
    }

    #[test]
    fn starter_kit_is_bound_unbreakable() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        for id in STARTER {
            let stack = stack(id, Some(owner));
            assert_eq!(binding::bound_owner(&stack), Some(owner));
            assert!(stack.has(UNBREAKABLE), "{id:?} should be unbreakable");
            let lore = format!("{:?}", stack.get(LORE));
            assert!(
                !lore.contains("Без привязки"),
                "{id:?} should not show rank gate"
            );
        }
    }

    #[test]
    fn robe_armor_has_magic_bonus_and_class_restriction() {
        init_vanilla_registry();
        assert!(SerialId::RobeChestplate.magic_bonus() > 0.0);
        assert_eq!(
            SerialId::RobeChestplate.allowed_classes(),
            Some(&[HunterClass::Mage][..])
        );
        assert_eq!(
            SerialId::ImprovedRobeChestplate.magic_bonus(),
            IMPROVED_ROBE_MAGIC_BONUS_PER_PIECE
        );
    }

    #[test]
    fn improved_heavy_needs_strength() {
        init_vanilla_registry();
        assert_eq!(
            SerialId::ImprovedHeavyChestplate.required_strength(),
            IMPROVED_HEAVY_STRENGTH
        );
        assert_eq!(SerialId::HeavyChestplate.required_strength(), 0);
    }

    #[test]
    fn serial_stacks_are_not_artifacts() {
        init_vanilla_registry();
        for id in ALL {
            let stack = stack(id, None);
            assert!(crate::item::artifact::of(&stack).is_none(), "{id:?}");
        }
    }

    #[test]
    fn elite_and_supreme_names_round_trip() {
        init_vanilla_registry();
        for id in tiered::ELITE.into_iter().chain(tiered::SUPREME) {
            assert_eq!(SerialId::from_name(id.name()), Some(id), "{id:?}");
            let stack = stack(id, None);
            assert_eq!(of(&stack), Some(id));
        }
    }

    #[test]
    fn elite_and_supreme_are_crafted_tiers() {
        for id in tiered::ELITE {
            assert_eq!(id.crafted_tier(), Some(CraftedTier::Elite));
        }
        for id in tiered::SUPREME {
            assert_eq!(id.crafted_tier(), Some(CraftedTier::Supreme));
        }
    }

    #[test]
    fn elite_and_supreme_armor_beats_improved() {
        init_vanilla_registry();
        fn armor(id: SerialId) -> f64 {
            id.serial()
                .modifiers
                .iter()
                .find(|modifier| modifier.attribute.key == vanilla_attributes::ARMOR.key)
                .map(|modifier| modifier.amount)
                .unwrap_or(0.0)
        }
        for index in 0..16 {
            let improved = armor(IMPROVED[index]);
            let elite = armor(tiered::ELITE[index]);
            let supreme = armor(tiered::SUPREME[index]);
            assert!(improved < elite, "index {index}");
            assert!(elite < supreme, "index {index}");
        }
    }

    #[test]
    fn elite_and_supreme_weapons_deal_more_damage() {
        init_vanilla_registry();
        fn damage(id: SerialId) -> f64 {
            id.serial()
                .modifiers
                .iter()
                .find(|modifier| modifier.attribute.key == vanilla_attributes::ATTACK_DAMAGE.key)
                .map(|modifier| modifier.amount)
                .unwrap_or(0.0)
        }
        for index in 16..21 {
            let improved = damage(IMPROVED[index]);
            let elite = damage(tiered::ELITE[index]);
            let supreme = damage(tiered::SUPREME[index]);
            assert!(improved < elite, "index {index}");
            assert!(elite < supreme, "index {index}");
        }
    }

    #[test]
    fn ranged_serials_use_bow_holders_and_enchant_as_bows() {
        init_vanilla_registry();
        for id in [
            SerialId::HunterBow,
            SerialId::ImprovedHunterBow,
            SerialId::EliteHunterBow,
            SerialId::SupremeHunterBow,
            SerialId::HunterCrossbow,
            SerialId::ImprovedHunterCrossbow,
            SerialId::EliteHunterCrossbow,
            SerialId::SupremeHunterCrossbow,
        ] {
            assert_eq!(id.enchant_kind(), Some(ItemKind::Bow), "{id:?}");
            assert!(id.gear_kind().is_none(), "{id:?} is held, not worn");
            assert_eq!(id.line(), None, "{id:?} is no armor line");
            let stack = stack(id, None);
            assert_eq!(of(&stack), Some(id));
        }
    }

    #[test]
    fn a_bow_enchants_like_a_weapon_and_keeps_its_enchantments() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        // Both enchantments are attribute effects, so each adds one modifier.
        let applied = [Enchantment::Frail, Enchantment::Sluggish];
        for id in [SerialId::HunterBow, SerialId::SupremeHunterCrossbow] {
            let stack = enchanted_stack(id, Some(owner), &applied);
            assert_eq!(of(&stack), Some(id));
            assert_eq!(binding::bound_owner(&stack), Some(owner));
            assert_eq!(enchantments_of(&stack), applied);
            let modifiers = stack.get(ATTRIBUTE_MODIFIERS).expect("modifiers");
            assert_eq!(
                modifiers.modifiers.len(),
                id.serial().modifiers.len() + applied.len()
            );
            assert!(
                modifiers
                    .modifiers
                    .iter()
                    .all(|modifier| modifier.slot == ATTACK)
            );
        }
    }

    #[test]
    fn bow_powers_and_drawbacks_fit_the_bow_kind() {
        assert!(Enchantment::Fury.fits(ItemKind::Bow));
        assert!(Enchantment::Hunter.fits(ItemKind::Bow));
        assert!(Enchantment::Sluggish.fits(ItemKind::Bow));
        assert!(!Enchantment::Sharpness.fits(ItemKind::Bow));
        assert!(!Enchantment::Lifesteal.fits(ItemKind::Bow));
        assert!(!Enchantment::Spellpower.fits(ItemKind::Bow));
    }

    #[test]
    fn robe_magic_bonus_and_heavy_strength_grow_by_tier() {
        init_vanilla_registry();
        assert!(
            SerialId::ImprovedRobeChestplate.magic_bonus()
                < SerialId::EliteRobeChestplate.magic_bonus()
        );
        assert!(
            SerialId::EliteRobeChestplate.magic_bonus()
                < SerialId::SupremeRobeChestplate.magic_bonus()
        );
        assert!(
            SerialId::ImprovedHeavyChestplate.required_strength()
                < SerialId::EliteHeavyChestplate.required_strength()
        );
        assert!(
            SerialId::EliteHeavyChestplate.required_strength()
                < SerialId::SupremeHeavyChestplate.required_strength()
        );
    }
}
