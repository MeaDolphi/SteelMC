//! Crafting orders: improved, elite and supreme serial gear made from dungeon
//! materials.
//!
//! Every recipe is fixed: no failure chance, no timer. The materials are
//! consumed and the master's work is paid in Won in the same transaction
//! that hands out the new item. Access follows the hunter's rank, never a
//! count of finished raids, and owning bought materials does not bypass it.
//! Only transferable materials are accepted, so the result is transferable.
//!
//! Tier table (all values relative to the improved tier):
//!
//! | Tier     | Rank | Material quality | Armor multiplier | Toughness multiplier |
//! |----------|------|------------------|------------------|----------------------|
//! | Improved | D    | Common           | 1.0              | 1.0                  |
//! | Elite    | C    | Reinforced       | 1.4              | 1.4                  |
//! | Supreme  | A    | Superior         | 2.0              | 2.0                  |
//!
//! Weapon damage uses the same multipliers; attack speed is Elite ×1.05 and
//! Supreme ×1.1. Material counts are multiplied by 1.5 (Elite) and 2.0
//! (Supreme), rounded up; essence is 1/2/3 and the work fee is ×1/×2/×4.

use steel_registry::item_stack::ItemStack;

use crate::item::artifact::ArmorLine;
use crate::item::binding;
use crate::item::material::{self, Material, Quality};
use crate::item::serial::{self, CraftedTier, SerialId};
use crate::rank::HunterRank;

/// Lowest rank that may order the improved tier.
pub const IMPROVED_RANK: HunterRank = HunterRank::D;
/// Quality of the materials the improved tier is made from.
pub const IMPROVED_QUALITY: Quality = Quality::Common;
/// Lowest rank that may order the elite tier.
pub const ELITE_RANK: HunterRank = HunterRank::C;
/// Quality of the materials the elite tier is made from.
pub const ELITE_QUALITY: Quality = Quality::Reinforced;
/// Lowest rank that may order the supreme tier.
pub const SUPREME_RANK: HunterRank = HunterRank::A;
/// Quality of the materials the supreme tier is made from.
pub const SUPREME_QUALITY: Quality = Quality::Superior;

/// One fixed crafting recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    /// The item made.
    pub output: SerialId,
    /// Materials consumed, all of [`Recipe::quality`].
    pub materials: Vec<(Material, u32)>,
    /// Quality every material must have.
    pub quality: Quality,
    /// Won paid for the master's work.
    pub work: u64,
    /// Lowest rank allowed to place the order.
    pub rank: HunterRank,
}

/// Returns the primary and secondary material of an armor line.
const fn line_materials(line: ArmorLine) -> (Material, Material) {
    match line {
        ArmorLine::Light => (Material::ToughHide, Material::ManaFiber),
        ArmorLine::Reinforced => (Material::MagicMetal, Material::ToughHide),
        ArmorLine::Heavy => (Material::MagicMetal, Material::ChitinPlate),
        ArmorLine::Robe => (Material::ManaFiber, Material::ToughHide),
    }
}

/// Returns the recipe that makes `output`, if it is made to order.
#[must_use]
pub fn recipe(output: SerialId) -> Option<Recipe> {
    let (tier, index) = output.tier_index()?;
    let multiplier = tier.material_multiplier();
    let (materials, work) = if let Some(line) = output.line() {
        let (primary, secondary) = line_materials(line);
        // Pieces follow the four-per-line catalog order: helmet, chestplate,
        // leggings, boots.
        let piece = index % 4;
        let (main, side, base_work) = match piece {
            1 => (8, 4, 400),
            2 => (6, 3, 350),
            _ => (4, 2, 200),
        };
        (
            vec![
                (primary, (f64::from(main) * multiplier).ceil() as u32),
                (secondary, (f64::from(side) * multiplier).ceil() as u32),
            ],
            u64::try_from(base_work).expect("work fee is positive") * tier.work_multiplier(),
        )
    } else {
        (
            vec![
                (Material::MagicMetal, (6.0 * multiplier).ceil() as u32),
                (Material::ToughHide, (2.0 * multiplier).ceil() as u32),
            ],
            350_u64 * tier.work_multiplier(),
        )
    };
    let mut materials = materials;
    materials.push((Material::StableEssence, tier.essence()));
    Some(Recipe {
        output,
        materials,
        quality: tier.quality(),
        work,
        rank: tier.rank(),
    })
}

/// Every recipe, in catalog order.
#[must_use]
pub fn all() -> Vec<Recipe> {
    [
        serial::IMPROVED.as_slice(),
        serial::tiered::ELITE.as_slice(),
        serial::tiered::SUPREME.as_slice(),
    ]
    .into_iter()
    .flat_map(|tier| tier.iter().filter_map(|&id| recipe(id)))
    .collect()
}

