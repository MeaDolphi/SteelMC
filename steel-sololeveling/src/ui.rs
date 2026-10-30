//! Reusable chest-menu widgets, so no mechanic needs a chat command.
//!
//! [`open_buttons`] is a chest of buttons driven by one click handler;
//! [`pick_player`], [`pick_amount`], [`confirm`] and [`ask_text`] build the
//! inputs commands used to take as arguments: a hunter, a number, a yes, and
//! free text (typed into the anvil's rename field).

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

use crate::item::{model, named, with_lore};
use crate::state::SoloLevelingState;

/// What a button click asks of its menu.
pub enum After {
    /// Keep the menu as it is.
    Keep,
    /// Redraw the menu with new items.
    Redraw(Vec<ItemStack>),
    /// The click opened another menu; leave this one alone.
    Replaced,
}

/// A click handler: the clicked slot and mouse button.
pub type Handler = Box<dyn FnMut(&Player, usize, MouseButton) -> After + Send + Sync>;

/// Called with the chosen hunter's id and name.
pub type OnPlayer = Arc<dyn Fn(&Player, Uuid, String) + Send + Sync>;
/// Called with the chosen amount.
pub type OnAmount = Arc<dyn Fn(&Player, u64) + Send + Sync>;
/// Called when the hunter confirms.
pub type OnConfirm = Arc<dyn Fn(&Player) + Send + Sync>;
/// Called with the typed text.
pub type OnText = Arc<dyn Fn(&Player, String) + Send + Sync>;

/// Builds a named button with gray lore lines.
#[must_use]
pub fn button(icon: ItemRef, title: impl Into<String>, color: Color, lore: &[&str]) -> ItemStack {
    with_lore(
        named(
            ItemStack::new(icon),
            TextComponent::plain(title.into()).color(color),
        ),
        lore.iter()
            .map(|line| TextComponent::plain((*line).to_owned()).color(Color::Gray))
            .collect(),
    )
}

/// Gives a button the pack's menu icon `model`, e.g. `ui/next`.
#[must_use]
pub fn styled(mut stack: ItemStack, model: &str) -> ItemStack {
    model::apply(&mut stack, model);
    stack
}

/// The page-back button every paged menu shows.
#[must_use]
pub fn previous_page() -> ItemStack {
    styled(
        button(
            &vanilla_items::ARROW,
            "Предыдущая страница",
            Color::Yellow,
            &[],
        ),
        "ui/previous",
    )
}

/// The page-forward button every paged menu shows.
#[must_use]
pub fn next_page() -> ItemStack {
    styled(
        button(
            &vanilla_items::ARROW,
            "Следующая страница",
            Color::Yellow,
            &[],
        ),
        "ui/next",
    )
}

/// Sends one coloured chat line.
pub fn tell(player: &Player, text: &str, color: Color) {
    player.send_message(&TextComponent::plain(text.to_owned()).color(color));
}

/// Opens a chest of `rows` rows whose clicks all go to `handler`.
pub fn open_buttons(
    player: &Player,
    title: TextComponent,
    rows: usize,
    items: Vec<ItemStack>,
    handler: Handler,
) {
    open_buttons_inner(player, title, rows, items, handler, false);
}

/// Opens buttons that cannot receive or release real items by any menu action.
pub fn open_guarded_buttons(
    player: &Player,
    title: TextComponent,
    rows: usize,
    items: Vec<ItemStack>,
    handler: Handler,
) {
    open_buttons_inner(player, title, rows, items, handler, true);
}

fn open_buttons_inner(
    player: &Player,
    title: TextComponent,
    rows: usize,
    items: Vec<ItemStack>,
    handler: Handler,
    guarded: bool,
) {
    let menu_type = if rows > 3 {
        &vanilla_menu_types::GENERIC_9X6
    } else {
        &vanilla_menu_types::GENERIC_9X3
    };
    let slots = if rows > 3 { 54 } else { 27 };
    let mut items = items;
    items.resize(slots, ItemStack::empty());
    player.open_menu(title, move |context| {
        let container = SimpleContainer::from_items(items).into_shared();
        let mut builder = MenuBuilder::new(menu_type, context.container_id);
        let buttons = if guarded {
            builder.section_all_with(
                Arc::clone(&container),
                SectionKind::guarded(|_, _| false, |_, _, _, _| false),
            )
        } else {
            builder.section_all(Arc::clone(&container))
        };
        builder.player_inventory(&context.player.inventory);
        builder.build(ButtonMenuKind {
            buttons,
            container,
            handler,
        })
    });
}

struct ButtonMenuKind {
    buttons: Section,
    container: Shared<SimpleContainer>,
    handler: Handler,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for ButtonMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/buttons");
}

