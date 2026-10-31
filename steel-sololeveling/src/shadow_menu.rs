//! The Shadow Monarch army as chest menus.
//!
//! The army menu lists the roster, one spawn egg per shadow with its stats.
//! Left click summons or recalls one shadow, right click opens its upgrade
//! menu; the bottom row summons or recalls the whole army and turns pages.
//! Every action goes through the same state calls as the `/shadows` commands.

use std::sync::Arc;

use steel_core::inventory::container::Container as _;
use steel_core::inventory::prelude::*;
use steel_registry::items::ItemRef;
use steel_registry::{REGISTRY, RegistryExt as _, vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey, Identifier};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::item::{named, with_lore};
use crate::shadow::{
    ATTACK_PER_POINT, BossSource, ELITE_SHADOW_DAMAGE_MULTIPLIER, ELITE_SHADOW_HEALTH_MULTIPLIER,
    ELITE_SHADOW_SPEED_MULTIPLIER, EliteShadow, HEALTH_PER_POINT, MAX_ELITE_SHADOW_SLOTS,
    SPEED_PER_POINT, Shadow, ShadowOrigin, ShadowStat,
};
use crate::state::SoloLevelingState;
use crate::ui;
use steel_core::entity::AlliedMode;

const ARMY_SLOTS: usize = 54;
const PER_PAGE: usize = 45;
const PREVIOUS_SLOT: usize = 45;
const MODE_SLOT: usize = 46;
const SUMMON_ALL_SLOT: usize = 47;
const ELITE_SLOT: usize = 48;
const INFO_SLOT: usize = 49;
const RECALL_ALL_SLOT: usize = 51;
const NEXT_SLOT: usize = 53;

const DETAIL_SLOTS: usize = 27;
const DETAIL_SHADOW_SLOT: usize = 4;
const BACK_SLOT: usize = 18;
const TOGGLE_SLOT: usize = 22;
/// Upgrade buttons and the stat each one raises.
const UPGRADES: [(usize, ShadowStat); 3] = [
    (11, ShadowStat::Health),
    (13, ShadowStat::Attack),
    (15, ShadowStat::Speed),
];

/// Opens the army menu on its first page.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    open_army(player, state, 0);
}

fn open_army(player: &Player, state: Arc<SoloLevelingState>, page: usize) {
    player.open_menu(
        TextComponent::plain("Армия теней").color(Color::DarkPurple),
        move |context| {
            let view = ArmyView::read(&state, context.player.gameprofile.id(), page);
            let container = SimpleContainer::from_items(view.items()).into_shared();
            let mut builder =
                MenuBuilder::new(&vanilla_menu_types::GENERIC_9X6, context.container_id);
            let buttons = builder.section_all(Arc::clone(&container));
            builder.player_inventory(&context.player.inventory);
            builder.build(ArmyMenuKind {
                state,
                buttons,
                container,
                page: view.page,
            })
        },
    );
}

fn open_detail(player: &Player, state: Arc<SoloLevelingState>, shadow_id: Uuid, page: usize) {
    player.open_menu(
        TextComponent::plain("Тень").color(Color::DarkPurple),
        move |context| {
            let items = detail_items(&state, context.player.gameprofile.id(), shadow_id);
            let container = SimpleContainer::from_items(items).into_shared();
            let mut builder =
                MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, context.container_id);
            let buttons = builder.section_all(Arc::clone(&container));
            builder.player_inventory(&context.player.inventory);
            builder.build(DetailMenuKind {
                state,
                buttons,
                container,
                shadow_id,
                page,
            })
        },
    );
}

/// One read of the roster for the army menu.
struct ArmyView {
    roster: Vec<Shadow>,
    active: Vec<Uuid>,
    limit: usize,
    mode: AlliedMode,
    page: usize,
    pages: usize,
}

impl ArmyView {
    fn read(state: &SoloLevelingState, owner: Uuid, page: usize) -> Self {
        let roster = state.shadows(owner).unwrap_or_default();
        let pages = roster.len().div_ceil(PER_PAGE).max(1);
        Self {
            active: state.active_shadows(owner),
            limit: state.shadow_army_limit(owner),
            mode: state.shadow_mode(owner),
            page: page.min(pages - 1),
            pages,
            roster,
        }
    }

    fn shadow_at(&self, slot: usize) -> Option<&Shadow> {
        (slot < PER_PAGE)
            .then(|| self.roster.get(self.page * PER_PAGE + slot))
            .flatten()
    }

