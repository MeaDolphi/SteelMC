//! Dungeon materials: transferable ingredients for crafting orders.
//!
//! Five materials in three qualities. The quality follows the rank of the
//! gate the material came from, so materials of one quality serve two
//! neighbouring ranks. Materials are their own reward, never the gear a mob
//! wore, and they never replace magic crystals, whose job is to be sold.

use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::{named, read_kind, record, tag_string, with_lore, write_tag};
use crate::dungeon::mobs::MobClass;
use crate::rank::HunterRank;

/// Kind stored in the record of a material item.
const KIND: &str = "material";
/// Tag field holding the material.
const MATERIAL: &str = "material";
/// Tag field holding the quality.
const QUALITY: &str = "quality";

/// One crafting material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Material {
    /// Blades, axes, shields and metal armor.
    MagicMetal,
    /// Light armor, linings and grips.
    ToughHide,
    /// Shields, reinforced and heavy armor.
    ChitinPlate,
    /// Mage clothing, light armor and magic stitching.
    ManaFiber,
    /// The rare component every improved piece needs.
    StableEssence,
}

/// Every material, in display order.
pub const ALL: [Material; 5] = [
    Material::MagicMetal,
    Material::ToughHide,
    Material::ChitinPlate,
    Material::ManaFiber,
    Material::StableEssence,
];

impl Material {
    /// Returns the stable tag value.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MagicMetal => "magic_metal",
            Self::ToughHide => "tough_hide",
            Self::ChitinPlate => "chitin_plate",
            Self::ManaFiber => "mana_fiber",
            Self::StableEssence => "stable_essence",
        }
    }

    /// Returns the Russian display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::MagicMetal => "Осколки магического металла",
            Self::ToughHide => "Прочная шкура",
            Self::ChitinPlate => "Хитиновая пластина",
            Self::ManaFiber => "Мановые волокна",
            Self::StableEssence => "Стабильная эссенция",
        }
    }

    fn holder(self) -> ItemRef {
        match self {
            Self::MagicMetal => &vanilla_items::IRON_NUGGET,
            Self::ToughHide => &vanilla_items::LEATHER,
            Self::ChitinPlate => &vanilla_items::ARMADILLO_SCUTE,
            Self::ManaFiber => &vanilla_items::STRING,
            Self::StableEssence => &vanilla_items::PRISMARINE_CRYSTALS,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        ALL.into_iter().find(|material| material.name() == name)
    }
}

/// Material quality, set by the rank of the source gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Quality {
    /// From E- and D-rank gates.
    Common,
    /// From C- and B-rank gates.
    Reinforced,
    /// From A- and S-rank gates.
    Superior,
}

impl Quality {
    /// Returns the quality of materials from a gate of `rank`.
    #[must_use]
    pub const fn for_rank(rank: HunterRank) -> Self {
        match rank {
            HunterRank::E | HunterRank::D => Self::Common,
            HunterRank::C | HunterRank::B => Self::Reinforced,
            HunterRank::A | HunterRank::S => Self::Superior,
        }
    }

    /// Returns the stable tag value.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Reinforced => "reinforced",
            Self::Superior => "superior",
        }
    }

    /// Returns the Russian display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Common => "обычное",
            Self::Reinforced => "усиленное",
            Self::Superior => "высшее",
        }
    }

    const fn color(self) -> Color {
        match self {
            Self::Common => Color::White,
            Self::Reinforced => Color::Aqua,
            Self::Superior => Color::Gold,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        [Self::Common, Self::Reinforced, Self::Superior]
            .into_iter()
            .find(|quality| quality.name() == name)
    }
}

/// Builds `count` units of `material` in `quality`.
#[must_use]
pub fn stack(material: Material, quality: Quality, count: i32) -> ItemStack {
    let mut stack = named(
        ItemStack::with_count(material.holder(), count),
        TextComponent::plain(material.display_name().to_owned()).color(quality.color()),
    );
    stack = with_lore(
        stack,
        vec![
            TextComponent::plain(format!("Качество: {}", quality.display_name()))
                .color(quality.color()),
            TextComponent::plain("Материал для заказов у городского торговца.").color(Color::Gray),
        ],
    );
    let mut fields = record(KIND);
    fields.insert(MATERIAL, material.name().to_owned());
    fields.insert(QUALITY, quality.name().to_owned());
    write_tag(&mut stack, fields);
    stack
}

