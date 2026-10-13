//! Profil sistemi: itibar puanı (+rep, günde bir) ve evlilik.
//!
//! Profil kartının kendisi `/kartım profil` komutunda derlenir (seviye, bakiye, başarım, itibar, eş).

use poise::serenity_prelude::{GuildId, UserId};
use redis::aio::ConnectionManager;
use sqlx::PgPool;

use crate::db::snowflake;
use crate::guild_config::to_id;

/// Bir üye günde bir kez itibar verebilir.
const REP_COOLDOWN_SECS: u64 = 24 * 3600;

pub async fn reputation(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<i64> {
    let rep: Option<i64> = sqlx::query_scalar("SELECT rep FROM member_profiles WHERE guild_id = $1 AND user_id = $2")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .fetch_optional(db)
        .await?;
    Ok(rep.unwrap_or(0))
}

pub enum RepOutcome {
    /// Verildi; alanın yeni itibarı.
    Given(i64),
    /// Bekleme süresi dolmadı; kalan saniye.
    Wait(u64),
}

/// `from`, `to` üyesine bir itibar puanı verir (günde bir kez; bekleme Redis'te).
pub async fn give_rep(db: &PgPool, redis: &ConnectionManager, guild_id: GuildId, from: UserId, to: UserId) -> anyhow::Result<RepOutcome> {
    let key = format!("rep:{guild_id}:{from}");
    let mut conn = redis.clone();
    // NX: yalnızca bekleme yoksa yazılır; böylece aynı anda iki komut iki puan veremez.
    let claimed: Option<String> = redis::cmd("SET").arg(&key).arg(1).arg("EX").arg(REP_COOLDOWN_SECS).arg("NX").query_async(&mut conn).await?;
    if claimed.is_none() {
        let left: i64 = redis::cmd("TTL").arg(&key).query_async(&mut conn).await.unwrap_or(0);
        return Ok(RepOutcome::Wait(u64::try_from(left).unwrap_or(0)));
    }
    let rep: i64 = sqlx::query_scalar(
        "INSERT INTO member_profiles (guild_id, user_id, rep) VALUES ($1, $2, 1)
         ON CONFLICT (guild_id, user_id) DO UPDATE SET rep = member_profiles.rep + 1 RETURNING rep",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(to.get())?)
    .fetch_one(db)
    .await?;
    Ok(RepOutcome::Given(rep))
}

/// Üyenin eşi ve evlilik tarihi (Unix saniyesi).
pub async fn spouse(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<Option<(UserId, i64)>> {
    let row: Option<(i64, i64, i64)> = sqlx::query_as(
        "SELECT user_a, user_b, EXTRACT(EPOCH FROM married_at)::BIGINT FROM marriages WHERE guild_id = $1 AND (user_a = $2 OR user_b = $2)",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(user.get())?)
    .fetch_optional(db)
    .await?;
    Ok(row.and_then(|(a, b, since)| {
        let other = if to_id(Some(a)) == Some(user.get()) { b } else { a };
        to_id(Some(other)).map(|id| (UserId::new(id), since))
    }))
}

/// İki üyeyi evlendirir. Biri zaten evliyse `false` (aynı anda gelen iki teklif çift evlilik oluşturamaz).
pub async fn marry(db: &PgPool, guild_id: GuildId, a: UserId, b: UserId) -> anyhow::Result<bool> {
    let (guild, a, b) = (snowflake(guild_id.get())?, snowflake(a.get())?, snowflake(b.get())?);
    let inserted = sqlx::query(
        "INSERT INTO marriages (guild_id, user_a, user_b)
         SELECT $1, $2, $3 WHERE NOT EXISTS (SELECT 1 FROM marriages WHERE guild_id = $1 AND (user_a IN ($2, $3) OR user_b IN ($2, $3)))",
    )
    .bind(guild)
    .bind(a)
    .bind(b)
    .execute(db)
    .await?;
    Ok(inserted.rows_affected() > 0)
}

/// Evliliği bitirir; evli değildiyse `false`.
pub async fn divorce(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<bool> {
    let deleted = sqlx::query("DELETE FROM marriages WHERE guild_id = $1 AND (user_a = $2 OR user_b = $2)")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .execute(db)
        .await?;
    Ok(deleted.rows_affected() > 0)
}
