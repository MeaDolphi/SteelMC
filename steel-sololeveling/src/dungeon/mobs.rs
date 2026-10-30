//! Classes and nameplates of dungeon mobs.
//!
//! Every dungeon mob carries a visible nameplate with what it is, its class
//! and its health, so a hunter can read a room at a glance. The plate is
//! rewritten whenever the mob loses health.

use steel_core::entity::LivingEntity;
use steel_core::inventory::equipment::EquipmentSlot;
use steel_core::player::Player;
use steel_core::world::World;
use steel_protocol::packets::game::{
    CResetScore, CSetDisplayObjective, CSetObjective, CSetScore, DisplaySlot, NumberFormat,
    ObjectiveRenderType,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::{vanilla_attributes, vanilla_items};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::boss::BossGear;
use super::generator::Theme;
use crate::rank::HunterRank;

const HEALTH_OBJECTIVE: &str = "sl_mob_health";

/// Installs the below-name scoreboard line used for dungeon mob health.
pub fn install_health_line(player: &Player) {
    player.send_packet(CSetObjective::add(
        HEALTH_OBJECTIVE,
        TextComponent::plain(""),
        ObjectiveRenderType::Integer,
        None,
    ));
    player.send_packet(CSetDisplayObjective::new(
        DisplaySlot::BelowName,
        HEALTH_OBJECTIVE,
    ));
}

/// Sets or updates the health line for every viewer in `world`.
pub fn show_health_line(world: &World, living: &dyn LivingEntity, label: &MobLabel) {
    let owner = living.uuid().to_string();
    let text = label.health_text(living.get_health(), living.get_max_health());
    world.players.iter_players(|_, player| {
        // Below the name the client draws the score through its number
        // format, so the text replaces the number; a display name would only
        // show in a sidebar.
        player.send_packet(CSetScore::with_display(
            owner.clone(),
            HEALTH_OBJECTIVE,
            0,
            None,
            Some(NumberFormat::Fixed(Box::new(text.clone()))),
        ));
        true
    });
}

/// Removes a mob's health line from every viewer in `world`.
pub fn clear_health_line(world: &World, living: &dyn LivingEntity) {
    let owner = living.uuid().to_string();
    world.players.iter_players(|_, player| {
        player.send_packet(CResetScore::remove_one(owner.clone(), HEALTH_OBJECTIVE));
        true
    });
}

/// The fighting style of an ordinary dungeon mob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobClass {
    /// A balanced swordsman.
    Warrior,
    /// A slow armored defender with more health and less damage.
    Guard,
    /// A fast, frail axe-wielder that hits hard.
    Berserker,
    /// A frail caster that heals and empowers the mobs around it.
    Shaman,
    /// A fast runner with a charge on its head that blows up next to hunters.
    Bomber,
    /// A slow, tough mother that keeps hatching larvae.
    Broodmother,
}

impl MobClass {
    /// Every class, in roll order.
    pub const ALL: [Self; 6] = [
        Self::Warrior,
        Self::Guard,
        Self::Berserker,
        Self::Shaman,
        Self::Bomber,
        Self::Broodmother,
    ];

    /// Returns the class of a roll out of sixteen: six warriors, three guards,
    /// three berserkers, two shamans, one bomber and one broodmother.
    #[must_use]
    pub const fn from_roll(roll: u64) -> Self {
        match roll % 16 {
            0..=5 => Self::Warrior,
            6..=8 => Self::Guard,
            9..=11 => Self::Berserker,
            12 | 13 => Self::Shaman,
            14 => Self::Bomber,
            _ => Self::Broodmother,
        }
    }

    /// Returns the Russian class name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Warrior => "Воин",
            Self::Guard => "Страж",
            Self::Berserker => "Берсерк",
            Self::Shaman => "Шаман",
            Self::Bomber => "Подрывник",
            Self::Broodmother => "Матка роя",
        }
    }

    /// Returns the nameplate colour of the class.
    #[must_use]
    pub const fn color(self) -> Color {
        match self {
            Self::Warrior => Color::White,
            Self::Guard => Color::Aqua,
            Self::Berserker => Color::Red,
            Self::Shaman => Color::Green,
            Self::Bomber => Color::Gold,
            Self::Broodmother => Color::DarkPurple,
        }
    }

    /// Returns health, damage and movement speed relative to a warrior.
    #[must_use]
    pub const fn proportions(self) -> (f64, f64, f64) {
        match self {
            Self::Warrior => (1.0, 1.0, 1.0),
            Self::Guard => (1.4, 0.8, 0.85),
            Self::Berserker => (0.8, 1.4, 1.25),
            Self::Shaman => (0.7, 0.5, 0.95),
            Self::Bomber => (0.6, 0.5, 1.35),
            Self::Broodmother => (1.7, 0.7, 0.8),
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
            Self::Shaman => &vanilla_items::BLAZE_ROD,
            Self::Bomber | Self::Broodmother => &vanilla_items::AIR,
        };
        let mut items = vec![(EquipmentSlot::MainHand, ItemStack::new(weapon))];
        match self {
            // The charge is worn so hunters can tell a bomber from afar.
            Self::Bomber => items.push((EquipmentSlot::Head, ItemStack::new(&vanilla_items::TNT))),
            Self::Shaman => items.push((
                EquipmentSlot::Head,
                ItemStack::new(&vanilla_items::LEATHER_HELMET),
            )),
            Self::Broodmother => items.push((
                EquipmentSlot::Chest,
                ItemStack::new(&vanilla_items::CHAINMAIL_CHESTPLATE),
            )),
            Self::Warrior | Self::Guard | Self::Berserker => {}
        }
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
    /// Whether a shadow raised from this mob should draw hostile aggro.
    pub is_tank: bool,
    /// How rare the mob is, which decides how hard it is to raise.
    pub rarity: MobRarity,
}

