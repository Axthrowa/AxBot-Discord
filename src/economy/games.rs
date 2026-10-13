//! Şans oyunlarının kuralları (Discord'dan bağımsız, birim testli).

use crate::rand_util;

// ===================== Slot makinesi =====================

/// Semboller ve üçlü gelince ödeme çarpanları (nadir olan daha çok öder).
pub const SLOT_SYMBOLS: [(&str, u64); 7] = [
    ("🍒", 3),
    ("🍋", 4),
    ("🍇", 5),
    ("🔔", 8),
    ("⭐", 10),
    ("💎", 20),
    ("7️⃣", 50),
];

/// İkili gelince bahsin 1,5 katı ödenir (ondalıksız: ×3/2).
const PAIR_NUMERATOR: u64 = 3;
const PAIR_DENOMINATOR: u64 = 2;

pub fn spin() -> [usize; 3] {
    let n = SLOT_SYMBOLS.len() as u64;
    [0, 1, 2].map(|_| usize::try_from(rand_util::below(n)).unwrap_or(0))
}

pub fn slot_symbol(index: usize) -> &'static str {
    SLOT_SYMBOLS.get(index).map_or("❔", |(s, _)| s)
}

/// Bahis karşılığı ödenecek toplam (bahis dahil); kaybettiyse 0.
pub fn slot_payout(reels: [usize; 3], bet: u64) -> u64 {
    let [a, b, c] = reels;
    if a == b && b == c {
        let multiplier = SLOT_SYMBOLS.get(a).map_or(0, |(_, m)| *m);
        return bet.saturating_mul(multiplier);
    }
    if a == b || b == c || a == c {
        return bet.saturating_mul(PAIR_NUMERATOR) / PAIR_DENOMINATOR;
    }
    0
}

// ===================== Blackjack =====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Card {
    /// 1 = As, 11-13 = Vale/Kız/Papaz.
    pub rank: u8,
    pub suit: u8,
}

impl Card {
    pub fn label(self) -> String {
        let rank = match self.rank {
            1 => "A".to_owned(),
            11 => "J".to_owned(),
            12 => "Q".to_owned(),
            13 => "K".to_owned(),
            n => n.to_string(),
        };
        let suit = ["♠️", "♥️", "♦️", "♣️"].get(usize::from(self.suit)).copied().unwrap_or("♠️");
        format!("`{rank}`{suit}")
    }
}

pub fn shuffled_deck() -> Vec<Card> {
    let mut deck: Vec<Card> = (0..4u8)
        .flat_map(|suit| (1..=13u8).map(move |rank| Card { rank, suit }))
        .collect();
    rand_util::shuffle(&mut deck);
    deck
}

/// El değeri: As 11 sayılır, 21'i aşarsa 1'e düşer.
pub fn hand_value(hand: &[Card]) -> u32 {
    let mut total: u32 = hand.iter().map(|c| u32::from(c.rank.min(10))).sum();
    let aces = hand.iter().filter(|c| c.rank == 1).count();
    for _ in 0..aces {
        if total + 10 <= 21 {
            total += 10;
        }
    }
    total
}

pub fn is_blackjack(hand: &[Card]) -> bool {
    hand.len() == 2 && hand_value(hand) == 21
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// İlk iki kartla 21: bahsin 2,5 katı ödenir.
    Blackjack,
    Win,
    Push,
    Lose,
}

impl Outcome {
    /// Ödenecek toplam (bahis dahil).
    pub fn payout(self, bet: u64) -> u64 {
        match self {
            Self::Blackjack => bet.saturating_mul(5) / 2,
            Self::Win => bet.saturating_mul(2),
            Self::Push => bet,
            Self::Lose => 0,
        }
    }
}

/// Krupiye 17'ye kadar kart çeker (yumuşak 17'de durur).
pub fn dealer_play(dealer: &mut Vec<Card>, deck: &mut Vec<Card>) {
    while hand_value(dealer) < 17 {
        match deck.pop() {
            Some(card) => dealer.push(card),
            None => break,
        }
    }
}