impl MenuKind for ButtonMenuKind {
    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        click: Click,
        player: &Player,
    ) -> ClickOutcome {
        let Click::Pickup { slot, button } = click else {
            return if click.slot().is_some_and(|slot| self.buttons.contains(slot)) {
                ClickOutcome::Consume
            } else {
                ClickOutcome::Fallthrough
            };
        };
        if !self.buttons.contains(slot) {
            return ClickOutcome::Fallthrough;
        }
        if let After::Redraw(items) = (self.handler)(player, slot - self.buttons.start(), button) {
            let id = ContainerId::from_arc(&self.container);
            if let Some(container) = guard.get_typed_mut::<SimpleContainer>(id) {
                for (index, item) in items.into_iter().enumerate() {
                    container.set_item(index, item);
                }
            }
        }
        ClickOutcome::Consume
    }
}

/// Lists online hunters other than the viewer and reports the one clicked.
pub fn pick_player(
    player: &Player,
    state: &SoloLevelingState,
    title: &str,
    exclude: &[Uuid],
    on_pick: OnPlayer,
) {
    let viewer = player.gameprofile.id();
    let mut candidates: Vec<(Uuid, String)> = state
        .online_players()
        .into_iter()
        .map(|online| {
            (
                online.gameprofile.id(),
                online.gameprofile.name().to_owned(),
            )
        })
        .filter(|(uuid, _)| *uuid != viewer && !exclude.contains(uuid))
        .collect();
    candidates.sort_by_key(|candidate| candidate.1.to_lowercase());
    candidates.truncate(45);
    let mut items: Vec<ItemStack> = candidates
        .iter()
        .map(|(_, name)| {
            button(
                &vanilla_items::PLAYER_HEAD,
                name.clone(),
                Color::Aqua,
                &["Клик — выбрать."],
            )
        })
        .collect();
    if candidates.is_empty() {
        items.resize(22, ItemStack::empty());
        items.push(button(
            &vanilla_items::BARRIER,
            "Нет подходящих игроков онлайн",
            Color::Gray,
            &[],
        ));
    }
    open_buttons(
        player,
        TextComponent::plain(title.to_owned()).color(Color::DarkAqua),
        6,
        items,
        Box::new(move |player, slot, _| {
            if let Some((uuid, name)) = candidates.get(slot) {
                on_pick(player, *uuid, name.clone());
                return After::Replaced;
            }
            After::Keep
        }),
    );
}

/// Steps of the amount picker: slot and change.
const AMOUNT_STEPS: [(usize, i64); 8] = [
    (10, -1000),
    (11, -100),
    (12, -10),
    (13, -1),
    (14, 1),
    (15, 10),
    (16, 100),
    (17, 1000),
];
const AMOUNT_SHOW_SLOT: usize = 4;
const AMOUNT_CONFIRM_SLOT: usize = 22;
const AMOUNT_CANCEL_SLOT: usize = 18;

/// Applies one picker step, keeping the amount inside `min..=max`.
#[must_use]
pub fn step_amount(amount: u64, step: i64, min: u64, max: u64) -> u64 {
    let moved = if step < 0 {
        amount.saturating_sub(step.unsigned_abs())
    } else {
        amount.saturating_add(step.unsigned_abs())
    };
    moved.clamp(min, max)
}

fn amount_items(title: &str, unit: &str, amount: u64, min: u64, max: u64) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); 27];
    items[AMOUNT_SHOW_SLOT] = button(
        &vanilla_items::GOLD_INGOT,
        format!("{title}: {amount} {unit}"),
        Color::Gold,
        &[&format!("От {min} до {max}.")],
    );
    for (slot, step) in AMOUNT_STEPS {
        let (icon, color): (ItemRef, Color) = if step < 0 {
            (&vanilla_items::RED_STAINED_GLASS_PANE, Color::Red)
        } else {
            (&vanilla_items::LIME_STAINED_GLASS_PANE, Color::Green)
        };
        items[slot] = styled(
            button(icon, format!("{step:+}"), color, &[]),
            if step < 0 { "ui/minus" } else { "ui/plus" },
        );
    }
    items[AMOUNT_CONFIRM_SLOT] = styled(
        button(
            &vanilla_items::EMERALD_BLOCK,
            format!("Подтвердить: {amount} {unit}"),
            Color::Green,
            &[],
        ),
        "ui/confirm",
    );
    items[AMOUNT_CANCEL_SLOT] = styled(
        button(&vanilla_items::BARRIER, "Отмена", Color::Red, &[]),
        "ui/cancel",
    );
    items
}

/// Asks for a number between `min` and `max` with +/- buttons.
pub fn pick_amount(
    player: &Player,
    title: &str,
    unit: &str,
    initial: u64,
    min: u64,
    max: u64,
    on_pick: OnAmount,
) {
    let title = title.to_owned();
    let unit = unit.to_owned();
    let mut amount = initial.clamp(min, max);
    open_buttons(
        player,
        TextComponent::plain(title.clone()).color(Color::Gold),
        3,
        amount_items(&title, &unit, amount, min, max),
        Box::new(move |player, slot, _| {
            if slot == AMOUNT_CONFIRM_SLOT {
                on_pick(player, amount);
                return After::Replaced;
            }
            if slot == AMOUNT_CANCEL_SLOT {
                player.close_container();
                return After::Replaced;
            }
            let Some((_, step)) = AMOUNT_STEPS.iter().find(|(at, _)| *at == slot) else {
                return After::Keep;
            };
            amount = step_amount(amount, *step, min, max);
            After::Redraw(amount_items(&title, &unit, amount, min, max))
        }),
    );
}