/// How rare a dungeon mob is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobRarity {
    /// An ordinary wave mob.
    Common,
    /// A rarely rolled class with an ability.
    Rare,
    /// A dungeon boss.
    Boss,
}

impl MobRarity {
    /// Returns the chance, in percent, that a killed mob rises as a shadow.
    #[must_use]
    pub const fn arise_chance_percent(self) -> u32 {
        match self {
            Self::Common => 10,
            Self::Rare => 4,
            Self::Boss => 2,
        }
    }
}

impl MobLabel {
    /// Labels an ordinary mob of `class` in a dungeon of `theme`.
    #[must_use]
    pub fn mob(theme: Theme, class: MobClass) -> Self {
        Self {
            name: creature_name(theme).to_owned(),
            title: class.display_name().to_owned(),
            color: class.color(),
            is_tank: class == MobClass::Guard,
            rarity: if matches!(
                class,
                MobClass::Shaman | MobClass::Bomber | MobClass::Broodmother
            ) {
                MobRarity::Rare
            } else {
                MobRarity::Common
            },
        }
    }

    /// Returns the plate text for the given health.
    #[must_use]
    pub fn text(&self) -> TextComponent {
        TextComponent::plain(format!("{} [{}]", self.name, self.title)).color(self.color.clone())
    }

    /// Returns the separate below-name health line.
    #[must_use]
    pub fn health_text(&self, health: f32, max_health: f32) -> TextComponent {
        let health = health.max(0.0).ceil() as i32;
        let max_health = max_health.max(0.0).ceil() as i32;
        TextComponent::plain(format!("❤ {health}/{max_health}")).color(self.color.clone())
    }

    /// Writes the plate onto `living` with its current health.
    pub fn show(&self, world: &World, living: &dyn LivingEntity) {
        living.set_custom_name(Some(self.text()));
        living.set_custom_name_visible(true);
        show_health_line(world, living, self);
    }
}

#[cfg(test)]
mod tests {
    use super::{MobClass, MobLabel, MobRarity};
    use crate::dungeon::generator::Theme;

    #[test]
    fn classes_follow_their_share_of_rolls() {
        let count = |class| {
            (0..16)
                .filter(|roll| MobClass::from_roll(*roll) == class)
                .count()
        };
        assert_eq!(count(MobClass::Warrior), 6);
        assert_eq!(count(MobClass::Guard), 3);
        assert_eq!(count(MobClass::Berserker), 3);
        assert_eq!(count(MobClass::Shaman), 2);
        assert_eq!(count(MobClass::Bomber), 1);
        assert_eq!(count(MobClass::Broodmother), 1);
    }

    #[test]
    fn rare_classes_and_bosses_are_harder_to_raise() {
        let common = MobLabel::mob(Theme::Crypt, MobClass::Warrior).rarity;
        let rare = MobLabel::mob(Theme::Crypt, MobClass::Shaman).rarity;
        assert_eq!(common, MobRarity::Common);
        assert_eq!(rare, MobRarity::Rare);
        assert_eq!(common.arise_chance_percent(), 10);
        assert!(rare.arise_chance_percent() < common.arise_chance_percent());
        assert!(MobRarity::Boss.arise_chance_percent() < rare.arise_chance_percent());
    }

    #[test]
    fn guard_role_marks_the_mob_as_a_tank_for_shadow_inheritance() {
        assert!(MobLabel::mob(Theme::Crypt, MobClass::Guard).is_tank);
        assert!(!MobLabel::mob(Theme::Crypt, MobClass::Warrior).is_tank);
    }

    #[test]
    fn the_plate_shows_name_class_and_rounded_health() {
        let label = MobLabel::mob(Theme::Desert, MobClass::Guard);
        let text = format!("{:?}", label.text());
        assert!(text.contains("Мумия [Страж]"), "{text}");
        let health = format!("{:?}", label.health_text(12.2, 28.0));
        assert!(health.contains("❤ 13/28"), "{health}");
        let dead = format!("{:?}", label.health_text(-3.0, 28.0));
        assert!(dead.contains("❤ 0/28"), "{dead}");
    }
}
