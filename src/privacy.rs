//! Gizlilik: herkese açık gizlilik metni (`/gizlilik`), üyenin kendi verisini silmesi, sunucu verisinin
//! silinmesi (sahip komutu ya da bot sunucudan çıkarıldıktan `PURGE_AFTER_DAYS` gün sonra otomatik).
//!
//! Silinmeyenler (bilerek): moderasyon kayıtları, notlar, karantina, biletler, itirazlar, başvurular,
//! öneriler — sunucunun güvenliği için tutulur; sunucu verisi silinince onlar da gider.

use std::time::Duration;

use poise::serenity_prelude::{self as serenity, GuildId, UserId};
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use tracing::{info, warn};

use crate::db::snowflake;

/// Bot sunucudan çıkarıldıktan sonra verinin silineceği gün.
pub const PURGE_AFTER_DAYS: i64 = 30;

/// Üyenin kişisel verisi (bütün sunucularda). `$1` = kullanıcı kimliği.
const USER_SQL: &[&str] = &[
    "DELETE FROM animal_catches WHERE user_id = $1",
    "DELETE FROM appeal_tokens WHERE user_id = $1",
    "DELETE FROM birthdays WHERE user_id = $1",
    "DELETE FROM economy_inventory WHERE user_id = $1",
    "DELETE FROM economy_accounts WHERE user_id = $1",
    "DELETE FROM giveaway_entries WHERE user_id = $1",
    "DELETE FROM highlights WHERE user_id = $1",
    "DELETE FROM invite_joins WHERE user_id = $1",
    "DELETE FROM marriages WHERE user_a = $1 OR user_b = $1",
    "DELETE FROM member_achievements WHERE user_id = $1",
    "DELETE FROM member_prestige WHERE user_id = $1",
    "DELETE FROM member_profiles WHERE user_id = $1",
    "DELETE FROM member_xp WHERE user_id = $1",
    "DELETE FROM news_prefs WHERE user_id = $1",
    "DELETE FROM rank_cards WHERE user_id = $1",
    "DELETE FROM stock_holdings WHERE user_id = $1",
    "DELETE FROM suggestion_votes WHERE user_id = $1",
    "DELETE FROM user_playlists WHERE user_id = $1",
    "DELETE FROM member_daily_stats WHERE user_id = $1",
    "DELETE FROM member_voice_stats WHERE user_id = $1",
];

/// Sunucunun bütün verisi. `$1` = sunucu kimliği. Sıra önemli: önce bağlı (alt) tablolar.
const GUILD_SQL: &[&str] = &[
    "DELETE FROM server_daily_stats WHERE guild_id = $1",
    "DELETE FROM channel_daily_stats WHERE guild_id = $1",
    "DELETE FROM member_daily_stats WHERE guild_id = $1",
    "DELETE FROM member_voice_stats WHERE guild_id = $1",
    "DELETE FROM giveaway_entries WHERE giveaway_id IN (SELECT id FROM giveaways WHERE guild_id = $1)",
    "DELETE FROM suggestion_votes WHERE suggestion_id IN (SELECT id FROM suggestions WHERE guild_id = $1)",
    "DELETE FROM applications WHERE guild_id = $1",
    "DELETE FROM economy_inventory WHERE guild_id = $1",
    "DELETE FROM ai_channels WHERE guild_id = $1",
    "DELETE FROM animal_catches WHERE guild_id = $1",
    "DELETE FROM appeal_tokens WHERE guild_id = $1",
    "DELETE FROM automod_allowed_words WHERE guild_id = $1",
    "DELETE FROM automod_settings WHERE guild_id = $1",
    "DELETE FROM automod_words WHERE guild_id = $1",
    "DELETE FROM autoresponses WHERE guild_id = $1",
    "DELETE FROM ban_appeals WHERE guild_id = $1",
    "DELETE FROM birthdays WHERE guild_id = $1",
    "DELETE FROM economy_accounts WHERE guild_id = $1",
    "DELETE FROM guild_backups WHERE guild_id = $1",
    "DELETE FROM highlights WHERE guild_id = $1",
    "DELETE FROM invite_joins WHERE guild_id = $1",
    "DELETE FROM level_rewards WHERE guild_id = $1",
    "DELETE FROM marriages WHERE guild_id = $1",
    "DELETE FROM member_achievements WHERE guild_id = $1",
    "DELETE FROM member_prestige WHERE guild_id = $1",
    "DELETE FROM member_profiles WHERE guild_id = $1",
    "DELETE FROM member_xp WHERE guild_id = $1",
    "DELETE FROM mod_cases WHERE guild_id = $1",
    "DELETE FROM mod_notes WHERE guild_id = $1",
    "DELETE FROM news_prefs WHERE guild_id = $1",
    "DELETE FROM plugin_settings WHERE guild_id = $1",
    "DELETE FROM quarantines WHERE guild_id = $1",
    "DELETE FROM rank_cards WHERE guild_id = $1",
    "DELETE FROM starboard_extra_posts WHERE guild_id = $1",
    "DELETE FROM starboard_posts WHERE guild_id = $1",
    "DELETE FROM stat_channels WHERE guild_id = $1",
    "DELETE FROM stock_holdings WHERE guild_id = $1",
    "DELETE FROM stream_subscriptions WHERE guild_id = $1",
    "DELETE FROM tags WHERE guild_id = $1",
    "DELETE FROM temp_voice_channels WHERE guild_id = $1",
    "DELETE FROM ticket_settings WHERE guild_id = $1",
    "DELETE FROM tickets WHERE guild_id = $1",
    "DELETE FROM warn_punishments WHERE guild_id = $1",
    "DELETE FROM xp_exclusions WHERE guild_id = $1",
    "DELETE FROM youtube_subscriptions WHERE guild_id = $1",
    "DELETE FROM application_forms WHERE guild_id = $1",
    "DELETE FROM shop_items WHERE guild_id = $1",
    "DELETE FROM giveaways WHERE guild_id = $1",
    "DELETE FROM suggestions WHERE guild_id = $1",
    "DELETE FROM guild_settings WHERE guild_id = $1",
];

