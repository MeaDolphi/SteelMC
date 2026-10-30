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

pub mod abilities;
pub mod auction;
pub mod auction_menu;
pub mod city;
mod city_interior;
pub mod class;
pub mod class_menu;
pub mod combat_reward;
pub mod commands;
pub mod daily;
pub mod dungeon;
pub mod economy;
pub mod enchant_menu;
pub mod gear;
pub mod guild;
pub mod guild_bank;
pub mod guild_menu;
pub mod hooks;
pub mod hunter_stats_menu;
pub mod identifier;
pub mod item;
pub mod main_menu;
pub mod notice;
pub mod npc;
pub mod organization;
pub mod organization_menu;
pub mod party;
pub mod party_menu;
pub mod pay_menu;
pub mod player_data;
pub mod quest;
pub mod rank;
pub mod rank_icon;
pub mod resource_pack;
pub mod session;
pub mod shadow;
pub mod shadow_menu;
pub mod shadow_runtime;
pub mod shop;
pub mod shop_menu;
pub mod sidebar;
pub mod sound;
pub mod state;
pub mod stats;
pub mod ui;
