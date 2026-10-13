//! YouTube yeni video duyuruları. API anahtarı gerekmez: YouTube'un herkese açık RSS akışı
//! (`feeds/videos.xml?channel_id=…`) 5 dakikada bir okunur.
//!
//! Kanal bağlantısı (`@ad`, `/channel/UC…`) eklenirken kanal kimliğine çevrilir ve en son video
//! işaretlenir; böylece eklerken eski videolar duyurulmaz.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _};
use poise::serenity_prelude::{ChannelId, CreateAllowedMentions, CreateMessage, GuildId, Http, Mentionable, RoleId};
use regex::Regex;
use sqlx::PgPool;
use tracing::{debug, info, warn};

use crate::db::snowflake;
use crate::guild_config::to_id;

const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub const MAX_PER_GUILD: i64 = 25;
/// Bir kontrolde kanal başına en fazla bu kadar yeni video duyurulur.
const MAX_NEW_PER_POLL: usize = 3;
/// AB çerez onayı sayfasına yönlenmemek için.
const CONSENT_COOKIE: &str = "SOCS=CAI; CONSENT=YES+1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Video {
    pub id: String,
    pub title: String,
}

pub struct Feed {
    pub channel_name: String,
    /// En yeni başta.
    pub videos: Vec<Video>,
}

/// Bağlantıyı / `@ad`'ı kanal kimliğine (UC…) çevirir.
pub async fn resolve_channel(client: &reqwest::Client, input: &str) -> anyhow::Result<String> {
    let input = input.trim().trim_end_matches('/');
    let id_re = Regex::new(r"(UC[A-Za-z0-9_-]{22})")?;
    if let Some(m) = Regex::new(r"/channel/(UC[A-Za-z0-9_-]{22})")?.captures(input).and_then(|c| c.get(1)) {
        return Ok(m.as_str().to_owned());
    }
    if input.starts_with("UC") && id_re.is_match(input) && input.len() == 24 {
        return Ok(input.to_owned());
    }

    let url = if input.starts_with("http://") || input.starts_with("https://") {
        input.to_owned()
    } else {
        format!("https://www.youtube.com/@{}", input.trim_start_matches('@'))
    };
    if !url.contains("youtube.com/") {
        bail!("bu bir YouTube kanal bağlantısı değil");
    }
    let html = client
        .get(&url)
        .header(reqwest::header::COOKIE, CONSENT_COOKIE)
        .header(reqwest::header::ACCEPT_LANGUAGE, "tr-TR,tr;q=0.9")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .context("YouTube'a ulaşılamadı")?
        .error_for_status()
        .context("YouTube kanalı bulunamadı")?
        .text()
        .await?;
    // Sayfadaki asıl kanalın kimliği (başka kanallar da geçebildiği için önce kesin alanlar).
    for pattern in [
        r#"<link rel="canonical" href="https://www\.youtube\.com/channel/(UC[A-Za-z0-9_-]{22})""#,
        r#""externalId":"(UC[A-Za-z0-9_-]{22})""#,
        r#"<meta itemprop="identifier" content="(UC[A-Za-z0-9_-]{22})""#,
    ] {
        if let Some(m) = Regex::new(pattern)?.captures(&html).and_then(|c| c.get(1)) {
            return Ok(m.as_str().to_owned());
        }
    }
    Err(anyhow!("kanal kimliği sayfada bulunamadı"))
}

pub async fn fetch_feed(client: &reqwest::Client, channel_id: &str) -> anyhow::Result<Feed> {
    let xml = client
        .get(format!("https://www.youtube.com/feeds/videos.xml?channel_id={channel_id}"))
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .context("YouTube akışına ulaşılamadı")?
        .error_for_status()
        .context("YouTube akışı okunamadı")?
        .text()
        .await?;
    parse_feed(&xml)
}

