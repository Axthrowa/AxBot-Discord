//! Auto-mod ayarlarının okunup yazılması.
//!
//! Kaynak gerçek PostgreSQL'dedir; Redis paylaşımlı ikinci seviye önbellektir.
//! Okuma sırası: Redis → (yoksa) PostgreSQL → Redis'e geri yazılır.

use poise::serenity_prelude::GuildId;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::warn;

use crate::db::snowflake;

/// Redis'teki ayar kopyasının yaşam süresi. Yazma işlemleri anahtarı zaten günceller;
/// bu süre sadece unutulan sunucuların bellekte birikmemesi için.
const REDIS_TTL_SECS: u64 = 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct AutoModSettings {
    pub enabled: bool,
    pub block_invites: bool,
    pub block_links: bool,
    pub filter_profanity: bool,
    pub anti_spam: bool,
    pub spam_max_messages: i32,
    pub spam_window_secs: i32,
    pub exempt_staff: bool,
}

// Migration'daki DEFAULT değerleriyle aynı olmalı.
impl Default for AutoModSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            block_invites: true,
            block_links: false,
            filter_profanity: true,
            anti_spam: true,
            spam_max_messages: 5,
            spam_window_secs: 5,
            exempt_staff: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoModConfig {
    pub settings: AutoModSettings,
    pub custom_words: Vec<String>,
    /// Eski sürümün Redis'e yazdığı kayıtlarda bu alan yok.
    #[serde(default)]
    pub allowed_words: Vec<String>,
}

/// Sunucuya özel iki kelime listesi aynı şekilde yönetilir.
#[derive(Debug, Clone, Copy)]
pub enum WordList {
    Blocked,
    Allowed,
}

/// Tablo adını derleme anında SQL'e gömer; sonuç `&'static str` olduğu için sqlx'in
/// SQL enjeksiyonu korumasından (`SqlSafeStr`) geçer.
macro_rules! word_list_sql {
    ($list:expr, $before:literal, $after:literal) => {
        match $list {
            WordList::Blocked => concat!($before, "automod_words", $after),
            WordList::Allowed => concat!($before, "automod_allowed_words", $after),
        }
    };
}

fn redis_key(guild_id: GuildId) -> String {
    format!("automod:config:{guild_id}")
}

/// Önce Redis'e bakar; yoksa ya da Redis erişilemiyorsa PostgreSQL'den okur.
pub async fn load(
    db: &PgPool,
    redis: &ConnectionManager,
    guild_id: GuildId,
) -> anyhow::Result<AutoModConfig> {
    let mut conn = redis.clone();
    let cached: redis::RedisResult<Option<String>> = redis::cmd("GET")
        .arg(redis_key(guild_id))
        .query_async(&mut conn)
        .await;

    match cached {
        Ok(Some(json)) => match serde_json::from_str(&json) {
            Ok(config) => return Ok(config),
            Err(e) => warn!(guild = %guild_id, error = %e, "Redis'teki auto-mod ayarı bozuk, yeniden yükleniyor"),
        },
        Ok(None) => {}
        Err(e) => warn!(guild = %guild_id, error = %e, "Redis okunamadı, PostgreSQL kullanılıyor"),
    }

    let config = load_from_db(db, guild_id).await?;
    cache_in_redis(redis, guild_id, &config).await;
    Ok(config)
}

pub async fn load_from_db(db: &PgPool, guild_id: GuildId) -> anyhow::Result<AutoModConfig> {
    let (settings, custom_words, allowed_words) = tokio::try_join!(
        load_settings(db, guild_id),
        load_words(db, guild_id, WordList::Blocked),
        load_words(db, guild_id, WordList::Allowed),
    )?;

    Ok(AutoModConfig {
        settings,
        custom_words,
        allowed_words,
    })
}

async fn load_words(db: &PgPool, guild_id: GuildId, list: WordList) -> anyhow::Result<Vec<String>> {
    let words = sqlx::query_scalar(word_list_sql!(
        list,
        "SELECT word FROM ",
        " WHERE guild_id = $1 ORDER BY word"
    ))
    .bind(snowflake(guild_id.get())?)
    .fetch_all(db)
    .await?;
    Ok(words)
}

pub async fn load_settings(db: &PgPool, guild_id: GuildId) -> anyhow::Result<AutoModSettings> {
    let settings = sqlx::query_as::<_, AutoModSettings>(
        "SELECT enabled, block_invites, block_links, filter_profanity, anti_spam,
                spam_max_messages, spam_window_secs, exempt_staff
         FROM automod_settings WHERE guild_id = $1",
    )
    .bind(snowflake(guild_id.get())?)
    .fetch_optional(db)
    .await?;

    Ok(settings.unwrap_or_default())
}

