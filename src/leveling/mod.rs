//! Redis tabanlı seviye/XP sistemi.
//!
//! - Canlı XP Redis'te bir sorted set'tedir (`xp:{guild}` → üye → XP). Mesaj başına tek bir
//!   atomik Lua script çağrısı: cooldown kontrolü + XP artırma + "kirli" işaretleme.
//!   PostgreSQL'e mesaj başına hiç gidilmez.
//! - Arka plan görevi kirli kayıtları periyodik olarak toplu (tek sorgu) PostgreSQL'e yazar.
//! - Redis boşalırsa (ör. veri silindi) ilk kullanımda PostgreSQL'den geri doldurulur.

pub mod formula;
pub mod settings;

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::BuildHasher;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use dashmap::{DashMap, DashSet};
use poise::serenity_prelude::{ChannelId, GuildId, RoleId, UserId};
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use tracing::{debug, info, warn};

use crate::db::snowflake;

/// Bir üye en fazla bu aralıkla XP kazanır (spam ile XP kasılmasın).
const COOLDOWN_SECS: u64 = 60;
const XP_MIN: u64 = 15;
const XP_MAX: u64 = 25;

/// PostgreSQL'e yazılmayı bekleyen "guild:user" kayıtları.
const DIRTY_KEY: &str = "xp:dirty";
const FLUSH_BATCH: usize = 500;
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(30);

/// KEYS: 1=cooldown, 2=xp sorted set, 3=kirli set, 4=haftalık sıralama, 5=aylık sıralama
/// ARGV: 1=cooldown sn, 2=kazanılan XP, 3=üye, 4=kirli kayıt, 5=dönem anahtarlarının ömrü (sn)
/// Cooldown'daysa nil, değilse yeni toplam XP döner.
const AWARD_SCRIPT: &str = r"
if not redis.call('SET', KEYS[1], '1', 'EX', ARGV[1], 'NX') then
    return false
end
local total = redis.call('ZINCRBY', KEYS[2], ARGV[2], ARGV[3])
redis.call('SADD', KEYS[3], ARGV[4])
redis.call('ZINCRBY', KEYS[4], ARGV[2], ARGV[3])
redis.call('EXPIRE', KEYS[4], ARGV[5])
redis.call('ZINCRBY', KEYS[5], ARGV[2], ARGV[3])
redis.call('EXPIRE', KEYS[5], ARGV[5])
return total
";

/// Dönemlik sıralama anahtarları 70 gün sonra kendiliğinden silinir.
const PERIOD_TTL_SECS: u64 = 70 * 24 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum Period {
    #[name = "Bu hafta"]
    Week,
    #[name = "Bu ay"]
    Month,
}

/// Dönemin Redis anahtarı. Haftalar Pazartesi başlar; hafta ve ay Türkiye saatiyle (UTC+3) hesaplanır.
pub fn period_key(guild_id: GuildId, period: Period, unix: i64) -> String {
    let local = unix + 3 * 3600;
    match period {
        // 1970-01-01 Perşembe'ydi: gün sayısına 3 eklenince haftalar Pazartesi'den başlar.
        Period::Week => format!("xp:hafta:{guild_id}:{}", (local.div_euclid(86_400) + 3).div_euclid(7)),
        Period::Month => {
            let month = chrono::DateTime::from_timestamp(local, 0).map_or_else(String::new, |d| d.format("%Y%m").to_string());
            format!("xp:ay:{guild_id}:{month}")
        }
    }
}

fn xp_key(guild_id: u64) -> String {
    format!("xp:{guild_id}")
}

fn loaded_key(guild_id: GuildId) -> String {
    format!("xp:loaded:{guild_id}")
}