    fn items(&self) -> Vec<ItemStack> {
        let mut items = vec![ItemStack::empty(); ARMY_SLOTS];
        for (slot, item) in items.iter_mut().enumerate().take(PER_PAGE) {
            if let Some(shadow) = self.shadow_at(slot) {
                *item = shadow_item(shadow, self.active.contains(&shadow.id), true);
            }
        }
        if self.roster.is_empty() {
            items[22] = with_lore(
                named(
                    ItemStack::new(&vanilla_items::SOUL_LANTERN),
                    TextComponent::plain("Армия пуста").color(Color::Gray),
                ),
                vec![
                    TextComponent::plain("Убейте моба в данже рядом с собой,").color(Color::Gray),
                    TextComponent::plain("и его душа восстанет как тень.").color(Color::Gray),
                ],
            );
        }
        items[PREVIOUS_SLOT] = ui::previous_page();
        items[NEXT_SLOT] = ui::next_page();
        items[SUMMON_ALL_SLOT] = button(
            &vanilla_items::WITHER_SKELETON_SKULL,
            "Призвать армию",
            Color::DarkPurple,
        );
        items[ELITE_SLOT] = button(&vanilla_items::DRAGON_HEAD, "Элитные тени", Color::DarkRed);
        items[MODE_SLOT] = mode_item(self.mode);
        items[RECALL_ALL_SLOT] = button(&vanilla_items::BARRIER, "Отозвать всех", Color::Red);
        items[INFO_SLOT] = with_lore(
            named(
                ItemStack::new(&vanilla_items::ECHO_SHARD),
                TextComponent::plain(format!("Страница {} / {}", self.page + 1, self.pages))
                    .color(Color::DarkPurple),
            ),
            vec![
                TextComponent::plain(format!("Призвано: {} / {}", self.active.len(), self.limit))
                    .color(Color::Gray),
                TextComponent::plain(format!("Теней в армии: {}", self.roster.len()))
                    .color(Color::Gray),
                TextComponent::plain("ЛКМ по тени — призвать или отозвать.").color(Color::Yellow),
                TextComponent::plain("ПКМ по тени — прокачка.").color(Color::Yellow),
            ],
        );
        items
    }
}

/// Returns the mode after `mode` in the button's cycle.
const fn next_mode(mode: AlliedMode) -> AlliedMode {
    match mode {
        AlliedMode::Follow => AlliedMode::Hold,
        AlliedMode::Hold => AlliedMode::Passive,
        AlliedMode::Passive => AlliedMode::Follow,
    }
}

const fn mode_name(mode: AlliedMode) -> &'static str {
    match mode {
        AlliedMode::Follow => "Следовать и защищать",
        AlliedMode::Hold => "Держать позицию",
        AlliedMode::Passive => "Не нападать",
    }
}

fn mode_item(mode: AlliedMode) -> ItemStack {
    let (icon, detail): (ItemRef, &str) = match mode {
        AlliedMode::Follow => (
            &vanilla_items::LEAD,
            "Идут за вами, бьют монстров рядом и тех, кто напал на вас.",
        ),
        AlliedMode::Hold => (
            &vanilla_items::SHIELD,
            "Стоят на месте и бьют монстров в радиусе 10 блоков.",
        ),
        AlliedMode::Passive => (&vanilla_items::FEATHER, "Идут за вами и не начинают бой."),
    };
    with_lore(
        named(
            ItemStack::new(icon),
            TextComponent::plain(format!("Режим: {}", mode_name(mode))).color(Color::Aqua),
        ),
        vec![
            TextComponent::plain(detail).color(Color::Gray),
            TextComponent::plain("Цель, которую вы бьёте или которая бьёт вас,").color(Color::Gray),
            TextComponent::plain("тени атакуют в любом режиме, кроме «Не нападать».")
                .color(Color::Gray),
            TextComponent::plain("Игроков тени не трогают.").color(Color::Gray),
            TextComponent::plain(format!(
                "Клик — {}.",
                mode_name(next_mode(mode)).to_lowercase()
            ))
            .color(Color::Yellow),
        ],
    )
}

fn button(icon: ItemRef, title: &str, color: Color) -> ItemStack {
    named(
        ItemStack::new(icon),
        TextComponent::plain(title.to_owned()).color(color),
    )
}

