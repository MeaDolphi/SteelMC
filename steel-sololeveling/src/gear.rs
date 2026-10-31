//! System equipment: artifacts a hunter equips through the System instead of
//! wearing them.
//!
//! Gear sits in its own slots — head, body, legs, feet, gloves, two rings and
//! an amulet — so gloves or rings never compete with vanilla armor slots. The
//! hunter's body stays bare: gear only grants its attribute bonuses and
//! abilities. The loadout is part of the persisted player data, and a chest
//! menu moves artifacts between it and the inventory.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use steel_core::entity::attribute::AttributeModifier;
use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_core::player::player_inventory::PlayerInventory;
use steel_registry::attribute::AttributeRef;
use steel_registry::data_components::vanilla_components::ATTRIBUTE_MODIFIERS;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_items;
use steel_utils::Identifier;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::class::HunterClass;
use crate::identifier::from_sololeveling;
use crate::item::artifact::{self, Ability, ArtifactId};
use crate::item::{named, nbt, with_lore};
use crate::rank::HunterRank;
use crate::state::SoloLevelingState;
use crate::ui::{After, open_guarded_buttons};

/// What kind of gear a slot takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GearKind {
    /// Helmets and hoods.
    Head,
    /// Chestplates and robes.
    Chest,
    /// Leggings.
    Legs,
    /// Boots and shoes.
    Feet,
    /// Gloves and gauntlets.
    Gloves,
    /// Rings; a hunter wears two.
    Ring,
    /// Amulets and pendants.
    Amulet,
}

impl GearKind {
    /// Returns the Russian slot name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Head => "Голова",
            Self::Chest => "Тело",
            Self::Legs => "Ноги",
            Self::Feet => "Ступни",
            Self::Gloves => "Перчатки",
            Self::Ring => "Кольцо",
            Self::Amulet => "Амулет",
        }
    }

    fn icon(self) -> ItemRef {
        match self {
            Self::Head => &vanilla_items::CHAINMAIL_HELMET,
            Self::Chest => &vanilla_items::CHAINMAIL_CHESTPLATE,
            Self::Legs => &vanilla_items::CHAINMAIL_LEGGINGS,
            Self::Feet => &vanilla_items::CHAINMAIL_BOOTS,
            Self::Gloves => &vanilla_items::LEATHER_HORSE_ARMOR,
            Self::Ring => &vanilla_items::GOLD_NUGGET,
            Self::Amulet => &vanilla_items::HEART_OF_THE_SEA,
        }
    }
}

/// Number of gear slots.
pub const GEAR_SLOTS: usize = 8;
/// The kind each gear slot takes, by gear index.
pub const LAYOUT: [GearKind; GEAR_SLOTS] = [
    GearKind::Head,
    GearKind::Chest,
    GearKind::Legs,
    GearKind::Feet,
    GearKind::Gloves,
    GearKind::Ring,
    GearKind::Ring,
    GearKind::Amulet,
];
/// Returns the gear kind an item can be equipped as, if any.
#[must_use]
pub fn kind_of(stack: &ItemStack) -> Option<GearKind> {
    artifact::of(stack)?.gear_kind()
}

pub(crate) fn free_slot(items: &[ItemStack], stack: &ItemStack) -> Option<usize> {
    let kind = kind_of(stack)?;
    LAYOUT.iter().enumerate().find_map(|(index, allowed)| {
        (*allowed == kind && items.get(index).is_none_or(ItemStack::is_empty)).then_some(index)
    })
}

/// The persisted form of a loadout: each slot in Solo Leveling's stored NBT.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedGear {
    /// One entry per gear slot; `None` for an empty slot.
    pub slots: Vec<Option<Vec<u8>>>,
}

impl SavedGear {
    /// Encodes a live loadout.
    #[must_use]
    pub fn from_items(items: &[ItemStack]) -> Self {
        Self::from_items_preserving(items, &Self::default())
    }

    /// Encodes live items while retaining undecodable and unsupported stored slots.
    #[must_use]
    pub fn from_items_preserving(items: &[ItemStack], previous: &Self) -> Self {
        let len = items.len().max(previous.slots.len());
        Self {
            slots: (0..len)
                .map(|index| {
                    let Some(stack) = items.get(index).filter(|stack| !stack.is_empty()) else {
                        let stored = previous.slots.get(index)?.as_ref()?;
                        return nbt::decode(stored).is_err().then(|| stored.clone());
                    };
                    nbt::encode(stack)
                        .inspect_err(|error| {
                            log::error!("sololeveling: could not encode a gear item: {error}");
                        })
                        .ok()
                        .or_else(|| previous.slots.get(index).cloned().flatten())
                })
                .collect(),
        }
    }

