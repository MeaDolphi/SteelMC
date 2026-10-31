//! Hunter class selection as a chest menu.
//!
//! The top row shows the current class and the change rules; the middle row
//! holds one button per class. A click picks the class at once: the choice
//! touches only the account, never the inventory the click has locked. The
//! bottom row hands a lost grimoire to a Mage, opens the class vault and
//! starts the Second Awakening trial. A Mage also gets a button that hands
//! out a lost grimoire on the next tick.

use std::ops::Range;
use std::sync::Arc;

use steel_core::entity::Entity;
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
use crate::dungeon::is_instance_world;
use crate::item::{append_lore, named, with_lore};
use crate::rank::HunterRank;
use crate::sound::SystemSound;
use crate::state::SoloLevelingState;
use crate::ui::{After, open_guarded_buttons};

const MENU_SLOTS: usize = 27;
const INFO_SLOT: usize = 4;
const FIRST_CLASS_SLOT: usize = 11;
/// Button that hands a Mage a new grimoire.
const GRIMOIRE_SLOT: usize = 22;
/// Button that opens the class vault.
const VAULT_SLOT: usize = 20;
/// Button that starts the Second Awakening trial.
const AWAKENING_SLOT: usize = 24;
/// Vault menu page size.
const VAULT_PAGE_SIZE: usize = 45;

/// Opens the class menu for an online hunter.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    player.open_menu(
        TextComponent::plain("Класс охотника").color(Color::DarkAqua),
        move |context| build(context.container_id, context.player, state),
    );
}

fn vault_page_range(length: usize, page: usize) -> Range<usize> {
    let start = page.saturating_mul(VAULT_PAGE_SIZE).min(length);
    start..start.saturating_add(VAULT_PAGE_SIZE).min(length)
}

