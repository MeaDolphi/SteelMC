//! Crafting orders: improved serial gear made from dungeon materials.
//!
//! Every recipe is fixed: no failure chance, no timer. The materials are
//! consumed and the master's work is paid in Won in the same transaction
//! that hands out the new item. Access follows the hunter's rank, never a
//! count of finished raids, and owning bought materials does not bypass it.
//! Only transferable materials are accepted, so the result is transferable.

use steel_registry::item_stack::ItemStack;

use crate::item::artifact::{ArmorLine, ArtifactId, IMPROVED};
use crate::item::binding;
use crate::item::material::{self, Material, Quality};
use crate::rank::HunterRank;

/// Lowest rank that may order the improved tier.
pub const IMPROVED_RANK: HunterRank = HunterRank::D;
/// Quality of the materials the improved tier is made from.
pub const IMPROVED_QUALITY: Quality = Quality::Common;

/// One fixed crafting recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    /// The item made.
    pub output: ArtifactId,
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
pub fn recipe(output: ArtifactId) -> Option<Recipe> {
    if !output.is_improved() {
        return None;
    }
    let (materials, work) = if let Some(line) = output.line() {
        let (primary, secondary) = line_materials(line);
        // Pieces follow the four-per-line catalog order: helmet, chestplate,
        // leggings, boots.
        let piece = IMPROVED.iter().position(|id| *id == output)? % 4;
        let (main, side, work) = match piece {
            1 => (8, 4, 400),
            2 => (6, 3, 350),
            _ => (4, 2, 200),
        };
        (vec![(primary, main), (secondary, side)], work)
    } else {
        (
            vec![(Material::MagicMetal, 6), (Material::ToughHide, 2)],
            350,
        )
    };
    let mut materials = materials;
    materials.push((Material::StableEssence, 1));
    Some(Recipe {
        output,
        materials,
        quality: IMPROVED_QUALITY,
        work,
        rank: IMPROVED_RANK,
    })
}

/// Every recipe, in catalog order.
#[must_use]
pub fn all() -> Vec<Recipe> {
    IMPROVED.into_iter().filter_map(recipe).collect()
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
    use super::{IMPROVED_RANK, all, recipe};
    use crate::item::artifact::{ArtifactId, IMPROVED};
    use crate::item::material::Material;

    #[test]
    fn every_improved_item_has_one_recipe_with_essence() {
        assert_eq!(all().len(), IMPROVED.len());
        for recipe in all() {
            assert!(
                recipe
                    .materials
                    .iter()
                    .any(|(material, count)| *material == Material::StableEssence && *count == 1)
            );
            assert!(recipe.work > 0);
            assert_eq!(recipe.rank, IMPROVED_RANK);
        }
        assert_eq!(recipe(ArtifactId::HunterSword), None);
    }

    #[test]
    fn a_chestplate_costs_more_than_boots() {
        let chest = recipe(ArtifactId::ImprovedHeavyChestplate).expect("recipe");
        let boots = recipe(ArtifactId::ImprovedHeavyBoots).expect("recipe");
        assert!(chest.work > boots.work);
        assert_eq!(chest.materials[0], (Material::MagicMetal, 8));
        assert_eq!(chest.materials[1], (Material::ChitinPlate, 4));
    }
}
