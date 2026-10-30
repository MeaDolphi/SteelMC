//! `/enchant`: the item maker, starting with the artifact picker.

use std::sync::Arc;

use steel_core::command::{
    CommandError, CommandRegistration, CommandRegistrationError, CommandRegistry, literal,
};

use crate::enchant_menu;
use crate::identifier::from_sololeveling;
use crate::state::SoloLevelingState;

/// Registers `/enchant`.
pub fn register(
    registry: &mut CommandRegistry,
    state: Arc<SoloLevelingState>,
) -> Result<(), CommandRegistrationError> {
    let command = CommandRegistration::new(from_sololeveling("enchant"), move || {
        let state = Arc::clone(&state);
        literal("enchant").executes(move |context| {
            let Some(player) = context.source().player() else {
                return Err(CommandError::from("Команда доступна только игроку."));
            };
            enchant_menu::open_picker(player, Arc::clone(&state));
            Ok(1)
        })
    })
    .default_access();
    registry.register(command)?;
    Ok(())
}
