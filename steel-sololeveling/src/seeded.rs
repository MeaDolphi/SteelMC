//! Deterministic randomness for generated layouts.
//!
//! Dungeons and the Demon Castle are rebuilt from a seed alone, so these
//! helpers keep a seed meaning the same blocks regardless of the `rand`
//! crate version.

/// A small deterministic generator.
#[derive(Debug, Clone)]
pub struct SplitMix(u64);

impl SplitMix {
    /// Creates a generator from `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Returns the next raw value.
    pub const fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    /// Returns a value in `low..=high`.
    pub fn range(&mut self, low: i32, high: i32) -> i32 {
        let span = u64::try_from(high - low + 1).unwrap_or(1).max(1);
        low + i32::try_from(self.next() % span).unwrap_or(0)
    }

    /// Returns `true` with `percent` percent probability.
    pub const fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    /// Returns an index into a slice of `len` elements; zero for an empty one.
    pub fn index(&mut self, len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        usize::try_from(self.next() % len as u64).unwrap_or(0)
    }
}

/// Hashes a position into a stable pseudo-random value.
#[must_use]
pub const fn hash(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    let mut value = seed
        ^ (x as u64).wrapping_mul(73_856_093)
        ^ (y as u64).wrapping_mul(83_492_791)
        ^ (z as u64).wrapping_mul(2_654_435_761);
    value = (value ^ (value >> 31)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^ (value >> 29)
}