    /// Decodes into a live loadout of [`GEAR_SLOTS`] stacks.
    #[must_use]
    pub fn items(&self) -> Vec<ItemStack> {
        let mut items = vec![ItemStack::empty(); GEAR_SLOTS];
        for (index, bytes) in self.slots.iter().enumerate().take(GEAR_SLOTS) {
            if let Some(bytes) = bytes {
                match nbt::decode(bytes) {
                    Ok(stack) => items[index] = stack,
                    Err(error) => {
                        log::error!("sololeveling: could not decode a gear item: {error}");
                    }
                }
            }
        }
        items
    }
}

/// Returns the abilities of the gear `items` a hunter of `rank` can use.
#[must_use]
pub fn abilities(
    items: &[ItemStack],
    rank: HunterRank,
    class: Option<HunterClass>,
) -> Vec<Ability> {
    items
        .iter()
        .filter(|stack| artifact::usable_by(stack, rank) && wearable(stack, class))
        .filter_map(artifact::held)
        .flat_map(|artifact| artifact.abilities.iter().copied())
        .collect()
}

/// Returns whether a hunter of `class` may wear `stack`.
///
/// Specialization armor belongs to its classes; everything else, the
/// universal starter armor included, fits every hunter.
#[must_use]
pub fn wearable(stack: &ItemStack, class: Option<HunterClass>) -> bool {
    artifact::of(stack)
        .and_then(ArtifactId::allowed_classes)
        .is_none_or(|classes| class.is_some_and(|class| classes.contains(&class)))
}

/// Returns whether a hunter with `strength` meets the Strength requirement of
/// `stack`; a piece below it can be worn but grants nothing.
#[must_use]
pub fn strong_enough(stack: &ItemStack, strength: u32) -> bool {
    artifact::of(stack).is_none_or(|id| strength >= id.required_strength())
}

/// Returns the magic-damage bonus of the gear `items` a hunter of `class`
/// and `strength` wears, as a fraction.
#[must_use]
pub fn magic_bonus(items: &[ItemStack], class: Option<HunterClass>, strength: u32) -> f32 {
    items
        .iter()
        .filter(|stack| wearable(stack, class) && strong_enough(stack, strength))
        .filter_map(artifact::of)
        .map(ArtifactId::magic_bonus)
        .sum()
}

/// Returns the attribute bonuses of the gear `items` a hunter of `rank`,
/// `class` and `strength` can use, each with an id unique to its gear slot.
#[must_use]
pub fn modifiers(
    items: &[ItemStack],
    rank: HunterRank,
    class: Option<HunterClass>,
    strength: u32,
) -> Vec<(AttributeRef, AttributeModifier)> {
    items
        .iter()
        .enumerate()
        .filter(|(_, stack)| {
            artifact::of(stack).is_some()
                && artifact::usable_by(stack, rank)
                && wearable(stack, class)
                && strong_enough(stack, strength)
        })
        .flat_map(|(index, stack)| {
            stack
                .get(ATTRIBUTE_MODIFIERS)
                .map_or_else(Vec::new, |component| component.modifiers.clone())
                .into_iter()
                .map(move |entry| {
                    (
                        entry.attribute,
                        AttributeModifier {
                            id: from_sololeveling(format!("gear/{index}/{}", entry.id.path)),
                            amount: entry.amount,
                            operation: entry.operation,
                        },
                    )
                })
        })
        .collect()
}

/// Bonus ids last applied to one hunter, so a refresh can take them off.
pub type AppliedGear = Vec<(AttributeRef, Identifier)>;

/// Opens the equipment menu.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    let uuid = player.gameprofile.id();
    let Some(items) = state.with_session_mut(uuid, |session| session.gear.clone()) else {
        return;
    };
    let inventory = player.inventory.lock();
    let choices: Vec<ItemStack> = (0..PlayerInventory::INVENTORY_SIZE)
        .map(|slot| inventory.get_item(slot).clone())
        .collect();
    drop(inventory);
    let mut contents = vec![ItemStack::empty(); 54];
    for (index, kind) in LAYOUT.iter().enumerate() {
        contents[index] = items
            .get(index)
            .filter(|item| !item.is_empty())
            .cloned()
            .unwrap_or_else(|| label(*kind));
    }
    for (slot, stack) in choices.iter().enumerate() {
        if kind_of(stack).is_some() {
            contents[9 + slot] = stack.clone();
        }
    }
    contents[53] = info();
    open_guarded_buttons(
        player,
        TextComponent::plain("Снаряжение").color(Color::DarkPurple),
        6,
        contents,
        Box::new(move |player, slot, _| {
            let request = if slot < GEAR_SLOTS {
                items
                    .get(slot)
                    .filter(|stack| !stack.is_empty())
                    .map(|stack| GearTransfer::Unequip {
                        index: slot,
                        expected: stack.clone(),
                    })
            } else if slot >= 9 {
                let inventory_slot = slot - 9;
                choices
                    .get(inventory_slot)
                    .filter(|stack| kind_of(stack).is_some())
                    .map(|stack| GearTransfer::Equip {
                        slot: inventory_slot,
                        expected: stack.clone(),
                    })
            } else {
                None
            };
            if let Some(request) = request {
                state.request_gear_transfer(player, request);
            }
            After::Keep
        }),
    );
}

