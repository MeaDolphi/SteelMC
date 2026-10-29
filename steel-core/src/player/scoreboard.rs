//! Player helpers for sending client-side scoreboard sidebar displays.
//!
//! These methods talk directly to the client via `CSetObjective`,
//! `CSetDisplayObjective`, `CSetScore`, and `CResetScore`. They are purely
//! presentational: unlike [`crate::scoreboard::Scoreboard`] (the
//! `/scoreboard`-command-facing, persistent domain scoreboard), nothing sent
//! through these methods is visible to vanilla scoreboard commands or
//! selectors, and nothing here is persisted.

use steel_protocol::packets::game::{
    CResetScore, CSetDisplayObjective, CSetObjective, CSetScore, DisplaySlot, ObjectiveRenderType,
};
use text_components::TextComponent;

use crate::player::Player;

impl Player {
    /// Adds a new sidebar-presentational objective and binds it to the
    /// sidebar display slot.
    ///
    /// `objective_name` is a server-internal identifier never shown to the
    /// player; `display_name` is the sidebar's title.
    ///
    /// The client keeps an objective until it is removed, and disconnects on a
    /// second `add` for a name it already knows, so this must be called at most
    /// once per connection. A respawned player is a new [`Player`] on the same
    /// connection and has to use [`Player::rebind_sidebar`] instead.
    pub fn show_sidebar(
        &self,
        objective_name: impl Into<String>,
        display_name: impl Into<TextComponent>,
    ) {
        let objective_name = objective_name.into();
        self.send_packet(CSetObjective::add(
            objective_name.clone(),
            display_name,
            ObjectiveRenderType::Integer,
            None,
        ));
        self.rebind_sidebar(objective_name);
    }

    /// Binds an already added objective to the sidebar display slot.
    ///
    /// Re-sending the display slot is safe: the client only looks the objective
    /// up and binds it.
    pub fn rebind_sidebar(&self, objective_name: impl Into<String>) {
        self.send_packet(CSetDisplayObjective::new(
            DisplaySlot::Sidebar,
            objective_name,
        ));
    }

    /// Removes a previously shown sidebar objective, clearing the sidebar
    /// slot if it was bound to it.
    pub fn hide_sidebar(&self, objective_name: impl Into<String>) {
        self.send_packet(CSetObjective::remove(objective_name));
    }

    /// Sets one sidebar line's text and integer value.
    ///
    /// `owner` is the line's unique key within the objective (vanilla shows
    /// it as the line's label unless `display` overrides it); `display`
    /// overrides the shown text while keeping `owner` as the identity used
    /// for later updates/removal.
    pub fn set_sidebar_line(
        &self,
        objective_name: impl Into<String>,
        owner: impl Into<String>,
        value: i32,
        display: impl Into<TextComponent>,
    ) {
        self.send_packet(CSetScore::with_display(
            owner,
            objective_name,
            value,
            Some(display.into()),
            None,
        ));
    }

    /// Removes one sidebar line by its `owner` key from a single objective.
    pub fn remove_sidebar_line(&self, objective_name: impl Into<String>, owner: impl Into<String>) {
        self.send_packet(CResetScore::remove_one(owner, objective_name));
    }
}