/// Asks for a yes before an irreversible action.
pub fn confirm(player: &Player, question: &str, on_yes: OnConfirm) {
    let mut items = vec![ItemStack::empty(); 27];
    items[4] = button(&vanilla_items::PAPER, question.to_owned(), Color::Gold, &[]);
    items[11] = styled(
        button(&vanilla_items::LIME_CONCRETE, "Да", Color::Green, &[]),
        "ui/confirm",
    );
    items[15] = styled(
        button(&vanilla_items::RED_CONCRETE, "Нет", Color::Red, &[]),
        "ui/cancel",
    );
    open_buttons(
        player,
        TextComponent::plain("Подтверждение").color(Color::Gold),
        3,
        items,
        Box::new(move |player, slot, _| match slot {
            11 => {
                on_yes(player);
                After::Replaced
            }
            15 => {
                player.close_container();
                After::Replaced
            }
            _ => After::Keep,
        }),
    );
}

/// Asks for free text in the anvil rename field.
///
/// The hunter edits the paper's name and clicks the result (or the paper) to
/// submit. Nothing is ever repaired or renamed: the anvil is only an input.
pub fn ask_text(player: &Player, title: &str, placeholder: &str, on_submit: OnText) {
    let placeholder = placeholder.to_owned();
    player.open_menu(
        TextComponent::plain(title.to_owned()).color(Color::Gold),
        move |context| {
            let container = SimpleContainer::from_items(vec![
                named(
                    ItemStack::new(&vanilla_items::PAPER),
                    TextComponent::plain(placeholder.clone()),
                ),
                ItemStack::empty(),
                ItemStack::empty(),
            ])
            .into_shared();
            let mut builder = MenuBuilder::new(&vanilla_menu_types::ANVIL, context.container_id);
            let buttons = builder.section_all(Arc::clone(&container));
            builder.player_inventory(&context.player.inventory);
            builder.build(TextInputKind {
                buttons,
                container,
                placeholder,
                text: String::new(),
                on_submit,
            })
        },
    );
}

struct TextInputKind {
    buttons: Section,
    container: Shared<SimpleContainer>,
    placeholder: String,
    text: String,
    on_submit: OnText,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for TextInputKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/text_input");
}

impl MenuKind for TextInputKind {
    fn on_rename(&mut self, behavior: &mut MenuBehavior, name: String, player: &Player) {
        name.trim().clone_into(&mut self.text);
        let result = if self.text.is_empty() || self.text == self.placeholder {
            ItemStack::empty()
        } else {
            button(
                &vanilla_items::LIME_DYE,
                format!("Готово: {}", self.text),
                Color::Green,
                &["Клик — подтвердить."],
            )
        };
        {
            let mut guard = behavior.lock_all_containers();
            let id = ContainerId::from_arc(&self.container);
            if let Some(container) = guard.get_typed_mut::<SimpleContainer>(id) {
                container.set_item(2, result);
            }
        }
        behavior.broadcast_changes(&player.connection);
    }

    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        click: Click,
        player: &Player,
    ) -> ClickOutcome {
        let Some(slot) = click.slot() else {
            return ClickOutcome::Fallthrough;
        };
        if !self.buttons.contains(slot) {
            return ClickOutcome::Fallthrough;
        }
        if !matches!(click, Click::Pickup { .. }) {
            return ClickOutcome::Consume;
        }
        if self.text.is_empty() || self.text == self.placeholder {
            tell(player, "Сначала впишите текст в поле сверху.", Color::Red);
            return ClickOutcome::Consume;
        }
        (self.on_submit)(player, self.text.clone());
        ClickOutcome::Consume
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AMOUNT_CANCEL_SLOT, AMOUNT_CONFIRM_SLOT, AMOUNT_SHOW_SLOT, AMOUNT_STEPS, step_amount,
    };

    #[test]
    fn amount_steps_stay_inside_the_bounds() {
        assert_eq!(step_amount(5, -10, 1, 100), 1);
        assert_eq!(step_amount(95, 10, 1, 100), 100);
        assert_eq!(step_amount(50, 1, 1, 100), 51);
        assert_eq!(step_amount(u64::MAX, 1000, 0, u64::MAX), u64::MAX);
    }

    #[test]
    fn picker_buttons_do_not_overlap() {
        let mut slots: Vec<usize> = AMOUNT_STEPS.iter().map(|(slot, _)| *slot).collect();
        slots.extend([AMOUNT_SHOW_SLOT, AMOUNT_CONFIRM_SLOT, AMOUNT_CANCEL_SLOT]);
        let count = slots.len();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), count);
        assert!(slots.iter().all(|slot| *slot < 27));
    }
}
