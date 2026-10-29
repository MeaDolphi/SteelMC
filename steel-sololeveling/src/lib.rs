//! # Steel Solo Leveling
//!
//! Gameplay layer implementing a "Solo Leveling"-themed game mode on top of
//! Steel's vanilla server core. This crate owns all gameplay state and rules
//! specific to the mode (hunter stats, ranks, gates, the Shadow Monarch
//! system, and so on) and attaches to the vanilla server only through the
//! [`steel_core::gameplay_hooks::GameplayHooks`] trait.
//!
//! Solo Leveling player data is persisted entirely separately from vanilla
//! player data (see [`player_data`]); no vanilla save file or NBT format is
//! modified by this crate.

#![recursion_limit = "256"]

pub mod auction;
pub mod combat_reward;
pub mod commands;
pub mod daily;
pub mod dungeon;
pub mod economy;
pub mod guild;
pub mod guild_bank;
pub mod hooks;
pub mod hunter_stats_menu;
pub mod identifier;
pub mod item;
pub mod party;
pub mod player_data;
pub mod quest;
pub mod rank;
pub mod session;
pub mod shadow;
pub mod shadow_runtime;
pub mod shop;
pub mod sidebar;
pub mod state;
pub mod stats;
