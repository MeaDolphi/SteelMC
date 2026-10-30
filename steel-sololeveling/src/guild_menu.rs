//! The guild as chest menus: founding, members, ranks, bank and treasury.

use std::sync::Arc;

use steel_core::inventory::prelude::*;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_items;
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use crate::guild::{GUILD_MAX_MEMBERS, Guild, GuildRole};
use crate::guild_bank;
use crate::item::append_lore;
use crate::party_menu::hunter_name;
use crate::state::SoloLevelingState;
use crate::ui::{self, After};

const MEMBER_SLOTS: usize = 45;
const INVITE_SLOT: usize = 45;
const BANK_SLOT: usize = 46;
const DEPOSIT_SLOT: usize = 47;
const INFO_SLOT: usize = 49;
const LEAVE_SLOT: usize = 53;
const CREATE_SLOT: usize = 13;

/// Ranks a guildmaster may assign, with their member-menu slots.
const ROLE_BUTTONS: [(usize, GuildRole); 3] = [
    (11, GuildRole::Officer),
    (13, GuildRole::Member),
    (15, GuildRole::Recruit),
];
const KICK_SLOT: usize = 22;
const BACK_SLOT: usize = 18;

/// Returns the Russian rank name.
#[must_use]
pub const fn role_name(role: GuildRole) -> &'static str {
    match role {
        GuildRole::Guildmaster => "Гильдмастер",
        GuildRole::Officer => "Офицер",
        GuildRole::Member => "Участник",
        GuildRole::Recruit => "Новобранец",
    }
}

const fn role_order(role: GuildRole) -> u8 {
    match role {
        GuildRole::Guildmaster => 0,
        GuildRole::Officer => 1,
        GuildRole::Member => 2,
        GuildRole::Recruit => 3,
    }
}

/// Opens the guild menu, or the founding menu for a hunter without a guild.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    let uuid = player.gameprofile.id();
    match state.guild_of(uuid) {
        Some(guild) => open_guild(player, state, &guild),
        None => open_founding(player, state),
    }
}

fn open_founding(player: &Player, state: Arc<SoloLevelingState>) {
    let mut items = vec![ItemStack::empty(); 27];
    items[CREATE_SLOT] = ui::button(
        &vanilla_items::WHITE_BANNER,
        "Основать гильдию",
        Color::Gold,
        &[
            "Название: 3–24 латинских буквы, цифры или _.",
            "Вступить в чужую гильдию можно по приглашению.",
        ],
    );
    ui::open_buttons(
        player,
        TextComponent::plain("Гильдия").color(Color::Gold),
        3,
        items,
        Box::new(move |player, slot, _| {
            if slot != CREATE_SLOT {
                return After::Keep;
            }
            let create_state = Arc::clone(&state);
            ui::ask_text(
                player,
                "Название гильдии",
                "Название",
                Arc::new(move |player, name| {
                    match create_state.create_guild(name.clone(), player.gameprofile.id()) {
                        Ok(()) => {
                            ui::tell(player, &format!("Гильдия {name} основана."), Color::Gold);
                            open(player, Arc::clone(&create_state));
                        }
                        Err(error) => ui::tell(player, error.message(), Color::Red),
                    }
                }),
            );
            After::Replaced
        }),
    );
}

/// Members sorted by rank, then name, as the menu shows them.
fn sorted_members(state: &SoloLevelingState, guild: &Guild) -> Vec<(Uuid, GuildRole, String)> {
    let mut members: Vec<(Uuid, GuildRole, String)> = guild
        .members
        .iter()
        .map(|(uuid, role)| (*uuid, *role, hunter_name(state, *uuid)))
        .collect();
    members.sort_by(|left, right| {
        role_order(left.1)
            .cmp(&role_order(right.1))
            .then_with(|| left.2.to_lowercase().cmp(&right.2.to_lowercase()))
    });
    members
}

