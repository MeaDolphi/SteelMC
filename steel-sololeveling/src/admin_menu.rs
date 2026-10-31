//! Operator admin menu for editing live hunter state through ChestGUI.
//!
//! `/sladmin menu` opens the player list; choosing a hunter opens the editor.
//! Every mutation goes through [`SoloLevelingState`] and is queued for
//! persistence with [`SoloLevelingState::save_session`], so changes survive a
//! restart. The target hunter receives a chat notification after each edit.

use std::sync::Arc;

use steel_core::entity::LivingEntity;
use steel_core::inventory::prelude::*;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_items;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::abilities;
use crate::class::{self, ClassChoice, HunterClass};
use crate::economy::{SystemPoints, Won};
use crate::item::{named, with_lore};
use crate::rank::HunterRank;
use crate::state::SoloLevelingState;
use crate::stats::{HunterStat, HunterStats};
use crate::ui::{self, After, open_guarded_buttons};

const MENU_SLOTS: usize = 54;
const PROFILE_SLOT: usize = 4;
const CLASS_SLOT: usize = 9;
const LEVEL_PLUS_SLOT: usize = 10;
const LEVEL_MINUS_SLOT: usize = 11;
const RESET_XP_SLOT: usize = 12;
const RANK_SLOT: usize = 13;
const WON_PLUS_1K_SLOT: usize = 18;
const WON_PLUS_10K_SLOT: usize = 19;
const WON_MINUS_1K_SLOT: usize = 20;
const WON_MINUS_10K_SLOT: usize = 21;
const WON_INPUT_SLOT: usize = 22;
const POINTS_PLUS_1K_SLOT: usize = 27;
const POINTS_PLUS_10K_SLOT: usize = 28;
const POINTS_MINUS_1K_SLOT: usize = 29;
const POINTS_MINUS_10K_SLOT: usize = 30;
const POINTS_INPUT_SLOT: usize = 31;
const STAT_PLUS_START: usize = 36;
const STAT_MINUS_START: usize = 45;
const BACK_SLOT: usize = 52;
const CLOSE_SLOT: usize = 53;

const WON_PRESETS: [(usize, i64); 4] = [
    (WON_PLUS_1K_SLOT, 1_000),
    (WON_PLUS_10K_SLOT, 10_000),
    (WON_MINUS_1K_SLOT, -1_000),
    (WON_MINUS_10K_SLOT, -10_000),
];

const POINTS_PRESETS: [(usize, i64); 4] = [
    (POINTS_PLUS_1K_SLOT, 1_000),
    (POINTS_PLUS_10K_SLOT, 10_000),
    (POINTS_MINUS_1K_SLOT, -1_000),
    (POINTS_MINUS_10K_SLOT, -10_000),
];

const STATS: [HunterStat; 5] = [
    HunterStat::Strength,
    HunterStat::Vitality,
    HunterStat::Agility,
    HunterStat::Intelligence,
    HunterStat::Sense,
];

const RANKS: [HunterRank; 6] = [
    HunterRank::E,
    HunterRank::D,
    HunterRank::C,
    HunterRank::B,
    HunterRank::A,
    HunterRank::S,
];

/// Opens the admin player list for the operator.
pub fn open(admin: &Player, state: Arc<SoloLevelingState>) {
    let mut candidates: Vec<(Uuid, String, HunterRank, Option<HunterClass>)> = state
        .online_players()
        .into_iter()
        .map(|player| {
            let uuid = player.gameprofile.id();
            let name = player.gameprofile.name().to_owned();
            let (rank, class) = state
                .with_session_mut(uuid, |session| (session.stats.rank(), session.class.class))
                .unwrap_or((HunterRank::E, None));
            (uuid, name, rank, class)
        })
        .collect();
    candidates.sort_by_key(|candidate| candidate.1.to_lowercase());
    candidates.truncate(45);

    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    for (index, (uuid, name, rank, class)) in candidates.iter().enumerate() {
        items[index] = player_head_item(name, *rank, *class, *uuid);
    }
    if candidates.is_empty() {
        items[22] = ui::button(
            &vanilla_items::BARRIER,
            "Нет игроков онлайн",
            Color::Gray,
            &[],
        );
    }
    items[CLOSE_SLOT] = ui::button(&vanilla_items::BARRIER, "Закрыть", Color::Red, &[]);

    let candidates_for_handler = candidates.clone();
    open_guarded_buttons(
        admin,
        TextComponent::plain("Админ: выбор игрока").color(Color::DarkRed),
        6,
        items,
        Box::new(move |admin, slot, _| {
            if slot == CLOSE_SLOT {
                admin.close_container();
                return After::Replaced;
            }
            if let Some((uuid, name, _, _)) = candidates_for_handler.get(slot) {
                open_editor(admin, Arc::clone(&state), *uuid, name.clone());
                return After::Replaced;
            }
            After::Keep
        }),
    );
}

