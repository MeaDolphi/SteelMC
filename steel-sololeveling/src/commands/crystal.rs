//! Selling magic crystals to the Adventurer Organization.

use std::sync::Arc;

use steel_core::command::{
    CommandError, CommandRegistration, CommandRegistrationError, CommandRegistry, literal,
};
use text_components::TextComponent;

use crate::identifier::from_sololeveling;
use crate::item::crystal::CrystalSaleError;
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
            state
                .sell_crystals(player)
                .map_err(|error| CommandError::from(refusal(error)))?;
            player.send_message(&TextComponent::plain("Продажа кристаллов обрабатывается."));
            Ok(1)
        }))
    })
    .default_access();
    registry.register(command)?;
    Ok(())
}

const fn refusal(error: CrystalSaleError) -> &'static str {
    match error {
        CrystalSaleError::NoCrystals => "В инвентаре нет магических кристаллов.",
        CrystalSaleError::Storage => "Организация не может рассчитаться сейчас. Попробуйте позже.",
    }
}
