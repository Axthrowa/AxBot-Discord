//! Vurgu bildirimleri (Carl-bot "highlight"): üye birkaç kelime seçer; bu kelimeler göremediği değil, GÖREBİLDİĞİ
//! bir kanalda geçince bot ona özelden haber verir.
//!
//! Mesaj başına veritabanına gidilmez: sunucunun kelime listesi bellekte (değişince yeniden okunur). Üye o kanalda
//! son 5 dakikada yazdıysa (sohbetin içindeyse) ya da 5 dakika içinde zaten bildirim aldıysa haber verilmez.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use poise::serenity_prelude::{self as serenity, ChannelId, CreateEmbed, CreateEmbedAuthor, CreateMessage, GuildId, Message, Permissions, UserId};
use sqlx::PgPool;
use tracing::{debug, warn};

use crate::db::snowflake;
use crate::guild_config::to_id;

pub const MAX_WORDS: usize = 10;
pub const MIN_WORD_LEN: usize = 3;
pub const MAX_WORD_LEN: usize = 40;
/// Üye bu kadar süre içinde o kanalda yazdıysa sohbetin içindedir; bildirim gerekmez.
const ACTIVE_WINDOW: Duration = Duration::from_secs(5 * 60);
/// Aynı üyeye en fazla bu aralıkla bildirim.
const COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// Sunucu başına (kelime, üye) listesi; `None` = henüz okunmadı.
type WordList = Arc<Vec<(String, UserId)>>;
static WORDS: LazyLock<DashMap<GuildId, WordList>> = LazyLock::new(DashMap::new);
/// (kanal, üye) → üyenin o kanaldaki son mesajı.
static LAST_SEEN: LazyLock<DashMap<(ChannelId, UserId), Instant>> = LazyLock::new(DashMap::new);
/// (sunucu, üye) → son bildirim.
static LAST_SENT: LazyLock<DashMap<(GuildId, UserId), Instant>> = LazyLock::new(DashMap::new);

/// Büyük/küçük harf ve Türkçe I/İ farkı gözetmeden karşılaştırma için.
use crate::games::hangman::tr_lower as fold;

