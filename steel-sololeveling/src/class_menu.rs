//! Hunter class selection as a chest menu.
//!
//! The top row shows the current class and the change rules; the middle row
//! holds one button per class. A click picks the class at once: the choice
//! touches only the account, never the inventory the click has locked. A
//! Mage also gets a button that hands out a lost grimoire on the next tick.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_registry::items::ItemRef;
use steel_registry::{vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::class::{self, HunterClass};
use crate::item::{named, with_lore};
use crate::rank::HunterRank;
use crate::state::SoloLevelingState;

const MENU_SLOTS: usize = 27;
const INFO_SLOT: usize = 4;
const FIRST_CLASS_SLOT: usize = 11;
/// Button that hands a Mage a new grimoire.
const GRIMOIRE_SLOT: usize = 22;

/// Opens the class menu for an online hunter.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    player.open_menu(
        TextComponent::plain("Класс охотника").color(Color::DarkAqua),
        move |context| build(context.container_id, context.player, state),
    );
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>) -> Menu {
    let items = menu_items(&state, player.gameprofile.id());
    let container = SimpleContainer::from_items(items).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(ClassMenuKind {
        state,
        buttons,
        container,
    })
}

/// Returns the class shown at a menu slot.
fn class_at(slot: usize) -> Option<HunterClass> {
    slot.checked_sub(FIRST_CLASS_SLOT)
        .and_then(|index| class::ALL.get(index).copied())
}

fn icon(class: HunterClass) -> ItemRef {
    match class {
        HunterClass::Fighter => &vanilla_items::IRON_SWORD,
        HunterClass::Tanker => &vanilla_items::SHIELD,
        HunterClass::Assassin => &vanilla_items::IRON_HOE,
        HunterClass::Ranger => &vanilla_items::BOW,
        HunterClass::Mage => &vanilla_items::BLAZE_ROD,
    }
}

fn menu_items(state: &SoloLevelingState, uuid: Uuid) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    let current = state.class_of(uuid);
    let (rank, free_change) = state
        .with_session_mut(uuid, |session| {
            (session.stats.rank(), session.class.free_change)
        })
        .unwrap_or((HunterRank::E, false));
    let first_pick = current.is_none() || free_change;
    if current == Some(HunterClass::Mage) {
        items[GRIMOIRE_SLOT] = with_lore(
            named(
                ItemStack::new(&vanilla_items::BOOK),
                TextComponent::plain("Гримуар Системы").color(Color::LightPurple),
            ),
            vec![
                TextComponent::plain("Клик — получить гримуар, если его нет.").color(Color::Yellow),
            ],
        );
    }
    items[INFO_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::NETHER_STAR),
            TextComponent::plain(current.map_or_else(
                || "Класс не выбран".to_owned(),
                |class| format!("Ваш класс: {}", class.display_name()),
            ))
            .color(Color::Gold),
        ),
        vec![
            TextComponent::plain("Выбор доступен с ранга D.").color(Color::Gray),
            TextComponent::plain(if free_change {
                "Класс Целителя упразднён: следующий выбор бесплатный."
            } else {
                "Первый выбор — бесплатно."
            })
            .color(if free_change {
                Color::Green
            } else {
                Color::Gray
            }),
            TextComponent::plain(format!(
                "Смена — раз в 7 дней за {} Won.",
                class::CHANGE_PRICE
            ))
            .color(Color::Gray),
        ],
    );
    for (index, class) in class::ALL.into_iter().enumerate() {
        let chosen = current == Some(class);
        let action = if chosen {
            ("Выбран.".to_owned(), Color::Green)
        } else if first_pick {
            ("Клик — выбрать бесплатно.".to_owned(), Color::Yellow)
        } else {
            (
                format!("Клик — сменить за {} Won.", class::CHANGE_PRICE),
                Color::Yellow,
            )
        };
        items[FIRST_CLASS_SLOT + index] = with_lore(
            named(
                ItemStack::new(icon(class)),
                TextComponent::plain(class.display_name()).color(if chosen {
                    Color::Green
                } else {
                    Color::Aqua
                }),
            ),
            class
                .describe(rank)
                .into_iter()
                .map(|line| TextComponent::plain(line).color(Color::Gray))
                .chain([
                    TextComponent::plain("Бонусы растут с рангом: на S вдвое.")
                        .color(Color::DarkGray),
                    TextComponent::plain(action.0).color(action.1),
                ])
                .collect(),
        );
    }
    items
}

struct ClassMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for ClassMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/class");
}

impl MenuKind for ClassMenuKind {
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
        if slot - self.buttons.start() == GRIMOIRE_SLOT {
            if self.state.class_of(player.gameprofile.id()) == Some(HunterClass::Mage) {
                self.state.request_grimoire(player.gameprofile.id());
            }
            return ClickOutcome::Consume;
        }
        let Some(chosen) = class_at(slot - self.buttons.start()) else {
            return ClickOutcome::Consume;
        };
        match self.state.choose_class(player, chosen) {
            Ok(price) => {
                let paid = if price > 0 {
                    format!(" Списано {price} Won.")
                } else {
                    String::new()
                };
                player.send_message(
                    &TextComponent::plain(format!("Ваш класс: {}.{paid}", chosen.display_name()))
                        .color(Color::Gold),
                );
            }
            Err(error) => {
                player.send_message(&TextComponent::plain(error.message()).color(Color::Red));
            }
        }
        let id = ContainerId::from_arc(&self.container);
        let items = menu_items(&self.state, player.gameprofile.id());
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(id) {
            for (index, item) in items.into_iter().enumerate() {
                container.set_item(index, item);
            }
        }
        ClickOutcome::Consume
    }
}

#[cfg(test)]
mod tests {
    use super::{FIRST_CLASS_SLOT, GRIMOIRE_SLOT, INFO_SLOT, MENU_SLOTS, class_at};
    use crate::class;

    #[test]
    fn every_class_has_one_slot_inside_the_menu() {
        for (index, expected) in class::ALL.into_iter().enumerate() {
            let slot = FIRST_CLASS_SLOT + index;
            assert!(slot < MENU_SLOTS);
            assert_eq!(class_at(slot), Some(expected));
        }
        assert_eq!(class_at(INFO_SLOT), None);
        assert_eq!(class_at(GRIMOIRE_SLOT), None);
    }
}