pub async fn save_settings(
    db: &PgPool,
    guild_id: GuildId,
    s: &AutoModSettings,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO automod_settings (guild_id, enabled, block_invites, block_links,
             filter_profanity, anti_spam, spam_max_messages, spam_window_secs, exempt_staff)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (guild_id) DO UPDATE SET
             enabled = EXCLUDED.enabled,
             block_invites = EXCLUDED.block_invites,
             block_links = EXCLUDED.block_links,
             filter_profanity = EXCLUDED.filter_profanity,
             anti_spam = EXCLUDED.anti_spam,
             spam_max_messages = EXCLUDED.spam_max_messages,
             spam_window_secs = EXCLUDED.spam_window_secs,
             exempt_staff = EXCLUDED.exempt_staff,
             updated_at = now()",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(s.enabled)
    .bind(s.block_invites)
    .bind(s.block_links)
    .bind(s.filter_profanity)
    .bind(s.anti_spam)
    .bind(s.spam_max_messages)
    .bind(s.spam_window_secs)
    .bind(s.exempt_staff)
    .execute(db)
    .await?;

    Ok(())
}

pub async fn count_words(db: &PgPool, guild_id: GuildId, list: WordList) -> anyhow::Result<i64> {
    let count = sqlx::query_scalar(word_list_sql!(
        list,
        "SELECT COUNT(*) FROM ",
        " WHERE guild_id = $1"
    ))
    .bind(snowflake(guild_id.get())?)
    .fetch_one(db)
    .await?;
    Ok(count)
}

/// Kelime eklendiyse `true`, zaten listedeyse `false` döner.
pub async fn add_word(
    db: &PgPool,
    guild_id: GuildId,
    list: WordList,
    word: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(word_list_sql!(
        list,
        "INSERT INTO ",
        " (guild_id, word) VALUES ($1, $2) ON CONFLICT DO NOTHING"
    ))
    .bind(snowflake(guild_id.get())?)
    .bind(word)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Kelime silindiyse `true`, listede yoksa `false` döner.
pub async fn remove_word(
    db: &PgPool,
    guild_id: GuildId,
    list: WordList,
    word: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(word_list_sql!(
        list,
        "DELETE FROM ",
        " WHERE guild_id = $1 AND word = $2"
    ))
    .bind(snowflake(guild_id.get())?)
        .bind(word)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Redis'e yazma hatası kritik değildir: sadece loglanır, bir sonraki okuma PostgreSQL'e düşer.
pub async fn cache_in_redis(redis: &ConnectionManager, guild_id: GuildId, config: &AutoModConfig) {
    let json = match serde_json::to_string(config) {
        Ok(json) => json,
        Err(e) => {
            warn!(guild = %guild_id, error = %e, "Auto-mod ayarı serileştirilemedi");
            return;
        }
    };

    let mut conn = redis.clone();
    let result: redis::RedisResult<()> = redis::cmd("SET")
        .arg(redis_key(guild_id))
        .arg(json)
        .arg("EX")
        .arg(REDIS_TTL_SECS)
        .query_async(&mut conn)
        .await;

    if let Err(e) = result {
        warn!(guild = %guild_id, error = %e, "Auto-mod ayarı Redis'e yazılamadı");
    }
}

/// Sunucu başına her kelime listesinin en fazla uzunluğu.
pub const MAX_CUSTOM_WORDS: i64 = 500;
const WORD_MIN_LEN: usize = 2;
const WORD_MAX_LEN: usize = 32;

/// Kelimeyi küçük harfe çevirip doğrular; hata kullanıcıya gösterilecek kısa açıklamadır.
pub fn validate_word(raw: &str, list: WordList) -> Result<String, &'static str> {
    let word = raw.trim().to_lowercase();
    let len = word.chars().count();

    if !(WORD_MIN_LEN..=WORD_MAX_LEN).contains(&len) {
        return Err("2-32 karakter olmalı");
    }
    if !word
        .chars()
        .all(|c| c.is_alphanumeric() || c == ' ' || c == '*')
    {
        return Err("sadece harf, rakam, boşluk ve * kullanılabilir");
    }
    if word.chars().filter(|c| c.is_alphanumeric()).count() < WORD_MIN_LEN {
        return Err("en az 2 harf/rakam içermeli");
    }
    // İstisnalar tam kelime veya önek olarak eşleşir; `*` sadece sonda olabilir.
    if matches!(list, WordList::Allowed) && word.trim_end_matches('*').contains('*') {
        return Err("istisnalarda * sadece kelimenin sonunda kullanılabilir");
    }
    Ok(word)
}
