use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

/// Supplies the obfuscation keys for new item blobs (imports and duplicates).
///
/// Any `i32` is a valid key. The game writes random ones; tests inject a fixed sequence.
/// Every `FnMut() -> i32` closure is a `KeySource`.
pub trait KeySource {
    fn next_key(&mut self) -> i32;
}

impl<F: FnMut() -> i32> KeySource for F {
    fn next_key(&mut self) -> i32 {
        self()
    }
}

/// The default [`KeySource`]: unpredictable keys from the standard library's randomly
/// seeded hasher, mixed with the clock and a counter. Not for cryptography; item keys only
/// need to look like the game's.
#[derive(Debug, Clone, Default)]
pub struct EntropyKeys {
    counter: u64,
}

impl EntropyKeys {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeySource for EntropyKeys {
    fn next_key(&mut self) -> i32 {
        self.counter = self.counter.wrapping_add(1);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        // Each `RandomState` is seeded differently by the operating system's random source.
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(nanos);
        hasher.write_u64(self.counter);
        let hash = hasher.finish();
        (hash ^ (hash >> 32)) as u32 as i32
    }
}
