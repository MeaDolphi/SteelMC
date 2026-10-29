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

use super::binding;
use super::chest::{self, ChestKind, Reward};
use uuid::Uuid;

/// Opens the choice menu of a chest the player is holding in `hand`.
pub fn open(
    player: &Arc<Player>,
    hand: InteractionHand,
    bound_owner: Option<Uuid>,
    options: &[Reward; 3],
) {
    let title = TextComponent::plain("Выбор награды").color(Color::LightPurple);
    let options = *options;
    player.open_menu(title, move |context| {
        build(
            context.container_id,
            context.player,
            hand,
            bound_owner,
            &options,
        )
    });
}

fn build(
    container_id: u8,
    player: &Player,
    hand: InteractionHand,
    bound_owner: Option<Uuid>,
    options: &[Reward; 3],
) -> Menu {
    let container = SimpleContainer::from_items(menu_items(options, bound_owner)).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X1, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(ChoiceMenuKind {
        buttons,
        container,
        hand,
        bound_owner,
        options: *options,
    })
}

fn menu_items(options: &[Reward; 3], bound_owner: Option<Uuid>) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); 9];
    for (index, option) in options.iter().enumerate() {
        if let Some(slot) = items.get_mut(index) {
            *slot = reward_stack(*option, bound_owner);
        }
    }
    items
}

fn reward_stack(option: Reward, bound_owner: Option<Uuid>) -> ItemStack {
    let mut stack = option.stack(None);
    if let Some(owner) = bound_owner {
        binding::bind(&mut stack, owner);
    }
    stack
}

fn matches_open_chest(held: &ItemStack, options: &[Reward; 3], bound_owner: Option<Uuid>) -> bool {
    chest::kind_of(held) == Some(ChestKind::Choice)
        && chest::stored_options(held) == Some(*options)
        && binding::bound_owner(held) == bound_owner
}

fn can_fit_after_hand_shrink(
    inventory: &PlayerInventory,
    hand: InteractionHand,
    reward: &ItemStack,
) -> bool {
    let consumed_slot = match hand {
        InteractionHand::MainHand => usize::from(inventory.get_selected_slot()),
        InteractionHand::OffHand => 40,
    };
    (0..inventory.get_container_size()).any(|slot| {
        let mut current = inventory.get_item(slot).clone();
        if slot == consumed_slot {
            if current.count <= 1 {
                current = ItemStack::empty();
            } else {
                current.count -= 1;
            }
        }
        current.is_empty()
            || ItemStack::is_same_item_same_components(&current, reward)
                && current.count < current.max_stack_size()
    })
}

struct ChoiceMenuKind {
    buttons: Section,
    container: Shared<SimpleContainer>,
    hand: InteractionHand,
    bound_owner: Option<Uuid>,
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
        let reward = reward_stack(option, self.bound_owner);

        let inventory_id = ContainerId::from_arc(&player.inventory);
        let taken = {
            let Some(inventory) = guard.get_typed_mut::<PlayerInventory>(inventory_id) else {
                return ClickOutcome::Consume;
            };
            let held = inventory.get_item_in_hand(self.hand);
            if !matches_open_chest(held, &self.options, self.bound_owner) {
                return ClickOutcome::Consume;
            }
            if self.bound_owner.is_some()
                && !can_fit_after_hand_shrink(inventory, self.hand, &reward)
            {
                player.send_message(
                    &TextComponent::plain("Освободите место в инвентаре перед выбором.")
                        .color(Color::Red),
                );
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
        player.add_item_or_drop_with_guard(guard, reward, Prediction::ServerOnly);
        player.send_message(
            &TextComponent::plain(format!("Получено: {}.", option.display_name()))
                .color(Color::LightPurple),
        );
        ClickOutcome::Consume
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{matches_open_chest, reward_stack};
    use crate::item::{
        artifact::ArtifactId,
        binding,
        chest::{self, ChestKind, Reward},
        potion::BuffPotion,
    };

    fn options() -> [Reward; 3] {
        [
            Reward::Buff(BuffPotion::Speed),
            Reward::Weapon(ArtifactId::KnightsKiller),
            Reward::Shadow(1),
        ]
    }

    #[test]
    fn an_open_menu_only_consumes_the_matching_stored_choice() {
        let owner = Uuid::new_v4();
        let mut held = ChestKind::Choice.stack();
        binding::bind(&mut held, owner);
        chest::store_options(&mut held, &options());

        assert!(matches_open_chest(&held, &options(), Some(owner)));
        assert!(!matches_open_chest(&held, &options(), None));
        assert!(!matches_open_chest(&held, &options(), Some(Uuid::new_v4())));

        let changed = [
            Reward::Buff(BuffPotion::Strength),
            Reward::Weapon(ArtifactId::KnightsKiller),
            Reward::Shadow(1),
        ];
        assert!(!matches_open_chest(&held, &changed, Some(owner)));
        assert!(!matches_open_chest(
            &ChestKind::Choice.stack(),
            &options(),
            None
        ));
    }

    #[test]
    fn a_bound_chest_gives_a_bound_reward_to_its_owner() {
        let owner = Uuid::new_v4();
        let reward = reward_stack(Reward::Weapon(ArtifactId::KnightsKiller), Some(owner));
        assert_eq!(binding::bound_owner(&reward), Some(owner));
    }
}
