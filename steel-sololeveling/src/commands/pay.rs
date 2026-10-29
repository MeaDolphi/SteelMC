//! Online-player Won transfers.

use std::sync::Arc;

use steel_core::command::{
    CommandArgument, CommandError, CommandRegistration, CommandRegistrationError, CommandRegistry,
    argument, literal,
};
use text_components::TextComponent;

use crate::economy::PaymentError;
use crate::identifier::from_sololeveling;
use crate::state::SoloLevelingState;

/// Registers the player-facing payment command.
pub fn register(
    registry: &mut CommandRegistry,
    state: Arc<SoloLevelingState>,
) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(from_sololeveling("pay"), move || {
            let state = Arc::clone(&state);
            literal("pay").then(argument("player", CommandArgument::player()).then(
                argument("amount", CommandArgument::word()).executes(move |context| {
                    let player = context
                        .source()
                        .player()
                        .ok_or_else(|| CommandError::from("Команда доступна только игроку."))?;
                    let recipient = context.player("player")?;
                    let amount = context.string("amount")?.parse::<u64>().map_err(|_| {
                        CommandError::from("Укажите положительное целое количество Won.")
                    })?;
                    state
                        .request_payment(player, &recipient, amount)
                        .map_err(payment_error)?;
                    player.send_message(&TextComponent::plain("Перевод обрабатывается."));
                    Ok(1)
                }),
            ))
        })
        .default_access(),
    )?;
    Ok(())
}

fn payment_error(error: PaymentError) -> CommandError {
    CommandError::from(match error {
        PaymentError::Unavailable => "Оба игрока должны быть онлайн с загруженной сессией.",
        PaymentError::SelfPayment => "Нельзя перевести Won самому себе.",
        PaymentError::ZeroAmount => "Количество Won должно быть больше нуля.",
        PaymentError::InsufficientFunds => "Недостаточно Won.",
        PaymentError::Overflow => "Баланс получателя не вмещает перевод.",
        PaymentError::Storage => "Не удалось сохранить перевод. Обратитесь к администратору.",
        PaymentError::CommitUncertain => {
            "Результат сохранения перевода не подтверждён. Не повторяйте перевод; обратитесь к администратору."
        }
    })
}
