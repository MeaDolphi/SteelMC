//! Resource-pack models for Solo Leveling items.
//!
//! Each themed stack carries a `minecraft:item_model` pointing into the
//! `sololeveling` namespace of the server resource pack. A client without the
//! pack falls back to the vanilla holder item, so the pack is never required
//! to play. The ids here must match `resourcepack/tools/build.py`.

use steel_registry::data_components::vanilla_components::{EQUIPPABLE, ITEM_MODEL};
use steel_registry::item_stack::ItemStack;

use crate::identifier::from_sololeveling;

/// Points `stack` at the pack model `path`, e.g. `crystal/s`.
pub fn apply(stack: &mut ItemStack, path: &str) {
    stack.set(ITEM_MODEL, from_sololeveling(path.to_owned()));
}

/// Dresses a worn item in the pack's equipment asset `name`.
///
/// The holder's own equippable settings (slot, sound) are kept; only the
/// look on the body changes. Items that are not worn are left alone.
pub fn apply_equipment(stack: &mut ItemStack, name: &str) {
    let Some(mut equippable) = stack.get(EQUIPPABLE).cloned() else {
        return;
    };
    equippable.asset_id = Some(from_sololeveling(name.to_owned()));
    stack.set(EQUIPPABLE, equippable);
}

/// Returns whether `stack` already uses the pack model `path`.
#[must_use]
pub fn has(stack: &ItemStack, path: &str) -> bool {
    stack
        .get(ITEM_MODEL)
        .is_some_and(|model| *model == from_sololeveling(path.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::item::{artifact, chest, potion};
    use crate::rank::HunterRank;

    /// Every model id the code writes, as `build.py` must produce it.
    fn model_paths() -> Vec<String> {
        let mut paths: Vec<String> = artifact::ALL
            .iter()
            .map(|id| format!("artifact/{}", id.name()))
            .collect();
        for rank in [
            HunterRank::E,
            HunterRank::D,
            HunterRank::C,
            HunterRank::B,
            HunterRank::A,
            HunterRank::S,
        ] {
            paths.push(format!("crystal/{}", rank.name().to_lowercase()));
        }
        for chest in chest::ALL_KINDS {
            paths.push(format!("chest/{}", chest.name()));
        }
        for potion in potion::ALL {
            paths.push(format!("potion/{}", potion.name()));
        }
        paths.push("shadow/soul".to_owned());
        for icon in ["next", "previous", "confirm", "cancel", "plus", "minus"] {
            paths.push(format!("ui/{icon}"));
        }
        paths.push("system_book".to_owned());
        paths
    }

    #[test]
    fn worn_artifacts_use_an_equipment_asset_of_the_pack() {
        steel_registry::init_vanilla_registry();
        let equipment = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resourcepack/pack/assets/sololeveling/equipment");
        for id in artifact::ALL {
            if id.artifact().category != artifact::Category::Armor {
                continue;
            }
            let stack = artifact::stack(id, None);
            let asset = stack
                .get(super::EQUIPPABLE)
                .and_then(|equippable| equippable.asset_id.clone())
                .expect("armor keeps an equippable component");
            assert_eq!(asset.namespace.as_ref(), "sololeveling", "{id:?}");
            assert!(
                equipment.join(format!("{}.json", asset.path)).is_file(),
                "resource pack has no equipment asset for {id:?}; run resourcepack/tools/build.py"
            );
        }
    }

    #[test]
    fn every_item_model_exists_in_the_resource_pack() {
        let items = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resourcepack/pack/assets/sololeveling/items");
        for path in model_paths() {
            assert!(
                items.join(format!("{path}.json")).is_file(),
                "resource pack has no item definition for {path}; run resourcepack/tools/build.py"
            );
        }
    }
}