fn player_head_item(
    name: &str,
    rank: HunterRank,
    class: Option<HunterClass>,
    uuid: Uuid,
) -> ItemStack {
    let class_line = class.map_or_else(
        || "Класс: не выбран".to_owned(),
        |class| format!("Класс: {}", class.display_name()),
    );
    with_lore(
        named(
            ItemStack::new(&vanilla_items::PLAYER_HEAD),
            TextComponent::plain(name.to_owned()).color(Color::Aqua),
        ),
        vec![
            TextComponent::plain(format!("UUID: {uuid}")).color(Color::DarkGray),
            TextComponent::plain(format!("Ранг: {}", rank.name())).color(Color::White),
            TextComponent::plain(class_line).color(Color::White),
            TextComponent::plain("Клик — редактировать.").color(Color::Yellow),
        ],
    )
}

fn open_editor(admin: &Player, state: Arc<SoloLevelingState>, target: Uuid, target_name: String) {
    let Some(items) = editor_items(&state, target) else {
        admin.close_container();
        return;
    };
    open_guarded_buttons(
        admin,
        TextComponent::plain(format!("Админ: {target_name}")).color(Color::DarkRed),
        6,
        items,
        Box::new(move |admin, slot, _| {
            handle_editor_click(admin, Arc::clone(&state), target, target_name.clone(), slot)
        }),
    );
}

fn handle_editor_click(
    admin: &Player,
    state: Arc<SoloLevelingState>,
    target: Uuid,
    target_name: String,
    slot: usize,
) -> After {
    match slot {
        CLASS_SLOT => {
            open_class_submenu(admin, Arc::clone(&state), target, target_name);
            return After::Replaced;
        }
        LEVEL_PLUS_SLOT => {
            apply_change(admin, &state, target, "уровень +1", |state, uuid| {
                state.admin_adjust_level(uuid, 1)
            });
        }
        LEVEL_MINUS_SLOT => {
            apply_change(admin, &state, target, "уровень -1", |state, uuid| {
                state.admin_adjust_level(uuid, -1)
            });
        }
        RESET_XP_SLOT => {
            apply_change(
                admin,
                &state,
                target,
                "сброс опыта",
                |state, uuid| state.admin_reset_experience(uuid),
            );
        }
        RANK_SLOT => {
            open_rank_submenu(admin, Arc::clone(&state), target, target_name);
            return After::Replaced;
        }
        WON_INPUT_SLOT => {
            ask_won_amount(admin, Arc::clone(&state), target, target_name);
            return After::Replaced;
        }
        POINTS_INPUT_SLOT => {
            ask_points_amount(admin, Arc::clone(&state), target, target_name);
            return After::Replaced;
        }
        BACK_SLOT => {
            open(admin, Arc::clone(&state));
            return After::Replaced;
        }
        CLOSE_SLOT => {
            admin.close_container();
            return After::Replaced;
        }
        _ => {}
    }

    if let Some((_, delta)) = WON_PRESETS.iter().find(|(at, _)| *at == slot) {
        apply_change(
            admin,
            &state,
            target,
            &format!("Won {delta:+}"),
            |state, uuid| state.admin_adjust_won(uuid, *delta),
        );
    } else if let Some((_, delta)) = POINTS_PRESETS.iter().find(|(at, _)| *at == slot) {
        apply_change(
            admin,
            &state,
            target,
            &format!("очки Системы {delta:+}"),
            |state, uuid| state.admin_adjust_system_points(uuid, *delta),
        );
    } else if let Some(stat) = stat_at(slot, STAT_PLUS_START) {
        let name = stat_name(stat);
        apply_change(
            admin,
            &state,
            target,
            &format!("{name} +1"),
            |state, uuid| state.admin_adjust_stat(uuid, stat, 1),
        );
    } else if let Some(stat) = stat_at(slot, STAT_MINUS_START) {
        let name = stat_name(stat);
        apply_change(
            admin,
            &state,
            target,
            &format!("{name} -1"),
            |state, uuid| state.admin_adjust_stat(uuid, stat, -1),
        );
    }

    match editor_items(&state, target) {
        Some(items) => After::Redraw(items),
        None => {
            admin.close_container();
            After::Replaced
        }
    }
}