/// Every recipe of one crafted tier, in catalog order.
#[must_use]
pub fn tier_recipes(tier: CraftedTier) -> Vec<Recipe> {
    let ids = match tier {
        CraftedTier::Improved => &serial::IMPROVED,
        CraftedTier::Elite => &serial::tiered::ELITE,
        CraftedTier::Supreme => &serial::tiered::SUPREME,
    };
    ids.iter().filter_map(|&id| recipe(id)).collect()
}

/// Counts the transferable units of each recipe material in `slots`.
#[must_use]
pub fn available(recipe: &Recipe, slots: &[ItemStack]) -> Vec<u32> {
    recipe
        .materials
        .iter()
        .map(|(wanted, _)| {
            slots
                .iter()
                .filter(|stack| {
                    binding::bound_owner(stack).is_none()
                        && material::of(stack) == Some((*wanted, recipe.quality))
                })
                .map(|stack| u32::try_from(stack.count).unwrap_or(0))
                .sum()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        ELITE_QUALITY, ELITE_RANK, SUPREME_QUALITY, SUPREME_RANK, all, recipe, tier_recipes,
    };
    use crate::item::material::{Material, Quality};
    use crate::item::serial::{CraftedTier, SerialId, tiered};

    #[test]
    fn every_crafted_item_has_one_recipe_with_essence() {
        assert_eq!(all().len(), 57);
        for recipe in all() {
            let expected_essence = match recipe.output.crafted_tier() {
                Some(CraftedTier::Improved) => 1,
                Some(CraftedTier::Elite) => 2,
                Some(CraftedTier::Supreme) => 3,
                None => panic!("recipe must belong to a crafted tier"),
            };
            assert!(
                recipe
                    .materials
                    .iter()
                    .any(|(material, count)| *material == Material::StableEssence
                        && *count == expected_essence)
            );
            assert!(recipe.work > 0);
        }
        assert_eq!(recipe(SerialId::HunterSword), None);
    }

    #[test]
    fn a_chestplate_costs_more_than_boots() {
        let chest = recipe(SerialId::ImprovedHeavyChestplate).expect("recipe");
        let boots = recipe(SerialId::ImprovedHeavyBoots).expect("recipe");
        assert!(chest.work > boots.work);
        assert_eq!(chest.materials[0], (Material::MagicMetal, 8));
        assert_eq!(chest.materials[1], (Material::ChitinPlate, 4));
    }

    #[test]
    fn elite_and_supreme_recipes_use_higher_quality_and_rank() {
        let elite = recipe(SerialId::EliteLightHelmet).expect("recipe");
        assert_eq!(elite.quality, ELITE_QUALITY);
        assert_eq!(elite.rank, ELITE_RANK);
        let supreme = recipe(SerialId::SupremeLightHelmet).expect("recipe");
        assert_eq!(supreme.quality, SUPREME_QUALITY);
        assert_eq!(supreme.rank, SUPREME_RANK);
    }

    #[test]
    fn every_new_tier_item_has_a_recipe() {
        for id in tiered::ELITE.into_iter().chain(tiered::SUPREME) {
            assert!(recipe(id).is_some(), "{id:?} has no recipe");
        }
    }

    #[test]
    fn costs_grow_with_tier_and_piece() {
        let improved = recipe(SerialId::ImprovedHeavyChestplate).expect("recipe");
        let elite = recipe(SerialId::EliteHeavyChestplate).expect("recipe");
        let supreme = recipe(SerialId::SupremeHeavyChestplate).expect("recipe");
        assert!(improved.work < elite.work);
        assert!(elite.work < supreme.work);
        assert!(improved.materials[0].1 < elite.materials[0].1);
        assert!(elite.materials[0].1 < supreme.materials[0].1);

        let elite_boots = recipe(SerialId::EliteHeavyBoots).expect("recipe");
        let elite_chest = recipe(SerialId::EliteHeavyChestplate).expect("recipe");
        assert!(elite_boots.work < elite_chest.work);
        assert!(elite_boots.materials[0].1 < elite_chest.materials[0].1);
    }

    #[test]
    fn tier_recipes_match_tier_arrays() {
        assert_eq!(tier_recipes(CraftedTier::Improved).len(), 19);
        assert_eq!(tier_recipes(CraftedTier::Elite).len(), 19);
        assert_eq!(tier_recipes(CraftedTier::Supreme).len(), 19);
        for recipe in tier_recipes(CraftedTier::Elite) {
            assert_eq!(recipe.quality, Quality::Reinforced);
            assert_eq!(recipe.rank, ELITE_RANK);
        }
        for recipe in tier_recipes(CraftedTier::Supreme) {
            assert_eq!(recipe.quality, Quality::Superior);
            assert_eq!(recipe.rank, SUPREME_RANK);
        }
    }
}
