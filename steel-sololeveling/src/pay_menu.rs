//! Won transfers as menus: pick a hunter online, then the amount.

use std::sync::Arc;

use steel_core::player::Player;
use text_components::format::Color;

use crate::economy::PaymentError;
use crate::state::SoloLevelingState;
use crate::ui;

/// Returns the Russian message for a refused transfer.
#[must_use]
pub const fn payment_message(error: PaymentError) -> &'static str {
    match error {
        PaymentError::Unavailable => "Оба игрока должны быть онлайн с загруженной сессией.",
        PaymentError::SelfPayment => "Нельзя перевести Won самому себе.",
        PaymentError::ZeroAmount => "Количество Won должно быть больше нуля.",
        PaymentError::InsufficientFunds => "Недостаточно Won.",
        PaymentError::Overflow => "Баланс получателя не вмещает перевод.",
        PaymentError::Storage => "Не удалось сохранить перевод. Обратитесь к администратору.",
        PaymentError::CommitUncertain => {
            "Результат сохранения перевода не подтверждён. Не повторяйте перевод; обратитесь к администратору."
        }
    }
}

/// Starts a transfer: the recipient first, then the amount.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    let balance = state
        .with_session_mut(player.gameprofile.id(), |session| session.won.amount())
        .unwrap_or(0);
    if balance == 0 {
        ui::tell(player, "У вас нет Won для перевода.", Color::Red);
        return;
    }
    let pick_state = Arc::clone(&state);
    ui::pick_player(
        player,
        &state,
        "Кому перевести Won",
        &[],
        Arc::new(move |player, recipient, name| {
            let pay_state = Arc::clone(&pick_state);
            ui::pick_amount(
                player,
                &format!("Перевод: {name}"),
                "Won",
                balance.min(100),
                1,
                balance,
                Arc::new(move |player, amount| {
                    let online = pay_state.online_players();
                    let find = |uuid| {
                        online
                            .iter()
                            .find(|online| online.gameprofile.id() == uuid)
                            .cloned()
                    };
                    let (Some(sender), Some(target)) =
                        (find(player.gameprofile.id()), find(recipient))
                    else {
                        ui::tell(
                            player,
                            payment_message(PaymentError::Unavailable),
                            Color::Red,
                        );
                        player.close_container();
                        return;
                    };
                    match pay_state.request_payment(&sender, &target, amount) {
                        Ok(()) => ui::tell(
                            player,
                            &format!("Перевод {amount} Won для {name} обрабатывается."),
                            Color::Gold,
                        ),
                        Err(error) => ui::tell(player, payment_message(error), Color::Red),
                    }
                    player.close_container();
                }),
            );
        }),
    );
}
