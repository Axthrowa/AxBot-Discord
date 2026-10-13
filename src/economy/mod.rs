//! Sunucu ekonomisi: cüzdan + banka, günlük ödül (seri bonuslu), çalışma, soygun, mağaza.
//!
//! Para hareketleri tek bir koşullu SQL ifadesiyle yapılır (`… WHERE wallet >= $n`); böylece
//! aynı anda gelen iki komut parayı eksiye düşüremez veya aynı ödülü iki kez veremez.

pub mod games;
pub mod gather;
pub mod stocks;

use std::time::Duration;

use anyhow::bail;
use chrono::{DateTime, Utc};
use poise::serenity_prelude::{GuildId, RoleId, UserId};
use sqlx::PgPool;

use crate::db::snowflake;
use crate::guild_config::to_id;
use crate::rand_util;

pub const CURRENCY: &str = "🪙";
pub const DAILY_COOLDOWN: Duration = Duration::from_secs(24 * 3600);
/// Bu süre içinde alınmazsa günlük seri sıfırlanır.
const STREAK_WINDOW: Duration = Duration::from_secs(48 * 3600);
pub const WORK_COOLDOWN: Duration = Duration::from_secs(3600);
pub const ROB_COOLDOWN: Duration = Duration::from_secs(2 * 3600);
pub const MIN_BET: u64 = 10;
pub const MAX_BET: u64 = 1_000_000;

const DAILY_BASE: i64 = 200;
const DAILY_STREAK_BONUS: i64 = 40;
const DAILY_MAX_STREAK_BONUS_DAYS: i64 = 7;
const WORK_MIN: u64 = 60;
const WORK_MAX: u64 = 180;
/// Soygun başarı ihtimali (%).
const ROB_SUCCESS_PERCENT: u64 = 45;
/// Hedefin cüzdanında en az bu kadar olmalı.
pub const ROB_MIN_TARGET: i64 = 100;

/// 1234567 → "1.234.567 🪙"
pub fn money(amount: i64) -> String {
    let digits = amount.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(ch);
    }
    let sign = if amount < 0 { "-" } else { "" };
    format!("{sign}{out} {CURRENCY}")
}

/// "500", "1.5k", "2m", "hepsi", "yarısı" → miktar. `available` "hepsi"/"yarısı" için kullanılır.
pub fn parse_amount(input: &str, available: i64) -> Result<i64, String> {
    let text = input.trim().to_lowercase().replace(' ', "");
    let invalid = || format!("`{input}` geçerli bir miktar değil (örn. 500, 1.5k, hepsi).");
    let amount = match text.as_str() {
        "hepsi" | "tümü" | "tamamı" | "all" => available,
        "yarısı" | "yarisi" | "half" => available / 2,
        _ => {
            // "1.5k" / "1,5k" → ondalık; "1.500" → binlik ayracı.
            let (number, multiplier) = match text.strip_suffix('k') {
                Some(n) => (n.replace(',', "."), 1_000.0),
                None => match text.strip_suffix('m') {
                    Some(n) => (n.replace(',', "."), 1_000_000.0),
                    None => (text.replace(['.', ','], ""), 1.0),
                },
            };
            let value: f64 = number.parse().map_err(|_| invalid())?;
            let value = (value * multiplier).floor();
            if !value.is_finite() || value < 0.0 || value > 1e15 {
                return Err("Bu miktar geçerli değil.".to_owned());
            }
            value as i64
        }
    };
    if amount <= 0 {
        return Err("Miktar 0'dan büyük olmalı.".to_owned());
    }
    Ok(amount)
}