/// Returns the shadow's spawn egg, or a skull for a mob without one.
fn icon(shadow: &Shadow) -> ItemStack {
    if shadow.is_recovering() {
        return ItemStack::new(&vanilla_items::GRAY_DYE);
    }
    let egg = Identifier::new(
        shadow.entity_type.namespace.to_string(),
        format!("{}_spawn_egg", shadow.entity_type.path),
    );
    REGISTRY.items.by_key(&egg).map_or_else(
        || ItemStack::new(&vanilla_items::WITHER_SKELETON_SKULL),
        ItemStack::new,
    )
}

/// Returns maximum health and attack of a shadow, counting its upgrades.
fn combat_stats(shadow: &Shadow) -> Option<(f64, f64, f64)> {
    let origin = shadow.origin.as_ref()?;
    Some((
        ShadowOrigin::unscale(origin.max_health) + f64::from(shadow.health) * HEALTH_PER_POINT,
        ShadowOrigin::unscale(origin.attack_damage) + f64::from(shadow.attack) * ATTACK_PER_POINT,
        ShadowOrigin::unscale(origin.movement_speed) + f64::from(shadow.speed) * SPEED_PER_POINT,
    ))
}

fn shadow_item(shadow: &Shadow, summoned: bool, with_hints: bool) -> ItemStack {
    let mut lore = vec![
        TextComponent::plain(format!(
            "Уровень {} · XP {}/{}",
            shadow.level,
            shadow.experience,
            shadow.experience_to_next_level()
        ))
        .color(Color::Gray),
    ];
    match combat_stats(shadow) {
        Some((health, attack, speed)) => {
            lore.push(
                TextComponent::plain(format!(
                    "❤ {health:.0} · урон {attack:.1} · скорость {speed:.2}"
                ))
                .color(Color::Red),
            );
        }
        None => lore.push(
            TextComponent::plain("Старая тень: характеристики моба не сохранены.")
                .color(Color::DarkGray),
        ),
    }
    lore.push(
        TextComponent::plain(format!(
            "Прокачка: здоровье {}, атака {}, скорость {}",
            shadow.health, shadow.attack, shadow.speed
        ))
        .color(Color::Gray),
    );
    if let Some(origin) = &shadow.origin
        && !origin.equipment.is_empty()
    {
        lore.push(
            TextComponent::plain(format!("Снаряжение: {} предм.", origin.equipment.len()))
                .color(Color::Gray),
        );
    }
    if shadow.unspent_points > 0 {
        lore.push(
            TextComponent::plain(format!("Свободных очков: {}", shadow.unspent_points))
                .color(Color::Gold),
        );
    }
    let state = match (&shadow.recovery, summoned) {
        (Some(recovery), _) => (
            format!(
                "Восстанавливается: {}/{} HP",
                recovery.health, recovery.maximum_health
            ),
            Color::DarkGray,
        ),
        (None, true) => ("Призвана".to_owned(), Color::Green),
        (None, false) => ("В тени".to_owned(), Color::Gray),
    };
    lore.push(TextComponent::plain(state.0).color(state.1));
    if with_hints {
        lore.push(
            TextComponent::plain(if summoned {
                "ЛКМ — отозвать, ПКМ — прокачка."
            } else {
                "ЛКМ — призвать, ПКМ — прокачка."
            })
            .color(Color::Yellow),
        );
    }
    with_lore(
        named(
            icon(shadow),
            TextComponent::plain(shadow.name.clone()).color(Color::DarkPurple),
        ),
        lore,
    )
}

fn tell(player: &Player, text: &str, color: Color) {
    player.send_message(&TextComponent::plain(text.to_owned()).color(color));
}

/// Summons a resting shadow or recalls a summoned one.
fn toggle(state: &SoloLevelingState, player: &Player, shadow: &Shadow) {
    let owner = player.gameprofile.id();
    if state.dismiss_shadow(owner, shadow.id) {
        tell(player, &format!("«{}» отозвана.", shadow.name), Color::Gray);
        return;
    }
    match state.summon_shadow(player, &shadow.name) {
        Ok(()) => tell(
            player,
            &format!("«{}» призвана.", shadow.name),
            Color::DarkPurple,
        ),
        Err(error) => tell(player, error, Color::Red),
    }
}

fn refresh(
    guard: &mut ContainerLockGuard,
    container: &Shared<SimpleContainer>,
    items: Vec<ItemStack>,
) {
    let id = ContainerId::from_arc(container);
    if let Some(container) = guard.get_typed_mut::<SimpleContainer>(id) {
        for (index, item) in items.into_iter().enumerate() {
            container.set_item(index, item);
        }
    }
}

struct ArmyMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    page: usize,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for ArmyMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/shadow_army");
}

