//! ModMail: üye bota özelden yazar, mesaj yetkili kanalında o üyeye ait bir alt başlığa düşer;
//! yetkililer alt başlığa yazarak cevaplar (cevap üyeye botun özel mesajı olarak gider).
//!
//! - Alt başlıkta `.` ile başlayan mesajlar iç nottur, üyeye gitmez.
//! - `!kapat` konuşmayı kapatır (üyeye haber verilir, alt başlık arşivlenir).
//! - Eşleşmeler Redis'te: `modmail:{sunucu}:{üye}` → alt başlık, `modmail:baslik:{alt başlık}` → üye.

use std::time::Duration;

use poise::serenity_prelude::{
    self as serenity, AutoArchiveDuration, ChannelId, ChannelType, CreateAllowedMentions, CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, CreateMessage,
    CreateThread, EditThread, GuildId, Message, ReactionType, UserId,
};
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::data::Data;
use crate::settings::PluginConfig;

pub const MAX_GREETING: usize = 500;
const LINK_SECS: u64 = 30 * 24 * 3600;
/// Üye başına dakikada en fazla bu kadar özel mesaj iletilir.
const RATE_LIMIT: i64 = 6;
const NOTE_PREFIX: char = '.';
const CLOSE_COMMAND: &str = "!kapat";
const DEFAULT_GREETING: &str = "Mesajın yetkili ekibine iletildi. Cevap geldiğinde buradan haber vereceğim.";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModmailSettings {
    /// Alt başlıkların açılacağı yetkili kanalı; boşsa ModMail kapalı.
    pub channel: Option<u64>,
    /// Üyeye ilk mesajında gönderilen karşılama.
    pub greeting: Option<String>,
}

impl PluginConfig for ModmailSettings {
    const KEY: &'static str = "modmail";
}

fn user_key(guild_id: GuildId, user: UserId) -> String {
    format!("modmail:{guild_id}:{user}")
}

fn thread_key(thread: ChannelId) -> String {
    format!("modmail:baslik:{thread}")
}

/// Mesajın iletilecek metni: içerik + eklerin bağlantıları (Discord gömülü metin sınırına göre kırpılır).
pub fn relay_text(content: &str, attachments: &[String]) -> String {
    let mut text: String = content.trim().chars().take(3500).collect();
    for url in attachments.iter().take(6) {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("📎 ");
        text.push_str(url);
    }
    text
}

/// Alt başlıktaki yetkili mesajının türü.
#[derive(Debug, PartialEq, Eq)]
pub enum StaffMessage {
    Note,
    Close,
    Reply,
}

pub fn classify(content: &str) -> StaffMessage {
    let trimmed = content.trim();
    if trimmed.eq_ignore_ascii_case(CLOSE_COMMAND) {
        StaffMessage::Close
    } else if trimmed.starts_with(NOTE_PREFIX) {
        StaffMessage::Note
    } else {
        StaffMessage::Reply
    }
}

async fn redis_get(redis: &ConnectionManager, key: &str) -> Option<u64> {
    let mut conn = redis.clone();
    redis::cmd("GET").arg(key).query_async::<Option<u64>>(&mut conn).await.ok().flatten().filter(|id| *id != 0)
}

async fn redis_set(redis: &ConnectionManager, key: &str, value: u64) {
    let mut conn = redis.clone();
    let saved: redis::RedisResult<()> = redis::cmd("SET").arg(key).arg(value).arg("EX").arg(LINK_SECS).query_async(&mut conn).await;
    if let Err(e) = saved {
        warn!(error = %e, "ModMail eşleşmesi kaydedilemedi");
    }
}

async fn redis_del(redis: &ConnectionManager, keys: &[String]) {
    let mut conn = redis.clone();
    let _: redis::RedisResult<()> = redis::cmd("DEL").arg(keys).query_async(&mut conn).await;
}

async fn within_rate_limit(redis: &ConnectionManager, user: UserId) -> bool {
    let key = format!("modmail:hiz:{user}");
    let mut conn = redis.clone();
    let count: redis::RedisResult<i64> = redis::cmd("INCR").arg(&key).query_async(&mut conn).await;
    if count.as_ref().is_ok_and(|c| *c == 1) {
        let _: redis::RedisResult<()> = redis::cmd("EXPIRE").arg(&key).arg(60).query_async(&mut conn).await;
    }
    count.map_or(true, |c| c <= RATE_LIMIT)
}

