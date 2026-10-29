//! Themed item types built from vanilla items plus `custom_data` tags.
//!
//! Steel's client is unmodified vanilla Minecraft, so every Solo Leveling item
//! has to stay a vanilla item: the theme lives in the stack's `custom_data`
//! component, its custom name, and its lore. Nothing here registers new item
//! ids, and a plain vanilla item without these tags is never mistaken for a
//! themed one — that is what keeps an ordinary diamond from being sold as a
//! magic crystal.
//!
//! Every tag lives under one root compound so a stack carries at most one
//! Solo Leveling record and merging two tagged stacks can never mix fields of
//! different kinds.

pub mod artifact;
pub mod binding;
pub mod chest;
pub mod choice_menu;
pub mod consume;
pub mod crystal;
pub mod menu_book;
pub mod nbt;
pub mod potion;
pub mod shadow;

use simdnbt::owned::{NbtCompound, NbtTag};
use steel_registry::data_components::components::{CustomData, ItemLore};
use steel_registry::data_components::vanilla_components::{CUSTOM_DATA, CUSTOM_NAME, LORE};
use steel_registry::item_stack::ItemStack;
use text_components::TextComponent;

/// Root compound holding every Solo Leveling tag of one stack.
const ROOT: &str = "sololeveling";
/// Field naming the themed item kind, so tags of one kind are never read as another.
const KIND: &str = "kind";

/// Applies `fields` under the Solo Leveling root of `stack`.
pub(crate) fn write_tag(stack: &mut ItemStack, fields: NbtCompound) {
    let mut root = NbtCompound::new();
    root.insert(ROOT, fields);
    if let Some(data) = CustomData::try_from_compound(root) {
        stack.set_custom_data(&data);
    }
}

/// Returns the Solo Leveling record of `stack`, without checking its kind.
pub(crate) fn read_tag(stack: &ItemStack) -> Option<NbtCompound> {
    let data = stack.get(CUSTOM_DATA)?;
    match data.as_compound().get(ROOT)? {
        NbtTag::Compound(tag) => Some(tag.clone()),
        _ => None,
    }
}

/// Reads a string field of a Solo Leveling record.
pub(crate) fn tag_string(tag: &NbtCompound, field: &str) -> Option<String> {
    tag.string(field).map(ToString::to_string)
}

/// Returns the Solo Leveling record of `stack` when it names `kind`.
pub(crate) fn read_kind(stack: &ItemStack, kind: &str) -> Option<NbtCompound> {
    let tag = read_tag(stack)?;
    (tag_string(&tag, KIND)? == kind).then_some(tag)
}

/// Starts a Solo Leveling record of one kind.
pub(crate) fn record(kind: &str) -> NbtCompound {
    let mut fields = NbtCompound::new();
    fields.insert(KIND, kind.to_owned());
    fields
}

/// Gives `stack` a themed display name.
pub(crate) fn named(mut stack: ItemStack, name: TextComponent) -> ItemStack {
    stack.set(CUSTOM_NAME, name);
    stack
}

/// Replaces the lore of `stack`, ignoring an empty list.
pub(crate) fn with_lore(mut stack: ItemStack, lines: Vec<TextComponent>) -> ItemStack {
    if let Ok(lore) = ItemLore::new(lines) {
        stack.set(LORE, lore);
    }
    stack
}