#[derive(Debug, Clone, Copy)]
pub struct LevelUp {
    pub old: u32,
    pub new: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Reward {
    pub level: u32,
    pub role_id: RoleId,
}

pub struct Rank {
    pub total_xp: u64,
    /// 1'den başlayan sıra; hiç XP'si yoksa `None`.
    pub position: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusionKind {
    Channel,
    Role,
}

impl ExclusionKind {
    fn db(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Role => "role",
        }
    }
}

/// XP kazandırmayan kanal ve roller (ID → tür). Mesaj başına okunduğu için bellekte tutulur.
pub type Exclusions = HashMap<u64, ExclusionKind>;

pub struct Leveling {
    award: redis::Script,
    /// Bu süreçte Redis'e yüklendiği doğrulanmış sunucular.
    loaded: DashSet<GuildId>,
    rewards: DashMap<GuildId, Arc<Vec<Reward>>>,
    exclusions: DashMap<GuildId, Arc<Exclusions>>,
}

impl Leveling {
    pub fn new() -> Self {
        Self {
            award: redis::Script::new(AWARD_SCRIPT),
            loaded: DashSet::new(),
            rewards: DashMap::new(),
            exclusions: DashMap::new(),
        }
    }

    /// Mesaj başına çağrılır. Seviye atladıysa bilgisini döner.
    pub async fn on_message(
        &self,
        db: &PgPool,
        redis: &ConnectionManager,
        guild_id: GuildId,
        user_id: UserId,
        rate_percent: u16,
    ) -> anyhow::Result<Option<LevelUp>> {
        self.ensure_loaded(db, redis, guild_id).await?;

        let amount = scaled_xp(random_xp(), rate_percent);
        let now = poise::serenity_prelude::Timestamp::now().unix_timestamp();
        let mut conn = redis.clone();
        let total: Option<String> = self
            .award
            .key(format!("xp:cd:{guild_id}:{user_id}"))
            .key(xp_key(guild_id.get()))
            .key(DIRTY_KEY)
            .key(period_key(guild_id, Period::Week, now))
            .key(period_key(guild_id, Period::Month, now))
            .arg(COOLDOWN_SECS)
            .arg(amount)
            .arg(user_id.get())
            .arg(format!("{guild_id}:{user_id}"))
            .arg(PERIOD_TTL_SECS)
            .invoke_async(&mut conn)
            .await?;

        let Some(total) = total else {
            return Ok(None); // cooldown
        };
        let total = parse_score(&total)?;
        let old = formula::level(total.saturating_sub(amount));
        let new = formula::level(total);

        Ok((new > old).then_some(LevelUp { old, new }))
    }

    pub async fn rank(
        &self,
        db: &PgPool,
        redis: &ConnectionManager,
        guild_id: GuildId,
        user_id: UserId,
    ) -> anyhow::Result<Rank> {
        self.ensure_loaded(db, redis, guild_id).await?;

        let key = xp_key(guild_id.get());
        let mut conn = redis.clone();
        let (score, rank): (Option<f64>, Option<u64>) = redis::pipe()
            .cmd("ZSCORE")
            .arg(&key)
            .arg(user_id.get())
            .cmd("ZREVRANK")
            .arg(&key)
            .arg(user_id.get())
            .query_async(&mut conn)
            .await?;

        Ok(Rank {
            total_xp: score.map_or(0, score_to_xp),
            position: rank.map(|r| r + 1),
        })
    }

    pub async fn top(
        &self,
        db: &PgPool,
        redis: &ConnectionManager,
        guild_id: GuildId,
        count: usize,
    ) -> anyhow::Result<Vec<(UserId, u64)>> {
        self.ensure_loaded(db, redis, guild_id).await?;

        let mut conn = redis.clone();
        let rows: Vec<(u64, f64)> = redis::cmd("ZREVRANGE")
            .arg(xp_key(guild_id.get()))
            .arg(0)
            .arg(count.saturating_sub(1))
            .arg("WITHSCORES")
            .query_async(&mut conn)
            .await?;

        Ok(rows
            .into_iter()
            .filter(|(id, _)| *id != 0)
            .map(|(id, score)| (UserId::new(id), score_to_xp(score)))
            .collect())
    }