/// Üyenin bulunduğu ve ModMail'i açık olan ilk sunucu.
async fn target_guild(ctx: &serenity::Context, data: &Data, user: UserId) -> Option<(GuildId, ChannelId, std::sync::Arc<ModmailSettings>)> {
    for guild_id in ctx.cache.guilds() {
        let Ok(settings) = data.settings.get::<ModmailSettings>(&data.db, guild_id).await else { continue };
        let Some(channel) = settings.channel.filter(|c| *c != 0) else { continue };
        // Önbellek kilidi hemen bırakılır.
        let is_member = ctx.cache.guild(guild_id).is_some_and(|g| g.members.contains_key(&user));
        if is_member {
            return Some((guild_id, ChannelId::new(channel), settings));
        }
    }
    None
}

fn attachment_urls(msg: &Message) -> Vec<String> {
    msg.attachments.iter().map(|a| a.url.clone()).collect()
}

/// Bota özelden gelen mesaj.
pub async fn on_dm(ctx: &serenity::Context, data: &Data, msg: &Message) {
    let text = relay_text(&msg.content, &attachment_urls(msg));
    if text.is_empty() {
        return;
    }
    // ModMail hiçbir ortak sunucuda açık değilse sessiz kalınır.
    let Some((guild_id, staff_channel, settings)) = target_guild(ctx, data, msg.author.id).await else {
        return;
    };
    if !within_rate_limit(&data.redis, msg.author.id).await {
        return;
    }
    let embed = CreateEmbed::new()
        .author(CreateEmbedAuthor::new(msg.author.tag()).icon_url(msg.author.face()))
        .description(&text)
        .colour(serenity::Colour::from_rgb(52, 152, 219))
        .footer(CreateEmbedFooter::new(format!("Üye kimliği: {}", msg.author.id)));
    let relay = || CreateMessage::new().embed(embed.clone()).allowed_mentions(CreateAllowedMentions::new());

    // Açık konuşma varsa oraya; alt başlık silinmiş / arşivlenip kilitlenmişse yenisi açılır.
    let key = user_key(guild_id, msg.author.id);
    if let Some(thread) = redis_get(&data.redis, &key).await.map(ChannelId::new) {
        if thread.send_message(&ctx.http, relay()).await.is_ok() {
            let _ = msg.react(&ctx.http, ReactionType::Unicode("✅".to_owned())).await;
            return;
        }
        redis_del(&data.redis, &[key.clone(), thread_key(thread)]).await;
    }

    let name: String = format!("modmail-{}", msg.author.name).chars().take(90).collect();
    let builder = CreateThread::new(name).kind(ChannelType::PublicThread).auto_archive_duration(AutoArchiveDuration::OneWeek);
    let thread = match staff_channel.create_thread(&ctx.http, builder).await {
        Ok(thread) => thread,
        Err(e) => {
            warn!(guild = %guild_id, error = %e, "ModMail alt başlığı açılamadı");
            let _ = msg.channel_id.say(&ctx.http, "⚠️ Mesajın şu an iletilemedi; lütfen daha sonra tekrar dene.").await;
            return;
        }
    };
    let intro = format!(
        "📬 **Yeni ModMail** — <@{}> (`{}`)\nBu alt başlığa yazdığın her mesaj üyeye **yetkili ekibi** adına özelden gider. \
         `.` ile başlayan mesajlar iç nottur (gitmez). Konuşmayı bitirmek için `{CLOSE_COMMAND}` yaz.",
        msg.author.id, msg.author.id
    );
    let _ = thread.id.send_message(&ctx.http, CreateMessage::new().content(intro).allowed_mentions(CreateAllowedMentions::new())).await;
    if thread.id.send_message(&ctx.http, relay()).await.is_err() {
        return;
    }
    redis_set(&data.redis, &key, thread.id.get()).await;
    redis_set(&data.redis, &thread_key(thread.id), msg.author.id.get()).await;
    let greeting = settings.greeting.as_deref().filter(|g| !g.trim().is_empty()).unwrap_or(DEFAULT_GREETING);
    let _ = msg.channel_id.send_message(&ctx.http, CreateMessage::new().content(format!("📨 {greeting}")).allowed_mentions(CreateAllowedMentions::new())).await;
}

