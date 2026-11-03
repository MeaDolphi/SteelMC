//! The party as a chest menu: members, invitations, leaving and kicking.
//!
//! The menu is live: it redraws itself whenever the viewer's party or
//! invitation changes, so a leader sees a new member the tick the
//! invitation is accepted, without reopening the menu.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_registry::{vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use crate::main_menu;
use crate::party::Party;
use crate::state::SoloLevelingState;
use crate::ui;

const SLOTS: usize = 27;
const INFO_SLOT: usize = 4;
const FIRST_MEMBER_SLOT: usize = 10;
const INVITE_SLOT: usize = 20;
const ACCEPT_SLOT: usize = 22;
const LEAVE_SLOT: usize = 24;
const BACK_SLOT: usize = 26;

/// What the menu draws for one viewer: their party and pending invitation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PartyView {
    party: Option<Party>,
    /// The leader whose invitation is waiting for the viewer, if any.
    invitation: Option<Uuid>,
}

impl PartyView {
    fn capture(state: &SoloLevelingState, uuid: Uuid) -> Self {
        Self {
            party: state.party_of(uuid),
            invitation: state.party_invitation(uuid),
        }
    }
}

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
    player.open_menu(
        TextComponent::plain("Группа").color(Color::Aqua),
        move |context| build(context.container_id, context.player, state),
    );
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>) -> Menu {
    let uuid = player.gameprofile.id();
    let view = PartyView::capture(&state, uuid);
    let container = SimpleContainer::from_items(items(&state, uuid, &view)).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(PartyMenuKind {
        state,
        buttons,
        container,
        view,
    })
}

fn items(state: &SoloLevelingState, uuid: Uuid, view: &PartyView) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); SLOTS];
    let leader = view
        .party
        .as_ref()
        .is_none_or(|party| party.leader() == uuid);
    items[INFO_SLOT] = match &view.party {
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
    if let Some(party) = &view.party {
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
    if let Some(inviter) = view.invitation {
        items[ACCEPT_SLOT] = ui::button(
            &vanilla_items::LIME_DYE,
            format!("Принять приглашение от {}", hunter_name(state, inviter)),
            Color::Green,
            &["Текущую группу вы покинете."],
        );
    }
    if view.party.is_some() {
        items[LEAVE_SLOT] = ui::button(&vanilla_items::BARRIER, "Покинуть группу", Color::Red, &[]);
    }
    items[BACK_SLOT] = ui::main_menu_button();
    items
}

struct PartyMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    /// Last state drawn into the container.
    view: PartyView,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for PartyMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/party");
}

impl PartyMenuKind {
    fn redraw(&mut self, guard: &mut ContainerLockGuard, uuid: Uuid, view: PartyView) {
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in items(&self.state, uuid, &view).into_iter().enumerate() {
                container.set_item(index, item);
            }
            self.view = view;
        }
    }
}

impl MenuKind for PartyMenuKind {
    fn on_tick(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        let uuid = player.gameprofile.id();
        let view = PartyView::capture(&self.state, uuid);
        if view != self.view {
            self.redraw(guard, uuid, view);
        }
    }

    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        click: Click,
        player: &Player,
    ) -> ClickOutcome {
        let Click::Pickup { slot, .. } = click else {
            return if click.slot().is_some_and(|slot| self.buttons.contains(slot)) {
                ClickOutcome::Consume
            } else {
                ClickOutcome::Fallthrough
            };
        };
        if !self.buttons.contains(slot) {
            return ClickOutcome::Fallthrough;
        }
        if slot - self.buttons.start() == BACK_SLOT {
            main_menu::open(player, Arc::clone(&self.state));
            return ClickOutcome::Consume;
        }
        let uuid = player.gameprofile.id();
        match slot - self.buttons.start() {
            INVITE_SLOT => {
                self.invite(player);
                return ClickOutcome::Consume;
            }
            ACCEPT_SLOT => {
                match self
                    .state
                    .accept_party_invitation(uuid, self.state.current_tick())
                {
                    Ok(_) => ui::tell(player, "Вы вступили в группу.", Color::Green),
                    Err(error) => ui::tell(player, error.message(), Color::Red),
                }
            }
            LEAVE_SLOT => {
                if self.state.leave_party(uuid).is_some() {
                    ui::tell(player, "Вы покинули группу.", Color::Gray);
                }
            }
            slot => {
                self.kick(player, slot);
                return ClickOutcome::Consume;
            }
        }
        let view = PartyView::capture(&self.state, uuid);
        self.redraw(guard, uuid, view);
        ClickOutcome::Consume
    }
}

