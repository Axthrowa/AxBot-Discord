//! Harici bağımlılık olmadan rastgelelik (çekiliş kazananı seçme, karıştırma).
//!
//! `RandomState` her oluşturulduğunda işletim sisteminden tohumlanan anahtarlar kullanır;
//! artan bir sayaçla birlikte hash'lenerek ardışık çağrılar birbirinden bağımsız olur.
//! Kriptografik güvenlik gerekmeyen işler içindir.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn random_u64() -> u64 {
    RandomState::new().hash_one(COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// `0..upper` aralığında (upper > 0).
pub fn below(upper: u64) -> u64 {
    if upper == 0 {
        0
    } else {
        random_u64() % upper
    }
}

/// Fisher-Yates karıştırma.
pub fn shuffle<T>(items: &mut [T]) {
    for i in (1..items.len()).rev() {
        let j = below(i as u64 + 1) as usize;
        items.swap(i, j);
    }
}

/// Listeden en fazla `count` farklı eleman seçer.
pub fn pick<T: Clone>(items: &[T], count: usize) -> Vec<T> {
    let mut pool = items.to_vec();
    shuffle(&mut pool);
    pool.truncate(count);
    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_returns_distinct_subset() {
        let items: Vec<u32> = (0..50).collect();
        let picked = pick(&items, 10);
        assert_eq!(picked.len(), 10);
        let mut sorted = picked.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 10, "tekrar eden eleman olmamalı");
        assert!(picked.iter().all(|p| items.contains(p)));
    }

    #[test]
    fn pick_more_than_available_returns_all() {
        assert_eq!(pick(&[1, 2, 3], 10).len(), 3);
    }

    #[test]
    fn shuffle_is_not_constant() {
        let original: Vec<u32> = (0..20).collect();
        let changed = (0..10).any(|_| {
            let mut v = original.clone();
            shuffle(&mut v);
            v != original
        });
        assert!(changed);
    }
}
