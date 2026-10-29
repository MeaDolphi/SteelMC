//! Hostile entity implementations.
//!
//! Those mobs are aggressive creatures that attack players on sight.

/// Hostile endermite entity module.
pub mod endermite;
/// Hostile zombie entity module.
pub mod zombie;
pub use endermite::EndermiteEntity;
pub use zombie::ZombieEntity;
