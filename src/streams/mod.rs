//! Twitch / Kick canlı yayın duyuruları.
//!
//! Arka plan görevi belirli aralıklarla tüm abone kanalları toplu sorgular (platform başına
//! tek istek / 100 kanal). Yeni bir yayın görülünce abone sunuculara duyuru atılır; aynı yayın
//! `last_stream_id` sayesinde bot yeniden başlasa bile iki kez duyurulmaz.

mod kick;
mod token;
mod twitch;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateActionRow, CreateAllowedMentions,
    CreateButton, CreateEmbed, CreateEmbedAuthor, CreateMessage, GuildId, Http, Mentionable,
    RoleId,
};
use sqlx::PgPool;
use tracing::{debug, info, warn};

use self::kick::Kick;
use self::twitch::Twitch;
use crate::config::OAuthApp;
use crate::db::snowflake;

pub const MAX_SUBSCRIPTIONS_PER_GUILD: i64 = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, poise::ChoiceParameter)]
pub enum Platform {
    #[name = "Twitch"]
    Twitch,
    #[name = "Kick"]
    Kick,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Twitch => "twitch",
            Self::Kick => "kick",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "twitch" => Some(Self::Twitch),
            "kick" => Some(Self::Kick),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Twitch => "Twitch",
            Self::Kick => "Kick",
        }
    }

    pub fn env_hint(self) -> &'static str {
        match self {
            Self::Twitch => "TWITCH_CLIENT_ID / TWITCH_CLIENT_SECRET",
            Self::Kick => "KICK_CLIENT_ID / KICK_CLIENT_SECRET",
        }
    }

    fn colour(self) -> serenity::Colour {
        match self {
            Self::Twitch => serenity::Colour::from_rgb(145, 70, 255),
            Self::Kick => serenity::Colour::from_rgb(83, 252, 24),
        }
    }
}

/// Platformdan bağımsız canlı yayın bilgisi.
pub struct LiveStream {
    pub display_name: String,
    pub stream_id: String,
    pub title: String,
    pub category: Option<String>,
    pub viewers: Option<u64>,
    pub thumbnail: Option<String>,
    pub url: String,
}

pub struct Subscription {
    pub platform: Platform,
    pub login: String,
    pub channel_id: ChannelId,
    pub role_id: Option<RoleId>,
}

pub struct Streams {
    client: reqwest::Client,
    twitch: Option<Twitch>,
    kick: Option<Kick>,
}

impl Streams {
    pub fn new(client: reqwest::Client, twitch: Option<OAuthApp>, kick: Option<OAuthApp>) -> Self {
        Self {
            client,
            twitch: twitch.map(Twitch::new),
            kick: kick.map(Kick::new),
        }
    }

    pub fn is_configured(&self, platform: Platform) -> bool {
        match platform {
            Platform::Twitch => self.twitch.is_some(),
            Platform::Kick => self.kick.is_some(),
        }
    }

    /// Twitch kanalının profili. Twitch anahtarları tanımlı değilse hata döner.
    pub async fn twitch_profile(&self, login: &str) -> anyhow::Result<Option<twitch::User>> {
        match &self.twitch {
            Some(t) => t.profile(&self.client, login).await,
            None => anyhow::bail!("Twitch yapılandırılmamış"),
        }
    }

    /// Kanal platformda varsa görünen adını döner.
    pub async fn resolve(&self, platform: Platform, login: &str) -> anyhow::Result<Option<String>> {
        match platform {
            Platform::Twitch => match &self.twitch {
                Some(t) => t.display_name(&self.client, login).await,
                None => Ok(None),
            },
            Platform::Kick => match &self.kick {
                Some(k) => Ok(k.exists(&self.client, login).await?.then(|| login.to_owned())),
                None => Ok(None),
            },
        }
    }

    async fn live(&self, platform: Platform, logins: &[String]) -> anyhow::Result<HashMap<String, LiveStream>> {
        match platform {
            Platform::Twitch => match &self.twitch {
                Some(t) => t.live(&self.client, logins).await,
                None => Ok(HashMap::new()),
            },
            Platform::Kick => match &self.kick {
                Some(k) => k.live(&self.client, logins).await,
                None => Ok(HashMap::new()),
            },
        }
    }
}

/// "https://twitch.tv/Ad" gibi bağlantılardan veya düz addan kanal adını çıkarır.
pub fn normalize_login(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches('/');
    let login = trimmed.rsplit('/').next().unwrap_or(trimmed).trim_start_matches('@');
    let login = login.to_lowercase();
    let valid = (2..=25).contains(&login.len())
        && login
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then_some(login)
}

pub async fn count(db: &PgPool, guild_id: GuildId) -> anyhow::Result<i64> {
    let n = sqlx::query_scalar("SELECT COUNT(*) FROM stream_subscriptions WHERE guild_id = $1")
        .bind(snowflake(guild_id.get())?)
        .fetch_one(db)
        .await?;
    Ok(n)
}

