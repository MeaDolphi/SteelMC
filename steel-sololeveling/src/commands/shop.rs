//! The personal System shop.

use std::sync::Arc;

use steel_core::command::{
    CommandArgument, CommandError, CommandRegistration, CommandRegistrationError, CommandRegistry,
    argument, literal,
};
use steel_core::player::Player;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::daily;
use crate::identifier::from_sololeveling;
use crate::shop::{self, ShopError};
use crate::state::SoloLevelingState;

/// Registers `/shop` and `/shop buy <slot>`.
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
                show(&list_state, player);
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
                        let offer = shop::offer(slot).ok_or_else(|| {
                            CommandError::from(format!(
                                "Укажите номер слота от 0 до {}.",
                                shop::SLOTS - 1
                            ))
                        })?;
                        buy_state
                            .buy_shop_offer(player, slot)
                            .map_err(purchase_error)?;
                        player.send_message(
                            &TextComponent::plain(format!("Куплено: {}.", offer.display_name()))
                                .color(Color::Green),
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

/// Prints every offer with its price and today's purchase state.
fn show(state: &SoloLevelingState, player: &Arc<Player>) {
    let uuid = player.gameprofile.id();
    let today = daily::today();
    let bought = state
        .with_session_mut(uuid, |session| {
            let mut day = session.shop;
            day.roll_over(today);
            day
        })
        .unwrap_or_default();
    let points = state
        .with_session_mut(uuid, |session| session.system_points.amount())
        .unwrap_or(0);
    player.send_message(
        &TextComponent::plain(format!("Магазин Системы. Очков: {points}.")).color(Color::Gold),
    );
    for offer in shop::OFFERS {
        let state_text = if bought.bought(offer.slot) {
            "куплено сегодня".to_owned()
        } else {
            format!("{} очков", offer.price)
        };
        player.send_message(
            &TextComponent::plain(format!(
                "[{}] {} — {state_text}",
                offer.slot,
                offer.display_name()
            ))
            .color(if bought.bought(offer.slot) {
                Color::DarkGray
            } else {
                Color::Yellow
            }),
        );
    }
    player.send_message(&TextComponent::plain("Покупка: /shop buy <слот>.").color(Color::Gray));
}

fn purchase_error(error: ShopError) -> CommandError {
    CommandError::from(match error {
        ShopError::UnknownSlot => "Такого слота в магазине нет.",
        ShopError::NoSession => "Сессия охотника не загружена.",
        ShopError::AlreadyBought => "Этот слот уже куплен сегодня.",
        ShopError::Busy => "Дождитесь завершения другой финансовой операции.",
        ShopError::InsufficientPoints => "Недостаточно очков Системы.",
        ShopError::Storage => "Не удалось сохранить покупку. Обратитесь к администратору.",
    })
}
