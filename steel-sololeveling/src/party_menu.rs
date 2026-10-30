//! The party as a chest menu: members, invitations, leaving and kicking.

use std::sync::Arc;

use steel_core::inventory::prelude::*;
use steel_registry::vanilla_items;
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use crate::state::SoloLevelingState;
use crate::ui::{self, After};

const INFO_SLOT: usize = 4;
const FIRST_MEMBER_SLOT: usize = 10;
const INVITE_SLOT: usize = 20;
const ACCEPT_SLOT: usize = 22;
const LEAVE_SLOT: usize = 24;

/// Returns the name of an online hunter, or a short id for anyone else.
pub fn hunter_name(state: &SoloLevelingState, uuid: Uuid) -> String {
    state
        .online_players()
        .into_iter()
        .find(|player| player.gameprofile.id() == uuid)
        .map_or_else(
            || uuid.to_string().chars().take(8).collect(),
            |player| player.gameprofile.name().to_owned(),
        )
}

/// Opens the party menu.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    let items = items(&state, player.gameprofile.id());
    ui::open_buttons(
        player,
        TextComponent::plain("Группа").color(Color::Aqua),
        3,
        items,
        Box::new(move |player, slot, _| click(&state, player, slot)),
    );
}

fn items(state: &SoloLevelingState, uuid: Uuid) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); 27];
    let party = state.party_of(uuid);
    let leader = party.as_ref().is_none_or(|party| party.leader() == uuid);
    items[INFO_SLOT] = match &party {
        Some(party) => ui::button(
            &vanilla_items::BELL,
            format!("Группа: {} / 4", party.members().len()),
            Color::Gold,
            &["Опыт и задания группы делятся между участниками."],
        ),
        None => ui::button(
            &vanilla_items::BELL,
            "Вы не в группе",
            Color::Gray,
            &["Пригласите охотника — группа создастся сама."],
        ),
    };
    if let Some(party) = &party {
        for (index, member) in party.members().iter().enumerate() {
            let is_leader = *member == party.leader();
            let mut lore = vec![if is_leader {
                "Лидер группы."
            } else {
                "Участник."
            }];
            if leader && *member != uuid {
                lore.push("Клик — исключить.");
            }
            items[FIRST_MEMBER_SLOT + index] = ui::button(
                &vanilla_items::PLAYER_HEAD,
                hunter_name(state, *member),
                if is_leader { Color::Gold } else { Color::Aqua },
                &lore,
            );
        }
    }
    if leader {
        items[INVITE_SLOT] = ui::button(
            &vanilla_items::WRITABLE_BOOK,
            "Пригласить охотника",
            Color::Green,
            &["Выбрать игрока онлайн.", "Приглашение действует минуту."],
        );
    }
    if let Some(inviter) = state.party_invitation(uuid) {
        items[ACCEPT_SLOT] = ui::button(
            &vanilla_items::LIME_DYE,
            format!("Принять приглашение от {}", hunter_name(state, inviter)),
            Color::Green,
            &["Текущую группу вы покинете."],
        );
    }
    if party.is_some() {
        items[LEAVE_SLOT] = ui::button(&vanilla_items::BARRIER, "Покинуть группу", Color::Red, &[]);
    }
    items
}

fn click(state: &Arc<SoloLevelingState>, player: &Player, slot: usize) -> After {
    let uuid = player.gameprofile.id();
    match slot {
        INVITE_SLOT => {
            let exclude = state
                .party_of(uuid)
                .map_or_else(Vec::new, |party| party.members().to_vec());
            let invite_state = Arc::clone(state);
            ui::pick_player(
                player,
                state,
                "Пригласить в группу",
                &exclude,
                Arc::new(move |player, target, name| {
                    match invite_state.invite_to_party(
                        player.gameprofile.id(),
                        target,
                        invite_state.current_tick(),
                    ) {
                        Ok(()) => {
                            ui::tell(player, &format!("{name} приглашён в группу."), Color::Green);
                            if let Some(invited) = invite_state
                                .online_players()
                                .into_iter()
                                .find(|online| online.gameprofile.id() == target)
                            {
                                invited.send_message(
                                    &TextComponent::plain(format!(
                                        "{} приглашает вас в группу. Откройте «Группа» в книге Системы в течение минуты.",
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
            return After::Replaced;
        }
        ACCEPT_SLOT => match state.accept_party_invitation(uuid, state.current_tick()) {
            Ok(_) => ui::tell(player, "Вы вступили в группу.", Color::Green),
            Err(error) => ui::tell(player, error.message(), Color::Red),
        },
        LEAVE_SLOT => {
            if state.leave_party(uuid).is_some() {
                ui::tell(player, "Вы покинули группу.", Color::Gray);
            }
        }
        _ => {
            let Some(party) = state.party_of(uuid) else {
                return After::Keep;
            };
            let Some(member) = slot
                .checked_sub(FIRST_MEMBER_SLOT)
                .and_then(|index| party.members().get(index).copied())
            else {
                return After::Keep;
            };
            if member == uuid || party.leader() != uuid {
                return After::Keep;
            }
            let name = hunter_name(state, member);
            let kick_state = Arc::clone(state);
            ui::confirm(
                player,
                &format!("Исключить {name} из группы?"),
                Arc::new(move |player| {
                    match kick_state.kick_from_party(player.gameprofile.id(), member) {
                        Ok(()) => ui::tell(player, &format!("{name} исключён."), Color::Gray),
                        Err(error) => ui::tell(player, error.message(), Color::Red),
                    }
                    open(player, Arc::clone(&kick_state));
                }),
            );
            return After::Replaced;
        }
    }
    After::Redraw(items(state, uuid))
}

#[cfg(test)]
mod tests {
    use super::{ACCEPT_SLOT, FIRST_MEMBER_SLOT, INFO_SLOT, INVITE_SLOT, LEAVE_SLOT};
    use crate::party::PARTY_MAX_MEMBERS;

    #[test]
    fn member_slots_never_reach_the_buttons() {
        let last_member = FIRST_MEMBER_SLOT + PARTY_MAX_MEMBERS - 1;
        for button in [INFO_SLOT, INVITE_SLOT, ACCEPT_SLOT, LEAVE_SLOT] {
            assert!(!(FIRST_MEMBER_SLOT..=last_member).contains(&button));
        }
    }
}
