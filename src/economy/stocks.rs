//! Sahte borsa: hayali şirketlerin hisseleri sunucu parasıyla alınıp satılır.
//!
//! Fiyatlar saklanmaz; saatin numarasından hesaplanan, her saat değişen bir dalgalanmadır (aynı saatte
//! herkes aynı fiyatı görür, bot yeniden başlasa da değişmez). Gerçek parayla ya da gerçek şirketlerle
//! ilgisi yoktur.

use poise::serenity_prelude::{GuildId, UserId};
use sqlx::PgPool;

use crate::db::snowflake;

pub struct Stock {
    pub symbol: &'static str,
    pub name: &'static str,
    pub emoji: &'static str,
    /// Fiyatın etrafında dalgalandığı değer.
    pub base: i64,
    /// Saatlik oynaklık (yüzde).
    pub volatility: i64,
}

pub const STOCKS: &[Stock] = &[
    Stock { symbol: "AXT", name: "AxT Teknoloji", emoji: "💻", base: 500, volatility: 6 },
    Stock { symbol: "CAY", name: "Rize Çay Bahçeleri", emoji: "🍵", base: 120, volatility: 3 },
    Stock { symbol: "SIM", name: "Simit Sarayı", emoji: "🥯", base: 60, volatility: 4 },
    Stock { symbol: "UZY", name: "Uzay Madencilik", emoji: "🚀", base: 900, volatility: 12 },
    Stock { symbol: "KDI", name: "Kedi Maması A.Ş.", emoji: "🐈", base: 250, volatility: 5 },
    Stock { symbol: "DLM", name: "Dolmuş Hatları", emoji: "🚐", base: 180, volatility: 8 },
];

/// Tek işlemde ve toplamda tutulabilecek en fazla hisse.
pub const MAX_SHARES: i64 = 100_000;

pub fn find(symbol: &str) -> Option<&'static Stock> {
    let wanted = symbol.trim().to_uppercase();
    STOCKS.iter().find(|s| s.symbol == wanted)
}

/// Sayıyı iyice karıştıran küçük bir özet fonksiyonu (splitmix64).
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Hissenin `hour` numaralı saatteki fiyatı. Son 48 saatin küçük rastgele adımlarının toplamıdır; böylece
/// fiyat saatten saate az oynar ama günler içinde belirgin iniş çıkışlar olur. Tabanın %40'ı ile %250'si arasında kalır.
pub fn price_at(stock: &Stock, hour: i64) -> i64 {
    let seed = stock.symbol.bytes().fold(0_u64, |acc, b| acc.wrapping_mul(131).wrapping_add(u64::from(b)));
    let mut percent: i64 = 0;
    for step in 0..48 {
        let roll = mix(seed ^ (hour - step).cast_unsigned());
        // Her adım -volatility..=+volatility arasında bir yüzde.
        percent += i64::try_from(roll % (2 * stock.volatility.cast_unsigned() + 1)).unwrap_or(0) - stock.volatility;
    }
    // 48 adımın toplamı çok büyümesin diye yumuşatılır.
    let change = (percent * 100 / 4).clamp(-6000, 15_000);
    (stock.base * (10_000 + change) / 10_000).max(1)
}

pub fn hour_of(unix: i64) -> i64 {
    unix.div_euclid(3600)
}

/// Şimdiki fiyat ve bir önceki saate göre yüzde değişim.
pub fn quote(stock: &Stock, unix: i64) -> (i64, f64) {
    let hour = hour_of(unix);
    let (now, before) = (price_at(stock, hour), price_at(stock, hour - 1));
    (now, (now - before) as f64 * 100.0 / before as f64)
}

pub async fn holdings(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as("SELECT symbol, shares FROM stock_holdings WHERE guild_id = $1 AND user_id = $2 AND shares > 0 ORDER BY symbol")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .fetch_all(db)
        .await?)
}

pub enum Trade {
    /// İşlem yapıldı: toplam tutar, yeni cüzdan, elde kalan hisse.
    Done { total: i64, wallet: i64, shares: i64 },
    NotEnoughMoney,
    NotEnoughShares(i64),
    TooMany,
}

pub async fn buy(db: &PgPool, guild_id: GuildId, user: UserId, stock: &Stock, count: i64, unix: i64) -> anyhow::Result<Trade> {
    let (guild, member) = (snowflake(guild_id.get())?, snowflake(user.get())?);
    let owned: Option<i64> = sqlx::query_scalar("SELECT shares FROM stock_holdings WHERE guild_id = $1 AND user_id = $2 AND symbol = $3")
        .bind(guild)
        .bind(member)
        .bind(stock.symbol)
        .fetch_optional(db)
        .await?;
    if owned.unwrap_or(0) + count > MAX_SHARES {
        return Ok(Trade::TooMany);
    }
    let total = price_at(stock, hour_of(unix)) * count;
    // Para düşme ve hisse ekleme tek işlemde: ikincisi hata verirse para da geri gelir.
    let mut tx = db.begin().await?;
    // Para yetmiyorsa `spend` hiçbir şey düşmez.
    let Some(wallet) = super::spend(&mut *tx, guild_id, user, total).await? else {
        return Ok(Trade::NotEnoughMoney);
    };
    let shares: i64 = sqlx::query_scalar(
        "INSERT INTO stock_holdings (guild_id, user_id, symbol, shares) VALUES ($1, $2, $3, $4)
         ON CONFLICT (guild_id, user_id, symbol) DO UPDATE SET shares = stock_holdings.shares + EXCLUDED.shares RETURNING shares",
    )
    .bind(guild)
    .bind(member)
    .bind(stock.symbol)
    .bind(count)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Trade::Done { total, wallet, shares })
}