/// Sunucu mesajı bir ModMail alt başlığındaysa işler. Ayar bellekten, alt başlık bilgisi önbellekten okunur;
/// Redis'e yalnızca ModMail kanalının alt başlıklarında gidilir.
pub async fn on_staff_message(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message) {
    let Ok(settings) = data.settings.get::<ModmailSettings>(&data.db, guild_id).await else { return };
    let Some(staff_channel) = settings.channel else { return };
    let in_modmail_thread = ctx
        .cache
        .guild(guild_id)
        .is_some_and(|g| g.threads.iter().any(|t| t.id == msg.channel_id && t.parent_id.map(ChannelId::get) == Some(staff_channel)));
    if !in_modmail_thread {
        return;
    }
    let Some(user) = redis_get(&data.redis, &thread_key(msg.channel_id)).await.map(UserId::new) else {
        return;
    };
    let guild_name = ctx.cache.guild(guild_id).map_or_else(|| "Sunucu".to_owned(), |g| g.name.clone());
    let send_dm = |text: String, colour: serenity::Colour| {
        let embed = CreateEmbed::new()
            .author(CreateEmbedAuthor::new(format!("{guild_name} — yetkili ekibi")))
            .description(text)
            .colour(colour)
            .footer(CreateEmbedFooter::new("Cevap vermek için bu sohbete yaz."));
        async move {
            let dm = user.create_dm_channel(&ctx.http).await?;
            dm.id.send_message(&ctx.http, CreateMessage::new().embed(embed).allowed_mentions(CreateAllowedMentions::new())).await
        }
    };
    match classify(&msg.content) {
        StaffMessage::Note => {}
        StaffMessage::Close => {
            let _ = send_dm("🔒 Bu konuşma yetkili ekibi tarafından kapatıldı. Yeniden yazarsan yeni bir konuşma açılır.".to_owned(), serenity::Colour::DARK_GREY).await;
            redis_del(&data.redis, &[user_key(guild_id, user), thread_key(msg.channel_id)]).await;
            let _ = msg.channel_id.say(&ctx.http, "🔒 Konuşma kapatıldı.").await;
            // Arşivleme biraz gecikmeli: yukarıdaki mesaj alt başlığı yeniden açmasın.
            tokio::time::sleep(Duration::from_millis(400)).await;
            if let Err(e) = msg.channel_id.edit_thread(&ctx.http, EditThread::new().archived(true)).await {
                debug!(error = %e, "ModMail alt başlığı arşivlenemedi");
            }
        }
        StaffMessage::Reply => {
            let text = relay_text(&msg.content, &attachment_urls(msg));
            if text.is_empty() {
                return;
            }
            let reaction = match send_dm(text, serenity::Colour::from_rgb(46, 204, 113)).await {
                Ok(_) => "✅",
                Err(_) => {
                    let _ = msg.channel_id.say(&ctx.http, "❌ Mesaj iletilemedi: üye özel mesajlarını kapatmış ya da sunucudan ayrılmış olabilir.").await;
                    "❌"
                }
            };
            let _ = msg.react(&ctx.http, ReactionType::Unicode(reaction.to_owned())).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_staff_messages() {
        assert_eq!(classify("  !KAPAT "), StaffMessage::Close);
        assert_eq!(classify(". bunu üye görmesin"), StaffMessage::Note);
        assert_eq!(classify("Merhaba, nasıl yardımcı olabiliriz?"), StaffMessage::Reply);
        assert_eq!(classify("!kapat lütfen"), StaffMessage::Reply);
    }

    #[test]
    fn relay_text_includes_attachments_and_is_bounded() {
        assert_eq!(relay_text("  selam ", &[]), "selam");
        assert_eq!(relay_text("", &["https://cdn/a.png".to_owned()]), "📎 https://cdn/a.png");
        assert_eq!(relay_text("bak", &["https://cdn/a.png".to_owned()]), "bak\n📎 https://cdn/a.png");
        assert!(relay_text(&"a".repeat(9000), &[]).chars().count() <= 3500);
        assert_eq!(relay_text("   ", &[]), "");
    }
}
