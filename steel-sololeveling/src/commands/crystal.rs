//! Selling magic crystals to the Adventurer Organization.

use std::sync::Arc;

use steel_core::command::{
    CommandError, CommandRegistration, CommandRegistrationError, CommandRegistry, literal,
};
use steel_core::player::Player;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::identifier::from_sololeveling;
use crate::item::crystal::{CrystalSale, CrystalSaleError};
use crate::state::SoloLevelingState;

/// Registers `/crystals sell`.
pub fn register(
    registry: &mut CommandRegistry,
    state: Arc<SoloLevelingState>,
) -> Result<(), CommandRegistrationError> {
    let command = CommandRegistration::new(from_sololeveling("crystals"), move || {
        let state = Arc::clone(&state);
        literal("crystals").then(literal("sell").executes(move |context| {
            let player = context
                .source()
                .player()
                .ok_or_else(|| CommandError::from("Команда доступна только игроку."))?;
            let sale = state
                .sell_crystals(player)
                .map_err(|error| CommandError::from(refusal(error)))?;
            announce(player, sale);
            Ok(1)
        }))
    })
    .default_access();
    registry.register(command)?;
    Ok(())
}

/// Sells a hunter's crystals and reports the result in chat.
pub fn sell(state: &SoloLevelingState, player: &Player) {
    match state.sell_crystals(player) {
        Ok(sale) => announce(player, sale),
        Err(error) => {
            player.send_message(&TextComponent::plain(refusal(error)).color(Color::Red));
        }
    }
}

fn announce(player: &Player, sale: CrystalSale) {
    player.send_message(
        &TextComponent::plain(format!(
            "Организация авантюристов купила {} кристалл(ов) за {} Won.",
            sale.count, sale.total
        ))
        .color(Color::Gold),
    );
}

const fn refusal(error: CrystalSaleError) -> &'static str {
    match error {
        CrystalSaleError::NoCrystals => "В инвентаре нет магических кристаллов.",
        CrystalSaleError::Storage => "Организация не может рассчитаться сейчас. Попробуйте позже.",
    }
}