fn stat_at(slot: usize, base: usize) -> Option<HunterStat> {
    let index = slot.checked_sub(base)?;
    STATS.get(index).copied()
}

fn apply_change<T>(
    admin: &Player,
    state: &SoloLevelingState,
    target: Uuid,
    change_name: &str,
    mut operation: impl FnMut(&SoloLevelingState, Uuid) -> Option<T>,
) {
    if operation(state, target).is_some() {
        refresh_target_attributes(state, target);
        notify_target_change(state, target, admin, change_name);
    }
}

fn refresh_target_attributes(state: &SoloLevelingState, target: Uuid) {
    let Some(player) = find_target_player(state, target) else {
        return;
    };
    state.with_session_mut(target, |session| {
        session.stats.apply_to(&mut player.attributes().lock());
        session
            .class
            .apply_to(&mut player.attributes().lock(), session.stats.rank());
    });
}

fn notify_target_change(state: &SoloLevelingState, target: Uuid, admin: &Player, change: &str) {
    let admin_name = admin.gameprofile.name();
    if let Some(player) = find_target_player(state, target) {
        player.send_message(
            &TextComponent::plain(format!("Оператор {admin_name} изменил вам: {change}."))
                .color(Color::Gold),
        );
    }
}

fn find_target_player(state: &SoloLevelingState, target: Uuid) -> Option<Arc<Player>> {
    state
        .online_players()
        .into_iter()
        .find(|player| player.gameprofile.id() == target)
}

fn editor_items(state: &SoloLevelingState, target: Uuid) -> Option<Vec<ItemStack>> {
    let (stats, won, points, class) = state.with_session_mut(target, |session| {
        (
            session.stats,
            session.won,
            session.system_points,
            session.class.class,
        )
    })?;

    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    items[PROFILE_SLOT] = profile_item(target, stats, won, points, class);
    items[CLASS_SLOT] = action_item(
        &vanilla_items::IRON_SWORD,
        "Класс",
        &class.map_or_else(|| "не выбран".to_owned(), |c| c.display_name().to_owned()),
        "Клик — сменить класс.",
    );
    items[LEVEL_PLUS_SLOT] = ui::button(
        &vanilla_items::LIME_STAINED_GLASS_PANE,
        "Уровень +1",
        Color::Green,
        &["Клик — повысить уровень."],
    );
    items[LEVEL_MINUS_SLOT] = ui::button(
        &vanilla_items::RED_STAINED_GLASS_PANE,
        "Уровень -1",
        Color::Red,
        &["Клик — понизить уровень."],
    );
    items[RESET_XP_SLOT] = action_item(
        &vanilla_items::EXPERIENCE_BOTTLE,
        "Сброс опыта",
        &format!("Текущий опыт: {}", stats.experience),
        "Клик — обнулить опыт.",
    );
    items[RANK_SLOT] = action_item(
        &vanilla_items::NETHER_STAR,
        "Ранг",
        stats.rank().name(),
        "Клик — выбрать ранг E–S.",
    );

    for (slot, delta) in WON_PRESETS {
        let (icon, color) = if delta > 0 {
            (&vanilla_items::LIME_STAINED_GLASS_PANE, Color::Green)
        } else {
            (&vanilla_items::RED_STAINED_GLASS_PANE, Color::Red)
        };
        items[slot] = ui::button(icon, format!("Won {delta:+}"), color, &[]);
    }
    items[WON_INPUT_SLOT] = action_item(
        &vanilla_items::GOLD_INGOT,
        "Won — ввод",
        "",
        "Клик — ввести точную сумму.",
    );

    for (slot, delta) in POINTS_PRESETS {
        let (icon, color) = if delta > 0 {
            (&vanilla_items::LIME_STAINED_GLASS_PANE, Color::Green)
        } else {
            (&vanilla_items::RED_STAINED_GLASS_PANE, Color::Red)
        };
        items[slot] = ui::button(icon, format!("Очки Системы {delta:+}"), color, &[]);
    }
    items[POINTS_INPUT_SLOT] = action_item(
        &vanilla_items::END_CRYSTAL,
        "Очки — ввод",
        "",
        "Клик — ввести точную сумму.",
    );

    for (index, stat) in STATS.iter().enumerate() {
        let value = stat_value(&stats, *stat);
        let name = stat_name(*stat);
        items[STAT_PLUS_START + index] = ui::button(
            &vanilla_items::LIME_STAINED_GLASS_PANE,
            format!("{name} +1"),
            Color::Green,
            &[&format!("Текущее: {value}")],
        );
        items[STAT_MINUS_START + index] = ui::button(
            &vanilla_items::RED_STAINED_GLASS_PANE,
            format!("{name} -1"),
            Color::Red,
            &[&format!("Текущее: {value}")],
        );
    }

    items[BACK_SLOT] = ui::styled(
        ui::button(&vanilla_items::ARROW, "Назад", Color::Yellow, &[]),
        "ui/previous",
    );
    items[CLOSE_SLOT] = ui::button(&vanilla_items::BARRIER, "Закрыть", Color::Red, &[]);

    Some(items)
}

