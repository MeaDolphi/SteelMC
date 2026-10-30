//! Non-player characters standing in the city.
//!
//! Each character is a vanilla mannequin: it looks like a person, never moves
//! and cannot be hurt. Mannequins are saved with their chunk like any other
//! entity, so the runtime only adopts the one already standing at a post and
//! spawns a replacement when the post is empty while hunters are nearby.

use std::sync::Arc;

use glam::DVec3;
use steel_core::entity::entities::MannequinEntity;
use steel_core::entity::{Entity, RemovalReason, SharedEntity, spawn_mob};
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::vanilla_entities;
use steel_utils::{BlockPos, Downcast as _, WorldAabb};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::city;

/// Ticks between checks that every post has its character.
const CHECK_INTERVAL_TICKS: u64 = 100;
/// How far from its post a character still counts as standing there.
const POST_RADIUS: f64 = 1.5;
/// How close a hunter has to be before an empty post is filled, which keeps
/// the post's chunk loaded while the character spawns.
const SPAWN_DISTANCE: f64 = 48.0;

/// What a character does when a hunter talks to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpcRole {
    /// Hands out and accepts Organization contracts.
    OrganizationAgent,
}

struct Post {
    role: NpcRole,
    position: DVec3,
    yaw: f32,
    name: &'static str,
    description: &'static str,
}

const POSTS: [Post; 1] = [Post {
    role: NpcRole::OrganizationAgent,
    position: city::ORGANIZATION_AGENT_POS,
    yaw: city::ORGANIZATION_AGENT_YAW,
    name: "Агент Организации",
    description: "Контракты для охотников",
}];

/// Makes sure every city post has its character.
pub fn tick(server: &Arc<Server>, tick: u64) {
    if !tick.is_multiple_of(CHECK_INTERVAL_TICKS) {
        return;
    }
    let Some(world) = server
        .worlds
        .values()
        .find(|world| city::is_city_world(world))
    else {
        return;
    };
    for post in &POSTS {
        let standing = characters_at(world, post);
        if let Some((first, extra)) = standing.split_first() {
            // A post holds one character; extras come from an older save.
            for duplicate in extra {
                duplicate.set_removed(RemovalReason::Discarded);
            }
            dress(first.as_ref(), post);
            continue;
        }
        if hunters_nearby(world, post.position) {
            spawn(world, post);
        }
    }
}

/// Returns the role of `target` when it is one of the city characters.
#[must_use]
pub fn role_of(world: &World, target: &dyn Entity) -> Option<NpcRole> {
    if !city::is_city_world(world) || target.entity_type() != &vanilla_entities::MANNEQUIN {
        return None;
    }
    POSTS
        .iter()
        .find(|post| target.position().distance(post.position) <= POST_RADIUS)
        .map(|post| post.role)
}

fn characters_at(world: &World, post: &Post) -> Vec<SharedEntity> {
    let min = post.position - DVec3::splat(POST_RADIUS);
    let max = post.position + DVec3::splat(POST_RADIUS + 2.0);
    let area = WorldAabb::new(min.x, min.y, min.z, max.x, max.y, max.z);
    world.get_entities_in_aabb_matching(&area, |entity| {
        entity.entity_type() == &vanilla_entities::MANNEQUIN && !entity.is_removed()
    })
}

fn hunters_nearby(world: &World, position: DVec3) -> bool {
    let mut nearby = false;
    world.players.iter_players(|_, player| {
        nearby = player.position().distance(position) <= SPAWN_DISTANCE;
        !nearby
    });
    nearby
}

fn spawn(world: &Arc<World>, post: &Post) {
    match spawn_mob(
        world,
        &vanilla_entities::MANNEQUIN,
        BlockPos::from(post.position),
    ) {
        Ok(entity) => {
            dress(entity.as_ref(), post);
            log::info!("sololeveling: placed the city character \"{}\"", post.name);
        }
        Err(error) => log::warn!(
            "sololeveling: unable to place the city character \"{}\": {error:?}",
            post.name
        ),
    }
}

/// Puts a character exactly on its post and restores how it looks; cheap
/// enough to repeat on every check, so a moved or reloaded character heals.
fn dress(entity: &dyn Entity, post: &Post) {
    if entity.position().distance(post.position) > 0.05
        && let Err(error) = entity.try_set_position(post.position)
    {
        log::debug!("sololeveling: city character position update failed: {error}");
    }
    entity.set_rotation((post.yaw, 0.0));
    if let Some(living) = entity.as_living_entity() {
        living.set_y_head_rot(post.yaw);
    }
    entity.set_invulnerable(true);
    entity.set_custom_name(Some(TextComponent::plain(post.name).color(Color::Aqua)));
    entity.set_custom_name_visible(true);
    if let Some(mannequin) = entity.downcast_ref::<MannequinEntity>() {
        mannequin.set_immovable(true);
        mannequin.set_description(Some(
            TextComponent::plain(post.description).color(Color::Gray),
        ));
    }
}
