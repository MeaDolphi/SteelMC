//! Operator-only maintenance commands for the Solo Leveling game layer.

use std::sync::Arc;

use crate::admin_menu;
use steel_core::command::{
    CommandArgument, CommandContext, CommandError, CommandRegistration, CommandRegistrationError,
    CommandRegistry, CommandSource, argument, literal,
};
use steel_core::permission::{PermissionExpr, PermissionKey, PermissionState};
use steel_core::player::Player;
use steel_core::player::player_data_storage::transaction::TransactionCommit;
use steel_core::server::Server;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::identifier::from_sololeveling;
use crate::item::artifact::{self, ArtifactId};
use crate::item::serial::{self, SerialId};
use crate::state::SoloLevelingState;

/// Either a named artifact or a serial item, accepted by the admin give command.
#[derive(Debug, Clone, Copy)]
enum CatalogId {
    Artifact(ArtifactId),
    Serial(SerialId),
}

impl CatalogId {
    #[must_use]
    fn name(self) -> &'static str {
        match self {
            Self::Artifact(id) => id.name(),
            Self::Serial(id) => id.name(),
        }
    }
}

/// Registers `/sladmin shop refresh` and `/sladmin artifact give`.
#[expect(
    clippy::too_many_lines,
    reason = "the nested command tree is clearer kept together with shared operator guards"
)]
pub fn register(
    registry: &mut CommandRegistry,
    state: Arc<SoloLevelingState>,
) -> Result<(), CommandRegistrationError> {
    let command = CommandRegistration::new(from_sololeveling("sladmin"), move || {
        let state = Arc::clone(&state);
        literal("sladmin")
            .requires(operator_source)
            .then({
                let state = Arc::clone(&state);
                literal("shop")
                    .then({
                        let state = Arc::clone(&state);
                        literal("refresh").then(
                            argument("player", CommandArgument::player()).executes(
                                move |context| {
                                    require_operator(*context)?;
                                    let target = context.player("player")?;
                                    state
                                        .request_shop_refresh(target.gameprofile.id())
                                        .map_err(|error| CommandError::from(error.message()))?;
                                    operator_message(
                                        *context,
                                        "Ассортимент магазина поставлен в очередь обновления.",
                                    );
                                    Ok(1)
                                },
                            ),
                        )
                    })
                    .then({
                        let state = Arc::clone(&state);
                        literal("refresh-all").executes(move |context| {
                            require_operator(*context)?;
                            let mut queued = 0;
                            for target in context.source().server().get_players() {
                                if state.request_shop_refresh(target.gameprofile.id()).is_ok() {
                                    queued += 1;
                                }
                            }
                            operator_message(
                                *context,
                                &format!(
                                    "Обновление ассортимента поставлено в очередь для {queued} игроков."
                                ),
                            );
                            Ok(queued)
                        })
                    })
            })
            .then({
                let state = Arc::clone(&state);
                literal("menu").executes(move |context| {
                    require_operator(*context)?;
                    let Some(player) = context.source().player() else {
                        return Err(CommandError::from(
                            "Команда доступна только игроку.",
                        ));
                    };
                    admin_menu::open(player, Arc::clone(&state));
                    operator_message(*context, "Открыто админ-меню.");
                    Ok(1)
                })
            })
            .then({
                literal("artifact")
                    .then({
                        literal("give").then(
                            argument("player", CommandArgument::player()).then(
                                argument("artifact", CommandArgument::word())
                                    .executes(move |context| {
                                        require_operator(*context)?;
                                        let target = context.player("player")?;
                                        let id = catalog_id(*context)?;
                                        give_artifact(
                                            context.source().server(),
                                            &target,
                                            id,
                                            None,
                                        )?;
                                        operator_message(
                                            *context,
                                            &format!("Выдача предмета {} поставлена в очередь.{}", id.name(), rank_note(id)),
                                        );
                                        Ok(1)
                                    })
                                    .then(literal("personal").executes(move |context| {
                                        require_operator(*context)?;
                                        let target = context.player("player")?;
                                        let id = catalog_id(*context)?;
                                        give_artifact(
                                            context.source().server(),
                                            &target,
                                            id,
                                            Some(target.gameprofile.id()),
                                        )?;
                                        operator_message(
                                            *context,
                                            &format!("Выдача личного предмета {} поставлена в очередь.", id.name()),
                                        );
                                        Ok(1)
                                    })),
                            ),
                        )
                    })
                    .then({
                        literal("self").then(
                            argument("artifact", CommandArgument::word())
                                .executes(move |context| {
                                    require_operator(*context)?;
                                    let target = context
                                        .source()
                                        .player()
                                        .cloned()
                                        .ok_or_else(|| {
                                            CommandError::from(
                                                "Из консоли используйте /sladmin artifact give <игрок> <артефакт>.",
                                            )
                                        })?;
                                    let id = catalog_id(*context)?;
                                    give_artifact(
                                        context.source().server(),
                                        &target,
                                        id,
                                        None,
                                    )?;
                                    operator_message(
                                        *context,
                                        &format!("Выдача предмета {} поставлена в очередь.{}", id.name(), rank_note(id)),
                                    );
                                    Ok(1)
                                })
                                .then(literal("personal").executes(move |context| {
                                    require_operator(*context)?;
                                    let target = context
                                        .source()
                                        .player()
                                        .cloned()
                                        .ok_or_else(|| {
                                            CommandError::from(
                                                "Из консоли используйте /sladmin artifact give <игрок> <артефакт> personal.",
                                            )
                                        })?;
                                    let id = catalog_id(*context)?;
                                    give_artifact(
                                        context.source().server(),
                                        &target,
                                        id,
                                        Some(target.gameprofile.id()),
                                    )?;
                                    operator_message(
                                        *context,
                                        &format!("Выдача личного предмета {} поставлена в очередь.", id.name()),
                                    );
                                    Ok(1)
                                })),
                        )
                    })
                    .then(literal("list").executes(move |context| {
                        require_operator(*context)?;
                        let catalog = artifact::ALL
                            .into_iter()
                            .map(|id| format!("{} — {}", id.name(), id.artifact().name))
                            .chain(serial::ALL.into_iter().map(|id| format!("{} — {}", id.name(), id.serial().name)))
                            .collect::<Vec<_>>()
                            .join("\n");
                        operator_message(*context, &catalog);
                        Ok(1)
                    }))
            })
    })
    .default_access();
    registry.register(command)?;
    Ok(())
}