/// Üyenin verisini siler; silinen satır sayısını döner. `guilds`: Redis'teki sunucu başına kayıtlar için.
pub async fn delete_user(db: &PgPool, redis: &ConnectionManager, guilds: &[GuildId], user: UserId) -> anyhow::Result<u64> {
    let id = snowflake(user.get())?;
    let mut tx = db.begin().await?;
    let mut rows = 0;
    for sql in USER_SQL {
        rows += sqlx::query(*sql).bind(id).execute(&mut *tx).await?.rows_affected();
    }
    tx.commit().await?;

    let now = serenity::Timestamp::now().unix_timestamp();
    let mut pipe = redis::pipe();
    for guild in guilds {
        let u = user.get();
        pipe.cmd("ZREM").arg(format!("xp:{guild}")).arg(u).ignore();
        for period in [crate::leveling::Period::Week, crate::leveling::Period::Month] {
            pipe.cmd("ZREM").arg(crate::leveling::period_key(*guild, period, now)).arg(u).ignore();
        }
        pipe.cmd("DEL").arg(format!("ach:{guild}:{u}")).arg(format!("rep:{guild}:{u}")).arg(format!("kalici-roller:{guild}:{u}")).ignore();
        pipe.cmd("HDEL").arg(format!("afk:{guild}")).arg(u).ignore();
        pipe.cmd("ZREM").arg(format!("katilim:{guild}")).arg(u).ignore();
        pipe.cmd("ZREM").arg(format!("klip:{guild}")).arg(format!("u:{u}")).ignore();
        crate::community::forget_afk(*guild, user);
    }
    let mut conn = redis.clone();
    pipe.query_async::<()>(&mut conn).await?;
    info!(user = %user, rows, "Üye verisi isteği üzerine silindi");
    Ok(rows)
}