/// Aboneliği ekler veya (aynı kanal varsa) duyuru kanalı/rolünü günceller.
pub async fn upsert(db: &PgPool, guild_id: GuildId, sub: &Subscription) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO stream_subscriptions (guild_id, platform, login, channel_id, role_id)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (guild_id, platform, login) DO UPDATE
             SET channel_id = EXCLUDED.channel_id, role_id = EXCLUDED.role_id",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(sub.platform.as_str())
    .bind(&sub.login)
    .bind(snowflake(sub.channel_id.get())?)
    .bind(sub.role_id.map(|r| snowflake(r.get())).transpose()?)
    .execute(db)
    .await?;
    Ok(())
}

/// Silindiyse `true`.
pub async fn remove(db: &PgPool, guild_id: GuildId, platform: Platform, login: &str) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "DELETE FROM stream_subscriptions WHERE guild_id = $1 AND platform = $2 AND login = $3",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(platform.as_str())
    .bind(login)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list(db: &PgPool, guild_id: GuildId) -> anyhow::Result<Vec<Subscription>> {
    let rows: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT platform, login, channel_id, role_id FROM stream_subscriptions
         WHERE guild_id = $1 ORDER BY platform, login",
    )
    .bind(snowflake(guild_id.get())?)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(platform, login, channel, role)| {
            Some(Subscription {
                platform: Platform::from_db(&platform)?,
                login,
                channel_id: ChannelId::new(to_id(channel)?),
                role_id: to_id(role.unwrap_or(0)).map(RoleId::new),
            })
        })
        .collect())
}

/// Bot açık kaldığı sürece yayınları periyodik kontrol eder.
pub fn spawn_poller(
    ctx: serenity::Context,
    db: PgPool,
    streams: Arc<Streams>,
    settings: crate::settings::PluginSettings,
    http_client: reqwest::Client,
    event_bus: tokio::sync::broadcast::Sender<crate::panel::RealtimeEvent>,
    interval: Duration,
) {
    if streams.twitch.is_none() && streams.kick.is_none() {
        info!("Twitch/Kick anahtarları yok, yayın duyuruları kapalı");
        return;
    }
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            crate::supervise::round("yayın duyuruları", std::time::Duration::from_secs(300), async {
                if let Err(e) = poll_once(&ctx.http, &ctx.cache, &db, &streams, &settings, &http_client, &event_bus).await {
                    warn!(error = ?e, "Yayın kontrolü başarısız");
                }
            })
            .await;
        }
    });
}

#[derive(sqlx::FromRow)]
struct DbRow {
    id: i64,
    guild_id: i64,
    platform: String,
    login: String,
    channel_id: i64,
    role_id: Option<i64>,
    last_stream_id: Option<String>,
}

struct Row {
    id: i64,
    guild_id: GuildId,
    platform: Platform,
    login: String,
    channel_id: ChannelId,
    role_id: Option<RoleId>,
    last_stream_id: Option<String>,
}