/// Opens one page of the hunter's class vault.
///
/// Frozen items are listed for review and cannot leave; items the current
/// class may use are withdrawn with one click.
pub fn open_vault(player: &Player, state: Arc<SoloLevelingState>, page: usize) {
    let uuid = player.gameprofile.id();
    // Opening the menu, from the tick or a click, settles a pending reopen.
    state.with_session_mut(uuid, |session| session.class_vault_menu_due = false);
    if player
        .level()
        .is_some_and(|world| is_instance_world(&world))
    {
        player.send_message(
            &TextComponent::plain("Хранилище недоступно в подземелье.").color(Color::Red),
        );
        return;
    }
    SystemSound::Open.play(player);
    let current = state.class_of(uuid);
    let entries = state.class_vault_items(uuid);
    let page_count = entries.len().div_ceil(VAULT_PAGE_SIZE).max(1);
    let page = page.min(page_count - 1);
    let range = vault_page_range(entries.len(), page);
    let mut contents = vec![ItemStack::empty(); 54];
    for (slot, (_, _, item)) in entries[range.clone()].iter().enumerate() {
        let frozen = class::is_class_frozen(item, uuid, current);
        let line = if frozen {
            TextComponent::plain("Заморожено: не подходит вашему классу.").color(Color::Red)
        } else {
            TextComponent::plain("Нажмите, чтобы забрать.").color(Color::Green)
        };
        contents[slot] = append_lore(item.clone(), vec![line]);
    }
    if page > 0 {
        contents[45] = named(
            ItemStack::new(&vanilla_items::ARROW),
            TextComponent::plain("Предыдущая страница").color(Color::Gold),
        );
    }
    if page + 1 < page_count {
        contents[53] = named(
            ItemStack::new(&vanilla_items::ARROW),
            TextComponent::plain("Следующая страница").color(Color::Gold),
        );
    }
    let frozen_count = entries
        .iter()
        .filter(|(_, _, item)| class::is_class_frozen(item, uuid, current))
        .count();
    contents[49] = with_lore(
        named(
            ItemStack::new(&vanilla_items::CHEST),
            TextComponent::plain("Хранилище класса").color(Color::Aqua),
        ),
        vec![
            TextComponent::plain(
                "Личные предметы, не подходящие вашему классу, ждут здесь подходящего класса.",
            )
            .color(Color::Gray),
            TextComponent::plain(format!(
                "Страница {} из {} · хранится {} · заморожено {}",
                page + 1,
                page_count,
                entries.len(),
                frozen_count
            ))
            .color(Color::Gray),
        ],
    );
    let selected: Vec<_> = entries
        .into_iter()
        .skip(range.start)
        .take(range.len())
        .collect();
    open_guarded_buttons(
        player,
        TextComponent::plain("Хранилище класса").color(Color::Aqua),
        6,
        contents,
        Box::new(move |player, slot, _| {
            if player
                .level()
                .is_some_and(|world| is_instance_world(&world))
            {
                player.send_message(
                    &TextComponent::plain("Хранилище недоступно в подземелье.").color(Color::Red),
                );
                return After::Keep;
            }
            if slot == 45 && page > 0 {
                open_vault(player, Arc::clone(&state), page - 1);
            } else if slot == 53 && page + 1 < page_count {
                open_vault(player, Arc::clone(&state), page + 1);
            } else if let Some((index, bytes, item)) = selected.get(slot) {
                if class::is_class_frozen(item, player.gameprofile.id(), current) {
                    player.send_message(
                        &TextComponent::plain("Этот предмет не подходит вашему классу.")
                            .color(Color::Red),
                    );
                } else {
                    state.request_class_vault_withdrawal(player, *index, bytes.clone());
                }
            }
            After::Keep
        }),
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

/// Builds the button that opens the class vault.
fn vault_button(stored: usize) -> ItemStack {
    with_lore(
        named(
            ItemStack::new(&vanilla_items::CHEST),
            TextComponent::plain("Хранилище класса").color(Color::Aqua),
        ),
        vec![
            TextComponent::plain("Личные предметы, не подходящие вашему классу, хранятся здесь.")
                .color(Color::Gray),
            TextComponent::plain(format!("Сейчас хранится: {stored}.")).color(Color::Gray),
            TextComponent::plain("Клик — открыть.").color(Color::Yellow),
        ],
    )
}

fn menu_items(state: &SoloLevelingState, uuid: Uuid) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    let current = state.class_of(uuid);
    let (rank, free_change, evolved) = state
        .with_session_mut(uuid, |session| {
            (
                session.stats.rank(),
                session.class.free_change,
                session.class.evolved,
            )
        })
        .unwrap_or((HunterRank::E, false, false));
    let first_pick = current.is_none() || free_change;
    items[VAULT_SLOT] = vault_button(state.class_vault_items(uuid).len());
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
                .describe(rank, evolved)
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
    if state.job_change_available(uuid) {
        items[AWAKENING_SLOT] = with_lore(
            named(
                ItemStack::new(&vanilla_items::ENDER_EYE),
                TextComponent::plain("Второе пробуждение").color(Color::LightPurple),
            ),
            vec![
                TextComponent::plain("Клик — бросить вызов Рыцарю Игрису.").color(Color::Yellow),
                TextComponent::plain("Доступно с 40 уровня.").color(Color::Gray),
            ],
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
            let uuid = player.gameprofile.id();
            if self.state.class_of(uuid) == Some(HunterClass::Mage) {
                if self.state.class_vault_has_grimoire(uuid) {
                    player.send_message(
                        &TextComponent::plain(
                            "Ваш гримуар в Хранилище класса — заберите его оттуда.",
                        )
                        .color(Color::Yellow),
                    );
                } else {
                    self.state.request_grimoire(uuid);
                }
            }
            return ClickOutcome::Consume;
        }
        if slot - self.buttons.start() == VAULT_SLOT {
            open_vault(player, Arc::clone(&self.state), 0);
            return ClickOutcome::Consume;
        }
        if slot - self.buttons.start() == AWAKENING_SLOT {
            match self.state.start_job_change(player) {
                Ok(()) => {
                    player.send_message(
                        &TextComponent::plain("Второе пробуждение: арена готовится.")
                            .color(Color::Gold),
                    );
                }
                Err(error) => {
                    player.send_message(&TextComponent::plain(error).color(Color::Red));
                }
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
    use super::{
        AWAKENING_SLOT, FIRST_CLASS_SLOT, GRIMOIRE_SLOT, INFO_SLOT, MENU_SLOTS, VAULT_SLOT,
        class_at, vault_page_range,
    };
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
        assert_eq!(class_at(VAULT_SLOT), None);
        assert_eq!(class_at(AWAKENING_SLOT), None);
    }

    #[test]
    fn vault_menu_pages_select_each_stored_index_once() {
        let first = vault_page_range(91, 0);
        let second = vault_page_range(91, 1);
        let third = vault_page_range(91, 2);
        assert_eq!(first, 0..45);
        assert_eq!(second, 45..90);
        assert_eq!(third, 90..91);
        assert!(first.end <= second.start && second.end <= third.start);
    }
}
