//! The hunter sidebar: a purely presentational scoreboard sidebar showing a
//! player's current hunter level, unspent stat points, and allocated stats.
//!
//! Sent through [`steel_core::player::Player`]'s sidebar extension methods
//! (`show_sidebar`/`set_sidebar_line`/...), which talk to the client
//! directly and are not visible to vanilla `/scoreboard` commands.

use steel_core::player::Player;
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;

use crate::economy::{SystemPoints, Won};
use crate::quest::{REQUIRED_DISTANCE_BLOCKS, REQUIRED_JUMPS, REQUIRED_SNEAKS, SystemQuest};
use crate::stats::HunterStats;

/// The sidebar's server-internal objective name.
pub const OBJECTIVE_NAME: &str = "sololeveling.hunter";

/// The sidebar's title, shown at the top of the sidebar.
fn title() -> TextComponent {
    TextComponent::plain("Solo Leveling").color(Color::Gold)
}

/// One sidebar line: a stable `owner` key paired with the text shown for it.
struct Line {
    owner: &'static str,
    text: TextComponent,
}

fn lines(
    stats: &HunterStats,
    won: Won,
    system_points: SystemPoints,
    quest: &SystemQuest,
) -> [Line; 14] {
    [
        Line {
            owner: "level",
            text: TextComponent::plain(format!("Level: {}", stats.level)).color(Color::Aqua),
        },
        Line {
            owner: "rank",
            text: TextComponent::plain(format!("Rank: {}", stats.rank().name())).color(Color::Gold),
        },
        Line {
            owner: "won",
            text: TextComponent::plain(format!("Won: ₩{}", won.amount())).color(Color::Yellow),
        },
        Line {
            owner: "system_points",
            text: TextComponent::plain(format!("System: {}", system_points.amount()))
                .color(Color::LightPurple),
        },
        Line {
            owner: "points",
            text: TextComponent::plain(format!("Points: {}", stats.unspent_points))
                .color(Color::Yellow),
        },
        Line {
            owner: "strength",
            text: TextComponent::plain(format!("STR: {}", stats.strength)),
        },
        Line {
            owner: "vitality",
            text: TextComponent::plain(format!("VIT: {}", stats.vitality)),
        },
        Line {
            owner: "agility",
            text: TextComponent::plain(format!("AGI: {}", stats.agility)),
        },
        Line {
            owner: "intelligence",
            text: TextComponent::plain(format!("INT: {}", stats.intelligence)),
        },
        Line {
            owner: "sense",
            text: TextComponent::plain(format!("SEN: {}", stats.sense)),
        },
        Line {
            owner: "quest_jumps",
            text: TextComponent::plain(format!("Jumps: {}/{}", quest.jumps, REQUIRED_JUMPS)),
        },
        Line {
            owner: "quest_sneaks",
            text: TextComponent::plain(format!("Sneaks: {}/{}", quest.sneaks, REQUIRED_SNEAKS)),
        },
        Line {
            owner: "quest_distance",
            text: TextComponent::plain(format!(
                "Run: {:.0}/{REQUIRED_DISTANCE_BLOCKS:.0}",
                quest.distance_blocks
            )),
        },
        Line {
            owner: "penalty",
            text: TextComponent::plain(if quest.penalty_pending {
                "Penalty zone: pending"
            } else {
                "Penalty zone: clear"
            }),
        },
    ]
}

/// Creates and shows the hunter sidebar for a player who just joined,
/// populating it with their current stats.
///
/// Line scores are set in descending order so vanilla's score-ordered
/// sidebar rendering keeps them in the intended top-to-bottom order.
pub fn show(
    player: &Player,
    stats: &HunterStats,
    won: Won,
    system_points: SystemPoints,
    quest: &SystemQuest,
) {
    player.show_sidebar(OBJECTIVE_NAME, title());
    let entries = lines(stats, won, system_points, quest);
    let count = entries.len();
    for (index, line) in entries.into_iter().enumerate() {
        let score = i32::try_from(count - index).unwrap_or(0);
        player.set_sidebar_line(OBJECTIVE_NAME, line.owner, score, line.text);
    }
}

/// Restores the sidebar of a player who already has it on their client.
///
/// A respawn creates a new [`Player`] for the same connection, and that client
/// still holds the objective: adding it again makes the client reject the packet
/// and drop the connection. Only the display slot binding and the line values
/// are re-sent here.
pub fn restore(
    player: &Player,
    stats: &HunterStats,
    won: Won,
    system_points: SystemPoints,
    quest: &SystemQuest,
) {
    player.rebind_sidebar(OBJECTIVE_NAME);
    refresh(player, stats, won, system_points, quest);
}

/// Refreshes every sidebar line's text with the player's current stats.
///
/// Scores are re-sent at the same fixed values [`show`] used, since only the
/// displayed text (not the score ordering) changes as stats change.
pub fn refresh(
    player: &Player,
    stats: &HunterStats,
    won: Won,
    system_points: SystemPoints,
    quest: &SystemQuest,
) {
    let entries = lines(stats, won, system_points, quest);
    let count = entries.len();
    for (index, line) in entries.into_iter().enumerate() {
        let score = i32::try_from(count - index).unwrap_or(0);
        player.set_sidebar_line(OBJECTIVE_NAME, line.owner, score, line.text);
    }
}

/// Removes the hunter sidebar, e.g. before a player disconnects.
pub fn hide(player: &Player) {
    player.hide_sidebar(OBJECTIVE_NAME);
}
