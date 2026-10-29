//! Placing and removing Adventurer Organization boards.

use std::sync::Arc;

use steel_core::command::{
    CommandError, CommandRegistration, CommandRegistrationError, CommandRegistry, literal,
};
use steel_core::entity::Entity as _;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_blocks;
use steel_utils::types::UpdateFlags;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::identifier::from_sololeveling;
use crate::state::SoloLevelingState;

/// Blocks around the operator searched by `/organization board remove`.
const REMOVE_RADIUS: i32 = 3;

/// Registers the operator-only `/organization board place|remove`.
pub fn register(
    registry: &mut CommandRegistry,
    state: Arc<SoloLevelingState>,
) -> Result<(), CommandRegistrationError> {
    let command = CommandRegistration::new(from_sololeveling("organization"), move || {
        let place_state = Arc::clone(&state);
        let remove_state = Arc::clone(&state);
        literal("organization").then(
            literal("board")
                .then(literal("place").executes(move |context| {
                    let player = context
                        .source()
                        .player()
                        .ok_or_else(|| CommandError::from("Команда доступна только игроку."))?;
                    let world = player.get_world();
                    let pos = player.block_position();
                    if !world.get_block_state(pos).is_air() {
                        return Err(CommandError::from(
                            "Встаньте на свободное место: доска ставится у ваших ног.",
                        ));
                    }
                    if !world.set_block(
                        pos,
                        vanilla_blocks::LECTERN.default_state(),
                        UpdateFlags::UPDATE_ALL,
                    ) {
                        return Err(CommandError::from("Не удалось поставить кафедру."));
                    }
                    place_state
                        .add_organization_board(&world.key, pos)
                        .map_err(|error| {
                            CommandError::from(format!("Доска не сохранена: {error}"))
                        })?;
                    player.send_message(
                        &TextComponent::plain(
                            "Доска Организации поставлена. Правый клик открывает задания.",
                        )
                        .color(Color::Green),
                    );
                    Ok(1)
                }))
                .then(literal("remove").executes(move |context| {
                    let player = context
                        .source()
                        .player()
                        .ok_or_else(|| CommandError::from("Команда доступна только игроку."))?;
                    let world = player.get_world();
                    let removed = remove_state
                        .remove_organization_boards_near(
                            &world.key,
                            player.block_position(),
                            REMOVE_RADIUS,
                        )
                        .map_err(|error| {
                            CommandError::from(format!("Список досок не сохранён: {error}"))
                        })?;
                    if removed.is_empty() {
                        return Err(CommandError::from("Рядом нет досок Организации."));
                    }
                    for pos in &removed {
                        world.set_block(
                            *pos,
                            vanilla_blocks::AIR.default_state(),
                            UpdateFlags::UPDATE_ALL,
                        );
                    }
                    player.send_message(
                        &TextComponent::plain(format!("Убрано досок: {}.", removed.len()))
                            .color(Color::Green),
                    );
                    Ok(1)
                })),
        )
    });
    registry.register(command)?;
    Ok(())
}
