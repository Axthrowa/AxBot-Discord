//! Statbot analitik toplayıcısı ve toplu PostgreSQL yazıcısı.
//!
//! Mesaj, ses ve üye akışı sıcak yollarında doğrudan veritabanına gidilmez; sayaçlar
//! Redis hash'lerinde biriktirilir ve 60 saniyede bir `flush` göreviyle tek toplu
//! `UNNEST` sorgusuyla PostgreSQL tablolarına aktarılır.

pub mod voice;

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context as _;
use chrono::NaiveDate;
use poise::serenity_prelude::{ChannelId, GuildId, UserId};
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use tracing::{debug, warn};

pub use voice::spawn_voice_tracker;

use crate::db::snowflake;

/// Kirli analitik kayıtlarının Redis anahtarları.
const DIRTY_SERVERS: &str = "stat:dirty:s";
const DIRTY_CHANNELS: &str = "stat:dirty:c";
const DIRTY_MEMBERS: &str = "stat:dirty:m";
const DIRTY_VOICE: &str = "stat:dirty:mv";

const FLUSH_BATCH: usize = 500;
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(60);
const STAT_EXPIRE_SECS: u64 = 30 * 86_400;

/// Atomik hash oku ve sil: deltalari güvenle çeker.
const POP_HASH_SCRIPT: &str = r"
local val = redis.call('HGETALL', KEYS[1])
if #val > 0 then
    redis.call('DEL', KEYS[1])
end
return val
";

/// Türkiye saatiyle (UTC+3) bugünün tarihi.
pub fn today() -> NaiveDate {
    let local = chrono::Utc::now() + chrono::Duration::hours(3);
    local.date_naive()
}

pub fn server_key(guild_id: GuildId, date: NaiveDate) -> String {
    format!("stat:s:{guild_id}:{date}")
}

pub fn channel_key(guild_id: GuildId, channel_id: ChannelId, date: NaiveDate) -> String {
    format!("stat:c:{guild_id}:{channel_id}:{date}")
}

pub fn member_key(guild_id: GuildId, user_id: UserId, date: NaiveDate) -> String {
    format!("stat:m:{guild_id}:{user_id}:{date}")
}

pub fn voice_key(guild_id: GuildId, user_id: UserId) -> String {
    format!("stat:mv:{guild_id}:{user_id}")
}

/// Mesaj sayısını Redis tamponuna ekler (mesaj sıcak yolunda sıfır-DB).
pub async fn record_message(
    redis: &ConnectionManager,
    guild_id: GuildId,
    channel_id: ChannelId,
    user_id: UserId,
) {
    let date = today();
    let s_key = server_key(guild_id, date);
    let c_key = channel_key(guild_id, channel_id, date);
    let m_key = member_key(guild_id, user_id, date);
    let mut conn = redis.clone();

    let _: redis::RedisResult<()> = redis::pipe()
        .cmd("HINCRBY").arg(&s_key).arg("messages").arg(1).ignore()
        .cmd("EXPIRE").arg(&s_key).arg(STAT_EXPIRE_SECS).ignore()
        .cmd("SADD").arg(DIRTY_SERVERS).arg(format!("{guild_id}:{date}")).ignore()
        .cmd("HINCRBY").arg(&c_key).arg("messages").arg(1).ignore()
        .cmd("HSETNX").arg(&c_key).arg("is_voice").arg(0).ignore()
        .cmd("EXPIRE").arg(&c_key).arg(STAT_EXPIRE_SECS).ignore()
        .cmd("SADD").arg(DIRTY_CHANNELS).arg(format!("{guild_id}:{channel_id}:{date}")).ignore()
        .cmd("HINCRBY").arg(&m_key).arg("messages").arg(1).ignore()
        .cmd("EXPIRE").arg(&m_key).arg(STAT_EXPIRE_SECS).ignore()
        .cmd("SADD").arg(DIRTY_MEMBERS).arg(format!("{guild_id}:{user_id}:{date}")).ignore()
        .query_async(&mut conn)
        .await;
}