fn profile_item(
    target: Uuid,
    stats: HunterStats,
    won: Won,
    points: SystemPoints,
    class: Option<HunterClass>,
) -> ItemStack {
    let class_line = class.map_or_else(
        || "Класс: не выбран".to_owned(),
        |class| format!("Класс: {}", class.display_name()),
    );
    with_lore(
        named(
            ItemStack::new(&vanilla_items::PLAYER_HEAD),
            TextComponent::plain("Редактор охотника").color(Color::Gold),
        ),
        vec![
            TextComponent::plain(format!("UUID: {target}")).color(Color::Gray),
            TextComponent::plain(format!(
                "Уровень: {} · Опыт: {}",
                stats.level, stats.experience
            ))
            .color(Color::White),
            TextComponent::plain(format!("Ранг: {}", stats.rank().name())).color(Color::White),
            TextComponent::plain(class_line).color(Color::White),
            TextComponent::plain(format!("Won: {}", won.amount())).color(Color::White),
            TextComponent::plain(format!("Очки Системы: {}", points.amount())).color(Color::White),
            TextComponent::plain(format!("Очки характеристик: {}", stats.unspent_points))
                .color(Color::White),
            TextComponent::plain(format!(
                "Сила {} · Живучесть {} · Ловкость {} · Интеллект {} · Чутьё {}",
                stats.strength, stats.vitality, stats.agility, stats.intelligence, stats.sense
            ))
            .color(Color::Gray),
        ],
    )
}

fn action_item(icon: ItemRef, title: &str, subtitle: &str, hint: &str) -> ItemStack {
    let mut lore = Vec::new();
    if !subtitle.is_empty() {
        lore.push(TextComponent::plain(subtitle.to_owned()).color(Color::White));
    }
    lore.push(TextComponent::plain(hint.to_owned()).color(Color::Yellow));
    with_lore(
        named(
            ItemStack::new(icon),
            TextComponent::plain(title.to_owned()).color(Color::Aqua),
        ),
        lore,
    )
}

fn stat_value(stats: &HunterStats, stat: HunterStat) -> u32 {
    match stat {
        HunterStat::Strength => stats.strength,
        HunterStat::Vitality => stats.vitality,
        HunterStat::Agility => stats.agility,
        HunterStat::Intelligence => stats.intelligence,
        HunterStat::Sense => stats.sense,
    }
}

