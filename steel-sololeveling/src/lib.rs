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
pub mod admin_menu;
pub mod auction;
pub mod auction_menu;
pub mod castle;
pub mod castle_menu;
pub mod city;
mod city_interior;
pub mod class;
pub mod class_menu;
pub mod combat_reward;
pub mod commands;
pub mod crafting;
pub mod crafting_menu;
pub mod daily;
pub mod deferred_inventory_menu;
pub mod dungeon;
pub mod economy;
pub mod enchant_menu;
pub mod gate_menu;
pub mod gear;
pub mod guild;
pub mod guild_bank;
pub mod guild_menu;
pub mod hooks;
pub mod hunter_stats_menu;
pub mod identifier;
pub mod island;
pub mod item;
pub mod job_change;
pub mod main_menu;
pub mod merchant;
pub mod merchant_menu;
pub mod notice;
pub mod npc;
pub mod organization;
pub mod organization_menu;
pub mod party;
pub mod party_menu;
pub mod pay_menu;
pub mod player_data;
pub mod quest;
pub mod quest_book;
pub mod quest_menu;
pub mod rank;
pub mod rank_icon;
pub mod resource_pack;
pub mod scenario;
pub mod session;
pub mod shadow;
pub mod shadow_menu;
pub mod shadow_runtime;
pub mod shop;
pub mod shop_menu;
pub mod sidebar;
pub mod sound;
pub mod spell;
pub mod spell_fx;
pub mod spell_menu;
pub mod starter;
pub mod starter_menu;
pub mod state;
pub mod stats;
pub mod temple;
pub mod ui;
