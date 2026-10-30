//! The personal System shop.

use std::sync::Arc;

use steel_core::command::{
    CommandArgument, CommandError, CommandRegistration, CommandRegistrationError, CommandRegistry,
    argument, literal,
};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::identifier::from_sololeveling;
use crate::shop::{self, ShopError};
use crate::shop_menu;
use crate::state::SoloLevelingState;

/// Registers `/shop` (the shop menu) and `/shop buy <slot>`.
pub fn register(
    registry: &mut CommandRegistry,
    state: Arc<SoloLevelingState>,
) -> Result<(), CommandRegistrationError> {
    let shop_state = Arc::clone(&state);
    let list = CommandRegistration::new(from_sololeveling("shop"), move || {
        let list_state = Arc::clone(&shop_state);
        let buy_state = Arc::clone(&shop_state);
        literal("shop")
            .executes(move |context| {
                let Some(player) = context.source().player() else {
                    return Err(CommandError::from("Команда доступна только игроку."));
                };
                shop_menu::open(player, Arc::clone(&list_state));
                Ok(1)
            })
            .then(
                literal("buy").then(argument("slot", CommandArgument::word()).executes(
                    move |context| {
                        let player = context
                            .source()
                            .player()
                            .ok_or_else(|| CommandError::from("Команда доступна только игроку."))?;
                        let slot = context.string("slot")?.parse::<usize>().map_err(|_| {
                            CommandError::from(format!(
                                "Укажите номер слота от 0 до {}.",
                                shop::SLOTS - 1
                            ))
                        })?;
                        buy_state
                            .request_shop_purchase(player, slot, false)
                            .map_err(purchase_error)?;
                        player.send_message(
                            &TextComponent::plain("Покупка обрабатывается.").color(Color::Gold),
                        );
                        Ok(1)
                    },
                )),
            )
    })
    .default_access();
    registry.register(list)?;
    Ok(())
}

fn purchase_error(error: ShopError) -> CommandError {
    CommandError::from(error.message())
}