/// Yeni üye katılımını Redis tamponuna ekler.
pub async fn record_member_join(redis: &ConnectionManager, guild_id: GuildId) {
    let date = today();
    let s_key = server_key(guild_id, date);
    let mut conn = redis.clone();

    let _: redis::RedisResult<()> = redis::pipe()
        .cmd("HINCRBY").arg(&s_key).arg("joined").arg(1).ignore()
        .cmd("EXPIRE").arg(&s_key).arg(STAT_EXPIRE_SECS).ignore()
        .cmd("SADD").arg(DIRTY_SERVERS).arg(format!("{guild_id}:{date}")).ignore()
        .query_async(&mut conn)
        .await;
}

/// Üye ayrılışını Redis tamponuna ekler.
pub async fn record_member_leave(redis: &ConnectionManager, guild_id: GuildId) {
    let date = today();
    let s_key = server_key(guild_id, date);
    let mut conn = redis.clone();

    let _: redis::RedisResult<()> = redis::pipe()
        .cmd("HINCRBY").arg(&s_key).arg("left").arg(1).ignore()
        .cmd("EXPIRE").arg(&s_key).arg(STAT_EXPIRE_SECS).ignore()
        .cmd("SADD").arg(DIRTY_SERVERS).arg(format!("{guild_id}:{date}")).ignore()
        .query_async(&mut conn)
        .await;
}

/// Bir dakikalık ses oturumunu (60 sn ve kazanılan XP) Redis tamponuna işler.
pub async fn record_voice_minute(
    redis: &ConnectionManager,
    guild_id: GuildId,
    channel_id: ChannelId,
    user_id: UserId,
    xp_earned: u64,
) {
    let date = today();
    let s_key = server_key(guild_id, date);
    let c_key = channel_key(guild_id, channel_id, date);
    let m_key = member_key(guild_id, user_id, date);
    let mv_key = voice_key(guild_id, user_id);
    let mut conn = redis.clone();

    let _: redis::RedisResult<()> = redis::pipe()
        .cmd("HINCRBY").arg(&s_key).arg("voice_secs").arg(60).ignore()
        .cmd("EXPIRE").arg(&s_key).arg(STAT_EXPIRE_SECS).ignore()
        .cmd("SADD").arg(DIRTY_SERVERS).arg(format!("{guild_id}:{date}")).ignore()
        .cmd("HINCRBY").arg(&c_key).arg("voice_secs").arg(60).ignore()
        .cmd("HSET").arg(&c_key).arg("is_voice").arg(1).ignore()
        .cmd("EXPIRE").arg(&c_key).arg(STAT_EXPIRE_SECS).ignore()
        .cmd("SADD").arg(DIRTY_CHANNELS).arg(format!("{guild_id}:{channel_id}:{date}")).ignore()
        .cmd("HINCRBY").arg(&m_key).arg("voice_secs").arg(60).ignore()
        .cmd("HINCRBY").arg(&m_key).arg("voice_xp").arg(xp_earned).ignore()
        .cmd("EXPIRE").arg(&m_key).arg(STAT_EXPIRE_SECS).ignore()
        .cmd("SADD").arg(DIRTY_MEMBERS).arg(format!("{guild_id}:{user_id}:{date}")).ignore()
        .cmd("HINCRBY").arg(&mv_key).arg("voice_secs").arg(60).ignore()
        .cmd("HINCRBY").arg(&mv_key).arg("voice_xp").arg(xp_earned).ignore()
        .cmd("SADD").arg(DIRTY_VOICE).arg(format!("{guild_id}:{user_id}")).ignore()
        .query_async(&mut conn)
        .await;
}

/// Tüm kirli analitik sayaçlarını PostgreSQL'e toplu olarak aktarır.
pub async fn flush(db: &PgPool, redis: &ConnectionManager) -> anyhow::Result<usize> {
    let script = redis::Script::new(POP_HASH_SCRIPT);
    let mut written = 0;

    written += flush_servers(db, redis, &script).await?;
    written += flush_channels(db, redis, &script).await?;
    written += flush_members(db, redis, &script).await?;
    written += flush_member_voice(db, redis, &script).await?;

    Ok(written)
}

