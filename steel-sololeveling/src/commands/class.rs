//! `/class`: the hunter class menu and `/class <name>` to pick one.

use std::sync::Arc;

use steel_core::command::{
    CommandArgument, CommandError, CommandRegistration, CommandRegistrationError, CommandRegistry,
    argument, literal,
};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::class::HunterClass;
use crate::class_menu;
use crate::identifier::from_sololeveling;
use crate::state::SoloLevelingState;

/// Registers `/class` (the class menu) and `/class <name>`.
pub fn register(
    registry: &mut CommandRegistry,
    state: Arc<SoloLevelingState>,
) -> Result<(), CommandRegistrationError> {
    let command = CommandRegistration::new(from_sololeveling("class"), move || {
        let list_state = Arc::clone(&state);
        let pick_state = Arc::clone(&state);
        literal("class")
            .executes(move |context| {
                let Some(player) = context.source().player() else {
                    return Err(CommandError::from("Команда доступна только игроку."));
                };
                class_menu::open(player, Arc::clone(&list_state));
                Ok(1)
            })
            .then(
                argument("name", CommandArgument::word()).executes(move |context| {
                    let player = context
                        .source()
                        .player()
                        .ok_or_else(|| CommandError::from("Команда доступна только игроку."))?;
                    let name = context.string("name")?;
                    let chosen = HunterClass::parse(name).ok_or_else(|| {
                        CommandError::from("Неизвестный класс. /class — список классов.")
                    })?;
                    let price = pick_state
                        .choose_class(player, chosen)
                        .map_err(|error| CommandError::from(error.message()))?;
                    let paid = if price > 0 {
                        format!(" Списано {price} Won.")
                    } else {
                        String::new()
                    };
                    player.send_message(
                        &TextComponent::plain(format!(
                            "Ваш класс: {}.{paid}",
                            chosen.display_name()
                        ))
                        .color(Color::Gold),
                    );
                    Ok(1)
                }),
            )
    })
    .default_access();
    registry.register(command)?;
    Ok(())
}
