//! "10dk", "1sa30dk", "2g", "1h" gibi süre ifadelerinin ayrıştırılması.

use std::time::Duration;

/// Discord'un izin verdiği en uzun timeout süresi.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(28 * 24 * 60 * 60);

/// Birden fazla parça birleştirilebilir: "1g12sa", "1h 30m".
/// Birimler: `sn`/`s` saniye, `dk`/`m` dakika, `sa`/`h` saat, `g`/`d` gün, `hf`/`w` hafta.
pub fn parse(input: &str) -> Result<Duration, String> {
    let compact: String = input
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    if compact.is_empty() {
        return Err("süre boş olamaz".to_owned());
    }

    let mut total: u64 = 0;
    let mut rest = compact.as_str();

    while !rest.is_empty() {
        let digits_end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        if digits_end == 0 {
            return Err(format!("`{rest}` bir sayıyla başlamalı (örn. 10dk)"));
        }
        let (number, after) = rest.split_at(digits_end);
        let value: u64 = number
            .parse()
            .map_err(|_| format!("`{number}` çok büyük bir sayı"))?;

        let unit_end = after.find(|c: char| c.is_ascii_digit()).unwrap_or(after.len());
        let (unit, next) = after.split_at(unit_end);
        let seconds_per_unit = match unit {
            "s" | "sn" | "saniye" => 1,
            "m" | "dk" | "dakika" => 60,
            "h" | "sa" | "saat" => 60 * 60,
            "d" | "g" | "gün" | "gun" => 24 * 60 * 60,
            "w" | "hf" | "hafta" => 7 * 24 * 60 * 60,
            "" => return Err(format!("`{number}` için birim eksik (örn. {number}dk)")),
            other => return Err(format!("bilinmeyen birim `{other}` (sn, dk, sa, g, hf)")),
        };

        total = value
            .checked_mul(seconds_per_unit)
            .and_then(|s| total.checked_add(s))
            .ok_or_else(|| "süre çok uzun".to_owned())?;
        rest = next;
    }

    if total == 0 {
        return Err("süre sıfır olamaz".to_owned());
    }
    Ok(Duration::from_secs(total))
}

/// İnsan tarafından okunabilir Türkçe süre: "1 gün 2 saat 5 dakika".
pub fn format(d: Duration) -> String {
    let secs = d.as_secs();
    let parts = [
        (secs / 86_400, "gün"),
        (secs % 86_400 / 3_600, "saat"),
        (secs % 3_600 / 60, "dakika"),
        (secs % 60, "saniye"),
    ];
    let text = parts
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, unit)| format!("{n} {unit}"))
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        "0 saniye".to_owned()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_turkish_and_english_units() -> Result<(), String> {
        assert_eq!(parse("10dk")?, Duration::from_secs(600));
        assert_eq!(parse("10m")?, Duration::from_secs(600));
        assert_eq!(parse("1sa30dk")?, Duration::from_secs(5400));
        assert_eq!(parse("1h 30m")?, Duration::from_secs(5400));
        assert_eq!(parse("2g")?, Duration::from_secs(172_800));
        assert_eq!(parse("1hf")?, Duration::from_secs(604_800));
        assert_eq!(parse("45SN")?, Duration::from_secs(45));
        Ok(())
    }

    #[test]
    fn rejects_invalid_input() {
        for bad in ["", "abc", "10", "10x", "0dk", "dk10", "99999999999999999999g"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn formats_human_readable() {
        assert_eq!(format(Duration::from_secs(600)), "10 dakika");
        assert_eq!(format(Duration::from_secs(93_900)), "1 gün 2 saat 5 dakika");
    }
}