async fn flush_servers(
    db: &PgPool,
    redis: &ConnectionManager,
    script: &redis::Script,
) -> anyhow::Result<usize> {
    let mut conn = redis.clone();
    let mut written = 0;

    loop {
        let members: Vec<String> = redis::cmd("SPOP")
            .arg(DIRTY_SERVERS)
            .arg(FLUSH_BATCH)
            .query_async(&mut conn)
            .await?;
        if members.is_empty() {
            break;
        }

        let mut items = Vec::with_capacity(members.len());
        for m in &members {
            let Some((guild, date)) = parse_dirty_server(m) else { continue };
            let key = server_key(GuildId::new(guild), date);
            let raw: HashMap<String, i64> = script.key(&key).invoke_async(&mut conn).await.unwrap_or_default();
            if !raw.is_empty() {
                items.push((m.clone(), guild, date, raw));
            }
        }

        if items.is_empty() {
            if members.len() < FLUSH_BATCH {
                break;
            }
            continue;
        }

        let mut guilds = Vec::with_capacity(items.len());
        let mut dates = Vec::with_capacity(items.len());
        let mut messages = Vec::with_capacity(items.len());
        let mut voice_secs = Vec::with_capacity(items.len());
        let mut joined = Vec::with_capacity(items.len());
        let mut left = Vec::with_capacity(items.len());

        for (_, g, d, hash) in &items {
            guilds.push(snowflake(*g)?);
            dates.push(*d);
            messages.push(hash.get("messages").copied().unwrap_or(0));
            voice_secs.push(hash.get("voice_secs").copied().unwrap_or(0));
            joined.push(i32::try_from(hash.get("joined").copied().unwrap_or(0)).unwrap_or(0));
            left.push(i32::try_from(hash.get("left").copied().unwrap_or(0)).unwrap_or(0));
        }

        let result = sqlx::query(
            "INSERT INTO server_daily_stats (guild_id, stat_date, messages_count, voice_seconds, members_joined, members_left)
             SELECT * FROM UNNEST($1::BIGINT[], $2::DATE[], $3::BIGINT[], $4::BIGINT[], $5::INT[], $6::INT[])
             ON CONFLICT (guild_id, stat_date) DO UPDATE SET
                 messages_count = server_daily_stats.messages_count + EXCLUDED.messages_count,
                 voice_seconds = server_daily_stats.voice_seconds + EXCLUDED.voice_seconds,
                 members_joined = server_daily_stats.members_joined + EXCLUDED.members_joined,
                 members_left = server_daily_stats.members_left + EXCLUDED.members_left",
        )
        .bind(&guilds)
        .bind(&dates)
        .bind(&messages)
        .bind(&voice_secs)
        .bind(&joined)
        .bind(&left)
        .execute(db)
        .await;

        if let Err(e) = result {
            // Hata durumunda deltalari geri koy
            for (m, g, d, hash) in &items {
                let key = server_key(GuildId::new(*g), *d);
                let mut pipe = redis::pipe();
                for (k, v) in hash {
                    pipe.cmd("HINCRBY").arg(&key).arg(k).arg(*v).ignore();
                }
                pipe.cmd("SADD").arg(DIRTY_SERVERS).arg(m).ignore();
                let _: redis::RedisResult<()> = pipe.query_async(&mut conn).await;
            }
            return Err(e).context("server_daily_stats PostgreSQL'e yazılamadı");
        }

        written += items.len();
        if members.len() < FLUSH_BATCH {
            break;
        }
    }

    Ok(written)
}