fn operator_source(source: CommandSource<'_>) -> bool {
    let Ok(key) = PermissionKey::parse("sololeveling.admin") else {
        return false;
    };
    source.permission_state(&PermissionExpr::key(key)) == Some(PermissionState::Allow)
}

fn require_operator(context: CommandContext<'_>) -> Result<(), CommandError> {
    if operator_source(context.source()) {
        Ok(())
    } else {
        Err(CommandError::from("Команда доступна только оператору."))
    }
}

fn operator_message(context: CommandContext<'_>, message: &str) {
    if let Some(player) = context.source().player() {
        player.send_message(&TextComponent::plain(message.to_owned()).color(Color::Gold));
    }
    log::info!("sololeveling operator: {message}");
}

fn catalog_id(context: CommandContext<'_>) -> Result<CatalogId, CommandError> {
    let name = context.string("artifact")?;
    if let Some(id) = ArtifactId::from_name(name) {
        return Ok(CatalogId::Artifact(id));
    }
    if let Some(id) = SerialId::from_name(name) {
        return Ok(CatalogId::Serial(id));
    }
    Err(CommandError::from(format!(
        "Неизвестный предмет «{name}». Используйте id из каталога артефактов или серийных вещей."
    )))
}

/// Reminds the operator that an unbound artifact obeys the rank gate.
fn rank_note(id: CatalogId) -> String {
    match id {
        CatalogId::Artifact(id) => id
            .artifact()
            .rarity
            .transferable_minimum_rank()
            .map_or_else(String::new, |rank| {
                format!(
                    " Без привязки он работает только с ранга {}; для проверки на любом ранге добавьте personal.",
                    rank.name()
                )
            }),
        CatalogId::Serial(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use steel_core::permission::{PermissionExpr, PermissionKey};

    #[test]
    fn sladmin_requires_a_real_operator_permission_key() {
        let key = PermissionKey::parse("sololeveling.admin").expect("valid permission key");
        let expr = PermissionExpr::key(key);
        assert!(matches!(expr, PermissionExpr::Key(_)));
    }
}

fn give_artifact(
    server: &Arc<Server>,
    target: &Arc<Player>,
    id: CatalogId,
    owner: Option<Uuid>,
) -> Result<(), CommandError> {
    let mut reservation = target
        .reserve_stored_inventory()
        .map_err(|_| CommandError::from("Инвентарь игрока сейчас занят."))?;
    let Some(slot) =
        (0..36).find(|slot| reservation.item(*slot).is_ok_and(|stack| stack.is_empty()))
    else {
        return Err(CommandError::from(
            "У игрока нет свободного места в инвентаре.",
        ));
    };
    let stack = match id {
        CatalogId::Artifact(id) => artifact::stack(id, owner),
        CatalogId::Serial(id) => serial::stack(id, owner),
    };
    reservation
        .set_item(slot, stack)
        .map_err(|_| CommandError::from("Не удалось подготовить инвентарь игрока."))?;

    let recipient = Arc::clone(target);
    server
        .submit_stored_inventory_transaction(vec![reservation], Vec::new(), move |result| {
            match result {
                Ok(TransactionCommit::Committed) => recipient.send_message(
                    &TextComponent::plain("Оператор выдал вам предмет.")
                        .color(Color::LightPurple),
                ),
                Ok(TransactionCommit::DurabilityUncertain) => recipient.send_message(
                    &TextComponent::plain(
                        "Артефакт выдан, но сохранение не подтверждено. Не выбрасывайте его до перезапуска.",
                    )
                    .color(Color::Red),
                ),
                Err(error) => log::error!(
                    "sololeveling: artifact grant to {} failed: {error}",
                    recipient.gameprofile.id()
                ),
            }
        })
        .map_err(|_| CommandError::from("Не удалось запустить сохранение артефакта."))?;
    Ok(())
}