impl MenuKind for ArmyMenuKind {
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
        let owner = player.gameprofile.id();
        let local = slot - self.buttons.start();
        let view = ArmyView::read(&self.state, owner, self.page);
        match local {
            PREVIOUS_SLOT => self.page = view.page.saturating_sub(1),
            NEXT_SLOT => self.page = (view.page + 1).min(view.pages - 1),
            SUMMON_ALL_SLOT => match self.state.summon_all_shadows(player) {
                Some(count) => tell(
                    player,
                    &format!("Призвано теней: {count}."),
                    Color::DarkPurple,
                ),
                None => tell(player, "Призвать теней сейчас нельзя.", Color::Red),
            },
            MODE_SLOT => {
                let mode = next_mode(view.mode);
                self.state.set_shadow_mode(owner, mode);
                tell(
                    player,
                    &format!("Режим армии: {}.", mode_name(mode)),
                    Color::DarkPurple,
                );
            }
            ELITE_SLOT => {
                open_elite_army(player, Arc::clone(&self.state), 0);
                return ClickOutcome::Consume;
            }
            RECALL_ALL_SLOT => {
                let recalled = self.state.recall_shadows(owner);
                tell(player, &format!("Отозвано теней: {recalled}."), Color::Gray);
            }
            _ => {
                let Some(shadow) = view.shadow_at(local) else {
                    return ClickOutcome::Consume;
                };
                if button == MouseButton::Right {
                    open_detail(player, Arc::clone(&self.state), shadow.id, view.page);
                    return ClickOutcome::Consume;
                }
                toggle(&self.state, player, shadow);
            }
        }
        let items = ArmyView::read(&self.state, owner, self.page).items();
        refresh(guard, &self.container, items);
        ClickOutcome::Consume
    }
}

fn detail_items(state: &SoloLevelingState, owner: Uuid, shadow_id: Uuid) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); DETAIL_SLOTS];
    let Some(shadow) = state
        .shadows(owner)
        .and_then(|roster| roster.into_iter().find(|shadow| shadow.id == shadow_id))
    else {
        return items;
    };
    let summoned = state.active_shadows(owner).contains(&shadow.id);
    items[DETAIL_SHADOW_SLOT] = shadow_item(&shadow, summoned, false);
    for (slot, stat) in UPGRADES {
        let (icon, title, gain, have): (ItemRef, &str, String, u16) = match stat {
            ShadowStat::Health => (
                &vanilla_items::GOLDEN_APPLE,
                "Здоровье",
                format!("+{HEALTH_PER_POINT:.0} макс. здоровья"),
                shadow.health,
            ),
            ShadowStat::Attack => (
                &vanilla_items::IRON_SWORD,
                "Атака",
                format!("+{ATTACK_PER_POINT:.1} урона"),
                shadow.attack,
            ),
            ShadowStat::Speed => (
                &vanilla_items::FEATHER,
                "Скорость",
                format!("+{SPEED_PER_POINT:.2} скорости"),
                shadow.speed,
            ),
        };
        let action = if shadow.unspent_points > 0 {
            ("Клик — вложить очко.", Color::Yellow)
        } else {
            (
                "Нет свободных очков: они даются за уровни тени.",
                Color::Red,
            )
        };
        items[slot] = with_lore(
            named(
                ItemStack::new(icon),
                TextComponent::plain(format!("{title}: {have}")).color(Color::Aqua),
            ),
            vec![
                TextComponent::plain(gain).color(Color::Gray),
                TextComponent::plain(format!("Свободных очков: {}", shadow.unspent_points))
                    .color(Color::Gold),
                TextComponent::plain(action.0).color(action.1),
            ],
        );
    }
    items[BACK_SLOT] = button(&vanilla_items::ARROW, "Назад к армии", Color::Yellow);
    items[TOGGLE_SLOT] = if summoned {
        button(&vanilla_items::BARRIER, "Отозвать", Color::Red)
    } else {
        button(
            &vanilla_items::WITHER_SKELETON_SKULL,
            "Призвать",
            Color::DarkPurple,
        )
    };
    items
}

struct DetailMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    shadow_id: Uuid,
    page: usize,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for DetailMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/shadow");
}