/// Returns the material and quality of a stack, or `None` for other items.
#[must_use]
pub fn of(stack: &ItemStack) -> Option<(Material, Quality)> {
    let tag = read_kind(stack, KIND)?;
    Some((
        Material::from_name(&tag_string(&tag, MATERIAL)?)?,
        Quality::from_name(&tag_string(&tag, QUALITY)?)?,
    ))
}

/// Percent chance of stable essence from any ordinary wave mob.
pub const ESSENCE_PERCENT: u32 = 3;

/// Returns what one ordinary mob of `class` may drop: material and percent.
///
/// Rare materials have a small secondary source, so one recipe never
/// depends on a single room type appearing.
#[must_use]
pub const fn mob_table(class: MobClass) -> &'static [(Material, u32)] {
    match class {
        MobClass::Warrior => &[(Material::MagicMetal, 60)],
        MobClass::Guard => &[(Material::MagicMetal, 60), (Material::ChitinPlate, 15)],
        MobClass::Berserker => &[(Material::MagicMetal, 40), (Material::ToughHide, 40)],
        MobClass::Shaman => &[(Material::ManaFiber, 60)],
        MobClass::Bomber => &[(Material::ToughHide, 20), (Material::ManaFiber, 20)],
        MobClass::Broodmother => &[
            (Material::ChitinPlate, 70),
            (Material::ToughHide, 30),
            (Material::ManaFiber, 30),
        ],
    }
}

/// Rolls the materials one ordinary mob of `class` drops in a gate of `rank`.
///
/// `roll` returns a uniform number below 100 on each call.
pub fn roll_mob(
    class: MobClass,
    rank: HunterRank,
    mut roll: impl FnMut() -> u32,
) -> Vec<ItemStack> {
    let quality = Quality::for_rank(rank);
    let mut drops: Vec<ItemStack> = mob_table(class)
        .iter()
        .filter(|(_, percent)| roll() < *percent)
        .map(|(material, _)| stack(*material, quality, 1))
        .collect();
    if roll() < ESSENCE_PERCENT {
        drops.push(stack(Material::StableEssence, quality, 1));
    }
    drops
}

/// Base materials a boss may add to its guaranteed essence.
const BOSS_POOL: [Material; 4] = [
    Material::MagicMetal,
    Material::ToughHide,
    Material::ChitinPlate,
    Material::ManaFiber,
];

/// Rolls the boss share of one survivor: one essence and two base materials.
pub fn roll_boss(rank: HunterRank, mut roll: impl FnMut() -> u32) -> Vec<ItemStack> {
    let quality = Quality::for_rank(rank);
    let mut drops = vec![stack(Material::StableEssence, quality, 1)];
    for _ in 0..2 {
        let index = (roll() as usize) % BOSS_POOL.len();
        drops.push(stack(BOSS_POOL[index], quality, 1));
    }
    drops
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::{ALL, Material, Quality, of, roll_boss, roll_mob, stack};
    use crate::dungeon::mobs::MobClass;
    use crate::rank::HunterRank;

    #[test]
    fn materials_round_trip_and_stack_by_quality() {
        init_vanilla_registry();
        for material in ALL {
            for quality in [Quality::Common, Quality::Reinforced, Quality::Superior] {
                assert_eq!(of(&stack(material, quality, 3)), Some((material, quality)));
            }
        }
        let common = stack(Material::MagicMetal, Quality::Common, 1);
        assert_eq!(common, stack(Material::MagicMetal, Quality::Common, 1));
        assert_ne!(common, stack(Material::MagicMetal, Quality::Reinforced, 1));
    }

    #[test]
    fn quality_follows_the_gate_rank() {
        assert_eq!(Quality::for_rank(HunterRank::E), Quality::Common);
        assert_eq!(Quality::for_rank(HunterRank::B), Quality::Reinforced);
        assert_eq!(Quality::for_rank(HunterRank::S), Quality::Superior);
    }

    #[test]
    fn rolls_respect_the_tables() {
        init_vanilla_registry();
        assert_eq!(roll_mob(MobClass::Warrior, HunterRank::E, || 99).len(), 0);
        let lucky = roll_mob(MobClass::Broodmother, HunterRank::E, || 0);
        assert_eq!(lucky.len(), 4, "three base materials and the essence");
        let boss = roll_boss(HunterRank::C, || 1);
        assert_eq!(boss.len(), 3);
        assert_eq!(
            of(&boss[0]),
            Some((Material::StableEssence, Quality::Reinforced))
        );
    }
}
