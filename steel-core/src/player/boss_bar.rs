//! Player helpers for sending client-side boss bars.
//!
//! These methods talk directly to the client via `CBossEvent` and are purely
//! presentational: unlike a vanilla `ServerBossEvent`, no server-side object
//! tracks recipients, so the caller owns showing, updating, and hiding the bar.

use steel_protocol::packets::game::CBossEvent;
use text_components::TextComponent;
use uuid::Uuid;

use crate::player::Player;

pub use steel_protocol::packets::game::{BossBarColor, BossBarOverlay, BossBarProperties};

impl Player {
    /// Adds a boss bar to the player's HUD.
    ///
    /// `id` identifies the bar for later updates and removal; the client keeps
    /// showing the last received name and progress until the bar is removed.
    pub fn show_boss_bar(
        &self,
        id: Uuid,
        name: impl Into<TextComponent>,
        progress: f32,
        color: BossBarColor,
        overlay: BossBarOverlay,
        properties: BossBarProperties,
    ) {
        self.send_packet(CBossEvent::add(
            id,
            name.into(),
            progress.clamp(0.0, 1.0),
            color,
            overlay,
            properties,
        ));
    }

    /// Updates the fill fraction, in `0.0..=1.0`, of a shown boss bar.
    pub fn update_boss_bar_progress(&self, id: Uuid, progress: f32) {
        self.send_packet(CBossEvent::update_progress(id, progress.clamp(0.0, 1.0)));
    }

    /// Removes a boss bar from the player's HUD.
    pub fn hide_boss_bar(&self, id: Uuid) {
        self.send_packet(CBossEvent::remove(id));
    }
}
