//! The hunter sidebar: a purely presentational scoreboard sidebar showing a
//! player's level, currencies, stats, daily quest and taken contracts.
//!
//! Sent through [`steel_core::player::Player`]'s sidebar extension methods
//! (`show_sidebar`/`set_sidebar_line`/...), which talk to the client
//! directly and are not visible to vanilla `/scoreboard` commands.
//!
//! The client shows at most fifteen lines, so the stats share two lines and at
//! most [`CONTRACT_LINES`] contracts are listed, with a count of the rest.

use steel_core::player::Player;
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;

use crate::economy::{SystemPoints, Won};
use crate::organization::Contract;
use crate::quest::{REQUIRED_DISTANCE_BLOCKS, REQUIRED_JUMPS, REQUIRED_SNEAKS, SystemQuest};
use crate::rank_icon;
use crate::stats::HunterStats;

/// The sidebar's server-internal objective name.
pub const OBJECTIVE_NAME: &str = "sololeveling.hunter";
/// Most contract lines shown before the rest are summarised.
const CONTRACT_LINES: usize = 3;

/// The sidebar's title, shown at the top of the sidebar.
fn title() -> TextComponent {
    TextComponent::plain("Система").color(Color::Gold)
}

/// Whose contract a sidebar line shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractScope {
    /// The hunter's own contract.
    Personal,
    /// A contract of the hunter's party.
    Group,
    /// A contract of the hunter's guild.
    Guild,
}

/// Everything the sidebar shows, compared to skip unchanged refreshes.
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarView {
    /// Level, rank and stats.
    pub stats: HunterStats,
    /// Won balance.
    pub won: Won,
    /// System points balance.
    pub system_points: SystemPoints,
    /// Daily quest progress.
    pub quest: SystemQuest,
    /// Taken contracts, personal first.
    pub contracts: Vec<(ContractScope, Contract)>,
}

/// One sidebar line: a stable `owner` key paired with the text shown for it.
struct Line {
    owner: String,
    text: TextComponent,
}

fn line(owner: &str, text: impl Into<String>, color: Color) -> Line {
    Line {
        owner: owner.to_owned(),
        text: TextComponent::plain(text.into()).color(color),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the sidebar reads top to bottom, so its lines are listed in that order"
)]
fn lines(view: &SidebarView) -> Vec<Line> {
    let stats = &view.stats;
    let quest = &view.quest;
    let mut lines = vec![
        Line {
            owner: "level".to_owned(),
            text: TextComponent::plain(format!("Уровень {} · ранг ", stats.level))
                .color(Color::Aqua)
                .add_child(rank_icon::badge(stats.rank()))
                .add_child(TextComponent::plain(format!(" {}", stats.rank().name()))),
        },
        line("won", format!("Won: ₩{}", view.won.amount()), Color::Yellow),
        line(
            "system_points",
            format!("Очки Системы: {}", view.system_points.amount()),
            Color::LightPurple,
        ),
        line(
            "points",
            format!("Очки характеристик: {}", stats.unspent_points),
            Color::Yellow,
        ),
        line(
            "stats_body",
            format!(
                "СИЛ {} · ВЫН {} · ЛОВ {}",
                stats.strength, stats.vitality, stats.agility
            ),
            Color::White,
        ),
        line(
            "stats_mind",
            format!("ИНТ {} · ЧУВ {}", stats.intelligence, stats.sense),
            Color::White,
        ),
        line(
            "daily",
            if quest.completed {
                "Ежедневное: выполнено"
            } else {
                "Ежедневное задание:"
            },
            if quest.completed {
                Color::Green
            } else {
                Color::Gray
            },
        ),
        line(
            "quest_moves",
            format!(
                " Прыжки {}/{REQUIRED_JUMPS} · присед {}/{REQUIRED_SNEAKS}",
                quest.jumps, quest.sneaks
            ),
            Color::White,
        ),
        line(
            "quest_distance",
            format!(
                " Бег {:.0}/{REQUIRED_DISTANCE_BLOCKS:.0}",
                quest.distance_blocks
            ),
            Color::White,
        ),
        if quest.penalty_pending {
            line("penalty", "Штрафная зона: ожидает", Color::Red)
        } else {
            line("penalty", "Штрафная зона: нет", Color::DarkGray)
        },
    ];
    if !view.contracts.is_empty() {
        lines.push(line("contracts", "Задания:", Color::Gray));
        for (index, (scope, contract)) in view.contracts.iter().take(CONTRACT_LINES).enumerate() {
            let prefix = match scope {
                ContractScope::Personal => "",
                ContractScope::Group => "Гр ",
                ContractScope::Guild => "Гл ",
            };
            let (text, color) = if contract.is_complete() {
                (
                    format!(" {prefix}{} — сдать", contract.template.short_title()),
                    Color::Green,
                )
            } else {
                (
                    format!(
                        " {prefix}{} {}/{}",
                        contract.template.short_title(),
                        contract.progress,
                        contract.target()
                    ),
                    Color::White,
                )
            };
            lines.push(line(&format!("contract_{index}"), text, color));
        }
        let rest = view.contracts.len().saturating_sub(CONTRACT_LINES);
        if rest > 0 {
            lines.push(line(
                "contract_more",
                format!(" …и ещё {rest} (меню → Мои задания)"),
                Color::DarkGray,
            ));
        }
    }
    lines
}

