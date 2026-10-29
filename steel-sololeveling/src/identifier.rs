//! Namespacing helpers for Solo Leveling identifiers.
//!
//! Mirrors the `Identifier::from_steel` convention in `steel_utils` so Solo
//! Leveling data (stats, ranks, gates, quests, ...) is namespaced separately
//! from vanilla (`minecraft`) and core Steel (`steel`) identifiers.

use std::borrow::Cow;

use steel_utils::Identifier;

/// The namespace used for all Solo Leveling identifiers.
pub const SOLOLEVELING_NAMESPACE: &str = "sololeveling";

/// Creates a new [`Identifier`] in the Solo Leveling namespace.
#[must_use]
pub fn from_sololeveling(path: impl Into<Cow<'static, str>>) -> Identifier {
    Identifier::new(SOLOLEVELING_NAMESPACE, path)
}