/// Sunucunun bütün verisini siler (veritabanı + adında sunucu kimliği geçen Redis anahtarları).
pub async fn delete_guild(db: &PgPool, redis: &ConnectionManager, settings: &crate::settings::PluginSettings, guild: GuildId) -> anyhow::Result<u64> {
    let id = snowflake(guild.get())?;
    let mut tx = db.begin().await?;
    let mut rows = 0;
    for sql in GUILD_SQL {
        rows += sqlx::query(*sql).bind(id).execute(&mut *tx).await?.rows_affected();
    }
    tx.commit().await?;
    settings.forget_guild(guild);

    // Sunucu kimliği 17-20 haneli; başka bir kimliğin parçası olarak geçme ihtimali yok sayılır.
    let mut conn = redis.clone();
    let mut cursor: u64 = 0;
    loop {
        let (next, keys): (u64, Vec<String>) =
            redis::cmd("SCAN").arg(cursor).arg("MATCH").arg(format!("*{guild}*")).arg("COUNT").arg(1000).query_async(&mut conn).await?;
        if !keys.is_empty() {
            redis::cmd("DEL").arg(&keys).query_async::<()>(&mut conn).await?;
        }
        if next == 0 {
            break;
        }
        cursor = next;
    }
    info!(guild = %guild, rows, "Sunucu verisi silindi");
    Ok(rows)
}

fn left_key(guild: GuildId) -> String {
    format!("ayrildi:{guild}")
}

/// Bot sunucudan çıkarıldı (sunucu çökmesi değil): silme sayacı başlar.
pub async fn on_removed(redis: &ConnectionManager, guild: GuildId) {
    let mut conn = redis.clone();
    let now = serenity::Timestamp::now().unix_timestamp();
    let _: redis::RedisResult<()> = redis::cmd("SET").arg(left_key(guild)).arg(now).arg("NX").query_async(&mut conn).await;
    info!(guild = %guild, days = PURGE_AFTER_DAYS, "Bot sunucudan çıkarıldı; geri eklenmezse veri silinecek");
}

/// Bot sunucuya (yeniden) girdi: silme iptal.
pub async fn on_present(redis: &ConnectionManager, guild: GuildId) {
    let mut conn = redis.clone();
    let _: redis::RedisResult<()> = redis::cmd("DEL").arg(left_key(guild)).query_async(&mut conn).await;
}

/// Günde bir, çıkarılalı `PURGE_AFTER_DAYS` gün olan sunucuların verisini siler.
pub fn spawn_purge(ctx: serenity::Context, db: PgPool, redis: ConnectionManager, settings: crate::settings::PluginSettings) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(24 * 3600));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            crate::supervise::round("çıkarılan sunucu verisi", Duration::from_secs(600), async {
                let mut conn = redis.clone();
                let keys: Vec<String> = redis::cmd("KEYS").arg("ayrildi:*").query_async(&mut conn).await.unwrap_or_default();
                let now = serenity::Timestamp::now().unix_timestamp();
                for key in keys {
                    let Some(guild) = key.strip_prefix("ayrildi:").and_then(|g| g.parse::<u64>().ok()).filter(|g| *g != 0).map(GuildId::new) else { continue };
                    let since: i64 = redis::cmd("GET").arg(&key).query_async(&mut conn).await.unwrap_or(now);
                    if now - since < PURGE_AFTER_DAYS * 86_400 || ctx.cache.guild(guild).is_some() {
                        continue;
                    }
                    // Anahtar adında sunucu kimliği geçtiği için `delete_guild` onu da siler.
                    if let Err(e) = delete_guild(&db, &redis, &settings, guild).await {
                        warn!(guild = %guild, error = ?e, "Çıkarılan sunucunun verisi silinemedi");
                    }
                }
                if let Err(e) = crate::analytics::purge_old_stats(&db).await {
                    warn!(error = ?e, "Eski analitik verileri temizlenemedi");
                }
            })
            .await;
        }
    });
}

// ---------------------------------------------------------------- Gizlilik metni

pub const POLICY_TR: &str = "\
# AxBot Gizlilik Bildirimi

## Hangi veriler tutulur
- **Discord kimlikleri:** sunucu, kanal, rol ve kullanıcı kimlikleri (sayılar). Ad ve avatar saklanmaz; gerektiğinde Discord'dan okunur.
- **Etkinlik verileri:** XP ve seviye, ekonomi hesabı (cüzdan, banka, envanter, hisseler), başarımlar, prestij, itibar puanı, rütbe kartı ayarı, doğum günü (verirsen), kişisel çalma listeleri, haber tercihleri, vurgu kelimeleri, davetle gelme kaydı.
- **Moderasyon ve destek kayıtları:** uyarı, susturma, yasak gibi vakalar ve sebepleri, yetkili notları, karantina, biletler, ban itirazları, başvuru cevapları, öneriler.
- **Sunucu ayarları:** yöneticilerin bot ve panel üzerinden yaptığı ayarlar ve sunucu yapısı yedekleri (roller, kanallar, izinler; mesaj içeriği değil).