    /// Bu haftanın / ayın sıralaması (yalnızca o dönemde mesajla kazanılan XP).
    pub async fn top_period(&self, redis: &ConnectionManager, guild_id: GuildId, period: Period, count: usize) -> anyhow::Result<Vec<(UserId, u64)>> {
        let mut conn = redis.clone();
        let rows: Vec<(u64, f64)> = redis::cmd("ZREVRANGE")
            .arg(period_key(guild_id, period, poise::serenity_prelude::Timestamp::now().unix_timestamp()))
            .arg(0)
            .arg(count.saturating_sub(1))
            .arg("WITHSCORES")
            .query_async(&mut conn)
            .await?;
        Ok(rows.into_iter().filter(|(id, _)| *id != 0).map(|(id, score)| (UserId::new(id), score_to_xp(score))).collect())
    }

    /// Bir üyenin XP'sini `delta` kadar değiştirir (eksi olabilir; sıfırın altına inmez).
    /// `(önceki toplam, yeni toplam)` döner.
    pub async fn add_xp(
        &self,
        db: &PgPool,
        redis: &ConnectionManager,
        guild_id: GuildId,
        user_id: UserId,
        delta: i64,
    ) -> anyhow::Result<(u64, u64)> {
        self.ensure_loaded(db, redis, guild_id).await?;
        let key = xp_key(guild_id.get());
        let mut conn = redis.clone();
        let current: Option<f64> = redis::cmd("ZSCORE").arg(&key).arg(user_id.get()).query_async(&mut conn).await?;
        let old = current.map_or(0, score_to_xp);
        let new = if delta >= 0 {
            old.saturating_add(delta.unsigned_abs())
        } else {
            old.saturating_sub(delta.unsigned_abs())
        };
        let _: () = redis::pipe()
            .cmd("ZADD")
            .arg(&key)
            .arg(new)
            .arg(user_id.get())
            .ignore()
            .cmd("SADD")
            .arg(DIRTY_KEY)
            .arg(format!("{guild_id}:{user_id}"))
            .ignore()
            .query_async(&mut conn)
            .await?;
        Ok((old, new))
    }

    /// Redis'te bu sunucunun XP verisi yoksa PostgreSQL'den yükler.
    /// `xp:loaded` bayrağı Redis'le birlikte yaşar: Redis silinirse bayrak da gider ve yeniden yüklenir.
    async fn ensure_loaded(
        &self,
        db: &PgPool,
        redis: &ConnectionManager,
        guild_id: GuildId,
    ) -> anyhow::Result<()> {
        if self.loaded.contains(&guild_id) {
            return Ok(());
        }

        let mut conn = redis.clone();
        let claimed: Option<String> = redis::cmd("SET")
            .arg(loaded_key(guild_id))
            .arg(1)
            .arg("NX")
            .query_async(&mut conn)
            .await?;

        if claimed.is_some() {
            if let Err(e) = hydrate(db, &mut conn, guild_id).await {
                // Bayrağı geri al ki bir sonraki mesajda tekrar denensin.
                let _: redis::RedisResult<()> =
                    redis::cmd("DEL").arg(loaded_key(guild_id)).query_async(&mut conn).await;
                return Err(e);
            }
        }

        self.loaded.insert(guild_id);
        Ok(())
    }

    pub async fn rewards(&self, db: &PgPool, guild_id: GuildId) -> anyhow::Result<Arc<Vec<Reward>>> {
        if let Some(rewards) = self.rewards.get(&guild_id) {
            return Ok(Arc::clone(rewards.value()));
        }

        let rows: Vec<(i32, i64)> = sqlx::query_as(
            "SELECT level, role_id FROM level_rewards WHERE guild_id = $1 ORDER BY level",
        )
        .bind(snowflake(guild_id.get())?)
        .fetch_all(db)
        .await?;

        let rewards: Vec<Reward> = rows
            .into_iter()
            .filter_map(|(level, role)| {
                let level = u32::try_from(level).ok()?;
                let role = u64::try_from(role).ok().filter(|r| *r != 0)?;
                Some(Reward {
                    level,
                    role_id: RoleId::new(role),
                })
            })
            .collect();

        let rewards = Arc::new(rewards);
        self.rewards.insert(guild_id, Arc::clone(&rewards));
        Ok(rewards)
    }