pub async fn sell(db: &PgPool, guild_id: GuildId, user: UserId, stock: &Stock, count: i64, unix: i64) -> anyhow::Result<Trade> {
    let (guild, member) = (snowflake(guild_id.get())?, snowflake(user.get())?);
    // Hisse düşme ve para ekleme tek işlemde: ikincisi hata verirse hisseler geri gelir.
    let mut tx = db.begin().await?;
    // Koşullu güncelleme: yeterli hisse yoksa satır değişmez (aynı anda iki satış fazladan hisse satamaz).
    let left: Option<i64> = sqlx::query_scalar(
        "UPDATE stock_holdings SET shares = shares - $4 WHERE guild_id = $1 AND user_id = $2 AND symbol = $3 AND shares >= $4 RETURNING shares",
    )
    .bind(guild)
    .bind(member)
    .bind(stock.symbol)
    .bind(count)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(shares) = left else {
        let owned: Option<i64> = sqlx::query_scalar("SELECT shares FROM stock_holdings WHERE guild_id = $1 AND user_id = $2 AND symbol = $3")
            .bind(guild)
            .bind(member)
            .bind(stock.symbol)
            .fetch_optional(db)
            .await?;
        return Ok(Trade::NotEnoughShares(owned.unwrap_or(0)));
    };
    let total = price_at(stock, hour_of(unix)) * count;
    let wallet = super::add(&mut *tx, guild_id, user, total).await?;
    tx.commit().await?;
    Ok(Trade::Done { total, wallet, shares })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Alış / satış tek işlemde: `cargo test live_stock_trades -- --ignored --nocapture` (sahte sunucu 1).
    #[tokio::test]
    #[ignore = "gerçek PostgreSQL gerektirir (docker compose up)"]
    async fn live_stock_trades() -> anyhow::Result<()> {
        let _ = dotenvy::dotenv();
        let config = crate::config::DatabaseConfig { url: std::env::var("DATABASE_URL")?, max_connections: 2 };
        let db = crate::db::connect(&config).await?;
        let (guild, user) = (GuildId::new(1), UserId::new(33));
        let clean = || async {
            sqlx::query("DELETE FROM economy_accounts WHERE guild_id = 1").execute(&db).await?;
            sqlx::query("DELETE FROM stock_holdings WHERE guild_id = 1").execute(&db).await?;
            anyhow::Ok(())
        };
        clean().await?;
        let stock = STOCKS.first().ok_or_else(|| anyhow::anyhow!("hisse yok"))?;
        let unix = 1_700_000_000;
        let price = price_at(stock, hour_of(unix));
        assert!(matches!(buy(&db, guild, user, stock, 1, unix).await?, Trade::NotEnoughMoney));
        super::super::add(&db, guild, user, price * 3).await?;
        assert!(matches!(buy(&db, guild, user, stock, 2, unix).await?, Trade::Done { shares: 2, .. }));
        assert!(matches!(sell(&db, guild, user, stock, 3, unix).await?, Trade::NotEnoughShares(2)));
        let Trade::Done { wallet, shares, .. } = sell(&db, guild, user, stock, 2, unix).await? else { anyhow::bail!("satış olmadı") };
        assert_eq!((wallet, shares), (price * 3, 0), "alıp aynı fiyattan satınca para yerine dönmeli");
        clean().await?;
        Ok(())
    }

    #[test]
    fn prices_are_stable_within_an_hour_and_bounded() {
        for stock in STOCKS {
            let mut changed = false;
            for hour in 480_000..480_500 {
                let price = price_at(stock, hour);
                assert_eq!(price, price_at(stock, hour), "{}", stock.symbol);
                assert!(price >= stock.base * 40 / 100 && price <= stock.base * 250 / 100, "{} {price}", stock.symbol);
                changed |= price != price_at(stock, hour + 1);
            }
            assert!(changed, "{} hiç değişmiyor", stock.symbol);
        }
    }

    #[test]
    fn hourly_moves_are_small() {
        // Saatten saate değişim, tek adımın en büyük etkisini (oynaklık × 2 / 4) geçmez.
        for stock in STOCKS {
            for hour in 480_000..480_300 {
                let (a, b) = (price_at(stock, hour), price_at(stock, hour + 1));
                let limit = stock.base * (stock.volatility * 2) / 400 + 2;
                assert!((a - b).abs() <= limit, "{} {a} → {b}", stock.symbol);
            }
        }
    }

    #[test]
    fn finds_symbols_case_insensitively() {
        assert_eq!(find(" axt ").map(|s| s.symbol), Some("AXT"));
        assert!(find("YOK").is_none());
        assert_eq!(hour_of(7199), 1);
        let mut seen = std::collections::HashSet::new();
        assert!(STOCKS.iter().all(|s| seen.insert(s.symbol)));
    }
}