impl MenuKind for DetailMenuKind {
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
        let owner = player.gameprofile.id();
        let Some(shadow) = self.state.shadows(owner).and_then(|roster| {
            roster
                .into_iter()
                .find(|shadow| shadow.id == self.shadow_id)
        }) else {
            return ClickOutcome::Consume;
        };
        let local = slot - self.buttons.start();
        if local == BACK_SLOT {
            open_army(player, Arc::clone(&self.state), self.page);
            return ClickOutcome::Consume;
        }
        if local == TOGGLE_SLOT {
            toggle(&self.state, player, &shadow);
        } else if let Some((_, stat)) = UPGRADES.iter().find(|(at, _)| *at == local) {
            match self.state.upgrade_shadow(owner, &shadow.name, *stat) {
                Ok(upgraded) => {
                    if self.state.save_session(owner).is_err() {
                        tell(player, "Не удалось сохранить армию теней.", Color::Red);
                    } else {
                        tell(
                            player,
                            &format!(
                                "«{}» усилена. Осталось очков: {}.",
                                upgraded.name, upgraded.unspent_points
                            ),
                            Color::DarkPurple,
                        );
                    }
                }
                Err(error) => tell(player, error, Color::Red),
            }
        }
        let items = detail_items(&self.state, owner, self.shadow_id);
        refresh(guard, &self.container, items);
        ClickOutcome::Consume
    }
}

// ---------------------------------------------------------------------------
// Elite shadow army menu
// ---------------------------------------------------------------------------

const ELITE_ARMY_SLOT: usize = 48;

fn open_elite_army(player: &Player, state: Arc<SoloLevelingState>, page: usize) {
    player.open_menu(
        TextComponent::plain("Элитные тени").color(Color::DarkRed),
        move |context| {
            let view = EliteArmyView::read(&state, context.player.gameprofile.id(), page);
            let container = SimpleContainer::from_items(view.items()).into_shared();
            let mut builder =
                MenuBuilder::new(&vanilla_menu_types::GENERIC_9X6, context.container_id);
            let buttons = builder.section_all(Arc::clone(&container));
            builder.player_inventory(&context.player.inventory);
            builder.build(EliteArmyMenuKind {
                state,
                buttons,
                container,
                page: view.page,
            })
        },
    );
}

fn open_elite_detail(player: &Player, state: Arc<SoloLevelingState>, shadow_id: Uuid, page: usize) {
    player.open_menu(
        TextComponent::plain("Элитная тень").color(Color::DarkRed),
        move |context| {
            let items = elite_detail_items(&state, context.player.gameprofile.id(), shadow_id);
            let container = SimpleContainer::from_items(items).into_shared();
            let mut builder =
                MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, context.container_id);
            let buttons = builder.section_all(Arc::clone(&container));
            builder.player_inventory(&context.player.inventory);
            builder.build(EliteDetailMenuKind {
                state,
                buttons,
                container,
                shadow_id,
                page,
            })
        },
    );
}

struct EliteArmyView {
    roster: Vec<EliteShadow>,
    active: Vec<Uuid>,
    mode: AlliedMode,
    page: usize,
    pages: usize,
}

impl EliteArmyView {
    fn read(state: &SoloLevelingState, owner: Uuid, page: usize) -> Self {
        let roster = state.elite_shadows(owner).unwrap_or_default();
        let pages = roster.len().div_ceil(PER_PAGE).max(1);
        Self {
            active: state.active_elite_shadows(owner),
            mode: state.shadow_mode(owner),
            page: page.min(pages - 1),
            pages,
            roster,
        }
    }

    fn shadow_at(&self, slot: usize) -> Option<&EliteShadow> {
        (slot < PER_PAGE)
            .then(|| self.roster.get(self.page * PER_PAGE + slot))
            .flatten()
    }

