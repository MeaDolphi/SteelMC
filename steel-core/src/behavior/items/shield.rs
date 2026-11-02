use std::borrow::Cow;

use steel_macros::item_behavior;
use steel_registry::{data_components::vanilla_components::BASE_COLOR, item_stack::ItemStack};
use text_components::TextComponent;

use crate::behavior::{InteractionResult, ItemBehavior, UseItemContext};

use super::dynamic_name::{default_name, description_id, translated};

/// Shield behavior providing the base-color-specific name.
#[item_behavior]
pub struct ShieldItem;

impl ItemBehavior for ShieldItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        context.player.start_using_item(context.hand);
        InteractionResult::Consume
    }

    fn get_name<'a>(&self, stack: &'a ItemStack) -> Cow<'a, TextComponent> {
        let Some(color) = stack.get(BASE_COLOR) else {
            return default_name(stack);
        };
        let Some(description_id) = description_id(stack) else {
            return default_name(stack);
        };
        translated(
            format!("{description_id}.{}", color.serialized_name()),
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_registry::{item_stack::ItemStack, vanilla_items};
    use steel_utils::types::InteractionHand;

    use super::ShieldItem;
    use crate::behavior::{InteractionResult, ItemBehavior as _, UseItemContext, init_behaviors};
    use crate::entity::SharedEntity;
    use crate::inventory::container::Container as _;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world};

    #[test]
    fn shield_use_starts_and_release_stops_the_client_visible_block() {
        let world = fresh_test_world("shield-use");
        let player = TestPlayerBuilder::new(world.world.clone(), "Guardian", 1).build();
        player
            .inventory
            .lock()
            .set_item(40, ItemStack::new(&vanilla_items::SHIELD));
        init_behaviors();
        let mut context = UseItemContext::new(
            &player,
            InteractionHand::OffHand,
            &world.world,
            Arc::clone(&player.inventory),
        );
        assert_eq!(
            ShieldItem.use_item(&mut context),
            InteractionResult::Consume
        );
        assert_eq!(
            player.active_item_use_hand(),
            Some(InteractionHand::OffHand)
        );
        let entity: SharedEntity = player.clone();
        player.release_using_item(&entity);
        assert_eq!(player.active_item_use_hand(), None);
    }
}
