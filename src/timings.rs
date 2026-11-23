//! Komut süreleri: her komutun son çalışmaları bellekte tutulur, panelin Sistem sayfası komut başına
//! tipik (ortanca) ve en yavaş %5 süreyi gösterir.

use std::collections::VecDeque;
use std::sync::LazyLock;

use dashmap::DashMap;

/// Komut başına tutulan son ölçüm sayısı.
const KEEP: usize = 500;

// shortcut: ölçümler yalnızca bellekte, bot yeniden başlayınca sıfırlanır; geçmiş gerekirse Redis'e yazılmalı.
static SAMPLES: LazyLock<DashMap<String, VecDeque<u32>>> = LazyLock::new(DashMap::new);

/// Oyun, anket ve soygun komutları oyuncuları bekler (dakikalar sürer); süreleri hız ölçümüne girmez.
pub fn is_interactive(command: &str) -> bool {
    command.starts_with("oyun ") || matches!(command, "ekonomi blackjack" | "ekonomi soy" | "eğlence anket")
}

pub fn record(command: &str, ms: u32) {
    let mut samples = SAMPLES.entry(command.to_owned()).or_default();
    if samples.len() == KEEP {
        samples.pop_front();
    }
    samples.push_back(ms);
}

pub struct Summary {
    pub command: String,
    pub count: usize,
    pub p50: u32,
    pub p95: u32,
    pub max: u32,
}

/// Sıralı dizide yüzdelik (en yakın sıra yöntemi).
fn percentile(sorted: &[u32], pct: usize) -> u32 {
    let rank = (sorted.len() * pct).div_ceil(100).max(1);
    sorted.get(rank - 1).copied().unwrap_or(0)
}

fn summarize(command: &str, samples: &VecDeque<u32>) -> Option<Summary> {
    let mut sorted: Vec<u32> = samples.iter().copied().collect();
    sorted.sort_unstable();
    Some(Summary {
        command: command.to_owned(),
        count: sorted.len(),
        p50: percentile(&sorted, 50),
        p95: percentile(&sorted, 95),
        max: *sorted.last()?,
    })
}

/// Bütün komutlar, en yavaş %5 süresine göre (yavaştan hızlıya).
pub fn summaries() -> Vec<Summary> {
    let mut all: Vec<Summary> = SAMPLES.iter().filter_map(|e| summarize(e.key(), e.value())).collect();
    all.sort_by(|a, b| b.p95.cmp(&a.p95).then_with(|| a.command.cmp(&b.command)));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_nearest_rank() {
        let samples: VecDeque<u32> = (1..=100).collect();
        let s = summarize("x", &samples).expect("örnek var");
        assert_eq!((s.count, s.p50, s.p95, s.max), (100, 50, 95, 100));
        let one: VecDeque<u32> = [7].into_iter().collect();
        let s = summarize("y", &one).expect("örnek var");
        assert_eq!((s.p50, s.p95, s.max), (7, 7, 7));
        assert!(summarize("z", &VecDeque::new()).is_none());
    }

    #[test]
    fn keeps_only_recent_samples() {
        for ms in 0..(KEEP as u32 + 10) {
            record("test-tut", ms);
        }
        let samples = SAMPLES.get("test-tut").expect("kayıt var");
        assert_eq!(samples.len(), KEEP);
        assert_eq!(samples.front().copied(), Some(10));
    }

    #[test]
    fn games_are_not_timed() {
        assert!(is_interactive("oyun adam-asmaca"));
        assert!(is_interactive("ekonomi blackjack"));
        assert!(!is_interactive("ekonomi bakiye"));
    }
}