async fn poll_once(
    http: &Http,
    cache: &serenity::Cache,
    db: &PgPool,
    streams: &Streams,
    settings: &crate::settings::PluginSettings,
    http_client: &reqwest::Client,
    event_bus: &tokio::sync::broadcast::Sender<crate::panel::RealtimeEvent>,
) -> anyhow::Result<()> {
    let raw: Vec<DbRow> = sqlx::query_as(
        "SELECT id, guild_id, platform, login, channel_id, role_id, last_stream_id FROM stream_subscriptions",
    )
    .fetch_all(db)
    .await?;
    let rows: Vec<Row> = raw
        .into_iter()
        .filter_map(|r| {
            Some(Row {
                id: r.id,
                guild_id: GuildId::new(to_id(r.guild_id)?),
                platform: Platform::from_db(&r.platform)?,
                login: r.login,
                channel_id: ChannelId::new(to_id(r.channel_id)?),
                role_id: to_id(r.role_id.unwrap_or(0)).map(RoleId::new),
                last_stream_id: r.last_stream_id,
            })
        })
        .collect();
    if rows.is_empty() {
        return Ok(());
    }

    for platform in [Platform::Twitch, Platform::Kick] {
        let mut logins: Vec<String> = rows
            .iter()
            .filter(|r| r.platform == platform)
            .map(|r| r.login.clone())
            .collect();
        logins.sort();
        logins.dedup();
        if logins.is_empty() || !streams.is_configured(platform) {
            continue;
        }

        let live = match streams.live(platform, &logins).await {
            Ok(live) => live,
            Err(e) => {
                warn!(platform = platform.label(), error = ?e, "Canlı yayınlar sorgulanamadı");
                continue;
            }
        };
        debug!(platform = platform.label(), checked = logins.len(), live = live.len(), "Yayın kontrolü");

        for row in rows.iter().filter(|r| r.platform == platform) {
            match live.get(&row.login) {
                Some(stream) => {
                    // Yayın canlı
                    if row.last_stream_id.as_deref() == Some(stream.stream_id.as_str()) {
                        continue;
                    }

                    // Yeni yayın tespit edildi: önce kaydet ki hata olsa bile tekrar tekrar spam yapmasın
                    if let Err(e) = sqlx::query("UPDATE stream_subscriptions SET last_stream_id = $1 WHERE id = $2")
                        .bind(&stream.stream_id)
                        .bind(row.id)
                        .execute(db)
                        .await
                    {
                        warn!(row_id = row.id, error = ?e, "last_stream_id güncellenemedi");
                    }
                    announce(http, platform, row, stream).await;

                    // Realtime event
                    let _ = event_bus.send(crate::panel::RealtimeEvent::StreamStatus {
                        guild_id: row.guild_id.get(),
                        platform: platform.as_str().to_owned(),
                        streamer: stream.display_name.clone(),
                        live: true,
                        title: Some(stream.title.clone()),
                        game: stream.category.clone(),
                        viewers: stream.viewers,
                    });

                    // Otomasyon tetikleyici
                    crate::automations::run_stream(
                        http,
                        cache,
                        db,
                        settings,
                        http_client,
                        crate::automations::StreamEventContext {
                            guild_id: row.guild_id,
                            platform: platform.as_str(),
                            streamer: &stream.display_name,
                            live: true,
                            title: Some(&stream.title),
                            game: stream.category.as_deref(),
                            viewers: stream.viewers,
                        },
                    )
                    .await;
                }
                None => {
                    // Yayın kapandı: eğer daha önce yayındaysa (last_stream_id doluysa)
                    if row.last_stream_id.is_some() {
                        if let Err(e) = sqlx::query("UPDATE stream_subscriptions SET last_stream_id = NULL WHERE id = $1")
                            .bind(row.id)
                            .execute(db)
                            .await
                        {
                            warn!(row_id = row.id, error = ?e, "last_stream_id sıfırlanamadı");
                        }

                        // Realtime event
                        let _ = event_bus.send(crate::panel::RealtimeEvent::StreamStatus {
                            guild_id: row.guild_id.get(),
                            platform: platform.as_str().to_owned(),
                            streamer: row.login.clone(),
                            live: false,
                            title: None,
                            game: None,
                            viewers: None,
                        });

                        // Otomasyon tetikleyici
                        crate::automations::run_stream(
                            http,
                            cache,
                            db,
                            settings,
                            http_client,
                            crate::automations::StreamEventContext {
                                guild_id: row.guild_id,
                                platform: platform.as_str(),
                                streamer: &row.login,
                                live: false,
                                title: None,
                                game: None,
                                viewers: None,
                            },
                        )
                        .await;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn announce(http: &Http, platform: Platform, row: &Row, stream: &LiveStream) {
    let mut embed = CreateEmbed::new()
        .author(CreateEmbedAuthor::new(format!(
            "{} şimdi {}'te yayında!",
            stream.display_name,
            platform.label()
        )))
        .title(if stream.title.is_empty() {
            "Canlı yayın"
        } else {
            stream.title.as_str()
        })
        .url(&stream.url)
        .colour(platform.colour())
        .timestamp(serenity::Timestamp::now());
    if let Some(category) = &stream.category {
        embed = embed.field("Kategori", category, true);
    }
    if let Some(viewers) = stream.viewers {
        embed = embed.field("İzleyici", viewers.to_string(), true);
    }
    if let Some(thumbnail) = &stream.thumbnail {
        embed = embed.image(thumbnail);
    }

    let mut content = format!("🔴 **{}** yayında! {}", stream.display_name, stream.url);
    let mut mentions = CreateAllowedMentions::new();
    if let Some(role) = row.role_id {
        content = format!("{} {content}", role.mention());
        mentions = mentions.roles(vec![role]);
    }

    let button = CreateButton::new_link(&stream.url).label("Yayına Git");
    let message = CreateMessage::new()
        .content(content)
        .embed(embed)
        .allowed_mentions(mentions)
        .components(vec![CreateActionRow::Buttons(vec![button])]);

    match row.channel_id.send_message(http, message).await {
        Ok(_) => info!(platform = platform.label(), login = %row.login, channel = %row.channel_id, "Yayın duyuruldu"),
        Err(e) => warn!(
            platform = platform.label(),
            login = %row.login,
            channel = %row.channel_id,
            error = %e,
            "Yayın duyurusu gönderilemedi"
        ),
    }
}

fn to_id(raw: i64) -> Option<u64> {
    u64::try_from(raw).ok().filter(|id| *id != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_links_and_names() {
        assert_eq!(normalize_login("https://www.twitch.tv/Pokimane/"), Some("pokimane".into()));
        assert_eq!(normalize_login("https://kick.com/xQc"), Some("xqc".into()));
        assert_eq!(normalize_login("@elraenn"), Some("elraenn".into()));
        assert_eq!(normalize_login("wtcn"), Some("wtcn".into()));
        assert_eq!(normalize_login("geçersiz ad"), None);
        assert_eq!(normalize_login("a"), None);
    }
}
