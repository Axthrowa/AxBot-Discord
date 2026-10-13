//! Balık tutma ve madencilik: `/çalış tür:` ile oynanır. Her denemede ağırlıklı rastgele bir şey çıkar
//! ve değeri cüzdana eklenir. Bekleme süresi Redis'te tutulur (normal `/çalış` beklemesinden ayrıdır).

use poise::serenity_prelude::{GuildId, UserId};
use redis::aio::ConnectionManager;
use sqlx::PgPool;

/// Aynı işi yeniden yapmadan önce beklenecek süre.
pub const COOLDOWN_SECS: u64 = 20 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum Job {
    #[name = "Mesai (saatte bir)"]
    Work,
    #[name = "Balık tut (20 dakikada bir)"]
    Fishing,
    #[name = "Madencilik (20 dakikada bir)"]
    Mining,
}

pub struct Find {
    pub emoji: &'static str,
    pub name: &'static str,
    pub min: i64,
    pub max: i64,
    /// Çıkma ağırlığı (büyük = sık).
    pub weight: u64,
}

const fn find(emoji: &'static str, name: &'static str, min: i64, max: i64, weight: u64) -> Find {
    Find { emoji, name, min, max, weight }
}

pub const FISH: &[Find] = &[
    find("🥾", "eski bir çizme", 0, 5, 12),
    find("🌿", "yosun", 1, 10, 10),
    find("🐟", "hamsi", 20, 60, 30),
    find("🐠", "çipura", 50, 120, 20),
    find("🐡", "levrek", 80, 180, 14),
    find("🦑", "kalamar", 150, 300, 8),
    find("🦈", "köpek balığı", 400, 800, 4),
    find("💎", "batık hazine sandığı", 1200, 2500, 2),
];

pub const ORES: &[Find] = &[
    find("🪨", "sıradan taş", 0, 8, 14),
    find("⚫", "kömür", 20, 70, 30),
    find("🟤", "bakır", 50, 130, 20),
    find("⚙️", "demir", 90, 200, 14),
    find("🥈", "gümüş", 180, 350, 8),
    find("🥇", "altın", 400, 850, 4),
    find("💎", "elmas", 1300, 2600, 2),
];

impl Job {
    fn key(self) -> &'static str {
        match self {
            Self::Work => "mesai",
            Self::Fishing => "balik",
            Self::Mining => "maden",
        }
    }

    pub fn table(self) -> &'static [Find] {
        match self {
            Self::Fishing => FISH,
            Self::Mining => ORES,
            Self::Work => &[],
        }
    }
}

/// `roll` sayısına (0 ≤ roll < toplam ağırlık) karşılık gelen buluntu.
pub fn pick(table: &'static [Find], roll: u64) -> Option<&'static Find> {
    let mut left = roll;
    for item in table {
        if left < item.weight {
            return Some(item);
        }
        left -= item.weight;
    }
    table.last()
}

pub fn total_weight(table: &[Find]) -> u64 {
    table.iter().map(|f| f.weight).sum()
}

pub enum Outcome {
    Found { find: &'static Find, amount: i64, wallet: i64 },
    /// Kalan bekleme (saniye).
    Wait(u64),
}

pub async fn gather(db: &PgPool, redis: &ConnectionManager, guild_id: GuildId, user: UserId, job: Job) -> anyhow::Result<Outcome> {
    let table = job.table();
    let key = format!("eko:is:{}:{guild_id}:{user}", job.key());
    let mut conn = redis.clone();
    // NX: bekleme yoksa yazılır; aynı anda iki komut iki kez kazandıramaz.
    let claimed: Option<String> = redis::cmd("SET").arg(&key).arg(1).arg("EX").arg(COOLDOWN_SECS).arg("NX").query_async(&mut conn).await?;
    if claimed.is_none() {
        let left: i64 = redis::cmd("TTL").arg(&key).query_async(&mut conn).await.unwrap_or(0);
        return Ok(Outcome::Wait(u64::try_from(left).unwrap_or(0)));
    }
    let Some(found) = pick(table, crate::rand_util::below(total_weight(table).max(1))) else {
        anyhow::bail!("iş tablosu boş");
    };
    let span = u64::try_from(found.max - found.min + 1).unwrap_or(1);
    let amount = found.min + i64::try_from(crate::rand_util::below(span)).unwrap_or(0);
    let wallet = super::add(db, guild_id, user, amount).await?;
    Ok(Outcome::Found { find: found, amount, wallet })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolls_map_to_weighted_finds() {
        for table in [FISH, ORES] {
            let total = total_weight(table);
            assert_eq!(pick(table, 0).map(|f| f.name), table.first().map(|f| f.name));
            assert_eq!(pick(table, total - 1).map(|f| f.name), table.last().map(|f| f.name));
            // Her buluntu en az bir zarla çıkabilir ve değer aralığı tutarlıdır.
            for item in table {
                assert!((0..total).any(|roll| pick(table, roll).is_some_and(|f| f.name == item.name)), "{}", item.name);
                assert!(item.min >= 0 && item.max >= item.min && item.weight > 0, "{}", item.name);
            }
        }
        assert!(pick(&[], 0).is_none());
    }

    #[test]
    fn rare_finds_are_rare_and_valuable() {
        for table in [FISH, ORES] {
            let (Some(common), Some(rare)) = (table.iter().max_by_key(|f| f.weight), table.last()) else {
                continue;
            };
            assert!(rare.weight < common.weight && rare.min > common.max);
        }
    }
}