async fn flush_channels(
    db: &PgPool,
    redis: &ConnectionManager,
    script: &redis::Script,
) -> anyhow::Result<usize> {
    let mut conn = redis.clone();
    let mut written = 0;

    loop {
        let members: Vec<String> = redis::cmd("SPOP")
            .arg(DIRTY_CHANNELS)
            .arg(FLUSH_BATCH)
            .query_async(&mut conn)
            .await?;
        if members.is_empty() {
            break;
        }

        let mut items = Vec::with_capacity(members.len());
        for m in &members {
            let Some((guild, channel, date)) = parse_dirty_channel(m) else { continue };
            let key = channel_key(GuildId::new(guild), ChannelId::new(channel), date);
            let raw: HashMap<String, i64> = script.key(&key).invoke_async(&mut conn).await.unwrap_or_default();
            if !raw.is_empty() {
                items.push((m.clone(), guild, channel, date, raw));
            }
        }

        if items.is_empty() {
            if members.len() < FLUSH_BATCH {
                break;
            }
            continue;
        }

        let mut guilds = Vec::with_capacity(items.len());
        let mut channels = Vec::with_capacity(items.len());
        let mut dates = Vec::with_capacity(items.len());
        let mut is_voice = Vec::with_capacity(items.len());
        let mut messages = Vec::with_capacity(items.len());
        let mut voice_secs = Vec::with_capacity(items.len());

        for (_, g, c, d, hash) in &items {
            guilds.push(snowflake(*g)?);
            channels.push(snowflake(*c)?);
            dates.push(*d);
            is_voice.push(hash.get("is_voice").copied().unwrap_or(0) > 0);
            messages.push(hash.get("messages").copied().unwrap_or(0));
            voice_secs.push(hash.get("voice_secs").copied().unwrap_or(0));
        }

        let result = sqlx::query(
            "INSERT INTO channel_daily_stats (guild_id, channel_id, stat_date, is_voice, messages_count, voice_seconds)
             SELECT * FROM UNNEST($1::BIGINT[], $2::BIGINT[], $3::DATE[], $4::BOOLEAN[], $5::BIGINT[], $6::BIGINT[])
             ON CONFLICT (guild_id, channel_id, stat_date) DO UPDATE SET
                 is_voice = EXCLUDED.is_voice OR channel_daily_stats.is_voice,
                 messages_count = channel_daily_stats.messages_count + EXCLUDED.messages_count,
                 voice_seconds = channel_daily_stats.voice_seconds + EXCLUDED.voice_seconds",
        )
        .bind(&guilds)
        .bind(&channels)
        .bind(&dates)
        .bind(&is_voice)
        .bind(&messages)
        .bind(&voice_secs)
        .execute(db)
        .await;

        if let Err(e) = result {
            for (m, g, c, d, hash) in &items {
                let key = channel_key(GuildId::new(*g), ChannelId::new(*c), *d);
                let mut pipe = redis::pipe();
                for (k, v) in hash {
                    pipe.cmd("HINCRBY").arg(&key).arg(k).arg(*v).ignore();
                }
                pipe.cmd("SADD").arg(DIRTY_CHANNELS).arg(m).ignore();
                let _: redis::RedisResult<()> = pipe.query_async(&mut conn).await;
            }
            return Err(e).context("channel_daily_stats PostgreSQL'e yazılamadı");
        }

        written += items.len();
        if members.len() < FLUSH_BATCH {
            break;
        }
    }

    Ok(written)
}

async fn flush_members(
    db: &PgPool,
    redis: &ConnectionManager,
    script: &redis::Script,
) -> anyhow::Result<usize> {
    let mut conn = redis.clone();
    let mut written = 0;

    loop {
        let members: Vec<String> = redis::cmd("SPOP")
            .arg(DIRTY_MEMBERS)
            .arg(FLUSH_BATCH)
            .query_async(&mut conn)
            .await?;
        if members.is_empty() {
            break;
        }

        let mut items = Vec::with_capacity(members.len());
        for m in &members {
            let Some((guild, user, date)) = parse_dirty_member(m) else { continue };
            let key = member_key(GuildId::new(guild), UserId::new(user), date);
            let raw: HashMap<String, i64> = script.key(&key).invoke_async(&mut conn).await.unwrap_or_default();
            if !raw.is_empty() {
                items.push((m.clone(), guild, user, date, raw));
            }
        }

        if items.is_empty() {
            if members.len() < FLUSH_BATCH {
                break;
            }
            continue;
        }

        let mut guilds = Vec::with_capacity(items.len());
        let mut users = Vec::with_capacity(items.len());
        let mut dates = Vec::with_capacity(items.len());
        let mut messages = Vec::with_capacity(items.len());
        let mut voice_secs = Vec::with_capacity(items.len());
        let mut voice_xp = Vec::with_capacity(items.len());

        for (_, g, u, d, hash) in &items {
            guilds.push(snowflake(*g)?);
            users.push(snowflake(*u)?);
            dates.push(*d);
            messages.push(i32::try_from(hash.get("messages").copied().unwrap_or(0)).unwrap_or(0));
            voice_secs.push(i32::try_from(hash.get("voice_secs").copied().unwrap_or(0)).unwrap_or(0));
            voice_xp.push(hash.get("voice_xp").copied().unwrap_or(0));
        }

        let result = sqlx::query(
            "INSERT INTO member_daily_stats (guild_id, user_id, stat_date, messages_count, voice_seconds, voice_xp_earned)
             SELECT * FROM UNNEST($1::BIGINT[], $2::BIGINT[], $3::DATE[], $4::INT[], $5::INT[], $6::BIGINT[])
             ON CONFLICT (guild_id, user_id, stat_date) DO UPDATE SET
                 messages_count = member_daily_stats.messages_count + EXCLUDED.messages_count,
                 voice_seconds = member_daily_stats.voice_seconds + EXCLUDED.voice_seconds,
                 voice_xp_earned = member_daily_stats.voice_xp_earned + EXCLUDED.voice_xp_earned",
        )
        .bind(&guilds)
        .bind(&users)
        .bind(&dates)
        .bind(&messages)
        .bind(&voice_secs)
        .bind(&voice_xp)
        .execute(db)
        .await;

        if let Err(e) = result {
            for (m, g, u, d, hash) in &items {
                let key = member_key(GuildId::new(*g), UserId::new(*u), *d);
                let mut pipe = redis::pipe();
                for (k, v) in hash {
                    pipe.cmd("HINCRBY").arg(&key).arg(k).arg(*v).ignore();
                }
                pipe.cmd("SADD").arg(DIRTY_MEMBERS).arg(m).ignore();
                let _: redis::RedisResult<()> = pipe.query_async(&mut conn).await;
            }
            return Err(e).context("member_daily_stats PostgreSQL'e yazılamadı");
        }

        written += items.len();
        if members.len() < FLUSH_BATCH {
            break;
        }
    }

    Ok(written)
}

