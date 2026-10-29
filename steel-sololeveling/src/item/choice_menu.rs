//! The three-option menu of a choice chest.
//!
//! The options are read from the chest stack itself, so the menu only shows
//! what the stack already carries and never rerolls it.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_core::player::player_inventory::{PlayerInventory, Prediction};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_menu_types;
use steel_utils::locks::IntoShared;
use steel_utils::types::InteractionHand;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::chest::{self, ChestKind, Reward};

/// Opens the choice menu of a chest the player is holding in `hand`.
pub fn open(player: &Arc<Player>, hand: InteractionHand, options: &[Reward; 3]) {
    let title = TextComponent::plain("Выбор награды").color(Color::LightPurple);
    let options = *options;
    player.open_menu(title, move |context| {
        build(context.container_id, context.player, hand, &options)
    });
}

fn build(container_id: u8, player: &Player, hand: InteractionHand, options: &[Reward; 3]) -> Menu {
    let container = SimpleContainer::from_items(menu_items(options)).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X1, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(ChoiceMenuKind {
        buttons,
        container,
        hand,
        options: *options,
    })
}

fn menu_items(options: &[Reward; 3]) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); 9];
    for (index, option) in options.iter().enumerate() {
        if let Some(slot) = items.get_mut(index) {
            *slot = option.stack(None);
        }
    }
    items
}

struct ChoiceMenuKind {
    buttons: Section,
    container: Shared<SimpleContainer>,
    hand: InteractionHand,
    options: [Reward; 3],
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for ChoiceMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/choice_chest");
}

impl MenuKind for ChoiceMenuKind {
    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        click: Click,
        player: &Player,
    ) -> ClickOutcome {
        let Click::Pickup {
            slot,
            button: MouseButton::Left,
        } = click
        else {
            return if click.slot().is_some_and(|slot| self.buttons.contains(slot)) {
                ClickOutcome::Consume
            } else {
                ClickOutcome::Fallthrough
            };
        };
        if !self.buttons.contains(slot) {
            return ClickOutcome::Fallthrough;
        }
        let Some(option) = slot
            .checked_sub(self.buttons.start())
            .and_then(|index| self.options.get(index))
            .copied()
        else {
            return ClickOutcome::Consume;
        };

        let inventory_id = ContainerId::from_arc(&player.inventory);
        let taken = {
            let Some(inventory) = guard.get_typed_mut::<PlayerInventory>(inventory_id) else {
                return ClickOutcome::Consume;
            };
            if chest::kind_of(inventory.get_item_in_hand(self.hand)) != Some(ChestKind::Choice) {
                return ClickOutcome::Consume;
            }
            let mut held = inventory.get_item_in_hand(self.hand).clone();
            if held.count > 1 {
                held.count -= 1;
            } else {
                held = ItemStack::empty();
            }
            inventory.set_item_in_hand(self.hand, held);
            true
        };
        if !taken {
            return ClickOutcome::Consume;
        }

        // The chest is spent, so an already open menu must not offer a second
        // reward; every option slot is cleared before the player closes it.
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for index in 0..container.get_container_size() {
                container.set_item(index, ItemStack::empty());
            }
        }
        player.add_item_or_drop_with_guard(guard, option.stack(None), Prediction::ServerOnly);
        player.send_message(
            &TextComponent::plain(format!("Получено: {}.", option.display_name()))
                .color(Color::LightPurple),
        );
        ClickOutcome::Consume
    }
}