impl PartyMenuKind {
    /// Opens the hunter picker; the menu stays replaced until the pick
    /// callback reopens the party menu.
    fn invite(&self, player: &Player) {
        let uuid = player.gameprofile.id();
        let exclude = self
            .state
            .party_of(uuid)
            .map_or_else(Vec::new, |party| party.members().to_vec());
        let state = Arc::clone(&self.state);
        ui::pick_player(
            player,
            &self.state,
            "Пригласить в группу",
            &exclude,
            Arc::new(move |player, target, name| {
                match state.invite_to_party(player.gameprofile.id(), target, state.current_tick()) {
                    Ok(()) => {
                        ui::tell(player, &format!("{name} приглашён в группу."), Color::Green);
                        if let Some(invited) = state
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
                open(player, Arc::clone(&state));
            }),
        );
    }

    /// Confirms a leader's kick of the member at `slot`; the confirm menu
    /// replaces this one until its callback reopens the party menu.
    fn kick(&self, player: &Player, slot: usize) {
        let uuid = player.gameprofile.id();
        let Some(party) = self.state.party_of(uuid) else {
            return;
        };
        let Some(member) = slot
            .checked_sub(FIRST_MEMBER_SLOT)
            .and_then(|index| party.members().get(index).copied())
        else {
            return;
        };
        if member == uuid || party.leader() != uuid {
            return;
        }
        let name = hunter_name(&self.state, member);
        let state = Arc::clone(&self.state);
        ui::confirm(
            player,
            Arc::clone(&state),
            &format!("Исключить {name} из группы?"),
            Arc::new(move |player| {
                match state.kick_from_party(player.gameprofile.id(), member) {
                    Ok(()) => ui::tell(player, &format!("{name} исключён."), Color::Gray),
                    Err(error) => ui::tell(player, error.message(), Color::Red),
                }
                open(player, Arc::clone(&state));
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{ACCEPT_SLOT, FIRST_MEMBER_SLOT, INFO_SLOT, INVITE_SLOT, LEAVE_SLOT, PartyView};
    use crate::party::PARTY_MAX_MEMBERS;
    use crate::state::SoloLevelingState;
    use uuid::Uuid;

    #[test]
    fn member_slots_never_reach_the_buttons() {
        let last_member = FIRST_MEMBER_SLOT + PARTY_MAX_MEMBERS - 1;
        for button in [INFO_SLOT, INVITE_SLOT, ACCEPT_SLOT, LEAVE_SLOT] {
            assert!(!(FIRST_MEMBER_SLOT..=last_member).contains(&button));
        }
    }

    #[test]
    fn view_changes_when_the_invitation_is_accepted() {
        let directory =
            std::env::temp_dir().join(format!("sololeveling-party-menu-test-{}", Uuid::new_v4()));
        let state = SoloLevelingState::new(directory.clone()).expect("state should initialize");
        let leader = Uuid::new_v4();
        let member = Uuid::new_v4();
        state
            .invite_to_party(leader, member, 0)
            .expect("invite succeeds");

        let before = PartyView::capture(&state, member);
        assert_eq!(before.party, None, "no party before accepting");
        assert_eq!(before.invitation, Some(leader));

        state
            .accept_party_invitation(member, 0)
            .expect("accept succeeds");

        let after = PartyView::capture(&state, member);
        assert_ne!(before, after, "accepting must change the drawn view");
        let party = after.party.expect("member joins the leader's party");
        assert_eq!(party.members(), &[leader, member]);
        assert_eq!(after.invitation, None, "the invitation is consumed");

        // The leader's open menu sees the new member the same way.
        let leader_view = PartyView::capture(&state, leader);
        assert_eq!(leader_view.party.unwrap().members(), &[leader, member]);

        std::fs::remove_dir_all(directory).expect("cleanup should succeed");
    }

    #[test]
    fn view_changes_when_a_member_leaves_or_is_kicked() {
        let directory =
            std::env::temp_dir().join(format!("sololeveling-party-menu-test-{}", Uuid::new_v4()));
        let state = SoloLevelingState::new(directory.clone()).expect("state should initialize");
        let leader = Uuid::new_v4();
        let member = Uuid::new_v4();
        state.invite_to_party(leader, member, 0).expect("invite");
        state.accept_party_invitation(member, 0).expect("accept");
        let full = PartyView::capture(&state, leader);

        state.leave_party(member);
        let after_leave = PartyView::capture(&state, leader);
        assert_ne!(full, after_leave);
        assert_eq!(after_leave.party.unwrap().members(), &[leader]);

        std::fs::remove_dir_all(directory).expect("cleanup should succeed");
    }
}