## Mesaj içerikleri
Mesajlar kalıcı olarak saklanmaz. İstisnalar:
- Sunucuda kayıt kanalı ve \"mesaj silindi\" kaydı açıksa, silinen mesajı kayda yazabilmek için mesajların bir kopyası **en fazla 6 saat** bellekte (Redis) tutulur.
- Yapay zekâ sohbetinde kanalın son mesajları, sohbetin devamı için **1 saat** tutulur.
- Otomatik moderasyon, spam ve tekrar tespiti için kısa süreli sayaçlar tutar (dakikalar).

## Üçüncü taraflar
- Yapay zekâya yazdığın mesajlar cevap üretilmesi için yapay zekâ sağlayıcısına (NVIDIA NIM, yedek olarak Google Gemini) gönderilir.
- Müzik, arama ve bildirim özellikleri YouTube, Spotify, Twitch, Kick, Reddit gibi servislerden herkese açık veri okur; bu servislere senin kişisel verin gönderilmez.
- Veriler satılmaz, reklam için kullanılmaz.

## Silme
- **Kendi verin:** `/bilgi verilerimi-sil` bütün sunuculardaki etkinlik verini siler. Moderasyon ve destek kayıtları sunucunun güvenliği için tutulur.
- **Sunucu verisi:** sunucu sahibi `/bilgi sunucu-verisini-sil` ile hepsini siler (bot sunucudan da çıkar). Bot sunucudan çıkarılırsa ve 30 gün içinde geri eklenmezse sunucunun bütün verisi otomatik silinir.
- Günlük veritabanı yedekleri 7 gün sonra silinir; silinen veri en geç bu sürede yedeklerden de kalkar.

## İletişim
Soruların için botun sahibine Discord üzerinden yazabilirsin.
";

pub const POLICY_EN: &str = "\
# AxBot Privacy Notice

## What is stored
- **Discord IDs:** server, channel, role and user IDs (numbers). Names and avatars are not stored; they are read from Discord when needed.
- **Activity data:** XP and levels, economy account (wallet, bank, inventory, shares), achievements, prestige, reputation, rank card style, birthday (if you set it), personal playlists, news preferences, highlight words, invite joins.
- **Moderation and support records:** cases such as warnings, timeouts and bans with their reasons, staff notes, quarantines, tickets, ban appeals, application answers, suggestions.
- **Server settings:** settings made by admins in the bot and the dashboard, and server structure backups (roles, channels, permissions; no message content).

## Message content
Messages are not stored permanently. Exceptions:
- If a server has a log channel with \"message deleted\" logging on, a copy of messages is kept in memory (Redis) for **at most 6 hours** so deleted messages can be logged.
- In AI chat, the channel's recent messages are kept for **1 hour** to continue the conversation.
- Auto-moderation keeps short-lived counters (minutes) for spam and repetition checks.

## Third parties
- Messages you send to the AI are sent to the AI provider (NVIDIA NIM, Google Gemini as a fallback) to generate a reply.
- Music, search and notification features read public data from services such as YouTube, Spotify, Twitch, Kick and Reddit; your personal data is not sent to them.
- Data is never sold or used for advertising.

## Deletion
- **Your data:** `/bilgi verilerimi-sil` deletes your activity data in every server. Moderation and support records are kept for server safety.
- **Server data:** the server owner can delete everything with `/bilgi sunucu-verisini-sil` (the bot also leaves). If the bot is removed and not re-added within 30 days, all server data is deleted automatically.
- Daily database backups are deleted after 7 days; deleted data leaves the backups within that time.

## Contact
For questions, message the bot owner on Discord.
";