/// Writes every line of `view`, with scores descending so vanilla's
/// score-ordered rendering keeps them in the intended top-to-bottom order.
fn write(player: &Player, view: &SidebarView) -> Vec<String> {
    let entries = lines(view);
    let count = entries.len();
    let mut owners = Vec::with_capacity(count);
    for (index, line) in entries.into_iter().enumerate() {
        let score = i32::try_from(count - index).unwrap_or(0);
        player.set_sidebar_line(OBJECTIVE_NAME, line.owner.clone(), score, line.text);
        owners.push(line.owner);
    }
    owners
}

/// Creates and shows the hunter sidebar for a player who just joined.
pub fn show(player: &Player, view: &SidebarView) {
    player.show_sidebar(OBJECTIVE_NAME, title());
    write(player, view);
}

/// Restores the sidebar of a player who already has it on their client.
///
/// A respawn creates a new [`Player`] for the same connection, and that client
/// still holds the objective: adding it again makes the client reject the packet
/// and drop the connection. Only the display slot binding and the line values
/// are re-sent here.
pub fn restore(player: &Player, view: &SidebarView) {
    player.rebind_sidebar(OBJECTIVE_NAME);
    write(player, view);
}

/// Rewrites the sidebar and removes lines `previous` had that `view` lacks,
/// such as a contract that was turned in.
pub fn refresh(player: &Player, previous: Option<&SidebarView>, view: &SidebarView) {
    let owners = write(player, view);
    if let Some(previous) = previous {
        for old in lines(previous) {
            if !owners.contains(&old.owner) {
                player.remove_sidebar_line(OBJECTIVE_NAME, old.owner);
            }
        }
    }
}

/// Removes the hunter sidebar, e.g. before a player disconnects.
pub fn hide(player: &Player) {
    player.hide_sidebar(OBJECTIVE_NAME);
}

#[cfg(test)]
mod tests {
    use super::{CONTRACT_LINES, ContractScope, SidebarView, lines};
    use crate::economy::{SystemPoints, Won};
    use crate::organization::{Contract, Objective, Template};
    use crate::quest::SystemQuest;
    use crate::rank::HunterRank;
    use crate::stats::HunterStats;

    fn view(contracts: usize) -> SidebarView {
        let contract = Contract::new(Template {
            objective: Objective::Mobs,
            rank: HunterRank::E,
        });
        SidebarView {
            stats: HunterStats::new(),
            won: Won::default(),
            system_points: SystemPoints::zero(),
            quest: SystemQuest::new(1),
            contracts: vec![(ContractScope::Personal, contract); contracts],
        }
    }

    #[test]
    fn the_sidebar_never_exceeds_fifteen_lines() {
        for contracts in 0..=7 {
            assert!(lines(&view(contracts)).len() <= 15, "{contracts} contracts");
        }
        assert_eq!(lines(&view(0)).len(), 10);
        assert_eq!(lines(&view(1)).len(), 12);
        assert_eq!(lines(&view(7)).len(), 10 + 1 + CONTRACT_LINES + 1);
    }

    #[test]
    fn owners_are_unique() {
        let owners: Vec<_> = lines(&view(7)).into_iter().map(|line| line.owner).collect();
        for (index, owner) in owners.iter().enumerate() {
            assert!(!owners[index + 1..].contains(owner), "{owner} repeats");
        }
    }
}
