//! Crafting orders: improved, elite and supreme serial gear made from dungeon
//! materials.
//!
//! Every recipe is fixed: no failure chance, no timer. The materials and the
//! previous tier of the same line are consumed, and the master's work is paid
//! in Won, in the same transaction that hands out the new item; the new item
//! inherits the binding of the handed-in base. Access follows the hunter's
//! rank, never a count of finished raids, and owning bought materials does
//! not bypass it. Only transferable materials are accepted, so a transferable
//! base yields a transferable result.
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
use uuid::Uuid;

use crate::item::artifact::ArmorLine;
use crate::item::binding;
use crate::item::material::{self, Material, Quality};
use crate::item::serial::{self, CraftedTier, SerialId, tiered};
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

/// Returns the item an order of `output` hands in as its base: the same
/// catalog position of the previous tier. Improved armor builds on the
/// specialization pieces, improved weapons on the starter weapons and the
/// merchant bow and crossbow; elite and supreme build on the previous
/// crafted tier.
#[must_use]
pub fn base_of(output: SerialId) -> Option<SerialId> {
    let (tier, index) = output.tier_index()?;
    Some(match tier {
        CraftedTier::Improved => {
            if let Some(&base) = serial::SPECIALIZATION.get(index) {
                base
            } else {
                // The improved catalog lists the weapons after the armor:
                // sword, dagger, axe, then the merchant bow and crossbow.
                [
                    SerialId::HunterSword,
                    SerialId::HunterDagger,
                    SerialId::HunterAxe,
                    SerialId::HunterBow,
                    SerialId::HunterCrossbow,
                ]
                .get(index - serial::SPECIALIZATION.len())
                .copied()?
            }
        }
        CraftedTier::Elite => serial::IMPROVED[index],
        CraftedTier::Supreme => tiered::ELITE[index],
    })
}

/// Why an order cannot be staged against the carried slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    /// The hunter carries no usable copy of the previous-tier base item.
    MissingBase(SerialId),
    /// The hunter carries too few transferable material units.
    MissingMaterials,
}

impl PlanError {
    /// Returns the Russian message shown to the hunter.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::MissingBase(base) => format!(
                "Основа заказа — {}: положите этот предмет в инвентарь.",
                base.serial().name
            ),
            Self::MissingMaterials => "Не хватает материалов для заказа.".to_owned(),
        }
    }
}

/// The staged edits of one order: inventory slot replacements plus the
/// crafted item they make room for.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderPlan {
    /// Replacements applied to the hunter's slots, in staging order.
    pub replacements: Vec<(usize, ItemStack)>,
    /// The crafted item, bound like the base it replaces.
    pub result: ItemStack,
}