fn guild_items(state: &SoloLevelingState, viewer: Uuid, guild: &Guild) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); 54];
    let role = guild.role_of(viewer).unwrap_or(GuildRole::Recruit);
    let members = sorted_members(state, guild);
    for (slot, (uuid, member_role, name)) in members.iter().take(MEMBER_SLOTS).enumerate() {
        let manageable =
            *uuid != viewer && (role == GuildRole::Guildmaster || role.can_remove(*member_role));
        let lore: &[&str] = if manageable {
            &["Клик — ранг и исключение."]
        } else {
            &[]
        };
        let mut head = ui::button(
            &vanilla_items::PLAYER_HEAD,
            name.clone(),
            if *member_role == GuildRole::Guildmaster {
                Color::Gold
            } else {
                Color::Aqua
            },
            lore,
        );
        head = append_lore(
            head,
            vec![TextComponent::plain(role_name(*member_role)).color(Color::Gray)],
        );
        items[slot] = head;
    }
    let hidden = members.len().saturating_sub(MEMBER_SLOTS);
    let mut info = vec![
        format!("Уровень {}", guild.level),
        format!("Участников: {} / {GUILD_MAX_MEMBERS}", guild.members.len()),
        format!("Казна: {} Won", guild.treasury.amount()),
        format!("Ваш ранг: {}", role_name(role)),
    ];
    if hidden > 0 {
        info.push(format!("Ещё {hidden} участников не поместились в список."));
    }
    let info: Vec<&str> = info.iter().map(String::as_str).collect();
    items[INFO_SLOT] = ui::button(
        &vanilla_items::WHITE_BANNER,
        guild.name.clone(),
        Color::Gold,
        &info,
    );
    if role.can_invite() {
        items[INVITE_SLOT] = ui::button(
            &vanilla_items::WRITABLE_BOOK,
            "Пригласить охотника",
            Color::Green,
            &["Охотник вступит новобранцем."],
        );
    }
    items[BANK_SLOT] = ui::button(
        &vanilla_items::CHEST,
        "Банк гильдии",
        Color::Gold,
        &["Класть и брать могут мастер и офицеры."],
    );
    items[DEPOSIT_SLOT] = ui::button(
        &vanilla_items::GOLD_INGOT,
        "Взнос в казну",
        Color::Gold,
        &["Перевести свои Won в казну гильдии."],
    );
    if role != GuildRole::Guildmaster {
        items[LEAVE_SLOT] =
            ui::button(&vanilla_items::BARRIER, "Покинуть гильдию", Color::Red, &[]);
    }
    items
}

fn open_guild(player: &Player, state: Arc<SoloLevelingState>, guild: &Guild) {
    let viewer = player.gameprofile.id();
    let items = guild_items(&state, viewer, guild);
    ui::open_buttons(
        player,
        TextComponent::plain(format!("Гильдия {}", guild.name)).color(Color::Gold),
        6,
        items,
        Box::new(move |player, slot, _| guild_click(&state, player, slot)),
    );
}

#[expect(
    clippy::too_many_lines,
    reason = "one match keeps every guild button's rules side by side"
)]
fn guild_click(state: &Arc<SoloLevelingState>, player: &Player, slot: usize) -> After {
    let viewer = player.gameprofile.id();
    let Some(guild) = state.guild_of(viewer) else {
        open(player, Arc::clone(state));
        return After::Replaced;
    };
    let role = guild.role_of(viewer).unwrap_or(GuildRole::Recruit);
    match slot {
        INVITE_SLOT if role.can_invite() => {
            let exclude: Vec<Uuid> = guild.members.keys().copied().collect();
            let invite_state = Arc::clone(state);
            ui::pick_player(
                player,
                state,
                "Пригласить в гильдию",
                &exclude,
                Arc::new(move |player, target, name| {
                    match invite_state.add_guild_recruit(player.gameprofile.id(), target) {
                        Ok(()) => {
                            ui::tell(player, &format!("{name} принят в гильдию."), Color::Green);
                            if let Some(invited) = invite_state
                                .online_players()
                                .into_iter()
                                .find(|online| online.gameprofile.id() == target)
                            {
                                invited.send_message(
                                    &TextComponent::plain(format!(
                                        "{} принял вас в гильдию новобранцем.",
                                        player.gameprofile.name()
                                    ))
                                    .color(Color::Gold),
                                );
                            }
                        }
                        Err(error) => ui::tell(player, error.message(), Color::Red),
                    }
                    open(player, Arc::clone(&invite_state));
                }),
            );
            After::Replaced
        }
        BANK_SLOT => {
            if let Err(error) = guild_bank::open(player, Arc::clone(state)) {
                ui::tell(player, error.message(), Color::Red);
                return After::Keep;
            }
            After::Replaced
        }
        DEPOSIT_SLOT => {
            let balance = state
                .with_session_mut(viewer, |session| session.won.amount())
                .unwrap_or(0);
            if balance == 0 {
                ui::tell(player, "У вас нет Won для взноса.", Color::Red);
                return After::Keep;
            }
            let deposit_state = Arc::clone(state);
            ui::pick_amount(
                player,
                "Взнос в казну",
                "Won",
                balance.min(100),
                1,
                balance,
                Arc::new(move |player, amount| {
                    match deposit_state.deposit_guild_treasury(player.gameprofile.id(), amount) {
                        Ok(()) => ui::tell(player, "Взнос в казну обрабатывается.", Color::Gold),
                        Err(error) => ui::tell(player, error.message(), Color::Red),
                    }
                    open(player, Arc::clone(&deposit_state));
                }),
            );
            After::Replaced
        }
        LEAVE_SLOT if role != GuildRole::Guildmaster => {
            let leave_state = Arc::clone(state);
            ui::confirm(
                player,
                &format!("Покинуть гильдию {}?", guild.name),
                Arc::new(move |player| {
                    match leave_state.leave_guild(player.gameprofile.id()) {
                        Ok(()) => ui::tell(player, "Вы покинули гильдию.", Color::Gray),
                        Err(error) => ui::tell(player, error.message(), Color::Red),
                    }
                    player.close_container();
                }),
            );
            After::Replaced
        }
        slot if slot < MEMBER_SLOTS => {
            let members = sorted_members(state, &guild);
            let Some((target, target_role, name)) = members.get(slot).cloned() else {
                return After::Keep;
            };
            if target == viewer || !(role == GuildRole::Guildmaster || role.can_remove(target_role))
            {
                return After::Keep;
            }
            open_member(
                player,
                Arc::clone(state),
                target,
                name,
                role == GuildRole::Guildmaster,
            );
            After::Replaced
        }
        _ => After::Keep,
    }
}

