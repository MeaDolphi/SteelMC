//! Personal book that activates class and general combat skills.

use steel_core::inventory::container::Container as _;
use steel_core::player::Player;
use steel_core::player::player_inventory::Prediction;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::{binding, named, read_kind, record, with_lore, write_tag};

const KIND: &str = "combat_skill_book";
const MODEL: &str = "system_book";

#[must_use]
pub fn stack(owner: Uuid) -> ItemStack {
    let mut stack = with_lore(
        named(
            ItemStack::new(&vanilla_items::BOOK),
            TextComponent::plain("Книга боевых навыков").color(Color::Gold),
        ),
        vec![
            TextComponent::plain("ПКМ — классовый навык.").color(Color::Gray),
            TextComponent::plain("Shift + ПКМ — общий навык.").color(Color::Gray),
            TextComponent::plain("Набор меняется в меню «Навыки».").color(Color::Gray),
        ],
    );
    write_tag(&mut stack, record(KIND));
    super::model::apply(&mut stack, MODEL);
    binding::bound(stack, owner)
}

#[must_use]
pub fn is_combat_skill_book(stack: &ItemStack) -> bool {
    read_kind(stack, KIND).is_some()
}

pub fn keep_in_place(player: &Player) {
    if player.inventory_transaction_active() {
        return;
    }
    let mut found = false;
    {
        let mut inventory = player.inventory.lock();
        for slot in 0..inventory.get_container_size() {
            if !is_combat_skill_book(inventory.get_item(slot)) {
                continue;
            }
            if found {
                inventory.set_item(slot, ItemStack::empty());
            } else {
                found = true;
            }
        }
    }
    if !found {
        player.add_item_or_drop(stack(player.gameprofile.id()), Prediction::ServerOnly);
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;
    use uuid::Uuid;

    use super::{is_combat_skill_book, stack};
    use crate::item::binding;

    #[test]
    fn book_is_personal_and_recognised() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let book = stack(owner);
        assert!(is_combat_skill_book(&book));
        assert_eq!(binding::bound_owner(&book), Some(owner));
        assert!(!binding::may_drop(&book));
    }
}