/// Stages one order against the hunter's `slots`: transferable materials are
/// consumed and the previous-tier base item is replaced by the new one, which
/// inherits the base's binding. Unless every requirement is met, an error is
/// returned and no slot is changed.
pub fn plan(recipe: &Recipe, uuid: Uuid, slots: &[ItemStack]) -> Result<OrderPlan, PlanError> {
    let mut slots = slots.to_vec();
    let mut replacements = Vec::new();
    for (wanted, needed) in &recipe.materials {
        let mut left = i32::try_from(*needed).unwrap_or(i32::MAX);
        for (slot, stack) in slots.iter_mut().enumerate() {
            if left == 0 {
                break;
            }
            if binding::bound_owner(stack).is_some()
                || material::of(stack) != Some((*wanted, recipe.quality))
            {
                continue;
            }
            let taken = stack.count.min(left);
            left -= taken;
            let kept = stack.count - taken;
            let replacement = if kept > 0 {
                stack.copy_with_count(kept)
            } else {
                ItemStack::empty()
            };
            *stack = replacement.clone();
            replacements.push((slot, replacement));
        }
        if left > 0 {
            return Err(PlanError::MissingMaterials);
        }
    }
    // The base may be a personal copy or a transferable one, but never
    // another hunter's personal item; the result takes its binding over.
    let result = match base_of(recipe.output) {
        Some(base) => {
            let Some(slot) = slots.iter().enumerate().find_map(|(slot, stack)| {
                (serial::of(stack) == Some(base) && !binding::bound_to_other(stack, uuid))
                    .then_some(slot)
            }) else {
                return Err(PlanError::MissingBase(base));
            };
            let result = serial::stack(recipe.output, binding::bound_owner(&slots[slot]));
            replacements.push((slot, result.clone()));
            result
        }
        None => serial::stack(recipe.output, None),
    };
    Ok(OrderPlan {
        replacements,
        result,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ELITE_QUALITY, ELITE_RANK, PlanError, SUPREME_QUALITY, SUPREME_RANK, all, base_of, plan,
        recipe, tier_recipes,
    };
    use crate::item::binding;
    use crate::item::material::{self, Material, Quality};
    use crate::item::serial::{self, CraftedTier, SerialId, tiered};
    use steel_registry::init_vanilla_registry;
    use uuid::Uuid;

    /// Fills `slots` with the recipe's materials, leaving `base_slot` free.
    fn with_materials(
        recipe: &super::Recipe,
        base_slot: usize,
    ) -> Vec<steel_registry::item_stack::ItemStack> {
        let mut slots = vec![steel_registry::item_stack::ItemStack::empty(); 36];
        for (index, (wanted, needed)) in recipe.materials.iter().enumerate() {
            let slot = if index < base_slot { index } else { index + 1 };
            let count = i32::try_from(*needed).unwrap_or(i32::MAX);
            slots[slot] = material::stack(*wanted, recipe.quality, count);
        }
        slots
    }

    #[test]
    fn every_crafted_item_has_one_recipe_with_essence() {
        assert_eq!(all().len(), 63);
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
    fn ranged_weapons_are_ordered_by_tier_like_melee() {
        for id in [
            SerialId::ImprovedHunterBow,
            SerialId::ImprovedHunterCrossbow,
            SerialId::EliteHunterBow,
            SerialId::EliteHunterCrossbow,
            SerialId::SupremeHunterBow,
            SerialId::SupremeHunterCrossbow,
        ] {
            let made = recipe(id).expect("recipe");
            assert_eq!(made.output, id);
            assert_eq!(made.quality, id.crafted_tier().expect("tier").quality());
            assert_eq!(made.rank, id.crafted_tier().expect("tier").rank());
            assert!(
                made.materials
                    .iter()
                    .any(|(material, _)| *material == Material::StableEssence)
            );
        }
        let improved = recipe(SerialId::ImprovedHunterBow).expect("recipe");
        let elite = recipe(SerialId::EliteHunterBow).expect("recipe");
        let supreme = recipe(SerialId::SupremeHunterBow).expect("recipe");
        assert!(improved.work < elite.work && elite.work < supreme.work);
        assert_eq!(recipe(SerialId::HunterBow), None);
        assert_eq!(recipe(SerialId::HunterCrossbow), None);
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
        assert_eq!(tier_recipes(CraftedTier::Improved).len(), 21);
        assert_eq!(tier_recipes(CraftedTier::Elite).len(), 21);
        assert_eq!(tier_recipes(CraftedTier::Supreme).len(), 21);
        for recipe in tier_recipes(CraftedTier::Elite) {
            assert_eq!(recipe.quality, Quality::Reinforced);
            assert_eq!(recipe.rank, ELITE_RANK);
        }
        for recipe in tier_recipes(CraftedTier::Supreme) {
            assert_eq!(recipe.quality, Quality::Superior);
            assert_eq!(recipe.rank, SUPREME_RANK);
        }
    }

    #[test]
    fn upgrade_bases_follow_the_catalog_lines() {
        for index in 0..serial::IMPROVED.len() {
            let improved = serial::IMPROVED[index];
            let base = base_of(improved).expect("improved base");
            if index < serial::SPECIALIZATION.len() {
                assert_eq!(base, serial::SPECIALIZATION[index]);
            }
            assert_eq!(base_of(serial::tiered::ELITE[index]), Some(improved));
            assert_eq!(
                base_of(serial::tiered::SUPREME[index]),
                Some(serial::tiered::ELITE[index])
            );
        }
        assert_eq!(
            base_of(SerialId::ImprovedHunterSword),
            Some(SerialId::HunterSword)
        );
        assert_eq!(
            base_of(SerialId::ImprovedHunterBow),
            Some(SerialId::HunterBow)
        );
        assert_eq!(
            base_of(SerialId::ImprovedHunterCrossbow),
            Some(SerialId::HunterCrossbow)
        );
        // The base tier and items without a chain have no base.
        assert_eq!(base_of(SerialId::HunterSword), None);
        assert_eq!(base_of(SerialId::ApprenticeStaff), None);
    }

    #[test]
    fn a_plan_without_the_base_is_refused_with_its_reason() {
        init_vanilla_registry();
        let uuid = Uuid::new_v4();
        let recipe = recipe(SerialId::EliteHeavyChestplate).expect("recipe");
        let slots = with_materials(&recipe, 10);
        assert_eq!(
            plan(&recipe, uuid, &slots),
            Err(PlanError::MissingBase(SerialId::ImprovedHeavyChestplate))
        );
    }

    #[test]
    fn a_plan_never_takes_another_hunters_personal_base() {
        init_vanilla_registry();
        let uuid = Uuid::new_v4();
        let recipe = recipe(SerialId::EliteHeavyChestplate).expect("recipe");
        let mut slots = with_materials(&recipe, 10);
        slots[10] = serial::stack(SerialId::ImprovedHeavyChestplate, Some(Uuid::new_v4()));
        assert_eq!(
            plan(&recipe, uuid, &slots),
            Err(PlanError::MissingBase(SerialId::ImprovedHeavyChestplate))
        );
    }

    #[test]
    fn a_plan_consumes_the_base_and_hands_over_the_next_tier_with_its_binding() {
        init_vanilla_registry();
        let uuid = Uuid::new_v4();
        let recipe = recipe(SerialId::EliteHunterSword).expect("recipe");

        // A personal base yields a personal next tier in the same slot.
        let mut slots = with_materials(&recipe, 10);
        slots[10] = serial::stack(SerialId::ImprovedHunterSword, Some(uuid));
        let made = plan(&recipe, uuid, &slots).expect("plan");
        assert_eq!(serial::of(&made.result), Some(SerialId::EliteHunterSword));
        assert_eq!(binding::bound_owner(&made.result), Some(uuid));
        assert!(
            made.replacements
                .contains(&(10, serial::stack(SerialId::EliteHunterSword, Some(uuid)))),
            "the base slot must hold the crafted item"
        );

        // A transferable base yields a transferable next tier.
        let mut slots = with_materials(&recipe, 10);
        slots[10] = serial::stack(SerialId::ImprovedHunterSword, None);
        let made = plan(&recipe, uuid, &slots).expect("plan");
        assert_eq!(serial::of(&made.result), Some(SerialId::EliteHunterSword));
        assert_eq!(binding::bound_owner(&made.result), None);

        // Every material stack is emptied.
        assert!(
            made.replacements
                .iter()
                .filter(|(slot, _)| *slot != 10)
                .all(|(_, stack)| stack.is_empty()),
            "material slots must be consumed"
        );
    }

    #[test]
    fn a_plan_with_too_few_materials_is_refused_before_the_base() {
        init_vanilla_registry();
        let uuid = Uuid::new_v4();
        let recipe = recipe(SerialId::EliteHeavyChestplate).expect("recipe");
        let mut slots = with_materials(&recipe, 10);
        slots[0].count -= 1;
        slots[10] = serial::stack(SerialId::ImprovedHeavyChestplate, Some(uuid));
        assert_eq!(
            plan(&recipe, uuid, &slots),
            Err(PlanError::MissingMaterials)
        );
    }
}
