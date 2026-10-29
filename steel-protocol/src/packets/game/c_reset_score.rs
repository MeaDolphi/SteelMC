//! Clientbound reset score packet - removes a score entry for an owner, optionally
//! scoped to a single objective.

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_RESET_SCORE;
use steel_utils::codec::VarInt;
use steel_utils::serial::{PrefixedWrite as _, WriteTo};

/// Removes `owner`'s score. If `objective_name` is `None`, removes it from every
/// objective; otherwise removes it only from the named objective.
///
/// `objective_name`'s `bool`-prefixed encoding is written by hand because the `WriteTo`
/// derive's default field handling requires the field type itself to implement
/// `WriteTo`, and bare `String` only does through an explicit `#[write(as = ...)]`
/// strategy, which has no variant for "prefixed string wrapped in an optional".
#[derive(Debug, Clone, ClientPacket)]
#[packet_id(Play = C_RESET_SCORE)]
pub struct CResetScore {
    pub owner: String,
    pub objective_name: Option<String>,
}

impl WriteTo for CResetScore {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        self.owner.write_prefixed::<VarInt>(writer)?;
        match &self.objective_name {
            Some(name) => {
                true.write(writer)?;
                name.write_prefixed::<VarInt>(writer)
            }
            None => false.write(writer),
        }
    }
}

impl CResetScore {
    /// Creates a packet that removes `owner`'s score from every objective.
    #[must_use]
    pub fn remove_all(owner: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            objective_name: None,
        }
    }

    /// Creates a packet that removes `owner`'s score from a single objective.
    #[must_use]
    pub fn remove_one(owner: impl Into<String>, objective_name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            objective_name: Some(objective_name.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_utils::codec::VarInt;
    use steel_utils::serial::{PrefixedWrite as _, WriteTo as _};

    use super::CResetScore;

    #[test]
    fn remove_all_writes_owner_then_absent_flag() {
        let packet = CResetScore::remove_all("Steve");

        let mut encoded = Vec::new();
        let Ok(()) = packet.write(&mut encoded) else {
            panic!("reset score packet should encode");
        };

        let mut expected = Vec::new();
        let Ok(()) = "Steve".write_prefixed::<VarInt>(&mut expected) else {
            panic!("owner should encode");
        };
        expected.push(0); // objective_name absent

        assert_eq!(encoded, expected);
    }

    #[test]
    fn remove_one_writes_owner_then_present_flag_and_name() {
        let packet = CResetScore::remove_one("Steve", "sololeveling.level");

        let mut encoded = Vec::new();
        let Ok(()) = packet.write(&mut encoded) else {
            panic!("reset score packet should encode");
        };

        let mut expected = Vec::new();
        let Ok(()) = "Steve".write_prefixed::<VarInt>(&mut expected) else {
            panic!("owner should encode");
        };
        expected.push(1); // objective_name present
        let Ok(()) = "sololeveling.level".write_prefixed::<VarInt>(&mut expected) else {
            panic!("objective name should encode");
        };

        assert_eq!(encoded, expected);
    }
}
