//! Solo Leveling command registrations.

mod auction;
mod crystal;
mod organization;
mod pay;
mod shop;

use std::sync::Arc;

use steel_core::command::{
    CommandArgument, CommandError, CommandRegistration, CommandRegistrationError, CommandRegistry,
    argument, literal,
};
use text_components::TextComponent;

use crate::guild::{GuildError, GuildRole};
use crate::guild_bank;
use crate::hunter_stats_menu;
use crate::identifier::from_sololeveling;
use crate::party::PartyError;
use crate::shadow::ShadowStat;
use crate::state::SoloLevelingState;

/// Registers every Solo Leveling command required at server startup.
#[expect(
    clippy::too_many_lines,
    reason = "The complete command tree is intentionally kept together for review."
)]
pub fn register(
    registry: &mut CommandRegistry,
    state: Arc<SoloLevelingState>,
) -> Result<(), CommandRegistrationError> {
    pay::register(registry, Arc::clone(&state))?;
    shop::register(registry, Arc::clone(&state))?;
    crystal::register(registry, Arc::clone(&state))?;
    auction::register(registry, Arc::clone(&state))?;
    organization::register(registry, Arc::clone(&state))?;
    let hunter_stats_state = Arc::clone(&state);
    let command = CommandRegistration::new(from_sololeveling("hunterstats"), move || {
        let state = Arc::clone(&hunter_stats_state);
        literal("hunterstats").executes(move |context| {
            let Some(player) = context.source().player() else {
                return Err(CommandError::from(
                    "This command can only be used by a player",
                ));
            };
            hunter_stats_menu::open(Arc::clone(player), Arc::clone(&state));
            Ok(1)
        })
    })
    .default_access();
    registry.register(command)?;

    let shadows_state = Arc::clone(&state);
    let shadows = CommandRegistration::new(from_sololeveling("shadows"), move || {
        let state = Arc::clone(&shadows_state);
        literal("shadows")
            .then({
                let state = Arc::clone(&state);
                literal("list").executes(move |context| {
                    let Some(player) = context.source().player() else {
                        return Err(CommandError::from(
                            "This command can only be used by a player",
                        ));
                    };
                    let shadows = state
                        .shadows(player.gameprofile.id())
                        .ok_or_else(|| CommandError::from("Сессия охотника не загружена."))?;
                    let listing = if shadows.is_empty() {
                        "В вашей армии пока нет теней.".to_owned()
                    } else {
                        shadows
                            .iter()
                            .map(|shadow| {
                                format!(
                                    "{}: ур. {}, XP {}/{}, очки {}",
                                    shadow.name,
                                    shadow.level,
                                    shadow.experience,
                                    shadow.experience_to_next_level(),
                                    shadow.unspent_points
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    player.send_message(&TextComponent::plain(listing));
                    Ok(1)
                })
            })
            .then({
                let state = Arc::clone(&state);
                literal("summon").then(argument("name", CommandArgument::greedy_string()).executes(
                    move |context| {
                        let Some(player) = context.source().player() else {
                            return Err(CommandError::from(
                                "This command can only be used by a player",
                            ));
                        };
                        state
                            .summon_shadow(player, context.string("name")?)
                            .map_err(CommandError::from)?;
                        player.send_message(&TextComponent::plain("Тень призвана."));
                        Ok(1)
                    },
                ))
            })
            .then({
                let state = Arc::clone(&state);
                literal("recall").executes(move |context| {
                    let Some(player) = context.source().player() else {
                        return Err(CommandError::from(
                            "This command can only be used by a player",
                        ));
                    };
                    let recalled = state.recall_shadows(player.gameprofile.id());
                    player.send_message(&TextComponent::plain(format!(
                        "Отозвано теней: {recalled}."
                    )));
                    Ok(if recalled > i32::MAX as usize {
                        i32::MAX
                    } else {
                        recalled as i32
                    })
                })
            })
            .then({
                let state = Arc::clone(&state);
                literal("summon-all").executes(move |context| {
                    let Some(player) = context.source().player() else {
                        return Err(CommandError::from(
                            "This command can only be used by a player",
                        ));
                    };
                    let summoned = state
                        .summon_all_shadows(player)
                        .ok_or_else(|| CommandError::from("Сессия охотника не загружена."))?;
                    player.send_message(&TextComponent::plain(format!(
                        "Призвано теней: {summoned}."
                    )));
                    Ok(i32::try_from(summoned).unwrap_or(i32::MAX))
                })
            })
            .then({
                let state = Arc::clone(&state);
                literal("upgrade").then(argument("name", CommandArgument::string()).then(
                    argument("stat", CommandArgument::word()).executes(move |context| {
                        let Some(player) = context.source().player() else {
                            return Err(CommandError::from(
                                "This command can only be used by a player",
                            ));
                        };
                        let stat = match context.string("stat")? {
                            "health" => ShadowStat::Health,
                            "attack" => ShadowStat::Attack,
                            "speed" => ShadowStat::Speed,
                            _ => {
                                return Err(CommandError::from(
                                    "Стат должен быть health, attack или speed.",
                                ));
                            }
                        };
                        let shadow = state
                            .upgrade_shadow(player.gameprofile.id(), context.string("name")?, stat)
                            .map_err(CommandError::from)?;
                        state.save_session(player.gameprofile.id()).map_err(|_| {
                            CommandError::from("Не удалось сохранить ростер теней.")
                        })?;
                        player.send_message(&TextComponent::plain(format!(
                            "Тень {} улучшена. Осталось очков: {}.",
                            shadow.name, shadow.unspent_points
                        )));
                        Ok(1)
                    }),
                ))
            })
    })
    .default_access();
    registry.register(shadows)?;

    let party_state = Arc::clone(&state);
    let party = CommandRegistration::new(from_sololeveling("party"), move || {
        let state = Arc::clone(&party_state);
        literal("party")
            .then({
                let state = Arc::clone(&state);
                literal("invite").then(
                    argument("player", CommandArgument::player()).executes(move |context| {
                        let Some(player) = context.source().player() else {
                            return Err(CommandError::from("Команда доступна только игроку."));
                        };
                        let target = context.player("player")?;
                        state
                            .invite_to_party(
                                player.gameprofile.id(),
                                target.gameprofile.id(),
                                context.source().server().current_tick(),
                            )
                            .map_err(party_error)?;
                        target.send_message(&TextComponent::plain(format!(
                            "{} приглашает вас в группу. Используйте /party accept в течение минуты.",
                            player.gameprofile.name()
                        )));
                        Ok(1)
                    }),
                )
            })
            .then({
                let state = Arc::clone(&state);
                literal("accept").executes(move |context| {
                    let Some(player) = context.source().player() else {
                        return Err(CommandError::from("Команда доступна только игроку."));
                    };
                    state
                        .accept_party_invitation(
                            player.gameprofile.id(),
                            context.source().server().current_tick(),
                        )
                        .map_err(party_error)?;
                    player.send_message(&TextComponent::plain("Вы вступили в группу."));
                    Ok(1)
                })
            })
            .then({
                let state = Arc::clone(&state);
                literal("leave").executes(move |context| {
                    let Some(player) = context.source().player() else {
                        return Err(CommandError::from("Команда доступна только игроку."));
                    };
                    if state.leave_party(player.gameprofile.id()).is_none() {
                        return Err(CommandError::from("Вы не состоите в группе."));
                    }
                    player.send_message(&TextComponent::plain("Вы покинули группу."));
                    Ok(1)
                })
            })
            .then({
                let state = Arc::clone(&state);
                literal("kick").then(
                    argument("player", CommandArgument::player()).executes(move |context| {
                        let Some(player) = context.source().player() else {
                            return Err(CommandError::from("Команда доступна только игроку."));
                        };
                        let target = context.player("player")?;
                        state
                            .kick_from_party(player.gameprofile.id(), target.gameprofile.id())
                            .map_err(party_error)?;
                        target.send_message(&TextComponent::plain("Вы исключены из группы."));
                        Ok(1)
                    }),
                )
            })
    })
    .default_access();
    registry.register(party)?;

    let guild_state = Arc::clone(&state);
    let guild = CommandRegistration::new(from_sololeveling("guild"), move || {
        let state = Arc::clone(&guild_state);
        literal("guild")
            .then({
                let state = Arc::clone(&state);
                literal("create").then(argument("name", CommandArgument::word()).executes(
                    move |context| {
                        let Some(player) = context.source().player() else {
                            return Err(CommandError::from("Команда доступна только игроку."));
                        };
                        let name = context.string("name")?.to_owned();
                        state
                            .create_guild(name.clone(), player.gameprofile.id())
                            .map_err(guild_error)?;
                        player.send_message(&TextComponent::plain(format!(
                            "Гильдия {name} создана."
                        )));
                        Ok(1)
                    },
                ))
            })
            .then({
                let state = Arc::clone(&state);
                literal("invite").then(argument("player", CommandArgument::player()).executes(
                    move |context| {
                        let Some(player) = context.source().player() else {
                            return Err(CommandError::from("Команда доступна только игроку."));
                        };
                        let target = context.player("player")?;
                        state
                            .add_guild_recruit(player.gameprofile.id(), target.gameprofile.id())
                            .map_err(guild_error)?;
                        target.send_message(&TextComponent::plain(
                            "Вы приняты в гильдию в ранге новобранца.",
                        ));
                        Ok(1)
                    },
                ))
            })
            .then({
                let state = Arc::clone(&state);
                literal("kick").then(argument("player", CommandArgument::player()).executes(
                    move |context| {
                        let Some(player) = context.source().player() else {
                            return Err(CommandError::from("Команда доступна только игроку."));
                        };
                        let target = context.player("player")?;
                        state
                            .remove_guild_member(player.gameprofile.id(), target.gameprofile.id())
                            .map_err(guild_error)?;
                        target.send_message(&TextComponent::plain("Вы исключены из гильдии."));
                        Ok(1)
                    },
                ))
            })
            .then({
                let state = Arc::clone(&state);
                literal("info").executes(move |context| {
                    let Some(player) = context.source().player() else {
                        return Err(CommandError::from("Команда доступна только игроку."));
                    };
                    let guild = state
                        .guild_of(player.gameprofile.id())
                        .ok_or_else(|| CommandError::from("Вы не состоите в гильдии."))?;
                    player.send_message(&TextComponent::plain(format!(
                        "{}: уровень {}, участников {}/50, казна {} Won",
                        guild.name,
                        guild.level,
                        guild.members.len(),
                        guild.treasury.amount()
                    )));
                    Ok(1)
                })
            })
            .then({
                let state = Arc::clone(&state);
                literal("bank").executes(move |context| {
                    let Some(player) = context.source().player() else {
                        return Err(CommandError::from("Команда доступна только игроку."));
                    };
                    guild_bank::open(player, Arc::clone(&state)).map_err(guild_error)?;
                    Ok(1)
                })
            })
            .then({
                let state = Arc::clone(&state);
                literal("deposit").then(argument("amount", CommandArgument::word()).executes(
                    move |context| {
                        let Some(player) = context.source().player() else {
                            return Err(CommandError::from("Команда доступна только игроку."));
                        };
                        let amount = context
                            .string("amount")?
                            .parse::<u64>()
                            .map_err(|_| CommandError::from("Сумма должна быть целым числом."))?;
                        state
                            .deposit_guild_treasury(player.gameprofile.id(), amount)
                            .map_err(guild_error)?;
                        player.send_message(&TextComponent::plain(format!(
                            "В казну внесено {amount} Won."
                        )));
                        Ok(1)
                    },
                ))
            })
            .then({
                let state = Arc::clone(&state);
                literal("leave").executes(move |context| {
                    let Some(player) = context.source().player() else {
                        return Err(CommandError::from("Команда доступна только игроку."));
                    };
                    state
                        .leave_guild(player.gameprofile.id())
                        .map_err(guild_error)?;
                    player.send_message(&TextComponent::plain("Вы покинули гильдию."));
                    Ok(1)
                })
            })
            .then({
                let state = Arc::clone(&state);
                literal("role").then(argument("player", CommandArgument::player()).then(
                    argument("role", CommandArgument::word()).executes(move |context| {
                        let Some(player) = context.source().player() else {
                            return Err(CommandError::from("Команда доступна только игроку."));
                        };
                        let role = match context.string("role")? {
                            "officer" => GuildRole::Officer,
                            "member" => GuildRole::Member,
                            "recruit" => GuildRole::Recruit,
                            _ => {
                                return Err(CommandError::from(
                                    "Ранг: officer, member или recruit.",
                                ));
                            }
                        };
                        let target = context.player("player")?;
                        state
                            .set_guild_role(player.gameprofile.id(), target.gameprofile.id(), role)
                            .map_err(guild_error)?;
                        target.send_message(&TextComponent::plain("Ваш ранг в гильдии изменён."));
                        Ok(1)
                    }),
                ))
            })
    })
    .default_access();
    registry.register(guild)?;
    Ok(())
}

fn party_error(error: PartyError) -> CommandError {
    CommandError::from(match error {
        PartyError::NotInParty => "Игрок не состоит в этой группе.",
        PartyError::NotLeader => "Исключать участников может только создатель группы.",
        PartyError::Full => "В группе уже четыре участника.",
        PartyError::AlreadyMember => "Игрок уже состоит в этой группе.",
        PartyError::NoInvitation => "Активного приглашения в группу нет.",
    })
}

fn guild_error(error: GuildError) -> CommandError {
    CommandError::from(match error {
        GuildError::AlreadyInGuild => "Игрок уже состоит в гильдии.",
        GuildError::AlreadyMember => "Игрок уже состоит в этой гильдии.",
        GuildError::Full => "В гильдии уже 50 участников.",
        GuildError::Missing => "Гильдия или её данные не найдены.",
        GuildError::NameTaken => "Гильдия с таким названием уже существует.",
        GuildError::InvalidName => "Название: 3–24 символа, латиница, цифры и _.",
        GuildError::PermissionDenied => "Ваш ранг не даёт этого права.",
        GuildError::CannotRemoveGuildmaster => "Нельзя исключить гильдмастера.",
        GuildError::CannotChangeRole => "Менять ранги может только гильдмастер.",
        GuildError::Storage => "Не удалось сохранить данные гильдии.",
        GuildError::BankInUse => "Банк гильдии уже открыт другим участником.",
        GuildError::InsufficientFunds => "Недостаточно Won.",
        GuildError::TransactionPending => "Дождитесь завершения текущей транзакции.",
    })
}
