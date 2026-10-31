//! The Association's starter-kit desk as a chest menu.
//!
//! The top row explains the kit; the middle row shows the three starter
//! weapons with their stats. Clicking a weapon claims the kit with it, or,
//! after the claim and before the first instance entry, swaps the claimed
//! weapon for it. A kit claimed before the pickaxe joined it collects the
//! pickaxe from its own button.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_registry::{vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::item::serial::{self, STARTER_WEAPONS, SerialId};
use crate::item::{append_lore, named, with_lore};
use crate::merchant_menu;
use crate::starter::{self, StarterKit};
use crate::state::{SoloLevelingState, StarterError};

const MENU_SLOTS: usize = 27;
const INFO_SLOT: usize = 4;
/// Menu slots of the three starter weapons, in [`STARTER_WEAPONS`] order.
const WEAPON_SLOTS: [usize; 3] = [11, 13, 15];
/// Button that opens the city merchant.
const MERCHANT_SLOT: usize = 22;
/// Button that hands out a pickaxe an earlier claim still owes.
const PICKAXE_SLOT: usize = 20;

/// Opens the starter-kit desk.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    player.open_menu(
        TextComponent::plain("Помощь Ассоциации").color(Color::DarkGreen),
        move |context| build(context.container_id, context.player, state),
    );
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>) -> Menu {
    let uuid = player.gameprofile.id();
    let view = state.starter_kit(uuid);
    let container = SimpleContainer::from_items(menu_items(view, uuid)).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(StarterMenuKind {
        state,
        buttons,
        container,
        view,
    })
}

fn weapon_at(slot: usize) -> Option<SerialId> {
    WEAPON_SLOTS
        .iter()
        .position(|weapon_slot| *weapon_slot == slot)
        .map(|index| STARTER_WEAPONS[index])
}

fn menu_items(view: Option<StarterKit>, uuid: Uuid) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    let Some(view) = view else {
        return items;
    };
    items[INFO_SLOT] = info_item(view);
    for (slot, weapon) in WEAPON_SLOTS.into_iter().zip(STARTER_WEAPONS) {
        items[slot] = weapon_item(view, weapon, uuid);
    }
    if view.pickaxe_owed() {
        items[PICKAXE_SLOT] = append_lore(
            serial::stack(SerialId::HunterPickaxe, Some(uuid)),
            vec![
                TextComponent::plain("Кирка теперь входит в стартовый набор.").color(Color::White),
                TextComponent::plain("Клик — получить.").color(Color::Green),
            ],
        );
    }
    items[MERCHANT_SLOT] = with_lore(
        named(
            ItemStack::new(&vanilla_items::EMERALD),
            TextComponent::plain("Городской торговец").color(Color::Gold),
        ),
        vec![
            TextComponent::plain("Замена стартовых вещей и броня специализаций за Won.")
                .color(Color::Gray),
            TextComponent::plain("Клик — открыть витрины.").color(Color::Yellow),
        ],
    );
    items
}

fn info_item(view: StarterKit) -> ItemStack {
    let status = if view.claimable() {
        "Набор ещё не получен: выберите оружие ниже."
    } else if view.swappable() {
        "Набор получен. До первого входа во врата оружие можно обменять бесплатно."
    } else {
        "Набор получен. Новое оружие продаёт торговец."
    };
    with_lore(
        named(
            ItemStack::new(&vanilla_items::WRITABLE_BOOK),
            TextComponent::plain("Стартовый набор охотника").color(Color::Gold),
        ),
        vec![
            TextComponent::plain("Одно оружие на выбор, щит, четыре части брони")
                .color(Color::Gray),
            TextComponent::plain("и кирка для магических кристаллов в подземельях.")
                .color(Color::Gray),
            TextComponent::plain("Броня надевается в обычные слоты тела. Меню Системы")
                .color(Color::Gray),
            TextComponent::plain("принимает только артефакты.").color(Color::Gray),
            TextComponent::plain("Вещи привязаны к вам, не ломаются и не выпадают при смерти.")
                .color(Color::Gray),
            TextComponent::plain("Набор выдаётся один раз и не восстанавливается.")
                .color(Color::Red),
            TextComponent::plain(status).color(Color::Yellow),
        ],
    )
}

