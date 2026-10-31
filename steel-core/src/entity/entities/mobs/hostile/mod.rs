//! Hostile entity implementations.
//!
//! Those mobs are aggressive creatures that attack players on sight.

/// Hostile creeper entity module.
pub mod creeper;
/// Hostile endermite entity module.
pub mod endermite;
/// Hostile skeleton entity module.
pub mod skeleton;
/// Hostile spider entity module.
pub mod spider;
/// Hostile zombie entity module.
pub mod zombie;
pub use creeper::CreeperEntity;
pub use endermite::EndermiteEntity;
pub use skeleton::SkeletonEntity;
pub use spider::SpiderEntity;
pub use zombie::ZombieEntity;
