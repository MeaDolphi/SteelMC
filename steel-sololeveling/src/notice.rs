//! What a hunter is told about the rewards they just received.
//!
//! Rewards land in several places (account commits after a kill, loot handed
//! out after a dungeon), sometimes several in one tick. They are collected per
//! hunter and shown once on the hunter's next tick: experience and system
//! points on the action bar, level-ups and loot in chat.

use std::borrow::Cow;

use steel_core::player::Player;
use steel_registry::data_components::vanilla_components::CUSTOM_NAME;
use steel_registry::item_stack::ItemStack;
use text_components::format::Color;
use text_components::translation::TranslatedMessage;
use text_components::{Modifier as _, TextComponent};

use crate::rank::HunterRank;
use crate::sound::SystemSound;
use crate::stats::STAT_POINTS_PER_LEVEL;

/// Rewards collected for one hunter since they were last shown.
#[derive(Debug, Clone, Default)]
pub struct Notice {
    /// Hunter experience gained.
    pub experience: u64,
    /// System points gained.
    pub system_points: u64,
    /// Levels gained.
    pub levels: u32,
    /// Level reached, when `levels` is not zero.
    pub level: u32,
    /// Rank just promoted to, if these levels crossed a rank boundary.
    pub rank_up: Option<HunterRank>,
    /// These levels first crossed the Second Awakening threshold.
    pub awakening_hint: bool,
    /// Items received: identity used to merge repeats, shown name, count.
    pub loot: Vec<(String, TextComponent, i32)>,
}

impl Notice {
    /// Adds received items, merging repeats of the same name.
    pub fn add_loot(&mut self, stack: &ItemStack) {
        let (identity, name) = item_name(stack);
        if let Some((_, _, count)) = self
            .loot
            .iter_mut()
            .find(|(known, _, _)| *known == identity)
        {
            *count = count.saturating_add(stack.count);
        } else {
            self.loot.push((identity, name, stack.count));
        }
    }

    /// Returns the action-bar line, if any experience or points were gained.
    #[must_use]
    pub fn reward_line(&self) -> Option<String> {
        match (self.experience, self.system_points) {
            (0, 0) => None,
            (experience, 0) => Some(format!("+{experience} опыта")),
            (0, points) => Some(format!("+{points} очков Системы")),
            (experience, points) => Some(format!("+{experience} опыта · +{points} очков Системы")),
        }
    }

    /// Returns the chat line about received items, if there were any.
    #[must_use]
    pub fn loot_line(&self) -> Option<TextComponent> {
        if self.loot.is_empty() {
            return None;
        }
        let mut parts: Vec<TextComponent> = Vec::new();
        for (index, (_, name, count)) in self.loot.iter().enumerate() {
            if index > 0 {
                parts.push(TextComponent::plain(", "));
            }
            parts.push(name.clone());
            if *count > 1 {
                parts.push(TextComponent::plain(format!(" ×{count}")));
            }
        }
        parts.push(TextComponent::plain("."));
        Some(
            TextComponent::plain("Добыча: ")
                .color(Color::Green)
                .add_children(parts),
        )
    }

    /// Shows the collected rewards to `player`.
    pub fn show(&self, player: &Player) {
        if let Some(line) = self.reward_line() {
            player.send_action_bar(TextComponent::plain(line).color(Color::Gold));
        }
        if self.levels > 0 {
            SystemSound::LevelUp.play(player);
            player.send_message(
                &TextComponent::plain(format!(
                    "Уровень повышен до {}! Получено очков характеристик: {}.",
                    self.level,
                    self.levels.saturating_mul(STAT_POINTS_PER_LEVEL)
                ))
                .color(Color::Aqua),
            );
        }
        if let Some(rank) = self.rank_up {
            player.send_message(
                &TextComponent::plain(format!(
                    "Переоценка пройдена: ваш ранг охотника повышен до {}!",
                    rank.name()
                ))
                .color(Color::Gold)
                .bold(true),
            );
        }
        if self.awakening_hint {
            player.send_message(
                &TextComponent::plain(
                    "Система предлагает испытание: Второе пробуждение доступно (Система → Класс)."
                        .to_owned(),
                )
                .color(Color::LightPurple),
            );
        }
        if let Some(line) = self.loot_line() {
            if self.levels == 0 {
                SystemSound::Reward.play(player);
            }
            player.send_message(&line);
        }
    }
}

/// Returns a stable identity and the shown name of an item stack.
///
/// A vanilla item is named through its translation key, so every client sees
/// it in its own language; a themed item keeps its custom name.
#[must_use]
pub fn item_name(stack: &ItemStack) -> (String, TextComponent) {
    if let Some(name) = stack.get(CUSTOM_NAME) {
        return (format!("{name:?}"), name.clone());
    }
    let key = &stack.item.key;
    let translated = TranslatedMessage {
        key: Cow::Owned(format!("item.{}.{}", key.namespace, key.path)),
        fallback: Some(Cow::Owned(key.path.replace('_', " "))),
        args: None,
    };
    (key.to_string(), TextComponent::translated(translated))
}

#[cfg(test)]
mod tests {
    use super::Notice;

    #[test]
    fn reward_lines_skip_what_was_not_gained() {
        let mut notice = Notice::default();
        assert_eq!(notice.reward_line(), None);
        notice.experience = 20;
        assert_eq!(notice.reward_line().as_deref(), Some("+20 опыта"));
        notice.system_points = 4;
        assert_eq!(
            notice.reward_line().as_deref(),
            Some("+20 опыта · +4 очков Системы")
        );
    }

    #[test]
    fn an_empty_notice_has_no_loot_line() {
        assert!(Notice::default().loot_line().is_none());
    }
}
