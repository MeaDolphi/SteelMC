//! The Mage's grimoire: the personal book spells are cast from.
//!
//! Right click casts the selected spell, sneak and right click opens the
//! grimoire, and the drop key cycles the prepared spells; the book itself
//! never leaves its owner.

use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::{binding, named, read_kind, record, with_lore, write_tag};

/// Themed kind of the grimoire.
const KIND: &str = "grimoire";
/// Pack model of the grimoire.
const MODEL: &str = "grimoire";

/// Creates a grimoire bound to `owner`.
#[must_use]
pub fn stack(owner: Uuid) -> ItemStack {
    let mut stack = with_lore(
        named(
            ItemStack::new(&vanilla_items::BOOK),
            TextComponent::plain("Гримуар Системы").color(Color::LightPurple),
        ),
        vec![
            TextComponent::plain("ПКМ — применить выбранное заклинание.").color(Color::Gray),
            TextComponent::plain("Q — следующее заклинание.").color(Color::Gray),
            TextComponent::plain("Shift + ПКМ — открыть гримуар.").color(Color::Gray),
        ],
    );
    write_tag(&mut stack, record(KIND));
    super::model::apply(&mut stack, MODEL);
    binding::bound(stack, owner)
}

/// Returns whether `stack` is a grimoire.
#[must_use]
pub fn is_grimoire(stack: &ItemStack) -> bool {
    read_kind(stack, KIND).is_some()
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;
    use uuid::Uuid;

    use super::{is_grimoire, stack};
    use crate::item::binding;

    #[test]
    fn the_grimoire_is_personal_and_recognised() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let book = stack(owner);
        assert!(is_grimoire(&book));
        assert_eq!(binding::bound_owner(&book), Some(owner));
        assert!(!binding::may_drop(&book));
    }
}