    pub async fn set_reward(
        &self,
        db: &PgPool,
        guild_id: GuildId,
        level: u32,
        role_id: RoleId,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO level_rewards (guild_id, level, role_id) VALUES ($1, $2, $3)
             ON CONFLICT (guild_id, level) DO UPDATE SET role_id = EXCLUDED.role_id",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(i32::try_from(level)?)
        .bind(snowflake(role_id.get())?)
        .execute(db)
        .await?;
        self.rewards.remove(&guild_id);
        Ok(())
    }

    /// Ödül silindiyse `true`.
    pub async fn remove_reward(&self, db: &PgPool, guild_id: GuildId, level: u32) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM level_rewards WHERE guild_id = $1 AND level = $2")
            .bind(snowflake(guild_id.get())?)
            .bind(i32::try_from(level)?)
            .execute(db)
            .await?;
        self.rewards.remove(&guild_id);
        Ok(result.rows_affected() > 0)
    }

    pub async fn exclusions(&self, db: &PgPool, guild_id: GuildId) -> anyhow::Result<Arc<Exclusions>> {
        if let Some(found) = self.exclusions.get(&guild_id) {
            return Ok(Arc::clone(found.value()));
        }
        let rows: Vec<(i64, String)> = sqlx::query_as("SELECT target_id, kind FROM xp_exclusions WHERE guild_id = $1")
            .bind(snowflake(guild_id.get())?)
            .fetch_all(db)
            .await?;
        let map: Exclusions = rows
            .into_iter()
            .filter_map(|(id, kind)| {
                let id = u64::try_from(id).ok().filter(|id| *id != 0)?;
                let kind = if kind == "role" { ExclusionKind::Role } else { ExclusionKind::Channel };
                Some((id, kind))
            })
            .collect();
        let map = Arc::new(map);
        self.exclusions.insert(guild_id, Arc::clone(&map));
        Ok(map)
    }

    /// Mesaj bu kanalda/kategoride veya bu rollerle XP kazandırmıyorsa `true`.
    pub async fn is_excluded(
        &self,
        db: &PgPool,
        guild_id: GuildId,
        channels: &[ChannelId],
        roles: &[RoleId],
        settings: &settings::LevelSettings,
    ) -> anyhow::Result<bool> {
        let exclusions = self.exclusions(db, guild_id).await?;
        Ok(excluded(&exclusions, channels, roles, settings.channels_allow_only, settings.roles_allow_only))
    }

    /// XP listesini (kanallar + roller) verilenle değiştirir.
    pub async fn set_exclusions(&self, db: &PgPool, guild_id: GuildId, channels: &[u64], roles: &[u64]) -> anyhow::Result<()> {
        let gid = snowflake(guild_id.get())?;
        let mut targets = Vec::with_capacity(channels.len() + roles.len());
        let mut kinds = Vec::with_capacity(channels.len() + roles.len());
        for (list, kind) in [(channels, ExclusionKind::Channel), (roles, ExclusionKind::Role)] {
            for id in list {
                targets.push(snowflake(*id)?);
                kinds.push(kind.db().to_owned());
            }
        }
        let mut tx = db.begin().await?;
        sqlx::query("DELETE FROM xp_exclusions WHERE guild_id = $1").bind(gid).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO xp_exclusions (guild_id, target_id, kind)
             SELECT $1, * FROM UNNEST($2::BIGINT[], $3::TEXT[]) ON CONFLICT DO NOTHING",
        )
        .bind(gid)
        .bind(&targets)
        .bind(&kinds)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.exclusions.remove(&guild_id);
        Ok(())
    }

    /// Muafiyeti açar/kapatır; eklendiyse `true`, kaldırıldıysa `false`.
    pub async fn toggle_exclusion(&self, db: &PgPool, guild_id: GuildId, target: u64, kind: ExclusionKind) -> anyhow::Result<bool> {
        let removed = sqlx::query("DELETE FROM xp_exclusions WHERE guild_id = $1 AND target_id = $2")
            .bind(snowflake(guild_id.get())?)
            .bind(snowflake(target)?)
            .execute(db)
            .await?
            .rows_affected()
            > 0;
        if !removed {
            sqlx::query("INSERT INTO xp_exclusions (guild_id, target_id, kind) VALUES ($1, $2, $3)")
                .bind(snowflake(guild_id.get())?)
                .bind(snowflake(target)?)
                .bind(kind.db())
                .execute(db)
                .await?;
        }
        self.exclusions.remove(&guild_id);
        Ok(!removed)
    }
}