pub fn settle(player: &[Card], dealer: &[Card]) -> Outcome {
    let (p, d) = (hand_value(player), hand_value(dealer));
    match (is_blackjack(player), is_blackjack(dealer)) {
        (true, true) => return Outcome::Push,
        (true, false) => return Outcome::Blackjack,
        (false, true) => return Outcome::Lose,
        _ => {}
    }
    if p > 21 {
        Outcome::Lose
    } else if d > 21 || p > d {
        Outcome::Win
    } else if p == d {
        Outcome::Push
    } else {
        Outcome::Lose
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(rank: u8) -> Card {
        Card { rank, suit: 0 }
    }

    #[test]
    fn aces_count_as_one_or_eleven() {
        assert_eq!(hand_value(&[c(1), c(13)]), 21);
        assert_eq!(hand_value(&[c(1), c(1)]), 12);
        assert_eq!(hand_value(&[c(1), c(9), c(5)]), 15);
        assert_eq!(hand_value(&[c(1), c(1), c(9)]), 21);
        assert_eq!(hand_value(&[c(12), c(11), c(2)]), 22);
    }

    #[test]
    fn settles_hands() {
        assert_eq!(settle(&[c(1), c(12)], &[c(10), c(9)]), Outcome::Blackjack);
        assert_eq!(settle(&[c(1), c(12)], &[c(1), c(13)]), Outcome::Push);
        assert_eq!(settle(&[c(10), c(9)], &[c(10), c(8)]), Outcome::Win);
        assert_eq!(settle(&[c(10), c(8)], &[c(10), c(8)]), Outcome::Push);
        assert_eq!(settle(&[c(10), c(8), c(5)], &[c(2), c(3)]), Outcome::Lose);
        assert_eq!(settle(&[c(10), c(7)], &[c(10), c(6), c(9)]), Outcome::Win);
    }

    #[test]
    fn payouts() {
        assert_eq!(Outcome::Blackjack.payout(100), 250);
        assert_eq!(Outcome::Win.payout(100), 200);
        assert_eq!(Outcome::Push.payout(100), 100);
        assert_eq!(Outcome::Lose.payout(100), 0);
    }

    #[test]
    fn dealer_stops_at_seventeen() {
        let mut deck = vec![c(10), c(5), c(2)];
        let mut dealer = vec![c(6), c(4)];
        dealer_play(&mut dealer, &mut deck);
        // 10 + 2 = 12 → 5 → 17 dur.
        assert_eq!(hand_value(&dealer), 17);
    }

    #[test]
    fn deck_has_52_unique_cards() {
        let mut deck = shuffled_deck();
        assert_eq!(deck.len(), 52);
        deck.sort_by_key(|c| (c.suit, c.rank));
        deck.dedup();
        assert_eq!(deck.len(), 52);
    }

    #[test]
    fn slot_payouts() {
        assert_eq!(slot_payout([6, 6, 6], 10), 500);
        assert_eq!(slot_payout([0, 0, 0], 10), 30);
        assert_eq!(slot_payout([0, 0, 3], 10), 15);
        assert_eq!(slot_payout([2, 4, 2], 10), 15);
        assert_eq!(slot_payout([0, 1, 2], 10), 0);
    }

    /// Uzun vadede kasa kazanmalı (beklenen ödeme < bahis), ama çok cimri olmamalı.
    #[test]
    fn slot_house_edge_is_reasonable() {
        let n = SLOT_SYMBOLS.len();
        let mut total = 0u64;
        let mut count = 0u64;
        for a in 0..n {
            for b in 0..n {
                for c in 0..n {
                    total += slot_payout([a, b, c], 1000);
                    count += 1000;
                }
            }
        }
        let rtp = total as f64 / count as f64;
        assert!((0.75..1.0).contains(&rtp), "geri ödeme oranı {rtp}");
    }
}
