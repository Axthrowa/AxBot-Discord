//! Seviye eğrisi (MEE6 ile aynı): seviye `l`'den `l+1`'e geçmek için `5l² + 50l + 100` XP gerekir.

/// Seviye `level`'den bir sonrakine geçmek için gereken XP.
pub fn xp_for_next(level: u32) -> u64 {
    let l = u64::from(level);
    5 * l * l + 50 * l + 100
}

/// Toplam XP'den seviye ve mevcut seviyedeki ilerleme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub level: u32,
    /// Mevcut seviyeye girildikten sonra kazanılan XP.
    pub current: u64,
    /// Bir sonraki seviye için gereken toplam (mevcut seviyedeki) XP.
    pub needed: u64,
}

pub fn progress(total_xp: u64) -> Progress {
    let mut level = 0;
    let mut remaining = total_xp;
    loop {
        let needed = xp_for_next(level);
        if remaining < needed {
            return Progress {
                level,
                current: remaining,
                needed,
            };
        }
        remaining -= needed;
        level += 1;
    }
}

pub fn level(total_xp: u64) -> u32 {
    progress(total_xp).level
}

/// `▰▰▰▱▱▱▱▱▱▱` biçiminde ilerleme çubuğu.
pub fn bar(p: Progress, width: usize) -> String {
    let filled = if p.needed == 0 {
        0
    } else {
        let ratio = p.current as f64 / p.needed as f64;
        ((ratio * width as f64).round() as usize).min(width)
    };
    format!("{}{}", "▰".repeat(filled), "▱".repeat(width - filled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_mee6_curve() {
        assert_eq!(xp_for_next(0), 100);
        assert_eq!(xp_for_next(1), 155);
        assert_eq!(xp_for_next(10), 1100);
    }

    #[test]
    fn progress_crosses_levels_correctly() {
        assert_eq!(progress(0), Progress { level: 0, current: 0, needed: 100 });
        assert_eq!(progress(99).level, 0);
        assert_eq!(progress(100), Progress { level: 1, current: 0, needed: 155 });
        assert_eq!(progress(254).level, 1);
        assert_eq!(progress(255).level, 2);
    }

    #[test]
    fn bar_is_bounded() {
        let p = Progress { level: 1, current: 50, needed: 100 };
        assert_eq!(bar(p, 10), "▰▰▰▰▰▱▱▱▱▱");
        let full = Progress { level: 1, current: 100, needed: 100 };
        assert_eq!(bar(full, 4), "▰▰▰▰");
    }
}