/// Basit markdown (`#`, `##`, `- `, `**…**`, `` `…` ``) → HTML. Metin bizim sabitimiz; yine de kaçırılır.
pub fn policy_html(markdown: &str) -> String {
    let inline = |line: &str| {
        let mut out = String::new();
        let escaped = crate::web::escape(line);
        let mut bold = false;
        let mut code = false;
        let mut rest = escaped.as_str();
        while let Some(i) = rest.find(['*', '`']) {
            out.push_str(&rest[..i]);
            if rest[i..].starts_with("**") {
                out.push_str(if bold { "</b>" } else { "<b>" });
                bold = !bold;
                rest = &rest[i + 2..];
            } else if rest[i..].starts_with('`') {
                out.push_str(if code { "</code>" } else { "<code>" });
                code = !code;
                rest = &rest[i + 1..];
            } else {
                out.push('*');
                rest = &rest[i + 1..];
            }
        }
        out.push_str(rest);
        out
    };
    let mut html = String::new();
    let mut in_list = false;
    for line in markdown.lines() {
        let is_item = line.starts_with("- ");
        if in_list && !is_item {
            html.push_str("</ul>");
            in_list = false;
        }
        if let Some(h) = line.strip_prefix("## ") {
            html.push_str(&format!("<h2>{}</h2>", inline(h)));
        } else if let Some(h) = line.strip_prefix("# ") {
            html.push_str(&format!("<h1>{}</h1>", inline(h)));
        } else if let Some(item) = line.strip_prefix("- ") {
            if !in_list {
                html.push_str("<ul>");
                in_list = true;
            }
            html.push_str(&format!("<li>{}</li>", inline(item)));
        } else if !line.trim().is_empty() {
            html.push_str(&format!("<p>{}</p>", inline(line)));
        }
    }
    if in_list {
        html.push_str("</ul>");
    }
    html
}

/// `GET /gizlilik` (Türkçe) ve `GET /privacy` (İngilizce): oturumsuz, herkese açık.
pub async fn page_tr() -> axum::response::Html<String> {
    page(POLICY_TR, "tr", "<a href=\"/privacy\">English</a>")
}

pub async fn page_en() -> axum::response::Html<String> {
    page(POLICY_EN, "en", "<a href=\"/gizlilik\">Türkçe</a>")
}

fn page(markdown: &str, lang: &str, switch: &str) -> axum::response::Html<String> {
    axum::response::Html(format!(
        "<!doctype html><html lang=\"{lang}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>AxBot</title><style>body{{font-family:system-ui,sans-serif;max-width:760px;margin:0 auto;padding:16px;line-height:1.55;\
         background:#fff;color:#1d1d1f}}code{{background:#0001;padding:1px 4px;border-radius:4px}}\
         @media(prefers-color-scheme:dark){{body{{background:#17181c;color:#e6e6e6}}code{{background:#fff2}}a{{color:#8ab4ff}}}}</style></head>\
         <body><p style=\"text-align:right\">{switch}</p>{}</body></html>",
        policy_html(markdown)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_renders_without_raw_markup() {
        let html = policy_html("# Başlık\n- **kalın** ve `kod` <b>\nmetin");
        assert_eq!(html, "<h1>Başlık</h1><ul><li><b>kalın</b> ve <code>kod</code> &lt;b&gt;</li></ul><p>metin</p>");
        for text in [POLICY_TR, POLICY_EN] {
            let html = policy_html(text);
            assert!(!html.contains("**") && !html.contains('`'), "biçim işareti kaldı");
        }
    }

    /// Veritabanındaki her `guild_id` sütunlu tablo silme listesinde mi? (yeni tablo eklenince unutulmasın)
    #[tokio::test]
    #[ignore = "canlı veritabanı gerekir"]
    async fn live_guild_purge_covers_every_table() -> anyhow::Result<()> {
        let _ = dotenvy::dotenv();
        let url = std::env::var("DATABASE_URL")?;
        let db = crate::db::connect(&crate::config::DatabaseConfig { url, max_connections: 2 }).await?;
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT table_name::text FROM information_schema.columns WHERE table_schema = 'public' AND column_name = 'guild_id' AND table_name NOT LIKE '\\_%'",
        )
        .fetch_all(&db)
        .await?;
        for table in tables {
            assert!(GUILD_SQL.iter().any(|sql| sql.contains(&format!("FROM {table} "))), "{table} silme listesinde yok");
        }
        // Silme SQL'i gerçekten çalışıyor mu: var olmayan sunucuda, geri alınan işlemde.
        let mut tx = db.begin().await?;
        for sql in GUILD_SQL {
            sqlx::query(*sql).bind(1_i64).execute(&mut *tx).await?;
        }
        for sql in USER_SQL {
            sqlx::query(*sql).bind(1_i64).execute(&mut *tx).await?;
        }
        tx.rollback().await?;
        Ok(())
    }
}
