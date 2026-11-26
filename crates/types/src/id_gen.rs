//! Scoped identifier generation abstraction for deterministic simulation testing (DST).
//!
//! Production code defaults to [`SystemIdGen`] (backed by `Uuid::new_v4()`).
//! Simulation code injects [`DeterministicIdGen`] seeded from the simulation seed so
//! generated entity IDs, tokens, and UUIDs are reproducible across runs.
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

pub use uuid::Uuid;

/// Scoped identifier generator for entity IDs, bearer tokens, and UUIDv4 values.
pub trait IdGen: Send + Sync + 'static {
    /// Generate a random or deterministic RFC-4122 version-4 UUID.
    fn next_uuid(&self) -> Uuid;

    /// Generate a short 8-character lowercase hexadecimal entity ID (matches [`crate::id`]).
    fn next_id(&self) -> String {
        self.next_uuid().simple().to_string()[..8].to_owned()
    }

    /// Generate a 32-character lowercase hexadecimal token (matches [`crate::token_id`]).
    fn next_token(&self) -> String {
        self.next_uuid().simple().to_string()
    }
}

/// Default production identifier generator backed by `Uuid::new_v4()`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemIdGen;

impl IdGen for SystemIdGen {
    fn next_uuid(&self) -> Uuid {
        Uuid::new_v4()
    }
}

/// Convenience constructor for the default shared production identifier generator.
pub fn system_id_gen() -> Arc<dyn IdGen> {
    Arc::new(SystemIdGen)
}

#[inline]
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Seeded, lock-free deterministic identifier generator producing RFC-4122-shaped UUIDv4s.
///
/// Two instances created with the same `seed` produce identical sequences of UUIDs,
/// short IDs, and tokens when invoked in the same order. Safe to share across threads.
#[derive(Debug)]
pub struct DeterministicIdGen {
    seed_lo: u64,
    seed_hi: u64,
    counter: AtomicU64,
}

impl DeterministicIdGen {
    /// Create a new deterministic identifier generator from a 64-bit seed.
    pub fn new(seed: u64) -> Self {
        let seed_lo = splitmix64(seed ^ 0x243f_6a88_85a3_08d3);
        let seed_hi = splitmix64(seed_lo ^ 0x1319_8a2e_0370_7344);
        Self {
            seed_lo,
            seed_hi,
            counter: AtomicU64::new(0),
        }
    }

    /// Convenience constructor returning a shared trait object.
    pub fn arc(seed: u64) -> Arc<dyn IdGen> {
        Arc::new(Self::new(seed))
    }
}

impl IdGen for DeterministicIdGen {
    fn next_uuid(&self) -> Uuid {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed);
        let hi = splitmix64(self.seed_hi ^ idx.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let lo = splitmix64(self.seed_lo ^ idx.wrapping_mul(0xbf58_476d_1ce4_e5b9).wrapping_add(1));
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&hi.to_be_bytes());
        bytes[8..].copy_from_slice(&lo.to_be_bytes());
        // RFC 4122 version 4 (random) and variant 1 (10xx xxxx).
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes)
    }
}

/// Convenience constructor for a shared deterministic identifier generator.
pub fn deterministic_id_gen(seed: u64) -> Arc<dyn IdGen> {
    DeterministicIdGen::arc(seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use uuid::{Variant, Version};

    #[test]
    fn deterministic_id_gen_is_reproducible_rfc4122_and_seed_isolated() {
        let g1 = DeterministicIdGen::new(42);
        let g2 = DeterministicIdGen::new(42);
        let g3 = DeterministicIdGen::new(43);

        let mut seen = BTreeSet::new();
        for _ in 0..32 {
            let u1 = g1.next_uuid();
            let u2 = g2.next_uuid();
            let u3 = g3.next_uuid();
            assert_eq!(u1, u2);
            assert_ne!(u1, u3);
            assert_eq!(u1.get_version(), Some(Version::Random));
            assert_eq!(u1.get_variant(), Variant::RFC4122);
            assert!(seen.insert(u1), "duplicate uuid generated");
        }

        let id1 = g1.next_id();
        let id2 = g2.next_id();
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 8);
        assert!(id1.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));

        let tok1 = g1.next_token();
        let tok2 = g2.next_token();
        assert_eq!(tok1, tok2);
        assert_eq!(tok1.len(), 32);
        assert!(tok1.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    }

    #[test]
    fn deterministic_id_gen_is_thread_safe_and_unique_across_threads() {
        let id_gen = DeterministicIdGen::arc(999);
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let g = id_gen.clone();
                std::thread::spawn(move || (0..64).map(|_| g.next_uuid()).collect::<Vec<_>>())
            })
            .collect();
        let mut all = BTreeSet::new();
        for h in handles {
            for u in h.join().unwrap() {
                assert_eq!(u.get_version(), Some(Version::Random));
                assert_eq!(u.get_variant(), Variant::RFC4122);
                assert!(all.insert(u));
            }
        }
        assert_eq!(all.len(), 256);
    }
}