async fn hydrate(db: &PgPool, conn: &mut ConnectionManager, guild_id: GuildId) -> anyhow::Result<()> {
    let rows: Vec<(i64, i64)> = sqlx::query_as("SELECT user_id, xp FROM member_xp WHERE guild_id = $1")
        .bind(snowflake(guild_id.get())?)
        .fetch_all(db)
        .await?;

    let key = xp_key(guild_id.get());
    for chunk in rows.chunks(FLUSH_BATCH) {
        // GT: Redis'teki değer daha yüksekse (yeni kazanılmış XP) ezilmez.
        let mut cmd = redis::cmd("ZADD");
        cmd.arg(&key).arg("GT");
        for (user, xp) in chunk {
            cmd.arg(*xp).arg(*user);
        }
        let _: i64 = cmd.query_async(conn).await?;
    }

    if !rows.is_empty() {
        info!(guild = %guild_id, members = rows.len(), "XP verisi PostgreSQL'den Redis'e yüklendi");
    }
    Ok(())
}

/// Kirli XP kayıtlarını toplu halde PostgreSQL'e yazar. Yazılan kayıt sayısını döner.
pub async fn flush(db: &PgPool, redis: &ConnectionManager) -> anyhow::Result<usize> {
    let mut conn = redis.clone();
    let mut written = 0;

    loop {
        let members: Vec<String> = redis::cmd("SPOP")
            .arg(DIRTY_KEY)
            .arg(FLUSH_BATCH)
            .query_async(&mut conn)
            .await?;
        if members.is_empty() {
            break;
        }

        let parsed: Vec<(u64, u64)> = members.iter().filter_map(|m| parse_dirty(m)).collect();
        let mut pipe = redis::pipe();
        for (guild, user) in &parsed {
            pipe.cmd("ZSCORE").arg(xp_key(*guild)).arg(*user);
        }
        let scores: Vec<Option<f64>> = pipe.query_async(&mut conn).await?;

        let mut guilds = Vec::with_capacity(parsed.len());
        let mut users = Vec::with_capacity(parsed.len());
        let mut xps = Vec::with_capacity(parsed.len());
        for ((guild, user), score) in parsed.iter().zip(scores) {
            let Some(score) = score else { continue };
            guilds.push(snowflake(*guild)?);
            users.push(snowflake(*user)?);
            xps.push(i64::try_from(score_to_xp(score)).unwrap_or(i64::MAX));
        }

        let result = sqlx::query(
            "INSERT INTO member_xp (guild_id, user_id, xp)
             SELECT * FROM UNNEST($1::BIGINT[], $2::BIGINT[], $3::BIGINT[])
             ON CONFLICT (guild_id, user_id) DO UPDATE SET xp = EXCLUDED.xp, updated_at = now()",
        )
        .bind(&guilds)
        .bind(&users)
        .bind(&xps)
        .execute(db)
        .await;

        if let Err(e) = result {
            // Kayıtları geri koy; bir sonraki turda tekrar denenir.
            let _: i64 = redis::cmd("SADD")
                .arg(DIRTY_KEY)
                .arg(&members)
                .query_async(&mut conn)
                .await?;
            return Err(e).context("XP PostgreSQL'e yazılamadı");
        }

        written += guilds.len();
        if members.len() < FLUSH_BATCH {
            break;
        }
    }

    Ok(written)
}

/// Uygulama kapanana kadar periyodik olarak `flush` çalıştırır.
pub fn spawn_flusher(db: PgPool, redis: ConnectionManager) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(FLUSH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            crate::supervise::round("XP yazıcısı", std::time::Duration::from_secs(120), async {
                match flush(&db, &redis).await {
                    Ok(0) => {}
                    Ok(n) => debug!(records = n, "XP PostgreSQL'e yazıldı"),
                    Err(e) => warn!(error = ?e, "XP flush başarısız, sonraki turda tekrar denenecek"),
                }
            })
            .await;
        }
    });
}

