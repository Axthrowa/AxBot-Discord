//! Adam asmaca kuralları (Türkçe harflerle).

use std::collections::BTreeSet;

pub const MAX_WRONG: usize = 6;

/// Türkçeye uygun küçültme (İ → i, I → ı).
pub fn tr_lower(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'İ' => 'i',
            'I' => 'ı',
            c => c.to_lowercase().next().unwrap_or(c),
        })
        .collect()
}

/// Türkçeye uygun büyütme (i → İ, ı → I).
pub fn tr_upper_char(c: char) -> String {
    match c {
        'i' => "İ".to_owned(),
        'ı' => "I".to_owned(),
        c => c.to_uppercase().collect(),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Guess {
    Hit,
    Miss,
    AlreadyTried,
    Invalid,
}

pub struct Hangman {
    word: Vec<char>,
    tried: BTreeSet<char>,
    wrong: Vec<char>,
    solved: bool,
}

impl Hangman {
    pub fn new(word: &str) -> Self {
        Self { word: tr_lower(word).chars().collect(), tried: BTreeSet::new(), wrong: Vec::new(), solved: false }
    }

    /// Tek harf ya da kelimenin tamamı.
    pub fn guess(&mut self, input: &str) -> Guess {
        let input = tr_lower(input.trim());
        let chars: Vec<char> = input.chars().collect();
        match chars.as_slice() {
            [c] if c.is_alphabetic() => {
                if !self.tried.insert(*c) {
                    return Guess::AlreadyTried;
                }
                if self.word.contains(c) {
                    Guess::Hit
                } else {
                    self.wrong.push(*c);
                    Guess::Miss
                }
            }
            _ if chars.len() == self.word.len() && chars.iter().all(|c| c.is_alphabetic()) => {
                if chars == self.word {
                    self.solved = true;
                    return Guess::Hit;
                }
                // Yanlış kelime: harfleri tek tek denenmiş sayılır; kelimede olanlar açılır, olmayanlar hata olur.
                let (mut hit, mut new) = (false, false);
                for c in chars {
                    if self.tried.insert(c) {
                        new = true;
                        if self.word.contains(&c) {
                            hit = true;
                        } else {
                            self.wrong.push(c);
                        }
                    }
                }
                match (new, hit) {
                    (false, _) => Guess::AlreadyTried,
                    (true, true) => Guess::Hit,
                    (true, false) => Guess::Miss,
                }
            }
            _ => Guess::Invalid,
        }
    }

    pub fn masked(&self) -> String {
        self.word
            .iter()
            .map(|c| if self.solved || self.tried.contains(c) { tr_upper_char(*c) } else { "\\_".to_owned() })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn wrong_count(&self) -> usize {
        self.wrong.len()
    }

    pub fn wrong_letters(&self) -> String {
        self.wrong.iter().map(|c| tr_upper_char(*c)).collect::<Vec<_>>().join(" ")
    }

    pub fn is_won(&self) -> bool {
        self.solved || self.word.iter().all(|c| self.tried.contains(c))
    }

    pub fn is_lost(&self) -> bool {
        !self.is_won() && self.wrong.len() >= MAX_WRONG
    }

    pub fn word(&self) -> String {
        self.word.iter().map(|c| tr_upper_char(*c)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_a_round() {
        let mut g = Hangman::new("kirpi");
        assert_eq!(g.guess("K"), Guess::Hit);
        assert_eq!(g.guess("k"), Guess::AlreadyTried);
        assert_eq!(g.guess("z"), Guess::Miss);
        assert_eq!(g.guess("I"), Guess::Miss); // I → ı, kirpi'de ı yok
        assert_eq!(g.guess("İ"), Guess::Hit); // İ → i
        assert_eq!(g.masked(), "K İ \\_ \\_ İ");
        assert_eq!(g.guess("12"), Guess::Invalid);
        assert_eq!(g.guess("kirpi"), Guess::Hit);
        assert!(g.is_won());
    }

    #[test]
    fn loses_after_six_mistakes() {
        let mut g = Hangman::new("ay");
        for c in ["b", "c", "d", "e", "f"] {
            g.guess(c);
        }
        assert!(!g.is_lost());
        assert_eq!(g.guess("zz"), Guess::Miss); // yanlış kelimedeki yeni harf de hata
        assert!(g.is_lost());
        assert_eq!(g.wrong_letters(), "B C D E F Z");
    }

    #[test]
    fn wrong_word_reveals_its_letters() {
        let mut g = Hangman::new("kaplan");
        assert_eq!(g.guess("kartal"), Guess::Hit); // k, a, l açılır; r, t hata
        assert_eq!(g.masked(), "K A \\_ L A \\_");
        assert_eq!(g.wrong_letters(), "R T");
        assert_eq!(g.guess("kartal"), Guess::AlreadyTried);
        assert_eq!(g.guess("kaplan"), Guess::Hit);
        assert!(g.is_won());
    }

    #[test]
    fn turkish_lowercase() {
        assert_eq!(tr_lower("IĞDIR İZMİR"), "ığdır izmir");
    }
}