    fn items(&self) -> Vec<ItemStack> {
        let mut items = vec![ItemStack::empty(); ARMY_SLOTS];
        for (slot, item) in items.iter_mut().enumerate().take(PER_PAGE) {
            if let Some(shadow) = self.shadow_at(slot) {
                *item = elite_shadow_item(shadow, self.active.contains(&shadow.id), true);
            }
        }
        if self.roster.is_empty() {
            items[22] = with_lore(
                named(
                    ItemStack::new(&vanilla_items::SOUL_LANTERN),
                    TextComponent::plain("Элитные тени пусты").color(Color::Gray),
                ),
                vec![
                    TextComponent::plain("Убейте данж-босса, гнездо или Короля муравьёв,")
                        .color(Color::Gray),
                    TextComponent::plain("чтобы ритуалом «Восстань» заполучить элитную тень.")
                        .color(Color::Gray),
                ],
            );
        }
        items[PREVIOUS_SLOT] = ui::previous_page();
        items[NEXT_SLOT] = ui::next_page();
        items[SUMMON_ALL_SLOT] = button(
            &vanilla_items::WITHER_SKELETON_SKULL,
            "Призвать элитных",
            Color::DarkRed,
        );
        items[ELITE_ARMY_SLOT] = button(
            &vanilla_items::DRAGON_HEAD,
            "Армия теней",
            Color::DarkPurple,
        );
        items[MODE_SLOT] = mode_item(self.mode);
        items[RECALL_ALL_SLOT] = button(&vanilla_items::BARRIER, "Отозвать элитных", Color::Red);
        items[INFO_SLOT] = with_lore(
            named(
                ItemStack::new(&vanilla_items::ECHO_SHARD),
                TextComponent::plain(format!("Страница {} / {}", self.page + 1, self.pages))
                    .color(Color::DarkRed),
            ),
            vec![
                TextComponent::plain(format!(
                    "Призвано: {} / {}",
                    self.active.len(),
                    MAX_ELITE_SHADOW_SLOTS
                ))
                .color(Color::Gray),
                TextComponent::plain(format!("Элитных теней: {}", self.roster.len()))
                    .color(Color::Gray),
                TextComponent::plain("ЛКМ по тени — призвать или отозвать.").color(Color::Yellow),
                TextComponent::plain("ПКМ по тени — прокачка.").color(Color::Yellow),
            ],
        );
        items
    }
}

fn elite_icon(shadow: &EliteShadow) -> ItemStack {
    if shadow.is_recovering() {
        return ItemStack::new(&vanilla_items::GRAY_DYE);
    }
    let egg = Identifier::new(
        shadow.entity_type.namespace.to_string(),
        format!("{}_spawn_egg", shadow.entity_type.path),
    );
    REGISTRY.items.by_key(&egg).map_or_else(
        || ItemStack::new(&vanilla_items::WITHER_SKELETON_SKULL),
        ItemStack::new,
    )
}

fn elite_combat_stats(shadow: &EliteShadow) -> (f64, f64, f64) {
    (
        ShadowOrigin::unscale(shadow.origin.max_health) * ELITE_SHADOW_HEALTH_MULTIPLIER
            + f64::from(shadow.health) * HEALTH_PER_POINT,
        ShadowOrigin::unscale(shadow.origin.attack_damage) * ELITE_SHADOW_DAMAGE_MULTIPLIER
            + f64::from(shadow.attack) * ATTACK_PER_POINT,
        ShadowOrigin::unscale(shadow.origin.movement_speed) * ELITE_SHADOW_SPEED_MULTIPLIER
            + f64::from(shadow.speed) * SPEED_PER_POINT,
    )
}

fn elite_shadow_item(shadow: &EliteShadow, summoned: bool, with_hints: bool) -> ItemStack {
    let mut lore = vec![
        TextComponent::plain(format!(
            "Уровень {} · XP {}/{}",
            shadow.level,
            shadow.experience,
            shadow.experience_to_next_level()
        ))
        .color(Color::Gray),
    ];
    let (health, attack, speed) = elite_combat_stats(shadow);
    lore.push(
        TextComponent::plain(format!(
            "❤ {health:.0} · урон {attack:.1} · скорость {speed:.2}"
        ))
        .color(Color::Red),
    );
    lore.push(
        TextComponent::plain(format!(
            "Прокачка: здоровье {}, атака {}, скорость {}",
            shadow.health, shadow.attack, shadow.speed
        ))
        .color(Color::Gray),
    );
    if !shadow.origin.equipment.is_empty() {
        lore.push(
            TextComponent::plain(format!(
                "Снаряжение: {} предм.",
                shadow.origin.equipment.len()
            ))
            .color(Color::Gray),
        );
    }
    if shadow.unspent_points > 0 {
        lore.push(
            TextComponent::plain(format!("Свободных очков: {}", shadow.unspent_points))
                .color(Color::Gold),
        );
    }
    let source = match &shadow.source {
        BossSource::Dungeon {
            rank,
            archetype,
            red,
        } => {
            let red_mark = if *red { " ☠" } else { "" };
            format!("Источник: данж {}{red_mark} · {archetype}", rank.name())
        }
        BossSource::IslandNest {
            zone_rank,
            zone_name,
        } => {
            format!("Источник: остров {} · {zone_name}", zone_rank.name())
        }
        BossSource::IslandKing => "Источник: Король муравьёв".to_owned(),
        BossSource::CastleFloor { floor } => {
            format!("Источник: Замок демонов, этаж {floor}")
        }
    };
    lore.push(TextComponent::plain(source).color(Color::DarkGray));
    let state = match (&shadow.recovery, summoned) {
        (Some(recovery), _) => (
            format!(
                "Восстанавливается: {}/{} HP",
                recovery.health, recovery.maximum_health
            ),
            Color::DarkGray,
        ),
        (None, true) => ("Призвана".to_owned(), Color::Green),
        (None, false) => ("В тени".to_owned(), Color::Gray),
    };
    lore.push(TextComponent::plain(state.0).color(state.1));
    if with_hints {
        lore.push(
            TextComponent::plain(if summoned {
                "ЛКМ — отозвать, ПКМ — прокачка."
            } else {
                "ЛКМ — призвать, ПКМ — прокачка."
            })
            .color(Color::Yellow),
        );
    }
    with_lore(
        named(
            elite_icon(shadow),
            TextComponent::plain(shadow.name.clone()).color(Color::DarkRed),
        ),
        lore,
    )
}