fn parse_dirty(member: &str) -> Option<(u64, u64)> {
    let (guild, user) = member.split_once(':')?;
    Some((guild.parse().ok()?, user.parse().ok()?))
}

fn parse_score(raw: &str) -> anyhow::Result<u64> {
    let score: f64 = raw.parse().with_context(|| format!("geçersiz XP değeri: {raw}"))?;
    Ok(score_to_xp(score))
}

fn score_to_xp(score: f64) -> u64 {
    if score.is_finite() && score > 0.0 {
        score as u64
    } else {
        0
    }
}

/// Liste kuralı: yasak listesinde listedekiler XP kazanamaz; izin listesinde sadece listedekiler
/// kazanır. Boş izin listesi kısıtlama sayılmaz (yanlışlıkla herkesin XP'si kesilmesin).
fn excluded(list: &Exclusions, channels: &[ChannelId], roles: &[RoleId], channels_allow_only: bool, roles_allow_only: bool) -> bool {
    let blocked = |kind: ExclusionKind, listed: bool, allow_only: bool| {
        let any = list.values().any(|k| *k == kind);
        if allow_only {
            any && !listed
        } else {
            listed
        }
    };
    let channel_listed = channels.iter().any(|c| list.get(&c.get()) == Some(&ExclusionKind::Channel));
    let role_listed = roles.iter().any(|r| list.get(&r.get()) == Some(&ExclusionKind::Role));
    blocked(ExclusionKind::Channel, channel_listed, channels_allow_only) || blocked(ExclusionKind::Role, role_listed, roles_allow_only)
}

/// TP oranını uygular; en az 1 XP.
pub(crate) fn scaled_xp(amount: u64, rate_percent: u16) -> u64 {
    (amount * u64::from(rate_percent) / 100).max(1)
}

/// Harici bağımlılık olmadan [XP_MIN, XP_MAX] aralığında rastgele değer.
fn random_xp() -> u64 {
    let random = RandomState::new().hash_one(Instant::now());
    XP_MIN + random % (XP_MAX - XP_MIN + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_xp_stays_in_range() {
        for _ in 0..1000 {
            let xp = random_xp();
            assert!((XP_MIN..=XP_MAX).contains(&xp), "{xp}");
        }
    }

    #[test]
    fn rate_scales_xp_and_never_reaches_zero() {
        assert_eq!(scaled_xp(20, 100), 20);
        assert_eq!(scaled_xp(20, 300), 60);
        assert_eq!(scaled_xp(20, 25), 5);
        assert_eq!(scaled_xp(1, 25), 1);
    }

    #[test]
    fn deny_and_allow_lists() {
        let list: Exclusions = [(1, ExclusionKind::Channel), (9, ExclusionKind::Role)].into_iter().collect();
        let (c1, c2) = ([ChannelId::new(1)], [ChannelId::new(2)]);
        let (r9, r8) = ([RoleId::new(9)], [RoleId::new(8)]);
        // Yasak listesi: listedeki kanal ya da rol XP kazanamaz.
        assert!(excluded(&list, &c1, &r8, false, false));
        assert!(excluded(&list, &c2, &r9, false, false));
        assert!(!excluded(&list, &c2, &r8, false, false));
        // İzin listesi: sadece listedekiler kazanır.
        assert!(!excluded(&list, &c1, &r9, true, true));
        assert!(excluded(&list, &c2, &r9, true, true));
        assert!(excluded(&list, &c1, &r8, true, true));
        // Boş izin listesi kimseyi engellemez.
        assert!(!excluded(&Exclusions::new(), &c2, &r8, true, true));
    }

    #[test]
    fn parses_dirty_members() {
        assert_eq!(parse_dirty("123:456"), Some((123, 456)));
        assert_eq!(parse_dirty("bozuk"), None);
        assert_eq!(parse_dirty("1:x"), None);
    }

    #[test]
    fn score_conversion_is_safe() {
        assert_eq!(score_to_xp(42.0), 42);
        assert_eq!(score_to_xp(-5.0), 0);
        assert_eq!(score_to_xp(f64::NAN), 0);
    }
}