#[derive(Debug)]
pub(crate) enum GearTransfer {
    Equip { slot: usize, expected: ItemStack },
    Unequip { index: usize, expected: ItemStack },
}

fn label(kind: GearKind) -> ItemStack {
    with_lore(
        named(
            ItemStack::new(kind.icon()),
            TextComponent::plain(kind.display_name()).color(Color::Gold),
        ),
        vec![TextComponent::plain("Выберите подходящий артефакт ниже.").color(Color::Gray)],
    )
}

fn info() -> ItemStack {
    with_lore(
        named(
            ItemStack::new(&vanilla_items::BOOK),
            TextComponent::plain("Снаряжение Системы").color(Color::DarkPurple),
        ),
        vec![
            TextComponent::plain("Артефакты здесь не надеваются на тело:").color(Color::Gray),
            TextComponent::plain("они дают только бонусы и способности.").color(Color::Gray),
            TextComponent::plain("Артефакт выше вашего ранга не действует.").color(Color::Gray),
        ],
    )
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;
    use uuid::Uuid;

    use super::{
        GEAR_SLOTS, GearKind, LAYOUT, SavedGear, abilities, free_slot, kind_of, modifiers,
    };
    use crate::item::artifact::{self, ArtifactId};
    use crate::item::serial;
    use crate::rank::HunterRank;
    use steel_registry::item_stack::ItemStack;

    #[test]
    fn gloves_take_the_gloves_slot_not_the_chest() {
        init_vanilla_registry();
        let gloves = artifact::stack(ArtifactId::HighKnightsGauntlet, None);
        assert_eq!(kind_of(&gloves), Some(GearKind::Gloves));
        let sword = artifact::stack(ArtifactId::KnightsKiller, None);
        assert_eq!(kind_of(&sword), None);
        assert_eq!(LAYOUT.len(), GEAR_SLOTS);
    }

    #[test]
    fn serial_items_are_not_system_gear() {
        init_vanilla_registry();
        for id in serial::ALL {
            let stack = serial::stack(id, None);
            assert_eq!(kind_of(&stack), None, "{id:?}");
            let items: Vec<_> = (0..GEAR_SLOTS).map(|_| ItemStack::empty()).collect();
            assert_eq!(free_slot(&items, &stack), None);
        }
    }

    #[test]
    fn catalog_artifacts_only_fit_their_free_compatible_slots() {
        init_vanilla_registry();
        for id in artifact::ALL {
            let stack = artifact::stack(id, None);
            let items = vec![ItemStack::empty(); GEAR_SLOTS];
            match kind_of(&stack) {
                Some(kind) => {
                    let slot = free_slot(&items, &stack).expect("matching slot");
                    assert_eq!(LAYOUT[slot], kind);
                    let filled = LAYOUT
                        .iter()
                        .map(|allowed| {
                            if *allowed == kind {
                                stack.clone()
                            } else {
                                ItemStack::empty()
                            }
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(free_slot(&filled, &stack), None);
                }
                None => assert_eq!(free_slot(&items, &stack), None),
            }
        }
    }

    #[test]
    fn account_saves_preserve_undecodable_and_unsupported_gear_slots() {
        init_vanilla_registry();
        let corrupted = vec![0xFF, 0x00, 0x7F];
        let unsupported = vec![0x01, 0x02];
        let previous = SavedGear {
            slots: vec![Some(corrupted.clone()), None, Some(unsupported.clone())],
        };
        let items = vec![ItemStack::empty(); GEAR_SLOTS];

        let saved = SavedGear::from_items_preserving(&items, &previous);

        assert_eq!(saved.slots[0], Some(corrupted));
        assert_eq!(saved.slots[2], Some(unsupported));
    }

    #[test]
    fn a_loadout_round_trips_and_its_bonuses_follow_the_rank() {
        init_vanilla_registry();
        let mut items = vec![ItemStack::empty(); GEAR_SLOTS];
        items[0] = artifact::stack(ArtifactId::RedKnightsHelmet, None);
        items[3] = artifact::stack(ArtifactId::AssassinsShoes, Some(Uuid::new_v4()));
        let restored = SavedGear::from_items(&items).items();
        assert_eq!(restored, items);

        // The unbound legendary helmet needs rank A; the bound shoes do not.
        assert_eq!(abilities(&items, HunterRank::D, None).len(), 0);
        assert_ne!(abilities(&items, HunterRank::A, None).len(), 0);
        let low = modifiers(&items, HunterRank::D, None, 0);
        assert!(!low.is_empty(), "the shoes still give their bonus");
        assert!(modifiers(&items, HunterRank::A, None, 0).len() > low.len());
    }
}