fn open_member(
    player: &Player,
    state: Arc<SoloLevelingState>,
    target: Uuid,
    name: String,
    can_set_roles: bool,
) {
    let mut items = vec![ItemStack::empty(); 27];
    items[4] = ui::button(&vanilla_items::PLAYER_HEAD, name.clone(), Color::Aqua, &[]);
    if can_set_roles {
        for (slot, role) in ROLE_BUTTONS {
            let icon: ItemRef = match role {
                GuildRole::Officer => &vanilla_items::GOLDEN_HELMET,
                GuildRole::Member => &vanilla_items::IRON_HELMET,
                _ => &vanilla_items::LEATHER_HELMET,
            };
            items[slot] = ui::button(
                icon,
                format!("Назначить: {}", role_name(role)),
                Color::Yellow,
                &[],
            );
        }
    }
    items[KICK_SLOT] = ui::button(
        &vanilla_items::BARRIER,
        "Исключить из гильдии",
        Color::Red,
        &[],
    );
    items[BACK_SLOT] = ui::button(&vanilla_items::ARROW, "Назад", Color::Yellow, &[]);
    ui::open_buttons(
        player,
        TextComponent::plain(name.clone()).color(Color::Gold),
        3,
        items,
        Box::new(move |player, slot, _| {
            let actor = player.gameprofile.id();
            if slot == BACK_SLOT {
                open(player, Arc::clone(&state));
                return After::Replaced;
            }
            if slot == KICK_SLOT {
                let kick_state = Arc::clone(&state);
                let kicked = name.clone();
                ui::confirm(
                    player,
                    &format!("Исключить {kicked} из гильдии?"),
                    Arc::new(move |player| {
                        match kick_state.remove_guild_member(player.gameprofile.id(), target) {
                            Ok(()) => ui::tell(player, &format!("{kicked} исключён."), Color::Gray),
                            Err(error) => ui::tell(player, error.message(), Color::Red),
                        }
                        open(player, Arc::clone(&kick_state));
                    }),
                );
                return After::Replaced;
            }
            if let Some((_, role)) = ROLE_BUTTONS.iter().find(|(at, _)| *at == slot)
                && can_set_roles
            {
                match state.set_guild_role(actor, target, *role) {
                    Ok(()) => ui::tell(
                        player,
                        &format!("{name}: теперь {}.", role_name(*role)),
                        Color::Green,
                    ),
                    Err(error) => ui::tell(player, error.message(), Color::Red),
                }
                open(player, Arc::clone(&state));
                return After::Replaced;
            }
            After::Keep
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::{
        BACK_SLOT, BANK_SLOT, DEPOSIT_SLOT, INFO_SLOT, INVITE_SLOT, KICK_SLOT, LEAVE_SLOT,
        MEMBER_SLOTS, ROLE_BUTTONS,
    };

    #[test]
    fn guild_buttons_sit_below_the_member_list() {
        for slot in [INVITE_SLOT, BANK_SLOT, DEPOSIT_SLOT, INFO_SLOT, LEAVE_SLOT] {
            assert!((MEMBER_SLOTS..54).contains(&slot));
        }
        for (slot, _) in ROLE_BUTTONS {
            assert!(slot < 27 && slot != KICK_SLOT && slot != BACK_SLOT);
        }
    }
}