/// Kelime metinde TAM KELİME olarak geçiyor mu? ("ali" → "Ali'nin" evet, "kalite" hayır)
pub fn matches(text: &str, word: &str) -> bool {
    let check = |haystack: &str, needle: &str| {
        if needle.is_empty() {
            return false;
        }
        haystack.match_indices(needle).any(|(start, _)| {
            let before = haystack[..start].chars().next_back();
            let after = haystack[start + needle.len()..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
    };
    check(&fold(text), &fold(word)) || check(&text.to_lowercase(), &word.to_lowercase())
}

/// Kelimeyi kaydetmeden önce temizler; uygun değilse sebebi döner.
pub fn normalize(word: &str) -> Result<String, &'static str> {
    let word = word.split_whitespace().collect::<Vec<_>>().join(" ");
    let length = word.chars().count();
    if length < MIN_WORD_LEN {
        return Err("Kelime en az 3 harf olmalı.");
    }
    if length > MAX_WORD_LEN {
        return Err("Kelime en fazla 40 karakter olabilir.");
    }
    if !word.chars().any(char::is_alphanumeric) {
        return Err("Kelimede en az bir harf ya da rakam olmalı.");
    }
    Ok(fold(&word))
}

pub async fn list(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT word FROM highlights WHERE guild_id = $1 AND user_id = $2 ORDER BY created_at")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .fetch_all(db)
        .await?)
}

/// Kelime ekler. Sınır doluysa `Ok(false)`.
pub async fn add(db: &PgPool, guild_id: GuildId, user: UserId, word: &str) -> anyhow::Result<bool> {
    if list(db, guild_id, user).await?.len() >= MAX_WORDS {
        return Ok(false);
    }
    sqlx::query("INSERT INTO highlights (guild_id, user_id, word) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .bind(word)
        .execute(db)
        .await?;
    WORDS.remove(&guild_id);
    Ok(true)
}

/// Kelimeyi siler (`None` = hepsi); silinen sayısı.
pub async fn remove(db: &PgPool, guild_id: GuildId, user: UserId, word: Option<&str>) -> anyhow::Result<u64> {
    let done = sqlx::query("DELETE FROM highlights WHERE guild_id = $1 AND user_id = $2 AND ($3::TEXT IS NULL OR word = $3)")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .bind(word)
        .execute(db)
        .await?;
    WORDS.remove(&guild_id);
    Ok(done.rows_affected())
}

async fn words(db: &PgPool, guild_id: GuildId) -> anyhow::Result<WordList> {
    if let Some(words) = WORDS.get(&guild_id) {
        return Ok(Arc::clone(&words));
    }
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT word, user_id FROM highlights WHERE guild_id = $1")
        .bind(snowflake(guild_id.get())?)
        .fetch_all(db)
        .await?;
    let words = Arc::new(rows.into_iter().filter_map(|(w, u)| to_id(Some(u)).map(|u| (w, UserId::new(u)))).collect::<Vec<_>>());
    WORDS.insert(guild_id, Arc::clone(&words));
    Ok(words)
}

/// Mesaj sayacı (temizliğin periyodik ve seyrek yapılması için).
static MSG_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Her sunucu mesajında çağrılır.
pub async fn on_message(ctx: &serenity::Context, db: &PgPool, guild_id: GuildId, msg: &Message) {
    let now = Instant::now();
    // Bellek büyümesin: her 1000 mesajda bir ve yalnızca harita büyüdüğünde eski kayıtlar temizlenir.
    if MSG_COUNTER.fetch_add(1, Ordering::Relaxed).is_multiple_of(1000) && LAST_SEEN.len() > 10_000 {
        LAST_SEEN.retain(|_, at| now.duration_since(*at) < ACTIVE_WINDOW);
    }
    LAST_SEEN.insert((msg.channel_id, msg.author.id), now);
    if msg.author.bot || msg.content.is_empty() {
        return;
    }
    let words = match words(db, guild_id).await {
        Ok(words) => words,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Vurgu kelimeleri okunamadı");
            return;
        }
    };
    if words.is_empty() {
        return;
    }
    let mut targets: Vec<(UserId, &str)> = Vec::new();
    for (word, user) in words.iter() {
        if *user == msg.author.id || targets.iter().any(|(u, _)| u == user) || !matches(&msg.content, word) {
            continue;
        }
        let active = LAST_SEEN.get(&(msg.channel_id, *user)).is_some_and(|at| now.duration_since(*at) < ACTIVE_WINDOW);
        let cooling = LAST_SENT.get(&(guild_id, *user)).is_some_and(|at| now.duration_since(*at) < COOLDOWN);
        if !active && !cooling {
            targets.push((*user, word));
        }
    }
    for (user, word) in targets {
        // Üye kanalı görebiliyor mu? (gizli kanaldaki konuşma sızmasın) — önbellek kilidi `await`'ten önce bırakılır.
        let (can_see, guild_name, channel_name) = match ctx.cache.guild(guild_id) {
            Some(guild) => {
                let can_see = match (guild.members.get(&user), guild.channels.get(&msg.channel_id)) {
                    (Some(member), Some(channel)) => guild.user_permissions_in(channel, member).contains(Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY),
                    // Alt başlıklar `channels` içinde değildir; ana kanala bakılır.
                    (Some(member), None) => msg
                        .thread
                        .as_ref()
                        .and_then(|t| t.parent_id)
                        .or_else(|| guild.threads.iter().find(|t| t.id == msg.channel_id).and_then(|t| t.parent_id))
                        .and_then(|parent| guild.channels.get(&parent))
                        .is_some_and(|parent| guild.user_permissions_in(parent, member).contains(Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY)),
                    _ => false,
                };
                (can_see, guild.name.clone(), guild.channels.get(&msg.channel_id).map(|c| c.name.clone()))
            }
            None => (false, String::new(), None),
        };
        if !can_see {
            continue;
        }
        LAST_SENT.insert((guild_id, user), now);
        let content: String = msg.content.chars().take(900).collect();
        let embed = CreateEmbed::new()
            .author(CreateEmbedAuthor::new(msg.author.global_name.clone().unwrap_or_else(|| msg.author.name.clone())).icon_url(msg.author.face()))
            .description(format!("{content}\n\n[Mesaja git →]({})", msg.link()))
            .colour(serenity::Colour::GOLD)
            .footer(serenity::CreateEmbedFooter::new(format!("{guild_name} · #{}", channel_name.unwrap_or_default())))
            .timestamp(msg.timestamp);
        let dm = CreateMessage::new().content(format!("🔔 **{word}** kelimesi geçti: <#{}>", msg.channel_id)).embed(embed);
        let http = Arc::clone(&ctx.http);
        tokio::spawn(async move {
            if let Err(e) = user.direct_message(&http, dm).await {
                debug!(user = %user, error = %e, "Vurgu bildirimi gönderilemedi (özel mesajlar kapalı olabilir)");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_whole_words_only() {
        assert!(matches("Ali'nin oyunu", "ali"));
        assert!(matches("ALİ geldi mi?", "ali"));
        assert!(matches("IRMAK nerede", "ırmak"));
        assert!(matches("yeni IPHONE çıktı", "iphone"));
        assert!(matches("bu axbot harika", "AxBot"));
        assert!(matches("rust programlama dili", "rust programlama"));
        assert!(!matches("kalite önemli", "ali"));
        assert!(!matches("trusty", "rust"));
        assert!(!matches("", "ali"));
    }

    #[test]
    fn normalizes_words() {
        assert_eq!(normalize("  Rust   Dili "), Ok("rust dili".to_owned()));
        assert_eq!(normalize("İstanbul"), Ok("istanbul".to_owned()));
        assert!(normalize("ab").is_err());
        assert!(normalize("!!!").is_err());
        assert!(normalize(&"a".repeat(41)).is_err());
    }
}