fn stat_name(stat: HunterStat) -> &'static str {
    match stat {
        HunterStat::Strength => "Сила",
        HunterStat::Vitality => "Живучесть",
        HunterStat::Agility => "Ловкость",
        HunterStat::Intelligence => "Интеллект",
        HunterStat::Sense => "Чутьё",
    }
}

fn class_icon(class: HunterClass) -> ItemRef {
    match class {
        HunterClass::Fighter => &vanilla_items::IRON_SWORD,
        HunterClass::Tanker => &vanilla_items::SHIELD,
        HunterClass::Assassin => &vanilla_items::IRON_HOE,
        HunterClass::Ranger => &vanilla_items::BOW,
        HunterClass::Mage => &vanilla_items::BLAZE_ROD,
    }
}

fn open_class_submenu(
    admin: &Player,
    state: Arc<SoloLevelingState>,
    target: Uuid,
    target_name: String,
) {
    let mut items = vec![ItemStack::empty(); 27];
    items[4] = ui::button(
        &vanilla_items::BOOK,
        "Выбор класса",
        Color::Gold,
        &["Клик — назначить класс."],
    );
    for (index, class) in class::ALL.iter().enumerate() {
        items[10 + index] = ui::button(
            class_icon(*class),
            class.display_name().to_owned(),
            Color::Aqua,
            &["Клик — назначить этот класс."],
        );
    }
    items[18] = ui::styled(
        ui::button(&vanilla_items::ARROW, "Назад", Color::Yellow, &[]),
        "ui/previous",
    );

    open_guarded_buttons(
        admin,
        TextComponent::plain("Админ: класс").color(Color::DarkAqua),
        3,
        items,
        Box::new(move |admin, slot, _| {
            if slot == 18 {
                open_editor(admin, Arc::clone(&state), target, target_name.clone());
                return After::Replaced;
            }
            if let Some(class) = class::ALL.get(slot.checked_sub(10).unwrap_or(usize::MAX)) {
                apply_change(
                    admin,
                    &state,
                    target,
                    &format!("класс {}", class.display_name()),
                    |state, uuid| state.admin_set_class(uuid, *class),
                );
                open_editor(admin, Arc::clone(&state), target, target_name.clone());
            }
            After::Keep
        }),
    );
}

fn open_rank_submenu(
    admin: &Player,
    state: Arc<SoloLevelingState>,
    target: Uuid,
    target_name: String,
) {
    let mut items = vec![ItemStack::empty(); 27];
    items[4] = ui::button(
        &vanilla_items::NETHER_STAR,
        "Выбор ранга",
        Color::Gold,
        &["Клик — установить ранг."],
    );
    for (index, rank) in RANKS.iter().enumerate() {
        items[10 + index] = ui::button(
            &vanilla_items::EXPERIENCE_BOTTLE,
            format!("Ранг {}", rank.name()),
            Color::Aqua,
            &[&format!("Установит уровень {}", rank.min_level())],
        );
    }
    items[18] = ui::styled(
        ui::button(&vanilla_items::ARROW, "Назад", Color::Yellow, &[]),
        "ui/previous",
    );

    open_guarded_buttons(
        admin,
        TextComponent::plain("Админ: ранг").color(Color::DarkAqua),
        3,
        items,
        Box::new(move |admin, slot, _| {
            if slot == 18 {
                open_editor(admin, Arc::clone(&state), target, target_name.clone());
                return After::Replaced;
            }
            if let Some(rank) = RANKS.get(slot.checked_sub(10).unwrap_or(usize::MAX)) {
                apply_change(
                    admin,
                    &state,
                    target,
                    &format!("ранг {}", rank.name()),
                    |state, uuid| state.admin_set_rank(uuid, *rank),
                );
                open_editor(admin, Arc::clone(&state), target, target_name.clone());
                return After::Replaced;
            }
            After::Keep
        }),
    );
}