async fn flush_member_voice(
    db: &PgPool,
    redis: &ConnectionManager,
    script: &redis::Script,
) -> anyhow::Result<usize> {
    let mut conn = redis.clone();
    let mut written = 0;

    loop {
        let members: Vec<String> = redis::cmd("SPOP")
            .arg(DIRTY_VOICE)
            .arg(FLUSH_BATCH)
            .query_async(&mut conn)
            .await?;
        if members.is_empty() {
            break;
        }

        let mut items = Vec::with_capacity(members.len());
        for m in &members {
            let Some((guild, user)) = parse_dirty_voice(m) else { continue };
            let key = voice_key(GuildId::new(guild), UserId::new(user));
            let raw: HashMap<String, i64> = script.key(&key).invoke_async(&mut conn).await.unwrap_or_default();
            if !raw.is_empty() {
                items.push((m.clone(), guild, user, raw));
            }
        }

        if items.is_empty() {
            if members.len() < FLUSH_BATCH {
                break;
            }
            continue;
        }

        let mut guilds = Vec::with_capacity(items.len());
        let mut users = Vec::with_capacity(items.len());
        let mut voice_secs = Vec::with_capacity(items.len());
        let mut voice_xp = Vec::with_capacity(items.len());

        for (_, g, u, hash) in &items {
            guilds.push(snowflake(*g)?);
            users.push(snowflake(*u)?);
            voice_secs.push(hash.get("voice_secs").copied().unwrap_or(0));
            voice_xp.push(hash.get("voice_xp").copied().unwrap_or(0));
        }

        let result = sqlx::query(
            "INSERT INTO member_voice_stats (guild_id, user_id, total_voice_seconds, total_voice_xp)
             SELECT * FROM UNNEST($1::BIGINT[], $2::BIGINT[], $3::BIGINT[], $4::BIGINT[])
             ON CONFLICT (guild_id, user_id) DO UPDATE SET
                 total_voice_seconds = member_voice_stats.total_voice_seconds + EXCLUDED.total_voice_seconds,
                 total_voice_xp = member_voice_stats.total_voice_xp + EXCLUDED.total_voice_xp,
                 updated_at = now()",
        )
        .bind(&guilds)
        .bind(&users)
        .bind(&voice_secs)
        .bind(&voice_xp)
        .execute(db)
        .await;

        if let Err(e) = result {
            for (m, g, u, hash) in &items {
                let key = voice_key(GuildId::new(*g), UserId::new(*u));
                let mut pipe = redis::pipe();
                for (k, v) in hash {
                    pipe.cmd("HINCRBY").arg(&key).arg(k).arg(*v).ignore();
                }
                pipe.cmd("SADD").arg(DIRTY_VOICE).arg(m).ignore();
                let _: redis::RedisResult<()> = pipe.query_async(&mut conn).await;
            }
            return Err(e).context("member_voice_stats PostgreSQL'e yazılamadı");
        }

        written += items.len();
        if members.len() < FLUSH_BATCH {
            break;
        }
    }

    Ok(written)
}