fn elite_toggle(state: &SoloLevelingState, player: &Player, shadow: &EliteShadow) {
    let owner = player.gameprofile.id();
    if state.dismiss_elite_shadow(owner, shadow.id) {
        tell(player, &format!("«{}» отозвана.", shadow.name), Color::Gray);
        return;
    }
    match state.summon_elite_shadow(player, &shadow.name) {
        Ok(()) => tell(
            player,
            &format!("«{}» призвана.", shadow.name),
            Color::DarkRed,
        ),
        Err(error) => tell(player, error, Color::Red),
    }
}

struct EliteArmyMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    page: usize,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for EliteArmyMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/elite_shadow_army");
}

impl MenuKind for EliteArmyMenuKind {
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
        let owner = player.gameprofile.id();
        let local = slot - self.buttons.start();
        let view = EliteArmyView::read(&self.state, owner, self.page);
        match local {
            PREVIOUS_SLOT => self.page = view.page.saturating_sub(1),
            NEXT_SLOT => self.page = (view.page + 1).min(view.pages - 1),
            SUMMON_ALL_SLOT => match self.state.summon_all_elite_shadows(player) {
                Some(count) => tell(
                    player,
                    &format!("Призвано элитных теней: {count}."),
                    Color::DarkRed,
                ),
                None => tell(player, "Призвать элитных теней сейчас нельзя.", Color::Red),
            },
            ELITE_ARMY_SLOT => {
                open_army(player, Arc::clone(&self.state), 0);
                return ClickOutcome::Consume;
            }
            MODE_SLOT => {
                let mode = next_mode(view.mode);
                self.state.set_shadow_mode(owner, mode);
                tell(
                    player,
                    &format!("Режим армии: {}.", mode_name(mode)),
                    Color::DarkPurple,
                );
            }
            RECALL_ALL_SLOT => {
                let recalled = self.state.recall_elite_shadows(owner);
                tell(
                    player,
                    &format!("Отозвано элитных теней: {recalled}."),
                    Color::Gray,
                );
            }
            _ => {
                let Some(shadow) = view.shadow_at(local) else {
                    return ClickOutcome::Consume;
                };
                if button == MouseButton::Right {
                    open_elite_detail(player, Arc::clone(&self.state), shadow.id, view.page);
                    return ClickOutcome::Consume;
                }
                elite_toggle(&self.state, player, shadow);
            }
        }
        let items = EliteArmyView::read(&self.state, owner, self.page).items();
        refresh(guard, &self.container, items);
        ClickOutcome::Consume
    }
}

