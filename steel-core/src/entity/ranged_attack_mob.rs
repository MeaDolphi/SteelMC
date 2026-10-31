//! Marker trait for mobs that can perform ranged attacks.
//!
//! Mirrors vanilla `RangedAttackMob`.

use crate::entity::{Monster, SharedEntity};

/// A hostile mob capable of attacking from a distance.
pub trait RangedAttackMob: Monster {
    /// Performs a ranged attack against `target`.
    ///
    /// `entity` is the live shared instance of this mob (the attacker).
    /// `distance_sq` is the squared distance to the target, supplied by the
    /// controlling AI goal so the mob can adjust power/spread if needed.
    fn perform_ranged_attack(&self, entity: &SharedEntity, target: &SharedEntity, distance_sq: f64);
}