#[derive(Debug, Clone, Default)]
pub struct Account {
    pub wallet: i64,
    pub bank: i64,
    pub last_daily: Option<DateTime<Utc>>,
    pub daily_streak: i32,
    pub last_work: Option<DateTime<Utc>>,
    pub last_rob: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct AccountRow {
    wallet: i64,
    bank: i64,
    last_daily: Option<DateTime<Utc>>,
    daily_streak: i32,
    last_work: Option<DateTime<Utc>>,
    last_rob: Option<DateTime<Utc>>,
}

fn ids(guild_id: GuildId, user: UserId) -> anyhow::Result<(i64, i64)> {
    Ok((snowflake(guild_id.get())?, snowflake(user.get())?))
}

pub async fn account(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<Account> {
    let (g, u) = ids(guild_id, user)?;
    let row: Option<AccountRow> = sqlx::query_as(
        "SELECT wallet, bank, last_daily, daily_streak, last_work, last_rob
         FROM economy_accounts WHERE guild_id = $1 AND user_id = $2",
    )
    .bind(g)
    .bind(u)
    .fetch_optional(db)
    .await?;
    Ok(row.map_or_else(Account::default, |r| Account {
        wallet: r.wallet,
        bank: r.bank,
        last_daily: r.last_daily,
        daily_streak: r.daily_streak,
        last_work: r.last_work,
        last_rob: r.last_rob,
    }))
}

async fn ensure(db: &PgPool, g: i64, u: i64) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO economy_accounts (guild_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(g)
        .bind(u)
        .execute(db)
        .await?;
    Ok(())
}

/// Cüzdana para ekler; yeni bakiyeyi döner.
pub async fn add(db: &PgPool, guild_id: GuildId, user: UserId, amount: i64) -> anyhow::Result<i64> {
    let (g, u) = ids(guild_id, user)?;
    let wallet: i64 = sqlx::query_scalar(
        "INSERT INTO economy_accounts (guild_id, user_id, wallet) VALUES ($1, $2, GREATEST($3, 0))
         ON CONFLICT (guild_id, user_id) DO UPDATE SET wallet = GREATEST(economy_accounts.wallet + $3, 0)
         RETURNING wallet",
    )
    .bind(g)
    .bind(u)
    .bind(amount)
    .fetch_one(db)
    .await?;
    Ok(wallet)
}

/// Cüzdandan harcar; yetmiyorsa `None`.
pub async fn spend(db: &PgPool, guild_id: GuildId, user: UserId, amount: i64) -> anyhow::Result<Option<i64>> {
    let (g, u) = ids(guild_id, user)?;
    Ok(sqlx::query_scalar(
        "UPDATE economy_accounts SET wallet = wallet - $3
         WHERE guild_id = $1 AND user_id = $2 AND wallet >= $3 RETURNING wallet",
    )
    .bind(g)
    .bind(u)
    .bind(amount)
    .fetch_optional(db)
    .await?)
}

/// Cüzdan ↔ banka. `to_bank` ise yatırır, değilse çeker. Yetmiyorsa `None`.
pub async fn move_bank(db: &PgPool, guild_id: GuildId, user: UserId, amount: i64, to_bank: bool) -> anyhow::Result<Option<(i64, i64)>> {
    let (g, u) = ids(guild_id, user)?;
    let sql = if to_bank {
        "UPDATE economy_accounts SET wallet = wallet - $3, bank = bank + $3
         WHERE guild_id = $1 AND user_id = $2 AND wallet >= $3 RETURNING wallet, bank"
    } else {
        "UPDATE economy_accounts SET wallet = wallet + $3, bank = bank - $3
         WHERE guild_id = $1 AND user_id = $2 AND bank >= $3 RETURNING wallet, bank"
    };
    Ok(sqlx::query_as(sql).bind(g).bind(u).bind(amount).fetch_optional(db).await?)
}

/// Bir üyeden diğerine para gönderir (tek işlem). Yetmiyorsa `false`.
pub async fn transfer(db: &PgPool, guild_id: GuildId, from: UserId, to: UserId, amount: i64) -> anyhow::Result<bool> {
    let (g, f) = ids(guild_id, from)?;
    let t = snowflake(to.get())?;
    let mut tx = db.begin().await?;
    let taken = sqlx::query(
        "UPDATE economy_accounts SET wallet = wallet - $3 WHERE guild_id = $1 AND user_id = $2 AND wallet >= $3",
    )
    .bind(g)
    .bind(f)
    .bind(amount)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    if !taken {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO economy_accounts (guild_id, user_id, wallet) VALUES ($1, $2, $3)
         ON CONFLICT (guild_id, user_id) DO UPDATE SET wallet = economy_accounts.wallet + $3",
    )
    .bind(g)
    .bind(t)
    .bind(amount)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Bekleme süresi bittiyse `None`, bitmediyse kalan süre.
pub fn remaining(last: Option<DateTime<Utc>>, cooldown: Duration, now: DateTime<Utc>) -> Option<Duration> {
    let last = last?;
    let elapsed = (now - last).to_std().unwrap_or(Duration::ZERO);
    (elapsed < cooldown).then(|| cooldown.saturating_sub(elapsed))
}

/// "3 sa 12 dk", "45 dk", "20 sn"
pub fn format_wait(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    match (h, m) {
        (0, 0) => format!("{s} sn"),
        (0, m) => format!("{m} dk"),
        (h, 0) => format!("{h} sa"),
        (h, m) => format!("{h} sa {m} dk"),
    }
}

pub enum Claim {
    Ok { amount: i64, wallet: i64, streak: i32 },
    Wait(Duration),
}

/// Günlük ödül: seri her gün artar (7 güne kadar bonus), 48 saat alınmazsa sıfırlanır.
pub async fn claim_daily(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<Claim> {
    let (g, u) = ids(guild_id, user)?;
    ensure(db, g, u).await?;
    let acc = account(db, guild_id, user).await?;
    let now = Utc::now();
    if let Some(wait) = remaining(acc.last_daily, DAILY_COOLDOWN, now) {
        return Ok(Claim::Wait(wait));
    }
    let streak = if remaining(acc.last_daily, STREAK_WINDOW, now).is_some() { acc.daily_streak + 1 } else { 1 };
    let amount = DAILY_BASE + DAILY_STREAK_BONUS * i64::from(streak - 1).min(DAILY_MAX_STREAK_BONUS_DAYS);
    // Aynı anda iki istek gelirse sadece biri geçer (last_daily değişmemiş olmalı).
    let wallet: Option<i64> = sqlx::query_scalar(
        "UPDATE economy_accounts SET wallet = wallet + $3, last_daily = $4, daily_streak = $5
         WHERE guild_id = $1 AND user_id = $2 AND last_daily IS NOT DISTINCT FROM $6 RETURNING wallet",
    )
    .bind(g)
    .bind(u)
    .bind(amount)
    .bind(now)
    .bind(streak)
    .bind(acc.last_daily)
    .fetch_optional(db)
    .await?;
    match wallet {
        Some(wallet) => Ok(Claim::Ok { amount, wallet, streak }),
        None => Ok(Claim::Wait(DAILY_COOLDOWN)),
    }
}

const JOBS: &[&str] = &[
    "Kafede garsonluk yaptın",
    "Komşunun bilgisayarını tamir ettin",
    "Kurye olarak paket dağıttın",
    "Bir Discord sunucusunda moderatörlük yaptın",
    "Pazarda domates sattın",
    "Bir web sitesi kodladın",
    "Simit sattın",
    "Köpek gezdirdin",
    "Düğünde DJ'lik yaptın",
    "Bir YouTube videosunun kurgusunu yaptın",
    "Taksi şoförlüğü yaptın",
    "Matematik özel dersi verdin",
];

pub enum Work {
    Ok { job: &'static str, amount: i64, wallet: i64 },
    Wait(Duration),
}

pub async fn work(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<Work> {
    let (g, u) = ids(guild_id, user)?;
    ensure(db, g, u).await?;
    let acc = account(db, guild_id, user).await?;
    let now = Utc::now();
    if let Some(wait) = remaining(acc.last_work, WORK_COOLDOWN, now) {
        return Ok(Work::Wait(wait));
    }
    let amount = i64::try_from(WORK_MIN + rand_util::below(WORK_MAX - WORK_MIN + 1))?;
    let job = JOBS
        .get(usize::try_from(rand_util::below(JOBS.len() as u64)).unwrap_or(0))
        .copied()
        .unwrap_or("Çalıştın");
    let wallet: Option<i64> = sqlx::query_scalar(
        "UPDATE economy_accounts SET wallet = wallet + $3, last_work = $4
         WHERE guild_id = $1 AND user_id = $2 AND last_work IS NOT DISTINCT FROM $5 RETURNING wallet",
    )
    .bind(g)
    .bind(u)
    .bind(amount)
    .bind(now)
    .bind(acc.last_work)
    .fetch_optional(db)
    .await?;
    match wallet {
        Some(wallet) => Ok(Work::Ok { job, amount, wallet }),
        None => Ok(Work::Wait(WORK_COOLDOWN)),
    }
}

pub enum Rob {
    Success(i64),
    /// Yakalandı; ödenen ceza.
    Caught(i64),
    Wait(Duration),
    TargetTooPoor,
    /// Soyan kişinin cezayı ödeyecek parası yok.
    RobberTooPoor,
}

/// Soygun: başarılıysa hedefin cüzdanının %10-30'u alınır; yakalanırsa kendi cüzdanından ceza ödenir.
pub async fn rob(db: &PgPool, guild_id: GuildId, robber: UserId, target: UserId) -> anyhow::Result<Rob> {
    let (g, r) = ids(guild_id, robber)?;
    let t = snowflake(target.get())?;
    ensure(db, g, r).await?;
    let me = account(db, guild_id, robber).await?;
    let now = Utc::now();
    if let Some(wait) = remaining(me.last_rob, ROB_COOLDOWN, now) {
        return Ok(Rob::Wait(wait));
    }
    if me.wallet < ROB_MIN_TARGET / 2 {
        return Ok(Rob::RobberTooPoor);
    }
    let victim = account(db, guild_id, target).await?;
    if victim.wallet < ROB_MIN_TARGET {
        return Ok(Rob::TargetTooPoor);
    }

    let mut tx = db.begin().await?;
    let started = sqlx::query(
        "UPDATE economy_accounts SET last_rob = $3 WHERE guild_id = $1 AND user_id = $2 AND last_rob IS NOT DISTINCT FROM $4",
    )
    .bind(g)
    .bind(r)
    .bind(now)
    .bind(me.last_rob)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    if !started {
        return Ok(Rob::Wait(ROB_COOLDOWN));
    }

    let result = if rand_util::below(100) < ROB_SUCCESS_PERCENT {
        let percent = i64::try_from(10 + rand_util::below(21))?; // %10-30
        // Hedefin satırı kilitlenir; bu arada başka bir işlem cüzdanı değiştiremez.
        let victim_wallet: Option<i64> =
            sqlx::query_scalar("SELECT wallet FROM economy_accounts WHERE guild_id = $1 AND user_id = $2 FOR UPDATE")
                .bind(g)
                .bind(t)
                .fetch_optional(&mut *tx)
                .await?;
        let stolen = victim_wallet.filter(|w| *w >= ROB_MIN_TARGET).map(|w| w * percent / 100);
        match stolen {
            Some(stolen) if stolen > 0 => {
                sqlx::query("UPDATE economy_accounts SET wallet = wallet - $3 WHERE guild_id = $1 AND user_id = $2")
                    .bind(g)
                    .bind(t)
                    .bind(stolen)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE economy_accounts SET wallet = wallet + $3 WHERE guild_id = $1 AND user_id = $2")
                    .bind(g)
                    .bind(r)
                    .bind(stolen)
                    .execute(&mut *tx)
                    .await?;
                Rob::Success(stolen)
            }
            _ => Rob::TargetTooPoor,
        }
    } else {
        // Ceza: kendi cüzdanının %10-20'si (en az 50), cüzdandan fazla olamaz.
        let percent = i64::try_from(10 + rand_util::below(11))?;
        let fine = (me.wallet * percent / 100).max(50).min(me.wallet);
        sqlx::query(
            "UPDATE economy_accounts SET wallet = GREATEST(wallet - $3, 0) WHERE guild_id = $1 AND user_id = $2",
        )
        .bind(g)
        .bind(r)
        .bind(fine)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO economy_accounts (guild_id, user_id, wallet) VALUES ($1, $2, $3)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET wallet = economy_accounts.wallet + $3",
        )
        .bind(g)
        .bind(t)
        .bind(fine)
        .execute(&mut *tx)
        .await?;
        Rob::Caught(fine)
    };
    tx.commit().await?;
    Ok(result)
}

/// (üye, toplam) — cüzdan + banka.
pub async fn leaderboard(db: &PgPool, guild_id: GuildId, limit: i64) -> anyhow::Result<Vec<(UserId, i64)>> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT user_id, wallet + bank AS total FROM economy_accounts
         WHERE guild_id = $1 AND wallet + bank > 0 ORDER BY total DESC LIMIT $2",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(u, total)| to_id(Some(u)).map(|u| (UserId::new(u), total)))
        .collect())
}

/// Yöneticinin bakiyeyi sıfırlaması.
pub async fn reset(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<()> {
    let (g, u) = ids(guild_id, user)?;
    sqlx::query("UPDATE economy_accounts SET wallet = 0, bank = 0 WHERE guild_id = $1 AND user_id = $2")
        .bind(g)
        .bind(u)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM economy_inventory WHERE guild_id = $1 AND user_id = $2")
        .bind(g)
        .bind(u)
        .execute(db)
        .await?;
    Ok(())
}

// ===================== Mağaza =====================

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ShopItem {
    pub id: i64,
    pub name: String,
    pub price: i64,
    pub role_id: Option<i64>,
    pub description: Option<String>,
}

impl ShopItem {
    pub fn role(&self) -> Option<RoleId> {
        to_id(self.role_id).map(RoleId::new)
    }
}

pub const MAX_SHOP_ITEMS: i64 = 25;

pub async fn shop(db: &PgPool, guild_id: GuildId) -> anyhow::Result<Vec<ShopItem>> {
    Ok(sqlx::query_as("SELECT id, name, price, role_id, description FROM shop_items WHERE guild_id = $1 ORDER BY price, name")
        .bind(snowflake(guild_id.get())?)
        .fetch_all(db)
        .await?)
}

pub async fn find_item(db: &PgPool, guild_id: GuildId, name: &str) -> anyhow::Result<Option<ShopItem>> {
    Ok(sqlx::query_as(
        "SELECT id, name, price, role_id, description FROM shop_items WHERE guild_id = $1 AND lower(name) = lower($2)",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(name.trim())
    .fetch_optional(db)
    .await?)
}

pub async fn add_item(db: &PgPool, guild_id: GuildId, name: &str, price: i64, role: Option<RoleId>, description: Option<&str>) -> anyhow::Result<()> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shop_items WHERE guild_id = $1")
        .bind(snowflake(guild_id.get())?)
        .fetch_one(db)
        .await?;
    if count >= MAX_SHOP_ITEMS {
        bail!("mağazada en fazla {MAX_SHOP_ITEMS} ürün olabilir");
    }
    sqlx::query(
        "INSERT INTO shop_items (guild_id, name, price, role_id, description) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (guild_id, lower(name)) DO UPDATE
             SET price = EXCLUDED.price, role_id = EXCLUDED.role_id, description = EXCLUDED.description",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(name.trim())
    .bind(price)
    .bind(role.map(|r| snowflake(r.get())).transpose()?)
    .bind(description)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn remove_item(db: &PgPool, guild_id: GuildId, name: &str) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM shop_items WHERE guild_id = $1 AND lower(name) = lower($2)")
        .bind(snowflake(guild_id.get())?)
        .bind(name.trim())
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn add_to_inventory(db: &PgPool, guild_id: GuildId, user: UserId, item_id: i64) -> anyhow::Result<()> {
    let (g, u) = ids(guild_id, user)?;
    sqlx::query(
        "INSERT INTO economy_inventory (guild_id, user_id, item_id, quantity) VALUES ($1, $2, $3, 1)
         ON CONFLICT (guild_id, user_id, item_id) DO UPDATE SET quantity = economy_inventory.quantity + 1",
    )
    .bind(g)
    .bind(u)
    .bind(item_id)
    .execute(db)
    .await?;
    Ok(())
}

/// (ürün adı, adet)
pub async fn inventory(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<Vec<(String, i32)>> {
    let (g, u) = ids(guild_id, user)?;
    Ok(sqlx::query_as(
        "SELECT s.name, i.quantity FROM economy_inventory i JOIN shop_items s ON s.id = i.item_id
         WHERE i.guild_id = $1 AND i.user_id = $2 ORDER BY s.name",
    )
    .bind(g)
    .bind(u)
    .fetch_all(db)
    .await?)
}

/// Gerçek PostgreSQL ile para akışı (Discord'a bağlanmaz; sahte sunucu ID'si 1 kullanılır). Elle:
/// `cargo test live_economy_flow -- --ignored --nocapture`
#[cfg(test)]
mod live_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "gerçek PostgreSQL gerektirir (docker compose up)"]
    async fn live_economy_flow() -> anyhow::Result<()> {
        let _ = dotenvy::dotenv();
        let url = std::env::var("DATABASE_URL")?;
        let config = crate::config::DatabaseConfig { url, max_connections: 2 };
        let db = crate::db::connect(&config).await?;
        let guild = GuildId::new(1);
        let (a, b) = (UserId::new(11), UserId::new(22));
        let clean = || async {
            sqlx::query("DELETE FROM economy_accounts WHERE guild_id = 1").execute(&db).await?;
            sqlx::query("DELETE FROM shop_items WHERE guild_id = 1").execute(&db).await?;
            anyhow::Ok(())
        };
        clean().await?;

        // Günlük: ilk alım 200, hemen tekrar alınamaz.
        let Claim::Ok { amount, wallet, streak } = claim_daily(&db, guild, a).await? else {
            anyhow::bail!("ilk günlük alınamadı");
        };
        assert_eq!((amount, wallet, streak), (DAILY_BASE, DAILY_BASE, 1));
        assert!(matches!(claim_daily(&db, guild, a).await?, Claim::Wait(_)));

        // Aynı anda iki günlük isteği: sadece biri geçmeli.
        sqlx::query("UPDATE economy_accounts SET last_daily = now() - interval '25 hours' WHERE guild_id = 1 AND user_id = 11")
            .execute(&db)
            .await?;
        let (x, y) = tokio::join!(claim_daily(&db, guild, a), claim_daily(&db, guild, a));
        let granted = [x?, y?].iter().filter(|c| matches!(c, Claim::Ok { .. })).count();
        assert_eq!(granted, 1, "günlük iki kez verildi");
        let acc = account(&db, guild, a).await?;
        assert_eq!(acc.daily_streak, 2);
        assert_eq!(acc.wallet, DAILY_BASE * 2 + DAILY_STREAK_BONUS);

        // Banka ve harcama sınırları.
        assert_eq!(move_bank(&db, guild, a, 100, true).await?, Some((acc.wallet - 100, 100)));
        assert_eq!(move_bank(&db, guild, a, 1_000_000, true).await?, None);
        assert_eq!(spend(&db, guild, a, 1_000_000).await?, None);

        // Transfer: yeni hesaba gider, fazlası reddedilir.
        assert!(transfer(&db, guild, a, b, 50).await?);
        assert!(!transfer(&db, guild, a, b, 1_000_000).await?);
        assert_eq!(account(&db, guild, b).await?.wallet, 50);

        // Çalış: bir kez, sonra bekleme.
        assert!(matches!(work(&db, guild, b).await?, Work::Ok { .. }));
        assert!(matches!(work(&db, guild, b).await?, Work::Wait(_)));

        // Soygun: toplam para korunur (sadece el değiştirir).
        add(&db, guild, b, 1000).await?;
        let before = account(&db, guild, a).await?.wallet + account(&db, guild, b).await?.wallet;
        let result = rob(&db, guild, a, b).await?;
        let after = account(&db, guild, a).await?.wallet + account(&db, guild, b).await?.wallet;
        assert_eq!(before, after, "soygunda para yoktan var oldu veya kayboldu");
        println!("soygun sonucu: {}", match result {
            Rob::Success(n) => format!("başarılı +{n}"),
            Rob::Caught(n) => format!("yakalandı -{n}"),
            _ => "başka".to_owned(),
        });
        assert!(matches!(rob(&db, guild, a, b).await?, Rob::Wait(_)));

        // Cüzdan eksiye düşmez.
        assert_eq!(add(&db, guild, b, -1_000_000).await?, 0);

        // Mağaza: aynı adla ekleme günceller, envantere eklenir.
        add_item(&db, guild, "Çay", 25, None, Some("Demli")).await?;
        add_item(&db, guild, "çay", 30, None, None).await?;
        let items = shop(&db, guild).await?;
        assert_eq!(items.len(), 1);
        let tea = find_item(&db, guild, "ÇAY").await?;
        let tea_id = tea.as_ref().map(|t| t.id).ok_or_else(|| anyhow::anyhow!("ürün yok"))?;
        assert_eq!(tea.map(|t| t.price), Some(30));
        add_to_inventory(&db, guild, a, tea_id).await?;
        add_to_inventory(&db, guild, a, tea_id).await?;
        assert_eq!(inventory(&db, guild, a).await?, vec![("Çay".to_owned(), 2)]);

        let top = leaderboard(&db, guild, 10).await?;
        assert_eq!(top.first().map(|(u, _)| *u), Some(a));

        clean().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_money_turkish_style() {
        assert_eq!(money(0), "0 🪙");
        assert_eq!(money(999), "999 🪙");
        assert_eq!(money(1_000), "1.000 🪙");
        assert_eq!(money(1_234_567), "1.234.567 🪙");
        assert_eq!(money(-1_500), "-1.500 🪙");
    }

    #[test]
    fn parses_amounts() {
        assert_eq!(parse_amount("500", 0), Ok(500));
        assert_eq!(parse_amount("1.500", 0), Ok(1500));
        assert_eq!(parse_amount("1,5k", 0), Ok(1500));
        assert_eq!(parse_amount("2m", 0), Ok(2_000_000));
        assert_eq!(parse_amount("hepsi", 750), Ok(750));
        assert_eq!(parse_amount("yarısı", 751), Ok(375));
        assert!(parse_amount("0", 0).is_err());
        assert!(parse_amount("hepsi", 0).is_err());
        assert!(parse_amount("-5", 0).is_err());
        assert!(parse_amount("elma", 0).is_err());
    }

    #[test]
    fn cooldowns() {
        let now = Utc::now();
        assert_eq!(remaining(None, WORK_COOLDOWN, now), None);
        let recent = now - chrono::Duration::minutes(20);
        let left = remaining(Some(recent), WORK_COOLDOWN, now).map(|d| d.as_secs() / 60);
        assert_eq!(left, Some(40));
        let old = now - chrono::Duration::hours(2);
        assert_eq!(remaining(Some(old), WORK_COOLDOWN, now), None);
    }

    #[test]
    fn formats_waits() {
        assert_eq!(format_wait(Duration::from_secs(20)), "20 sn");
        assert_eq!(format_wait(Duration::from_secs(45 * 60)), "45 dk");
        assert_eq!(format_wait(Duration::from_secs(3 * 3600)), "3 sa");
        assert_eq!(format_wait(Duration::from_secs(3 * 3600 + 12 * 60)), "3 sa 12 dk");
    }
}