fn weapon_item(view: StarterKit, weapon: SerialId, uuid: Uuid) -> ItemStack {
    let action = match view.weapon {
        None => ("Клик — получить набор с этим оружием.", Color::Green),
        Some(chosen) if chosen == weapon => ("Ваше текущее оружие.", Color::Aqua),
        Some(_) if view.swappable() => (
            "Клик — обменять: старое оружие должно быть в инвентаре.",
            Color::Yellow,
        ),
        Some(_) => ("Обмен закрыт после входа во врата.", Color::DarkGray),
    };
    append_lore(
        serial::stack(weapon, Some(uuid)),
        vec![
            TextComponent::plain(starter::weapon_role(weapon)).color(Color::White),
            TextComponent::plain(action.0).color(action.1),
        ],
    )
}

pub(crate) struct StarterMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    view: Option<StarterKit>,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for StarterMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/starter");
}

impl MenuKind for StarterMenuKind {
    fn on_tick(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        let uuid = player.gameprofile.id();
        let view = self.state.starter_kit(uuid);
        if self.view == view {
            return;
        }
        let container_id = ContainerId::from_arc(&self.container);
        if let Some(container) = guard.get_typed_mut::<SimpleContainer>(container_id) {
            for (index, item) in menu_items(view, uuid).into_iter().enumerate() {
                container.set_item(index, item);
            }
            self.view = view;
        }
    }

    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
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
        if slot - self.buttons.start() == MERCHANT_SLOT {
            merchant_menu::open(player, Arc::clone(&self.state));
            return ClickOutcome::Consume;
        }
        if slot - self.buttons.start() == PICKAXE_SLOT {
            if self.view.is_some_and(StarterKit::pickaxe_owed) {
                tell_result(
                    player,
                    self.state
                        .request_starter_pickaxe(player)
                        .map_err(StarterError::message),
                );
            }
            return ClickOutcome::Consume;
        }
        let Some(weapon) = weapon_at(slot - self.buttons.start()) else {
            return ClickOutcome::Consume;
        };
        let Some(view) = self.state.starter_kit(player.gameprofile.id()) else {
            return ClickOutcome::Consume;
        };
        let result = match view.weapon {
            None => self.state.request_starter_kit(player, weapon),
            Some(chosen) if chosen == weapon => return ClickOutcome::Consume,
            Some(_) => self.state.request_starter_swap(player, weapon),
        };
        tell_result(player, result.map_err(StarterError::message));
        ClickOutcome::Consume
    }
}

fn tell_result(player: &Player, result: Result<(), &'static str>) {
    match result {
        Ok(()) => {
            player.send_message(&TextComponent::plain("Запрос обрабатывается.").color(Color::Gold));
        }
        Err(error) => {
            player.send_message(&TextComponent::plain(error).color(Color::Red));
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_items};
    use uuid::Uuid;

    use super::{INFO_SLOT, WEAPON_SLOTS, menu_items, weapon_at};
    use crate::item::serial::{self, STARTER_WEAPONS};
    use crate::starter::StarterKit;

    #[test]
    fn every_starter_weapon_has_one_button() {
        for (slot, weapon) in WEAPON_SLOTS.into_iter().zip(STARTER_WEAPONS) {
            assert_eq!(weapon_at(slot), Some(weapon));
        }
        assert_eq!(weapon_at(INFO_SLOT), None);
    }

    #[test]
    fn the_desk_shows_the_kit_and_the_weapons() {
        init_vanilla_registry();
        let items = menu_items(Some(StarterKit::default()), Uuid::new_v4());
        assert!(items[INFO_SLOT].is(&vanilla_items::WRITABLE_BOOK));
        for (slot, weapon) in WEAPON_SLOTS.into_iter().zip(STARTER_WEAPONS) {
            assert_eq!(serial::of(&items[slot]), Some(weapon));
        }
    }
}
