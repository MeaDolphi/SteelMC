//! Classes and nameplates of dungeon mobs.
//!
//! Every dungeon mob carries a visible nameplate with what it is, its class
//! and its health, so a hunter can read a room at a glance. The plate is
//! rewritten whenever the mob loses health.

use steel_core::entity::LivingEntity;
use steel_core::inventory::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::{vanilla_attributes, vanilla_items};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::boss::BossGear;
use super::generator::Theme;
use crate::rank::HunterRank;

/// The fighting style of an ordinary dungeon mob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobClass {
    /// A balanced swordsman.
    Warrior,
    /// A slow armored defender with more health and less damage.
    Guard,
    /// A fast, frail axe-wielder that hits hard.
    Berserker,
}

impl MobClass {
    /// Returns the class of a roll: half warriors, a quarter each of the rest.
    #[must_use]
    pub const fn from_roll(roll: u64) -> Self {
        match roll % 4 {
            0 | 1 => Self::Warrior,
            2 => Self::Guard,
            _ => Self::Berserker,
        }
    }

    /// Returns the Russian class name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Warrior => "Воин",
            Self::Guard => "Страж",
            Self::Berserker => "Берсерк",
        }
    }

    /// Returns the nameplate colour of the class.
    #[must_use]
    pub const fn color(self) -> Color {
        match self {
            Self::Warrior => Color::White,
            Self::Guard => Color::Aqua,
            Self::Berserker => Color::Red,
        }
    }

    /// Returns health, damage and movement speed relative to a warrior.
    #[must_use]
    pub const fn proportions(self) -> (f64, f64, f64) {
        match self {
            Self::Warrior => (1.0, 1.0, 1.0),
            Self::Guard => (1.4, 0.8, 0.85),
            Self::Berserker => (0.8, 1.4, 1.25),
        }
    }

    /// Equips the class's weapon and armor and adjusts its speed and armor.
    pub fn equip(self, living: &dyn LivingEntity, rank: HunterRank) {
        let gear = BossGear::for_rank(rank);
        let weapon: ItemRef = match self {
            Self::Warrior => gear.sword(),
            Self::Guard => &vanilla_items::SHIELD,
            Self::Berserker => match gear {
                BossGear::Iron => &vanilla_items::IRON_AXE,
                BossGear::Diamond => &vanilla_items::DIAMOND_AXE,
                BossGear::Netherite => &vanilla_items::NETHERITE_AXE,
            },
        };
        let mut items = vec![(EquipmentSlot::MainHand, ItemStack::new(weapon))];
        if self == Self::Guard {
            items.push((
                EquipmentSlot::Head,
                ItemStack::new(&vanilla_items::IRON_HELMET),
            ));
            items.push((
                EquipmentSlot::Chest,
                ItemStack::new(&vanilla_items::IRON_CHESTPLATE),
            ));
        }
        for (slot, item) in items {
            living.with_equipment_slot_mut(slot, &mut |stack| *stack = item.clone());
        }
        let (_, _, speed) = self.proportions();
        let base = living
            .attributes()
            .lock()
            .required_value(vanilla_attributes::MOVEMENT_SPEED);
        living.set_base_attribute_value(vanilla_attributes::MOVEMENT_SPEED, base * speed);
    }
}

/// Returns what the mobs of a themed dungeon are called.
#[must_use]
pub const fn creature_name(theme: Theme) -> &'static str {
    match theme {
        Theme::Crypt => "Упырь",
        Theme::Ice => "Ледяной мертвец",
        Theme::Nether => "Обугленный",
        Theme::Desert => "Мумия",
        Theme::Jungle => "Болотник",
        Theme::DeepDark => "Порождение тьмы",
        Theme::End => "Странник Края",
    }
}

/// The fixed part of a mob's nameplate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobLabel {
    /// What the mob is.
    pub name: String,
    /// Its class, or its boss title.
    pub title: String,
    /// Colour of the whole plate.
    pub color: Color,
}

impl MobLabel {
    /// Labels an ordinary mob of `class` in a dungeon of `theme`.
    #[must_use]
    pub fn mob(theme: Theme, class: MobClass) -> Self {
        Self {
            name: creature_name(theme).to_owned(),
            title: class.display_name().to_owned(),
            color: class.color(),
        }
    }

    /// Returns the plate text for the given health.
    #[must_use]
    pub fn text(&self, health: f32, max_health: f32) -> TextComponent {
        let health = health.max(0.0).ceil() as i32;
        let max_health = max_health.max(0.0).ceil() as i32;
        TextComponent::plain(format!(
            "{} [{}] ❤ {health}/{max_health}",
            self.name, self.title
        ))
        .color(self.color.clone())
    }

    /// Writes the plate onto `living` with its current health.
    pub fn show(&self, living: &dyn LivingEntity) {
        living.set_custom_name(Some(
            self.text(living.get_health(), living.get_max_health()),
        ));
        living.set_custom_name_visible(true);
    }
}

#[cfg(test)]
mod tests {
    use super::{MobClass, MobLabel};
    use crate::dungeon::generator::Theme;

    #[test]
    fn classes_follow_their_share_of_rolls() {
        let rolls: Vec<_> = (0..4).map(MobClass::from_roll).collect();
        assert_eq!(
            rolls,
            vec![
                MobClass::Warrior,
                MobClass::Warrior,
                MobClass::Guard,
                MobClass::Berserker
            ]
        );
    }

    #[test]
    fn the_plate_shows_name_class_and_rounded_health() {
        let label = MobLabel::mob(Theme::Desert, MobClass::Guard);
        let text = format!("{:?}", label.text(12.2, 28.0));
        assert!(text.contains("Мумия [Страж] ❤ 13/28"), "{text}");
        let dead = format!("{:?}", label.text(-3.0, 28.0));
        assert!(dead.contains("❤ 0/28"), "{dead}");
    }
}