fn ask_won_amount(
    admin: &Player,
    state: Arc<SoloLevelingState>,
    target: Uuid,
    target_name: String,
) {
    let state_for_submit = Arc::clone(&state);
    ui::ask_text(
        admin,
        "Сумма Won",
        "0",
        Arc::new(move |admin, text| {
            let Ok(delta) = text.trim().parse::<i64>() else {
                admin.send_message(
                    &TextComponent::plain("Сумма должна быть целым числом.").color(Color::Red),
                );
                open_editor(
                    admin,
                    Arc::clone(&state_for_submit),
                    target,
                    target_name.clone(),
                );
                return;
            };
            apply_change(
                admin,
                &state_for_submit,
                target,
                &format!("Won {delta:+}"),
                |state, uuid| state.admin_adjust_won(uuid, delta),
            );
            open_editor(
                admin,
                Arc::clone(&state_for_submit),
                target,
                target_name.clone(),
            );
        }),
    );
}

fn ask_points_amount(
    admin: &Player,
    state: Arc<SoloLevelingState>,
    target: Uuid,
    target_name: String,
) {
    let state_for_submit = Arc::clone(&state);
    ui::ask_text(
        admin,
        "Сумма очков",
        "0",
        Arc::new(move |admin, text| {
            let Ok(delta) = text.trim().parse::<i64>() else {
                admin.send_message(
                    &TextComponent::plain("Сумма должна быть целым числом.").color(Color::Red),
                );
                open_editor(
                    admin,
                    Arc::clone(&state_for_submit),
                    target,
                    target_name.clone(),
                );
                return;
            };
            apply_change(
                admin,
                &state_for_submit,
                target,
                &format!("очки Системы {delta:+}"),
                |state, uuid| state.admin_adjust_system_points(uuid, delta),
            );
            open_editor(
                admin,
                Arc::clone(&state_for_submit),
                target,
                target_name.clone(),
            );
        }),
    );
}

impl SoloLevelingState {
    /// Admin: assigns a hunter class directly, bypassing cooldown and cost.
    pub fn admin_set_class(&self, uuid: Uuid, class: HunterClass) -> Option<ClassChoice> {
        let choice = self.with_session_mut(uuid, |session| {
            let now = abilities::now_millis() / 1_000;
            session.class.class = Some(class);
            session.class.changed_at = now;
            session.class.free_change = false;
            session.class_rank = Some(session.stats.rank());
            session.class
        })?;
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: admin class change failed to queue save for {uuid}: {error}");
        }
        Some(choice)
    }

    /// Admin: adjusts a hunter level by `delta`, never below 1.
    pub fn admin_adjust_level(&self, uuid: Uuid, delta: i32) -> Option<HunterStats> {
        let stats = self.with_session_mut(uuid, |session| {
            let new_level = if delta < 0 {
                session
                    .stats
                    .level
                    .saturating_sub(delta.unsigned_abs())
                    .max(1)
            } else {
                session.stats.level.saturating_add(delta as u32)
            };
            session.stats.level = new_level;
            session.stats
        })?;
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: admin level change failed to queue save for {uuid}: {error}");
        }
        Some(stats)
    }

    /// Admin: resets accumulated experience to zero.
    pub fn admin_reset_experience(&self, uuid: Uuid) -> Option<HunterStats> {
        let stats = self.with_session_mut(uuid, |session| {
            session.stats.experience = 0;
            session.stats
        })?;
        if let Err(error) = self.save_session(uuid) {
            log::warn!(
                "sololeveling: admin experience reset failed to queue save for {uuid}: {error}"
            );
        }
        Some(stats)
    }

    /// Admin: sets a hunter's rank by moving level to the rank minimum.
    pub fn admin_set_rank(&self, uuid: Uuid, rank: HunterRank) -> Option<HunterStats> {
        let stats = self.with_session_mut(uuid, |session| {
            session.stats.level = rank.min_level();
            session.stats.experience = 0;
            session.stats
        })?;
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: admin rank change failed to queue save for {uuid}: {error}");
        }
        Some(stats)
    }

    /// Admin: adjusts Won balance by `delta`, clamping debit to the current balance.
    pub fn admin_adjust_won(&self, uuid: Uuid, delta: i64) -> Option<Won> {
        let won = self.with_session_mut(uuid, |session| {
            if delta >= 0 {
                session.won.credit(delta as u64);
            } else {
                let amount = delta.unsigned_abs().min(session.won.amount());
                let _ = session.won.debit(amount);
            }
            session.won
        })?;
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: admin Won change failed to queue save for {uuid}: {error}");
        }
        Some(won)
    }

    /// Admin: adjusts system points by `delta`, clamping debit to the current balance.
    pub fn admin_adjust_system_points(&self, uuid: Uuid, delta: i64) -> Option<SystemPoints> {
        let points = self.with_session_mut(uuid, |session| {
            if delta >= 0 {
                let _ = session.system_points.credit(delta as u64);
            } else {
                let amount = delta.unsigned_abs().min(session.system_points.amount());
                let _ = session.system_points.debit(amount);
            }
            session.system_points
        })?;
        if let Err(error) = self.save_session(uuid) {
            log::warn!(
                "sololeveling: admin system points change failed to queue save for {uuid}: {error}"
            );
        }
        Some(points)
    }

    /// Admin: adjusts one hunter stat by `delta`, clamping to zero.
    pub fn admin_adjust_stat(
        &self,
        uuid: Uuid,
        stat: HunterStat,
        delta: i32,
    ) -> Option<HunterStats> {
        let stats = self.with_session_mut(uuid, |session| {
            let value = stat_value_mut(&mut session.stats, stat);
            if delta < 0 {
                *value = value.saturating_sub(delta.unsigned_abs());
            } else {
                *value = value.saturating_add(delta as u32);
            }
            session.stats
        })?;
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: admin stat change failed to queue save for {uuid}: {error}");
        }
        Some(stats)
    }
}