/// 90 günden eski günlük analitik kayıtlarını temizler.
pub async fn purge_old_stats(db: &PgPool) -> anyhow::Result<u64> {
    let mut total = 0;
    total += sqlx::query("DELETE FROM server_daily_stats WHERE stat_date < CURRENT_DATE - INTERVAL '90 days'")
        .execute(db)
        .await?
        .rows_affected();
    total += sqlx::query("DELETE FROM channel_daily_stats WHERE stat_date < CURRENT_DATE - INTERVAL '90 days'")
        .execute(db)
        .await?
        .rows_affected();
    total += sqlx::query("DELETE FROM member_daily_stats WHERE stat_date < CURRENT_DATE - INTERVAL '90 days'")
        .execute(db)
        .await?
        .rows_affected();
    Ok(total)
}

/// Periyodik olarak `flush` çalıştıran arka plan görevini başlatır.
pub fn spawn_flusher(db: PgPool, redis: ConnectionManager) {
    tokio::spawn(async move {
        // Açılışta diğer servislerin yükünü yaymak için kısa bir gecikme
        tokio::time::sleep(Duration::from_secs(20)).await;
        let mut interval = tokio::time::interval(FLUSH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            crate::supervise::round("analitik yazıcısı", Duration::from_secs(120), async {
                match flush(&db, &redis).await {
                    Ok(0) => {}
                    Ok(n) => debug!(records = n, "Analitik verileri PostgreSQL'e yazıldı"),
                    Err(e) => warn!(error = ?e, "Analitik flush başarısız, sonraki turda tekrar denenecek"),
                }
            })
            .await;
        }
    });
}

pub fn parse_dirty_server(raw: &str) -> Option<(u64, NaiveDate)> {
    let (g, d) = raw.split_once(':')?;
    let guild = g.parse::<u64>().ok()?;
    let date = NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()?;
    Some((guild, date))
}

pub fn parse_dirty_channel(raw: &str) -> Option<(u64, u64, NaiveDate)> {
    let mut parts = raw.split(':');
    let guild = parts.next()?.parse::<u64>().ok()?;
    let channel = parts.next()?.parse::<u64>().ok()?;
    let date_str = parts.next()?;
    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()?;
    Some((guild, channel, date))
}

pub fn parse_dirty_member(raw: &str) -> Option<(u64, u64, NaiveDate)> {
    let mut parts = raw.split(':');
    let guild = parts.next()?.parse::<u64>().ok()?;
    let user = parts.next()?.parse::<u64>().ok()?;
    let date_str = parts.next()?;
    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()?;
    Some((guild, user, date))
}

pub fn parse_dirty_voice(raw: &str) -> Option<(u64, u64)> {
    let (g, u) = raw.split_once(':')?;
    let guild = g.parse::<u64>().ok()?;
    let user = u.parse::<u64>().ok()?;
    Some((guild, user))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dirty_keys_correctly() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap_or_default();

        assert_eq!(parse_dirty_server("123:2026-11-21"), Some((123, date)));
        assert_eq!(parse_dirty_server("invalid"), None);
        assert_eq!(parse_dirty_server("123:not-a-date"), None);

        assert_eq!(parse_dirty_channel("123:456:2026-11-21"), Some((123, 456, date)));
        assert_eq!(parse_dirty_channel("123:456"), None);

        assert_eq!(parse_dirty_member("123:789:2026-11-21"), Some((123, 789, date)));
        assert_eq!(parse_dirty_member("123:789:bad-date"), None);

        assert_eq!(parse_dirty_voice("123:789"), Some((123, 789)));
        assert_eq!(parse_dirty_voice("bad"), None);
    }

    #[test]
    fn key_builders_produce_expected_formats() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap_or_default();
        let g = GuildId::new(10);
        let c = ChannelId::new(20);
        let u = UserId::new(30);

        assert_eq!(server_key(g, date), "stat:s:10:2026-11-21");
        assert_eq!(channel_key(g, c, date), "stat:c:10:20:2026-11-21");
        assert_eq!(member_key(g, u, date), "stat:m:10:30:2026-11-21");
        assert_eq!(voice_key(g, u), "stat:mv:10:30");
    }
}
