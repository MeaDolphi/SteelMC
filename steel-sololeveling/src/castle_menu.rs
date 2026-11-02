//! The Demon Castle page of the System: the hunter's climb up the tower
//! floors, the free entry that returns every ten minutes, and the weekly
//! ranking of the highest cleared floor.

use std::sync::Arc;

use steel_core::inventory::prelude::*;
use steel_registry::vanilla_items;
use steel_registry::vanilla_menu_types;
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::castle::{self, CHECKPOINT_EVERY, CastleProgress, FREE_ENTRY_SECONDS, RankingEntry};
use crate::item::{named, with_lore};
use crate::merchant;
use crate::state::SoloLevelingState;

const MENU_SLOTS: usize = 27;
const PROGRESS_SLOT: usize = 11;
const ENTER_SLOT: usize = 13;
const RANKING_SLOT: usize = 15;

/// Everything the page shows; a change redraws it. The wait is part of the
/// view, so a spent free entry turns the enter button red on the next tick.
#[derive(Debug, Clone, PartialEq, Eq)]
struct View {
    progress: CastleProgress,
    ranking: Vec<RankingEntry>,
    rank: crate::rank::HunterRank,
    wait: u64,
}

fn view(state: &SoloLevelingState, uuid: Uuid) -> Option<View> {
    let rank = state.with_session_mut(uuid, |session| session.stats.rank())?;
    let (progress, ranking) = state.castle_view(uuid)?;
    Some(View {
        progress,
        ranking,
        rank,
        wait: progress.free_entry_in(merchant::unix_now()),
    })
}

/// Opens the castle page.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    let uuid = player.gameprofile.id();
    let current = view(&state, uuid);
    let container = SimpleContainer::from_items(menu_items(current.as_ref())).into_shared();
    player.open_menu(
        TextComponent::plain("Замок демонов").color(Color::DarkRed),
        move |context| {
            let mut builder =
                MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, context.container_id);
            let buttons = builder.section_all(Arc::clone(&container));
            builder.player_inventory(&context.player.inventory);
            builder.build(CastleMenuKind {
                state,
                buttons,
                container,
                view: current,
            })
        },
    );
}

fn menu_items(view: Option<&View>) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    let Some(view) = view else {
        return items;
    };
    let progress = view.progress;
    items[PROGRESS_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::NETHER_BRICK),
            TextComponent::plain("Ваше восхождение").color(Color::DarkRed),
        ),
        vec![
            line(&format!(
                "Следующий этаж: {} (ранг {})",
                progress.next_floor,
                castle::rank_for_floor(progress.next_floor).name()
            )),
            line(&format!("Рубеж: этаж {}", progress.checkpoint)),
            line(&format!("Лучший этаж: {}", progress.best)),
            line(&format!(
                "Рубеж сохраняется каждые {CHECKPOINT_EVERY} этажей; смерть возвращает к нему."
            )),
        ],
    );
    let allowed = view.rank >= crate::rank::HunterRank::D;
    let free = allowed && view.wait == 0;
    items[ENTER_SLOT] = with_lore(
        named(
            ItemStack::new(if free {
                &vanilla_items::OMINOUS_TRIAL_KEY
            } else {
                &vanilla_items::BARRIER
            }),
            TextComponent::plain(if !allowed {
                "Нужен ранг D".to_owned()
            } else if free {
                "Войти бесплатно".to_owned()
            } else {
                format!(
                    "Бесплатный вход через {} мин {:02} с",
                    view.wait / 60,
                    view.wait % 60
                )
            })
            .color(if free { Color::Green } else { Color::Red }),
        ),
        vec![
            line("Вход доступен охотникам с ранга D."),
            line(&format!(
                "Бесплатный вход раз в {} минут.",
                FREE_ENTRY_SECONDS / 60
            )),
            line("Дальше — «Ключ Замка демонов» (ПКМ ключом)."),
            line("Чёрный портал откроется рядом с вами в главном мире."),
            line("Башня бесконечна: этаж за этажом, пока не падёте или не выйдете сами."),
            line("За каждый зачищенный этаж — Won, материалы и запись в рейтинге."),
        ],
    );
    let mut lines: Vec<TextComponent> = view
        .ranking
        .iter()
        .enumerate()
        .map(|(place, entry)| {
            line(&format!(
                "{}. {} — этаж {}",
                place + 1,
                entry.name,
                entry.floor
            ))
        })
        .collect();
    if lines.is_empty() {
        lines.push(line("На этой неделе ещё никто не поднимался."));
    }
    items[RANKING_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::GOLDEN_HELMET),
            TextComponent::plain("Рейтинг недели").color(Color::Gold),
        ),
        lines,
    );
    items
}

pub(crate) struct CastleMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    view: Option<View>,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for CastleMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/castle");
}

impl CastleMenuKind {
    /// Recomputes the view and redraws it, so the enter button never stays
    /// green after the free entry was spent.
    fn refresh(&mut self, guard: &mut ContainerLockGuard, player: &Player) {
        let view = view(&self.state, player.gameprofile.id());
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in menu_items(view.as_ref()).into_iter().enumerate() {
                container.set_item(index, item);
            }
        }
        self.view = view;
    }
}

impl MenuKind for CastleMenuKind {
    fn on_tick(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        if player.tick_count() % 20 != 0 {
            return;
        }
        let view = view(&self.state, player.gameprofile.id());
        if self.view == view {
            return;
        }
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in menu_items(view.as_ref()).into_iter().enumerate() {
                container.set_item(index, item);
            }
        }
        self.view = view;
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
        if slot - self.buttons.start() != ENTER_SLOT {
            return ClickOutcome::Consume;
        }
        match self
            .state
            .request_castle_floor(player.gameprofile.id(), false)
        {
            Ok(floor) => player.send_message(
                &TextComponent::plain(format!(
                    "Замок демонов: чёрный портал на этаж {floor} открывается рядом с вами…"
                ))
                .color(Color::DarkRed),
            ),
            Err(message) => {
                player.send_message(&TextComponent::plain(message).color(Color::Red));
            }
        }
        self.refresh(guard, player);
        ClickOutcome::Consume
    }
}

fn line(text: &str) -> TextComponent {
    TextComponent::plain(text.to_owned()).color(Color::Gray)
}

#[cfg(test)]
mod tests {
    use super::{ENTER_SLOT, FREE_ENTRY_SECONDS, View, menu_items};
    use crate::castle::CastleProgress;
    use steel_registry::init_vanilla_registry;

    fn page(wait: u64) -> Vec<steel_registry::item_stack::ItemStack> {
        init_vanilla_registry();
        menu_items(Some(&View {
            progress: CastleProgress::default(),
            ranking: Vec::new(),
            rank: crate::rank::HunterRank::D,
            wait,
        }))
    }

    #[test]
    fn the_enter_button_tracks_the_free_entry() {
        let free = page(0);
        let waiting = page(FREE_ENTRY_SECONDS - 1);
        assert_ne!(
            free[ENTER_SLOT], waiting[ENTER_SLOT],
            "a spent free entry must not leave a green button"
        );
    }

    #[test]
    fn the_enter_button_requires_rank_d() {
        let items = menu_items(Some(&View {
            progress: CastleProgress::default(),
            ranking: Vec::new(),
            rank: crate::rank::HunterRank::E,
            wait: 0,
        }));
        let name = items[ENTER_SLOT]
            .get(steel_registry::data_components::vanilla_components::CUSTOM_NAME)
            .expect("entry name")
            .to_plain(&text_components::resolving::NoResolutor);
        assert_eq!(name, "Нужен ранг D");
    }
}
