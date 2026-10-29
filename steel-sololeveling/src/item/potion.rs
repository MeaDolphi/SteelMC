//! Themed buff potions handed out by chests and the personal shop.
//!
//! The effect is applied by this crate when the item is used, not by a vanilla
//! potion component: the client is unmodified, so the item is a `minecraft:potion`
//! carrying a Solo Leveling tag that names one of the fixed buffs below.

use steel_registry::item_stack::ItemStack;
use steel_registry::mob_effect::MobEffectRef;
use steel_registry::{vanilla_items, vanilla_mob_effects};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::{named, read_kind, record, tag_string, with_lore, write_tag};

/// Kind stored in the record of a buff potion.
const KIND: &str = "potion";
/// Tag field holding the buff name.
const BUFF: &str = "buff";
/// Duration of every starter buff, in seconds.
pub const BUFF_SECONDS: u64 = 180;
/// Effect level of every starter buff; vanilla amplifiers start at zero.
const BUFF_AMPLIFIER: i32 = 0;

/// One of the starter buffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuffPotion {
    /// Strength I: more melee damage.
    Strength,
    /// Speed I: faster movement.
    Speed,
    /// Resistance I: less incoming damage.
    Resistance,
    /// Regeneration I: health regeneration.
    Regeneration,
}

/// Every starter buff, used for uniform rolls.
pub const ALL: [BuffPotion; 4] = [
    BuffPotion::Strength,
    BuffPotion::Speed,
    BuffPotion::Resistance,
    BuffPotion::Regeneration,
];

impl BuffPotion {
    /// Returns the stored buff name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Strength => "strength",
            Self::Speed => "speed",
            Self::Resistance => "resistance",
            Self::Regeneration => "regeneration",
        }
    }

    /// Parses a stored buff name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        ALL.into_iter().find(|buff| buff.name() == name)
    }

    /// Returns the Russian display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Strength => "Зелье силы",
            Self::Speed => "Зелье скорости",
            Self::Resistance => "Зелье сопротивления",
            Self::Regeneration => "Зелье регенерации",
        }
    }

    /// Returns the vanilla effect this buff applies.
    #[must_use]
    pub const fn effect(self) -> MobEffectRef {
        match self {
            Self::Strength => vanilla_mob_effects::STRENGTH,
            Self::Speed => vanilla_mob_effects::SPEED,
            Self::Resistance => vanilla_mob_effects::RESISTANCE,
            Self::Regeneration => vanilla_mob_effects::REGENERATION,
        }
    }

    /// Creates one buff potion item.
    #[must_use]
    pub fn stack(self) -> ItemStack {
        let mut stack = named(
            ItemStack::new(&vanilla_items::POTION),
            TextComponent::plain(self.display_name().to_owned()).color(Color::LightPurple),
        );
        stack = with_lore(
            stack,
            vec![
                TextComponent::plain(format!("Уровень I, {BUFF_SECONDS} секунд."))
                    .color(Color::Gray),
                TextComponent::plain("Правый клик — выпить.").color(Color::Yellow),
            ],
        );
        let mut fields = record(KIND);
        fields.insert(BUFF, self.name().to_owned());
        write_tag(&mut stack, fields);
        stack
    }

    /// Returns the buff a stack holds, or `None` for an ordinary potion.
    #[must_use]
    pub fn of(stack: &ItemStack) -> Option<Self> {
        let tag = read_kind(stack, KIND)?;
        Self::from_name(&tag_string(&tag, BUFF)?)
    }
}

/// Returns the effect duration in ticks.
#[must_use]
pub const fn buff_duration_ticks() -> i32 {
    (BUFF_SECONDS as i32) * 20
}

/// Returns the amplifier applied for a buff.
#[must_use]
pub const fn buff_amplifier() -> i32 {
    BUFF_AMPLIFIER
}