fn elite_detail_items(state: &SoloLevelingState, owner: Uuid, shadow_id: Uuid) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); DETAIL_SLOTS];
    let Some(shadow) = state
        .elite_shadows(owner)
        .and_then(|roster| roster.into_iter().find(|shadow| shadow.id == shadow_id))
    else {
        return items;
    };
    let summoned = state.active_elite_shadows(owner).contains(&shadow.id);
    items[DETAIL_SHADOW_SLOT] = elite_shadow_item(&shadow, summoned, false);
    for (slot, stat) in UPGRADES {
        let (icon, title, gain, have): (ItemRef, &str, String, u16) = match stat {
            ShadowStat::Health => (
                &vanilla_items::GOLDEN_APPLE,
                "Здоровье",
                format!("+{HEALTH_PER_POINT:.0} макс. здоровья"),
                shadow.health,
            ),
            ShadowStat::Attack => (
                &vanilla_items::IRON_SWORD,
                "Атака",
                format!("+{ATTACK_PER_POINT:.1} урона"),
                shadow.attack,
            ),
            ShadowStat::Speed => (
                &vanilla_items::FEATHER,
                "Скорость",
                format!("+{SPEED_PER_POINT:.2} скорости"),
                shadow.speed,
            ),
        };
        let action = if shadow.unspent_points > 0 {
            ("Клик — вложить очко.", Color::Yellow)
        } else {
            (
                "Нет свободных очков: они даются за уровни тени.",
                Color::Red,
            )
        };
        items[slot] = with_lore(
            named(
                ItemStack::new(icon),
                TextComponent::plain(format!("{title}: {have}")).color(Color::Aqua),
            ),
            vec![
                TextComponent::plain(gain).color(Color::Gray),
                TextComponent::plain(format!("Свободных очков: {}", shadow.unspent_points))
                    .color(Color::Gold),
                TextComponent::plain(action.0).color(action.1),
            ],
        );
    }
    items[BACK_SLOT] = button(&vanilla_items::ARROW, "Назад к элитным", Color::Yellow);
    items[TOGGLE_SLOT] = if summoned {
        button(&vanilla_items::BARRIER, "Отозвать", Color::Red)
    } else {
        button(
            &vanilla_items::WITHER_SKELETON_SKULL,
            "Призвать",
            Color::DarkRed,
        )
    };
    items
}

struct EliteDetailMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
    container: Shared<SimpleContainer>,
    shadow_id: Uuid,
    page: usize,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for EliteDetailMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/elite_shadow");
}

impl MenuKind for EliteDetailMenuKind {
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
        let owner = player.gameprofile.id();
        let Some(shadow) = self.state.elite_shadows(owner).and_then(|roster| {
            roster
                .into_iter()
                .find(|shadow| shadow.id == self.shadow_id)
        }) else {
            return ClickOutcome::Consume;
        };
        let local = slot - self.buttons.start();
        if local == BACK_SLOT {
            open_elite_army(player, Arc::clone(&self.state), self.page);
            return ClickOutcome::Consume;
        }
        if local == TOGGLE_SLOT {
            elite_toggle(&self.state, player, &shadow);
        } else if let Some((_, stat)) = UPGRADES.iter().find(|(at, _)| *at == local) {
            match self.state.upgrade_elite_shadow(owner, &shadow.name, *stat) {
                Ok(upgraded) => {
                    if self.state.save_session(owner).is_err() {
                        tell(player, "Не удалось сохранить элитную тень.", Color::Red);
                    } else {
                        tell(
                            player,
                            &format!(
                                "«{}» усилена. Осталось очков: {}.",
                                upgraded.name, upgraded.unspent_points
                            ),
                            Color::DarkRed,
                        );
                    }
                }
                Err(error) => tell(player, error, Color::Red),
            }
        }
        let items = elite_detail_items(&self.state, owner, self.shadow_id);
        refresh(guard, &self.container, items);
        ClickOutcome::Consume
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ARMY_SLOTS, BACK_SLOT, DETAIL_SHADOW_SLOT, DETAIL_SLOTS, INFO_SLOT, MODE_SLOT, NEXT_SLOT,
        PER_PAGE, PREVIOUS_SLOT, RECALL_ALL_SLOT, SUMMON_ALL_SLOT, TOGGLE_SLOT, UPGRADES,
    };

    #[test]
    fn army_buttons_sit_below_the_roster_and_detail_buttons_do_not_overlap() {
        for slot in [
            PREVIOUS_SLOT,
            MODE_SLOT,
            SUMMON_ALL_SLOT,
            INFO_SLOT,
            RECALL_ALL_SLOT,
            NEXT_SLOT,
        ] {
            assert!((PER_PAGE..ARMY_SLOTS).contains(&slot));
        }
        let mut detail = vec![DETAIL_SHADOW_SLOT, BACK_SLOT, TOGGLE_SLOT];
        detail.extend(UPGRADES.iter().map(|(slot, _)| *slot));
        assert!(detail.iter().all(|slot| *slot < DETAIL_SLOTS));
        let mut unique = detail.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), detail.len());
    }

    #[test]
    fn the_mode_button_cycles_through_every_mode() {
        use super::next_mode;
        use steel_core::entity::AlliedMode;
        let mut mode = AlliedMode::Follow;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(mode);
            mode = next_mode(mode);
        }
        assert_eq!(mode, AlliedMode::Follow);
        assert_eq!(
            seen,
            [AlliedMode::Follow, AlliedMode::Hold, AlliedMode::Passive]
        );
    }
}