fn stat_value_mut(stats: &mut HunterStats, stat: HunterStat) -> &mut u32 {
    match stat {
        HunterStat::Strength => &mut stats.strength,
        HunterStat::Vitality => &mut stats.vitality,
        HunterStat::Agility => &mut stats.agility,
        HunterStat::Intelligence => &mut stats.intelligence,
        HunterStat::Sense => &mut stats.sense,
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use uuid::Uuid;

    use super::{
        BACK_SLOT, CLASS_SLOT, CLOSE_SLOT, LEVEL_MINUS_SLOT, LEVEL_PLUS_SLOT, MENU_SLOTS,
        POINTS_INPUT_SLOT, POINTS_PRESETS, PROFILE_SLOT, RANK_SLOT, RESET_XP_SLOT,
        STAT_MINUS_START, STAT_PLUS_START, STATS, WON_INPUT_SLOT, WON_PRESETS,
    };
    use crate::abilities::AbilityCooldowns;
    use crate::castle::CastleProgress;
    use crate::class::{ClassChoice, HunterClass};
    use crate::economy::{SystemPoints, Won};
    use crate::gear::SavedGear;
    use crate::merchant::MerchantRecord;
    use crate::organization::OrganizationBook;
    use crate::player_data::SoloLevelingPlayerData;
    use crate::quest::SystemQuest;
    use crate::quest_book::QuestBook;
    use crate::rank::HunterRank;
    use crate::shadow::{EliteShadowRoster, ShadowRoster};
    use crate::shop::ShopDay;
    use crate::spell::SpellBook;
    use crate::starter::StarterKit;
    use crate::state::SoloLevelingState;
    use crate::stats::{HunterStat, HunterStats};

    fn test_session() -> (SoloLevelingState, Uuid, std::path::PathBuf) {
        let dir = env::temp_dir().join(format!("sololeveling-admin-test-{}", Uuid::new_v4()));
        let state = SoloLevelingState::new(dir.clone()).expect("state should initialize");
        let uuid = Uuid::new_v4();
        state.begin_session(
            uuid,
            SoloLevelingPlayerData {
                uuid,
                stats: HunterStats::new(),
                won: Won::zero(),
                system_points: SystemPoints::zero(),
                quest: SystemQuest::default(),
                shadows: ShadowRoster::default(),
                elite_shadows: EliteShadowRoster::default(),
                shop: ShopDay::unset(),
                organization: OrganizationBook::new(),
                cooldowns: AbilityCooldowns::default(),
                class: ClassChoice::default(),
                gear: SavedGear::default(),
                deferred_inventory: Vec::new(),
                starter: StarterKit::default(),
                merchant: MerchantRecord::default(),
                castle: CastleProgress::default(),
                spells: SpellBook::default(),
                quests: QuestBook::default(),
            },
        );
        (state, uuid, dir)
    }

    #[test]
    fn editor_button_slots_are_unique_and_inside_the_menu() {
        let mut slots = vec![
            PROFILE_SLOT,
            CLASS_SLOT,
            LEVEL_PLUS_SLOT,
            LEVEL_MINUS_SLOT,
            RESET_XP_SLOT,
            RANK_SLOT,
            WON_INPUT_SLOT,
            POINTS_INPUT_SLOT,
            BACK_SLOT,
            CLOSE_SLOT,
        ];
        for (slot, _) in WON_PRESETS {
            slots.push(slot);
        }
        for (slot, _) in POINTS_PRESETS {
            slots.push(slot);
        }
        for index in 0..STATS.len() {
            slots.push(STAT_PLUS_START + index);
            slots.push(STAT_MINUS_START + index);
        }
        let count = slots.len();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), count, "editor slots overlap");
        assert!(
            slots.iter().all(|slot| *slot < MENU_SLOTS),
            "slot out of menu"
        );
    }

    #[test]
    fn admin_level_and_rank_round_trip_through_session() {
        let (state, uuid, dir) = test_session();
        state.admin_adjust_level(uuid, 5).expect("level up");
        assert_eq!(state.with_session_mut(uuid, |s| s.stats.level), Some(6));

        state.admin_set_rank(uuid, HunterRank::D).expect("rank set");
        assert_eq!(state.with_session_mut(uuid, |s| s.stats.level), Some(10));
        assert_eq!(
            state.with_session_mut(uuid, |s| s.stats.experience),
            Some(0)
        );
        assert_eq!(
            state.with_session_mut(uuid, |s| s.stats.rank()),
            Some(HunterRank::D)
        );

        state.admin_adjust_level(uuid, -100).expect("level clamps");
        assert_eq!(state.with_session_mut(uuid, |s| s.stats.level), Some(1));

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn admin_won_and_system_points_round_trip_and_clamp() {
        let (state, uuid, dir) = test_session();
        state.admin_adjust_won(uuid, 5_000).expect("credit");
        assert_eq!(
            state.with_session_mut(uuid, |s| s.won.amount()),
            Some(5_000)
        );

        state.admin_adjust_won(uuid, -10_000).expect("debit clamps");
        assert_eq!(state.with_session_mut(uuid, |s| s.won.amount()), Some(0));

        state
            .admin_adjust_system_points(uuid, 2_000)
            .expect("credit points");
        assert_eq!(
            state.with_session_mut(uuid, |s| s.system_points.amount()),
            Some(2_000)
        );

        state
            .admin_adjust_system_points(uuid, -500)
            .expect("debit points");
        assert_eq!(
            state.with_session_mut(uuid, |s| s.system_points.amount()),
            Some(1_500)
        );

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn admin_class_and_stat_round_trip() {
        let (state, uuid, dir) = test_session();
        state
            .admin_set_class(uuid, HunterClass::Mage)
            .expect("set class");
        assert_eq!(
            state.with_session_mut(uuid, |s| s.class.class),
            Some(Some(HunterClass::Mage))
        );

        state
            .admin_adjust_stat(uuid, HunterStat::Strength, 3)
            .expect("add strength");
        assert_eq!(state.with_session_mut(uuid, |s| s.stats.strength), Some(3));

        state
            .admin_adjust_stat(uuid, HunterStat::Strength, -10)
            .expect("subtract strength clamps");
        assert_eq!(state.with_session_mut(uuid, |s| s.stats.strength), Some(0));

        state
            .admin_adjust_stat(uuid, HunterStat::Agility, 1)
            .expect("add agility");
        assert_eq!(state.with_session_mut(uuid, |s| s.stats.agility), Some(1));

        state.admin_reset_experience(uuid).expect("reset xp");
        assert_eq!(
            state.with_session_mut(uuid, |s| s.stats.experience),
            Some(0)
        );

        fs::remove_dir_all(dir).expect("cleanup");
    }
}