/// Atom akışını ayrıştırır (basit yapı; XML kütüphanesi gerekmez).
fn parse_feed(xml: &str) -> anyhow::Result<Feed> {
    let entry_re = Regex::new(r"(?s)<entry>(.*?)</entry>")?;
    let id_re = Regex::new(r"<yt:videoId>([^<]+)</yt:videoId>")?;
    let title_re = Regex::new(r"(?s)<title>(.*?)</title>")?;

    let head = xml.split("<entry>").next().unwrap_or_default();
    let channel_name = title_re
        .captures(head)
        .and_then(|c| c.get(1))
        .map(|m| unescape(m.as_str()))
        .unwrap_or_else(|| "YouTube kanalı".to_owned());

    let videos = entry_re
        .captures_iter(xml)
        .filter_map(|entry| {
            let body = entry.get(1)?.as_str();
            let id = id_re.captures(body)?.get(1)?.as_str().to_owned();
            let title = title_re.captures(body).and_then(|c| c.get(1)).map_or_else(String::new, |m| unescape(m.as_str()));
            Some(Video { id, title })
        })
        .collect();
    Ok(Feed { channel_name, videos })
}

fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// `last` görülen son video; ondan yeni videolar eskiden yeniye (en fazla birkaç tane).
fn new_videos<'a>(feed: &'a [Video], last: Option<&str>) -> Vec<&'a Video> {
    let Some(last) = last else {
        return Vec::new();
    };
    let Some(position) = feed.iter().position(|v| v.id == last) else {
        // Son video akışta yoksa (silinmiş olabilir) sadece en yenisi duyurulur.
        return feed.first().into_iter().collect();
    };
    feed.iter().take(position).take(MAX_NEW_PER_POLL).rev().collect()
}

pub struct Subscription {
    pub yt_channel_id: String,
    pub name: String,
    pub channel_id: ChannelId,
    pub role_id: Option<RoleId>,
}

pub async fn add(db: &PgPool, guild_id: GuildId, sub: &Subscription, last_video: Option<&str>) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO youtube_subscriptions (guild_id, yt_channel_id, name, channel_id, role_id, last_video_id)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (guild_id, yt_channel_id) DO UPDATE
             SET name = EXCLUDED.name, channel_id = EXCLUDED.channel_id, role_id = EXCLUDED.role_id",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(&sub.yt_channel_id)
    .bind(&sub.name)
    .bind(snowflake(sub.channel_id.get())?)
    .bind(sub.role_id.map(|r| snowflake(r.get())).transpose()?)
    .bind(last_video)
    .execute(db)
    .await?;
    Ok(())
}

/// Ada veya kanal kimliğine göre siler.
pub async fn remove(db: &PgPool, guild_id: GuildId, name_or_id: &str) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "DELETE FROM youtube_subscriptions WHERE guild_id = $1 AND (yt_channel_id = $2 OR lower(name) = lower($2))",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(name_or_id.trim())
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list(db: &PgPool, guild_id: GuildId) -> anyhow::Result<Vec<Subscription>> {
    let rows: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT yt_channel_id, name, channel_id, role_id FROM youtube_subscriptions WHERE guild_id = $1 ORDER BY name",
    )
    .bind(snowflake(guild_id.get())?)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(yt, name, channel, role)| {
            Some(Subscription {
                yt_channel_id: yt,
                name,
                channel_id: ChannelId::new(to_id(Some(channel))?),
                role_id: to_id(role).map(RoleId::new),
            })
        })
        .collect())
}

pub async fn count(db: &PgPool, guild_id: GuildId) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM youtube_subscriptions WHERE guild_id = $1")
        .bind(snowflake(guild_id.get())?)
        .fetch_one(db)
        .await?)
}

pub fn spawn_poller(http: Arc<Http>, db: PgPool, client: reqwest::Client) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            crate::supervise::round("YouTube duyuruları", std::time::Duration::from_secs(300), async {
                if let Err(e) = poll_once(&http, &db, &client).await {
                    warn!(error = ?e, "YouTube kontrolü başarısız");
                }
            })
            .await;
        }
    });
}

#[derive(sqlx::FromRow)]
struct Row {
    guild_id: i64,
    yt_channel_id: String,
    name: String,
    channel_id: i64,
    role_id: Option<i64>,
    last_video_id: Option<String>,
}

