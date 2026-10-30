//! Transferable shadow-summon items.
//!
//! A shadow found in a chest is an item until its owner activates it: the item
//! can be sold, traded, or kept, and activation consumes it once and adds the
//! shadow to the activating hunter's roster. It never moves another player's
//! live shadow body or roster.

use steel_registry::{item_stack::ItemStack, vanilla_items};
use steel_utils::Identifier;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::{named, read_kind, record, tag_string, with_lore, write_tag};

/// Kind stored in the record of a shadow-summon item.
const KIND: &str = "shadow";
/// Tag field holding the shadow's vanilla entity type.
const ENTITY: &str = "entity";

/// Entity types a chest may hand out as a transferable shadow.
///
/// Only mobs Steel can actually spawn are listed; a shadow the server cannot
/// summon would be an item that silently does nothing.
pub const CATALOG: [Identifier; 2] = [
    Identifier::vanilla_static("zombie"),
    Identifier::vanilla_static("endermite"),
];

/// Creates one shadow-summon item for `entity`.
#[must_use]
pub fn stack(entity: &Identifier) -> ItemStack {
    let mut stack = named(
        ItemStack::new(&vanilla_items::SOUL_LANTERN),
        TextComponent::plain(display_name(entity)).color(Color::LightPurple),
    );
    stack = with_lore(
        stack,
        vec![
            TextComponent::plain("Призыв тени. Предмет можно передать.").color(Color::Gray),
            TextComponent::plain("Правый клик — присоединить тень к своей армии.")
                .color(Color::Yellow),
        ],
    );
    let mut fields = record(KIND);
    fields.insert(ENTITY, entity.to_string());
    write_tag(&mut stack, fields);
    super::model::apply(&mut stack, "shadow/soul");
    stack
}

/// Returns the entity type a shadow item summons.
#[must_use]
pub fn entity(stack: &ItemStack) -> Option<Identifier> {
    let tag = read_kind(stack, KIND)?;
    tag_string(&tag, ENTITY)?.parse().ok()
}

/// Returns the display name of a shadow of `entity`.
#[must_use]
pub fn display_name(entity: &Identifier) -> String {
    let kind = match entity.path.as_ref() {
        "zombie" => "зомби",
        "endermite" => "эндермита",
        other => other,
    };
    format!("Тень {kind}")
}

/// Returns the entity type at catalog `index`.
#[must_use]
pub fn catalog_identifier(index: usize) -> Identifier {
    CATALOG[index % CATALOG.len()].clone()
}

/// Returns the display name of catalog entry `index`.
#[must_use]
pub fn catalog_name(index: usize) -> String {
    display_name(&catalog_identifier(index))
}

/// Creates the item of catalog entry `index`.
///
/// Catalog indexes are the only shadow reference stored in a reward, so a chest
/// rolled before a catalog change still resolves to a concrete entity.
#[must_use]
pub fn catalog_stack(index: usize) -> ItemStack {
    stack(&catalog_identifier(index))
}