async fn poll_once(http: &Http, db: &PgPool, client: &reqwest::Client) -> anyhow::Result<()> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT guild_id, yt_channel_id, name, channel_id, role_id, last_video_id FROM youtube_subscriptions",
    )
    .fetch_all(db)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }

    // Aynı YouTube kanalı birden çok sunucuda olabilir; akış bir kez okunur.
    let mut feeds: HashMap<String, Option<Feed>> = HashMap::new();
    for row in &rows {
        if !feeds.contains_key(&row.yt_channel_id) {
            let feed = match fetch_feed(client, &row.yt_channel_id).await {
                Ok(feed) => Some(feed),
                Err(e) => {
                    debug!(channel = %row.yt_channel_id, error = ?e, "YouTube akışı okunamadı");
                    None
                }
            };
            feeds.insert(row.yt_channel_id.clone(), feed);
        }
    }

    for row in rows {
        let Some(Some(feed)) = feeds.get(&row.yt_channel_id) else {
            continue;
        };
        let Some(latest) = feed.videos.first() else {
            continue;
        };
        if row.last_video_id.as_deref() == Some(latest.id.as_str()) {
            continue;
        }
        let fresh = new_videos(&feed.videos, row.last_video_id.as_deref());
        sqlx::query("UPDATE youtube_subscriptions SET last_video_id = $3 WHERE guild_id = $1 AND yt_channel_id = $2")
            .bind(row.guild_id)
            .bind(&row.yt_channel_id)
            .bind(&latest.id)
            .execute(db)
            .await?;

        let Some(channel) = to_id(Some(row.channel_id)).map(ChannelId::new) else {
            continue;
        };
        let role = to_id(row.role_id).map(RoleId::new);
        for video in fresh {
            let mut text = String::new();
            if let Some(role) = role {
                text.push_str(&format!("{} ", role.mention()));
            }
            text.push_str(&format!(
                "📺 **{}** yeni bir video yükledi: **{}**\nhttps://www.youtube.com/watch?v={}",
                row.name, video.title, video.id
            ));
            let mentions = CreateAllowedMentions::new().roles(role.into_iter().collect::<Vec<_>>());
            if let Err(e) = channel.send_message(http, CreateMessage::new().content(text).allowed_mentions(mentions)).await {
                warn!(channel = %channel, error = %e, "YouTube duyurusu gönderilemedi");
            } else {
                info!(guild = row.guild_id, youtube = %row.name, video = %video.id, "YouTube videosu duyuruldu");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0"?><feed><title>Kanal &amp; Ad</title>
<entry><yt:videoId>v3</yt:videoId><title>Üçüncü &quot;video&quot;</title></entry>
<entry><yt:videoId>v2</yt:videoId><title>İkinci</title></entry>
<entry><yt:videoId>v1</yt:videoId><title>Birinci</title></entry></feed>"#;

    #[test]
    fn parses_atom_feed() -> anyhow::Result<()> {
        let feed = parse_feed(FEED)?;
        assert_eq!(feed.channel_name, "Kanal & Ad");
        let ids: Vec<&str> = feed.videos.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["v3", "v2", "v1"]);
        assert_eq!(feed.videos.first().map(|v| v.title.as_str()), Some("Üçüncü \"video\""));
        Ok(())
    }

    /// `cargo test live_resolves_youtube_channel -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "gerçek YouTube'a istek atar"]
    async fn live_resolves_youtube_channel() -> anyhow::Result<()> {
        let client = crate::net::http_client()?;
        for input in ["@YouTube", "https://www.youtube.com/@barisozcan", "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ"] {
            let started = std::time::Instant::now();
            let id = resolve_channel(&client, input).await?;
            let feed = fetch_feed(&client, &id).await?;
            println!(
                "{input} → {id} · {} · {} video · son: {:?} ({:.1} sn)",
                feed.channel_name,
                feed.videos.len(),
                feed.videos.first().map(|v| &v.title),
                started.elapsed().as_secs_f64()
            );
            assert!(id.starts_with("UC") && id.len() == 24);
            assert!(!feed.videos.is_empty());
        }
        Ok(())
    }

    #[test]
    fn finds_new_videos_oldest_first() -> anyhow::Result<()> {
        let feed = parse_feed(FEED)?;
        let ids = |v: Vec<&Video>| v.into_iter().map(|v| v.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(new_videos(&feed.videos, Some("v1"))), ["v2", "v3"]);
        assert!(new_videos(&feed.videos, Some("v3")).is_empty());
        assert!(new_videos(&feed.videos, None).is_empty());
        assert_eq!(ids(new_videos(&feed.videos, Some("silinmiş"))), ["v3"]);
        Ok(())
    }
}
