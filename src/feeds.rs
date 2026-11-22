//! Sosyal akış duyuruları: Reddit, Bluesky, RSS/Atom ve podcast. Hepsi herkese açık akışlardan
//! okunur (hesap ya da API anahtarı gerekmez) ve 5 dakikada bir kontrol edilir.
//!
//! Akışlar `PluginSettings` deposunda durur. Yeni paylaşımlar, daha önce görülenlerin kimlik
//! listesiyle (`seen`) karşılaştırılarak bulunur; akış eklenirken var olanlar görülmüş sayılır.
//!
//! X, Instagram ve TikTok herkese açık akış sunmaz (ücretli API ister). Bunlar için bir RSS köprüsü
//! (ör. RSSHub) adresi "RSS" türüyle eklenebilir.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Context as _};
use poise::serenity_prelude::{self as serenity, ChannelId, CreateAllowedMentions, CreateMessage, GuildId, Mentionable, RoleId};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{debug, info, warn};

use crate::settings::{PluginConfig, PluginSettings};

pub const MAX_FEEDS: usize = 20;
pub const MAX_MESSAGE_LEN: usize = 500;
const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_FEED_BYTES: usize = 3 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
/// Bir kontrolde akış başına en fazla bu kadar yeni paylaşım duyurulur.
const MAX_NEW_PER_POLL: usize = 3;
/// Akış başına hatırlanan paylaşım kimliği sayısı.
const MAX_SEEN: usize = 150;
/// Reddit tarayıcıya benzemeyen istemcileri reddeder.
const USER_AGENT: &str = concat!("Mozilla/5.0 (compatible; AxBot/", env!("CARGO_PKG_VERSION"), "; Discord bot feed reader)");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, poise::ChoiceParameter)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[name = "Reddit (subreddit adı)"]
    Reddit,
    #[name = "Bluesky (kullanıcı adı)"]
    Bluesky,
    #[name = "RSS / Atom (https adresi)"]
    Rss,
    #[name = "Podcast (RSS adresi)"]
    Podcast,
    #[name = "X / Twitter (kullanıcı adı; RSS köprüsü gerekir)"]
    X,
    #[name = "Instagram (kullanıcı adı; RSS köprüsü gerekir)"]
    Instagram,
    #[name = "TikTok (kullanıcı adı; RSS köprüsü gerekir)"]
    Tiktok,
}

impl Kind {
    pub const ALL: [Self; 7] = [Self::Reddit, Self::Bluesky, Self::Rss, Self::Podcast, Self::X, Self::Instagram, Self::Tiktok];

    /// Köprü üzerindeki yol (RSSHub adresleme düzeni); köprü gerektirmeyen türlerde `None`.
    fn bridge_path(self, name: &str) -> Option<String> {
        match self {
            Self::X => Some(format!("/twitter/user/{name}")),
            Self::Instagram => Some(format!("/instagram/user/{name}")),
            Self::Tiktok => Some(format!("/tiktok/user/@{name}")),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reddit => "reddit",
            Self::Bluesky => "bluesky",
            Self::Rss => "rss",
            Self::Podcast => "podcast",
            Self::X => "x",
            Self::Instagram => "instagram",
            Self::Tiktok => "tiktok",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == raw)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Reddit => "Reddit",
            Self::Bluesky => "Bluesky",
            Self::Rss => "RSS",
            Self::Podcast => "Podcast",
            Self::X => "X",
            Self::Instagram => "Instagram",
            Self::Tiktok => "TikTok",
        }
    }

    fn emoji(self) -> &'static str {
        match self {
            Self::Reddit => "🟠",
            Self::Bluesky => "🦋",
            Self::Rss => "📰",
            Self::Podcast => "🎙️",
            Self::X => "🐦",
            Self::Instagram => "📸",
            Self::Tiktok => "🎵",
        }
    }

    fn default_message(self) -> &'static str {
        match self {
            Self::Reddit => "**{kaynak}** içinde yeni gönderi: **{başlık}**\n{bağlantı}",
            Self::Bluesky => "**{kaynak}** yeni bir gönderi paylaştı:\n{bağlantı}",
            Self::Rss => "**{kaynak}**: **{başlık}**\n{bağlantı}",
            Self::Podcast => "**{kaynak}** yeni bölüm yayınladı: **{başlık}**\n{bağlantı}",
            Self::X | Self::Instagram | Self::Tiktok => "**{kaynak}** yeni bir paylaşım yaptı:\n{bağlantı}",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    /// Sunucu içinde benzersiz numara.
    pub id: u32,
    pub kind: Kind,
    /// Okunan akış adresi.
    pub url: String,
    /// Görünen ad (ör. `r/turkey`, `@ad.bsky.social`, akışın başlığı).
    pub name: String,
    pub channel: u64,
    #[serde(default)]
    pub role: Option<u64>,
    /// Özel duyuru metni; boşsa türün hazır metni.
    #[serde(default)]
    pub message: Option<String>,
    /// Daha önce görülen paylaşımların kimlikleri (en yeni sonda).
    #[serde(default)]
    pub seen: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedSettings {
    pub feeds: Vec<Feed>,
}

impl PluginConfig for FeedSettings {
    const KEY: &'static str = "feeds";
}

impl FeedSettings {
    pub fn next_id(&self) -> u32 {
        self.feeds.iter().map(|f| f.id).max().unwrap_or(0) + 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub link: String,
}

#[derive(Debug)]
pub struct Parsed {
    pub title: Option<String>,
    /// Akıştaki sırayla (genellikle en yeni başta).
    pub items: Vec<Item>,
}

/// Kullanıcının yazdığı kaynağı (subreddit adı, Bluesky adı, adres) akış adresine ve görünen ada çevirir.
pub fn resolve_source(kind: Kind, input: &str) -> Result<(String, String), &'static str> {
    resolve_with_bridge(kind, input, crate::integrations::get().rss_bridge_url.as_deref())
}

/// X / Instagram / TikTok için köprü adresi tanımlı değilse dönen hata.
pub const BRIDGE_MISSING: &str = "X, Instagram ve TikTok herkese açık akış sunmuyor. Bunlar için `.env` dosyasına `RSS_BRIDGE_URL` (RSSHub uyumlu bir köprünün adresi) yazılıp bot yeniden başlatılmalı.";

fn resolve_with_bridge(kind: Kind, input: &str, bridge: Option<&str>) -> Result<(String, String), &'static str> {
    let input = input.trim();
    match kind {
        Kind::Tiktok => {
            let name = input.trim_end_matches('/').rsplit('/').next().unwrap_or_default().trim_start_matches('@');
            let valid = (1..=30).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
            if !valid {
                return Err("Kullanıcı adını yaz (ör. `nasa`); harf, rakam, nokta ve alt çizgi içerebilir.");
            }
            if let Some(bridge) = bridge {
                if let Some(path) = kind.bridge_path(name) {
                    return Ok((format!("{bridge}{path}"), format!("@{name}")));
                }
            }
            Ok((format!("https://www.tiktok.com/@{name}"), format!("@{name}")))
        }
        Kind::X | Kind::Instagram => {
            let name = input.trim_end_matches('/').rsplit('/').next().unwrap_or_default().trim_start_matches('@');
            let valid = (1..=30).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
            if !valid {
                return Err("Kullanıcı adını yaz (ör. `nasa`); harf, rakam, nokta ve alt çizgi içerebilir.");
            }
            let bridge = bridge.ok_or(BRIDGE_MISSING)?;
            let path = kind.bridge_path(name).ok_or(BRIDGE_MISSING)?;
            Ok((format!("{bridge}{path}"), format!("@{name}")))
        }
        Kind::Reddit => {
            let name = input.trim_end_matches('/').rsplit('/').next().unwrap_or_default().trim_start_matches("r/");
            let valid = (2..=21).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                return Err("Subreddit adını yaz (ör. `turkey` ya da `r/turkey`).");
            }
            Ok((format!("https://www.reddit.com/r/{name}/new/.rss"), format!("r/{name}")))
        }
        Kind::Bluesky => {
            let handle = input.trim_end_matches('/').rsplit('/').next().unwrap_or_default().trim_start_matches('@').to_lowercase();
            let valid = (3..=253).contains(&handle.len())
                && handle.contains('.')
                && !handle.starts_with('.')
                && !handle.ends_with('.')
                && handle.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
            if !valid {
                return Err("Bluesky kullanıcı adını tam yaz (ör. `ad.bsky.social`).");
            }
            Ok((format!("https://bsky.app/profile/{handle}/rss"), format!("@{handle}")))
        }
        Kind::Rss | Kind::Podcast => {
            let ok = input.starts_with("https://") && input.len() <= 400 && url::Url::parse(input).is_ok_and(|u| u.host_str().is_some());
            if !ok {
                return Err("`https://` ile başlayan bir akış (RSS/Atom) adresi ver.");
            }
            Ok((input.to_owned(), String::new()))
        }
    }
}

pub fn extract_reddit_sub(url: &str) -> Option<String> {
    let lower = url.to_lowercase();
    let idx = lower.find("reddit.com/r/")?;
    let rest = &url[idx + "reddit.com/r/".len()..];
    let sub = rest.split(['/', '?', '#']).next()?;
    let sub = sub.trim_start_matches("r/");
    if (2..=21).contains(&sub.len()) && sub.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(sub.to_owned())
    } else {
        None
    }
}

pub fn extract_bluesky_handle(url: &str) -> Option<String> {
    let lower = url.to_lowercase();
    let idx = lower.find("bsky.app/profile/")?;
    let rest = &url[idx + "bsky.app/profile/".len()..];
    let handle = rest.split(['/', '?', '#']).next()?;
    let handle = handle.trim_start_matches('@');
    if (3..=253).contains(&handle.len()) && handle.contains('.') {
        Some(handle.to_owned())
    } else {
        None
    }
}

pub fn extract_tiktok_user(url: &str) -> Option<String> {
    let lower = url.to_lowercase();
    let idx = lower.find("tiktok.com/@")?;
    let rest = &url[idx + "tiktok.com/@".len()..];
    let user = rest.split(['/', '?', '#']).next()?;
    if (1..=30).contains(&user.len()) {
        Some(user.to_owned())
    } else {
        None
    }
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

pub fn parse_reddit_json(json_str: &str) -> anyhow::Result<Parsed> {
    let val: serde_json::Value = serde_json::from_str(json_str)?;
    let children = val
        .get("data")
        .and_then(|d| d.get("children"))
        .and_then(|c| c.as_array())
        .ok_or_else(|| anyhow::anyhow!("Geçersiz Reddit JSON formatı"))?;

    let mut items = Vec::new();
    for child in children {
        let Some(data) = child.get("data") else { continue };
        let id = data.get("id").and_then(|i| i.as_str()).unwrap_or_default();
        let title = data.get("title").and_then(|t| t.as_str()).unwrap_or_default();
        let permalink = data.get("permalink").and_then(|p| p.as_str()).unwrap_or_default();
        let over_18 = data.get("over_18").and_then(|o| o.as_bool()).unwrap_or(false);
        if id.is_empty() || permalink.is_empty() {
            continue;
        }
        let full_id = format!("t3_{id}");
        let title_with_nsfw = if over_18 {
            format!("[NSFW] {title}")
        } else {
            title.to_owned()
        };
        let link = format!("https://www.reddit.com{permalink}");
        items.push(Item {
            id: full_id,
            title: title_with_nsfw,
            link,
        });
    }
    Ok(Parsed {
        title: None,
        items,
    })
}

pub async fn fetch_reddit_json(sub: &str) -> anyhow::Result<Parsed> {
    let client = crate::net::fetch_client()?;
    let url = format!("https://www.reddit.com/r/{sub}/new.json?limit=10");
    let res = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await?;
    if !res.status().is_success() {
        bail!("Reddit HTTP {}", res.status());
    }
    let text = res.text().await?;
    parse_reddit_json(&text)
}

pub fn parse_bluesky_json(json_str: &str) -> anyhow::Result<Parsed> {
    let val: serde_json::Value = serde_json::from_str(json_str)?;
    let feed = val
        .get("feed")
        .and_then(|f| f.as_array())
        .ok_or_else(|| anyhow::anyhow!("Geçersiz Bluesky JSON formatı"))?;

    let mut items = Vec::new();
    for item in feed {
        if item.get("reply").is_some() {
            continue;
        }
        let Some(post) = item.get("post") else { continue };
        let uri = post.get("uri").and_then(|u| u.as_str()).unwrap_or_default();
        let handle = post
            .get("author")
            .and_then(|a| a.get("handle"))
            .and_then(|h| h.as_str())
            .unwrap_or_default();
        let rkey = uri.rsplit('/').next().unwrap_or("post");
        let text = post
            .get("record")
            .and_then(|r| r.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or_default();

        if uri.is_empty() || handle.is_empty() {
            continue;
        }
        let id = post.get("cid").and_then(|c| c.as_str()).unwrap_or(uri).to_owned();
        let link = format!("https://bsky.app/profile/{handle}/post/{rkey}");
        let clean_text = clean(text);
        let title = if clean_text.is_empty() {
            "Yeni gönderi".to_owned()
        } else {
            clean_text.chars().take(200).collect()
        };
        items.push(Item { id, title, link });
    }
    Ok(Parsed {
        title: None,
        items,
    })
}

pub async fn fetch_bluesky_feed(handle: &str) -> anyhow::Result<Parsed> {
    let client = crate::net::fetch_client()?;
    let url = format!("https://public.api.bsky.app/xrpc/app.bsky.feed.getAuthorFeed?actor={handle}&limit=10&filter=posts_no_replies");
    let res = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await?;
    if !res.status().is_success() {
        bail!("Bluesky HTTP {}", res.status());
    }
    let text = res.text().await?;
    parse_bluesky_json(&text)
}

pub fn parse_tiktok_oembed(json_str: &str, user: &str) -> anyhow::Result<Parsed> {
    let val: serde_json::Value = serde_json::from_str(json_str)?;
    let title = val
        .get("title")
        .and_then(|t| t.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Yeni TikTok videosu");
    let author = val
        .get("author_unique_id")
        .or_else(|| val.get("author_name"))
        .and_then(|a| a.as_str())
        .unwrap_or(user);
    let link = format!("https://www.tiktok.com/@{author}");
    let id = format!("tiktok_{author}_{:x}", hash_str(title));
    Ok(Parsed {
        title: Some(format!("@{author}")),
        items: vec![Item {
            id,
            title: title.to_owned(),
            link,
        }],
    })
}

pub async fn fetch_tiktok_oembed(user: &str) -> anyhow::Result<Parsed> {
    let client = crate::net::fetch_client()?;
    let url = format!("https://www.tiktok.com/oembed?url=https://www.tiktok.com/@{user}");
    let res = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await?;
    if !res.status().is_success() {
        bail!("TikTok HTTP {}", res.status());
    }
    let text = res.text().await?;
    parse_tiktok_oembed(&text, user)
}

/// Akışı indirir. Yönlendirmeler elle izlenir; her adım "herkese açık adres mi" diye denetlenir
/// (kullanıcının verdiği adres botu iç ağdaki bir servise yönlendiremesin).
pub async fn fetch(url: &str) -> anyhow::Result<Parsed> {
    if let Some(sub) = extract_reddit_sub(url) {
        if let Ok(parsed) = fetch_reddit_json(&sub).await {
            if !parsed.items.is_empty() {
                return Ok(parsed);
            }
        }
    } else if let Some(handle) = extract_bluesky_handle(url) {
        if let Ok(parsed) = fetch_bluesky_feed(&handle).await {
            if !parsed.items.is_empty() {
                return Ok(parsed);
            }
        }
    } else if let Some(user) = extract_tiktok_user(url) {
        if let Ok(parsed) = fetch_tiktok_oembed(&user).await {
            if !parsed.items.is_empty() {
                return Ok(parsed);
            }
        }
    }

    parse(&fetch_text(url).await?)
}

/// Akış metnini indirir (aynı güvenlik denetimleriyle).
pub async fn fetch_text(url: &str) -> anyhow::Result<String> {
    let client = crate::net::fetch_client()?;
    let mut current = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        // Köprü adresini bot sahibi `.env`'e yazar (iç ağda olabilir); kullanıcıdan yalnızca denetlenmiş kullanıcı adı gelir.
        let trusted = crate::integrations::get().rss_bridge_url.as_deref().is_some_and(|bridge| is_under(&current, bridge));
        if !trusted && (!current.starts_with("https://") || !crate::music::extra::is_public_url(&current).await) {
            bail!("adres herkese açık bir https adresi değil");
        }
        let mut response = client
            .get(&current)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/atom+xml, application/rss+xml, application/xml, text/xml, */*;q=0.5")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("akışa ulaşılamadı")?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|l| l.to_str().ok())
                .context("yönlendirme adresi yok")?;
            current = url::Url::parse(&current)?.join(location)?.to_string();
            continue;
        }
        if !response.status().is_success() {
            bail!("akış HTTP {} döndü", response.status());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > MAX_FEED_BYTES {
                bail!("akış çok büyük");
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok(decode_text(&bytes));
    }
    bail!("çok fazla yönlendirme")
}

/// İndirilen metni çözer: UTF-8 değilse (XML başlığında `ISO-8859-1` / `windows-1252` yazıyorsa) Latin-1 olarak
/// okunur. Örneğin Golem'in akışı Latin-1; UTF-8 sanılsa "jährlich" bozuk görünür.
pub fn decode_text(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]).to_lowercase();
    if ["iso-8859-1", "iso-8859-15", "windows-1252", "latin1", "latin-1"].iter().any(|e| head.contains(e)) {
        return bytes.iter().map(|b| char::from(*b)).collect();
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Adres köprünün altında mı? (`https://kopru` → `https://kopru/yol`; `https://kopru.kotu.com` sayılmaz)
fn is_under(url: &str, bridge: &str) -> bool {
    url.strip_prefix(bridge).is_some_and(|rest| rest.starts_with('/'))
}

/// RSS 2.0 (`<item>`) ve Atom (`<entry>`) akışlarını ayrıştırır (basit yapı; XML kütüphanesi gerekmez).
pub fn parse(xml: &str) -> anyhow::Result<Parsed> {
    let item_re = Regex::new(r"(?s)<(item|entry)[\s>](.*?)</(?:item|entry)>")?;
    let tag = |name: &str| Regex::new(&format!(r"(?s)<{name}(?:\s[^>]*)?>(.*?)</{name}>"));
    let (title_re, guid_re, id_re, link_re) = (tag("title")?, tag("guid")?, tag("id")?, tag("link")?);
    let href_re = Regex::new(r#"(?s)<link\s[^>]*?href\s*=\s*["']([^"']+)["'][^>]*>"#)?;
    let alternate_re = Regex::new(r#"(?s)<link\s[^>]*?rel\s*=\s*["']alternate["'][^>]*>"#)?;
    let enclosure_re = Regex::new(r#"(?s)<enclosure\s[^>]*?url\s*=\s*["']([^"']+)["']"#)?;
    let first = |re: &Regex, body: &str| re.captures(body).and_then(|c| c.get(1)).map(|m| clean(m.as_str())).filter(|s| !s.is_empty());

    if !xml.contains("<rss") && !xml.contains("<feed") && !xml.contains("<rdf:RDF") {
        bail!("bu bir RSS/Atom akışı değil");
    }
    let head_end = item_re.find(xml).map_or(xml.len(), |m| m.start());
    let title = first(&title_re, &xml[..head_end]);

    let items = item_re
        .captures_iter(xml)
        .filter_map(|found| {
            let body = found.get(2)?.as_str();
            // Atom: rel="alternate" olan bağlantı yeğlenir; RSS: <link>metin</link>.
            let link = alternate_re
                .find(body)
                .and_then(|m| first(&href_re, m.as_str()))
                .or_else(|| first(&link_re, body))
                .or_else(|| first(&href_re, body))
                .or_else(|| first(&enclosure_re, body))
                .filter(|l| l.starts_with("http://") || l.starts_with("https://"))?;
            let id = first(&guid_re, body).or_else(|| first(&id_re, body)).unwrap_or_else(|| link.clone());
            let title = first(&title_re, body).unwrap_or_default();
            Some(Item { id, title, link })
        })
        .collect();
    Ok(Parsed { title, items })
}

/// CDATA sarmalını, etiketleri ve HTML kaçışlarını temizler.
fn clean(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw);
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for c in raw.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Görülmemiş paylaşımları (eskiden yeniye, en fazla birkaç tane) döner ve hepsini görülmüş sayar.
/// İlk okumada (`seen` boş) hiçbiri duyurulmaz.
pub fn take_new(seen: &mut Vec<String>, items: &[Item]) -> Vec<Item> {
    let first_read = seen.is_empty();
    let mut fresh: Vec<Item> = items.iter().filter(|i| !seen.contains(&i.id)).cloned().collect();
    // Akışta en yeni başta durur: görülenler listesine eskiden yeniye eklenir.
    for item in fresh.iter().rev() {
        seen.push(item.id.clone());
    }
    if seen.len() > MAX_SEEN {
        seen.drain(..seen.len() - MAX_SEEN);
    }
    if first_read {
        return Vec::new();
    }
    fresh.truncate(MAX_NEW_PER_POLL);
    fresh.reverse();
    fresh
}

/// Duyuru metni. Paylaşım başlığındaki etiketler etkisizdir (bkz. gönderimdeki `allowed_mentions`).
pub fn render(feed: &Feed, item: &Item) -> String {
    let template = feed.message.as_deref().filter(|m| !m.trim().is_empty()).unwrap_or(feed.kind.default_message());
    let title: String = if item.title.is_empty() { "(başlıksız)".to_owned() } else { item.title.chars().take(200).collect() };
    let body = template
        .replace("{başlık}", &title.replace('`', "'"))
        .replace("{bağlantı}", &item.link)
        .replace("{kaynak}", &feed.name);
    let mention = feed.role.map(|r| format!("{} ", RoleId::new(r.max(1)).mention())).unwrap_or_default();
    format!("{mention}{} {body}", feed.kind.emoji()).chars().take(1900).collect()
}

pub fn spawn_poller(ctx: serenity::Context, db: PgPool, settings: PluginSettings) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            crate::supervise::round("sosyal akışlar", std::time::Duration::from_secs(300), async {
                // Aynı akış birden çok sunucuda olabilir; tur başına bir kez okunur.
                let mut fetched: HashMap<String, Option<Vec<Item>>> = HashMap::new();
                for guild_id in ctx.cache.guilds() {
                    if let Err(e) = poll_guild(&ctx, &db, &settings, guild_id, &mut fetched).await {
                        warn!(guild = %guild_id, error = ?e, "Akış kontrolü başarısız");
                    }
                }
            })
            .await;
        }
    });
}

async fn poll_guild(
    ctx: &serenity::Context,
    db: &PgPool,
    settings: &PluginSettings,
    guild_id: GuildId,
    fetched: &mut HashMap<String, Option<Vec<Item>>>,
) -> anyhow::Result<()> {
    let urls: Vec<String> = settings.get::<FeedSettings>(db, guild_id).await?.feeds.iter().map(|f| f.url.clone()).collect();
    if urls.is_empty() {
        return Ok(());
    }
    for url in urls {
        if fetched.get(&url).is_none() {
            let items = match fetch(&url).await {
                Ok(parsed) => Some(parsed.items),
                Err(e) => {
                    debug!(url, error = ?e, "Akış okunamadı");
                    None
                }
            };
            fetched.insert(url, items);
        }
    }

    // İndirme sırasında panelden değişiklik yapılmış olabilir: güncel ayar yeniden okunur.
    let mut updated = (*settings.get::<FeedSettings>(db, guild_id).await?).clone();
    let mut announcements = Vec::new();
    let mut changed = false;
    for feed in &mut updated.feeds {
        let Some(Some(items)) = fetched.get(&feed.url) else {
            continue;
        };
        let before = feed.seen.len();
        let last = feed.seen.last().cloned();
        let fresh = take_new(&mut feed.seen, items);
        changed |= feed.seen.len() != before || feed.seen.last() != last.as_ref();
        announcements.extend(fresh.into_iter().map(|item| (feed.clone(), item)));
    }
    if !changed {
        return Ok(());
    }
    // Önce görülenler yazılır: gönderim uzun sürse bile aynı paylaşım iki kez duyurulmaz.
    settings.set(db, guild_id, updated).await?;

    for (feed, item) in announcements {
        let mentions = CreateAllowedMentions::new().roles(feed.role.map(|r| RoleId::new(r.max(1))).into_iter().collect::<Vec<_>>());
        let message = CreateMessage::new().content(render(&feed, &item)).allowed_mentions(mentions);
        match ChannelId::new(feed.channel.max(1)).send_message(&ctx.http, message).await {
            Ok(_) => info!(guild = %guild_id, feed = %feed.name, "Akış paylaşımı duyuruldu"),
            Err(e) => warn!(guild = %guild_id, feed = %feed.name, error = %e, "Akış duyurusu gönderilemedi"),
        }
    }
    Ok(())
}

/// Yeni akışı doğrular (okunabiliyor mu) ve kaydedilecek hâlini hazırlar. Hata metni kullanıcıya gösterilir.
pub async fn prepare(
    kind: Kind,
    source: &str,
    channel: u64,
    role: Option<u64>,
    message: Option<String>,
    id: u32,
) -> Result<Feed, &'static str> {
    if message.as_deref().is_some_and(|m| m.chars().count() > MAX_MESSAGE_LEN) {
        return Err("Duyuru metni en fazla 500 karakter olabilir.");
    }
    let (url, name) = resolve_source(kind, source)?;
    let parsed = fetch(&url).await.map_err(|e| {
        debug!(url, error = ?e, "Yeni akış okunamadı");
        "Bu kaynak okunamadı. Adın/adresin doğru ve herkese açık olduğundan emin ol (akış bir RSS/Atom adresi olmalı)."
    })?;
    let name = if name.is_empty() {
        parsed.title.map(|t| t.chars().take(80).collect()).unwrap_or_else(|| "Akış".to_owned())
    } else {
        name
    };
    let mut seen = Vec::new();
    take_new(&mut seen, &parsed.items);
    Ok(Feed { id, kind, url, name, channel, role, message, seen })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_latin1_feeds() {
        let bytes = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><title>j\xe4hrlich</title>";
        assert!(decode_text(bytes).contains("jährlich"));
        assert_eq!(decode_text("çğü".as_bytes()), "çğü");
    }

    const RSS: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel><title><![CDATA[Haber & Blog]]></title>
<link>https://ornek.com</link>
<item><title>Üçüncü &amp; son</title><link>https://ornek.com/3</link><guid isPermaLink="false">g3</guid></item>
<item><title><![CDATA[İkinci <b>yazı</b>]]></title><link>https://ornek.com/2</link></item>
<item><title>Bölüm 1</title><enclosure url="https://ornek.com/1.mp3" type="audio/mpeg"/><guid>g1</guid></item>
<item><title>Bağlantısız</title></item>
</channel></rss>"#;

    const ATOM: &str = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>r/test</title>
<entry><id>t3_b</id><link href="https://www.reddit.com/r/test/b"/><title>Yeni gönderi</title></entry>
<entry><id>t3_a</id><link rel="self" href="https://api/a"/><link rel="alternate" type="text/html" href="https://www.reddit.com/r/test/a"/><title>Eski</title></entry>
</feed>"#;

    #[test]
    fn parses_rss() -> anyhow::Result<()> {
        let parsed = parse(RSS)?;
        assert_eq!(parsed.title.as_deref(), Some("Haber & Blog"));
        let got: Vec<(&str, &str, &str)> = parsed.items.iter().map(|i| (i.id.as_str(), i.title.as_str(), i.link.as_str())).collect();
        assert_eq!(
            got,
            [
                ("g3", "Üçüncü & son", "https://ornek.com/3"),
                ("https://ornek.com/2", "İkinci yazı", "https://ornek.com/2"),
                ("g1", "Bölüm 1", "https://ornek.com/1.mp3"),
            ]
        );
        Ok(())
    }

    #[test]
    fn parses_atom_and_prefers_alternate_link() -> anyhow::Result<()> {
        let parsed = parse(ATOM)?;
        assert_eq!(parsed.title.as_deref(), Some("r/test"));
        let links: Vec<&str> = parsed.items.iter().map(|i| i.link.as_str()).collect();
        assert_eq!(links, ["https://www.reddit.com/r/test/b", "https://www.reddit.com/r/test/a"]);
        assert_eq!(parsed.items.first().map(|i| i.id.as_str()), Some("t3_b"));
        assert!(parse("<html><body>sayfa</body></html>").is_err());
        Ok(())
    }

    fn item(id: &str) -> Item {
        Item { id: id.to_owned(), title: id.to_owned(), link: format!("https://x/{id}") }
    }

    #[test]
    fn first_read_announces_nothing_then_only_new_ones() {
        let mut seen = Vec::new();
        assert!(take_new(&mut seen, &[item("b"), item("a")]).is_empty());
        assert_eq!(seen, ["a", "b"]);
        // Akış: en yeni başta. Duyuru: eskiden yeniye.
        let fresh = take_new(&mut seen, &[item("d"), item("c"), item("b"), item("a")]);
        assert_eq!(fresh.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["c", "d"]);
        assert!(take_new(&mut seen, &[item("d"), item("c")]).is_empty());
    }

    #[test]
    fn floods_are_capped_and_memory_is_bounded() {
        let mut seen = vec!["eski".to_owned()];
        let many: Vec<Item> = (0..400).map(|i| item(&format!("n{i}"))).collect();
        let fresh = take_new(&mut seen, &many);
        // En yeni üç paylaşım (akışın başındakiler), eskiden yeniye.
        assert_eq!(fresh.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["n2", "n1", "n0"]);
        assert_eq!(seen.len(), MAX_SEEN);
        assert_eq!(seen.last().map(String::as_str), Some("n0"));
        // Hepsi görüldü: bir sonraki turda yeniden duyurulmaz.
        assert!(take_new(&mut seen, &many[..100]).is_empty());
    }

    #[test]
    fn resolves_sources() {
        assert_eq!(
            resolve_source(Kind::Reddit, "https://www.reddit.com/r/Turkey/"),
            Ok(("https://www.reddit.com/r/Turkey/new/.rss".to_owned(), "r/Turkey".to_owned()))
        );
        assert_eq!(resolve_source(Kind::Reddit, "r/rust").map(|r| r.1), Ok("r/rust".to_owned()));
        assert!(resolve_source(Kind::Reddit, "a b").is_err());
        assert!(resolve_source(Kind::Reddit, "x?y=1").is_err());
        assert_eq!(
            resolve_source(Kind::Bluesky, "@Ad.bsky.social"),
            Ok(("https://bsky.app/profile/ad.bsky.social/rss".to_owned(), "@ad.bsky.social".to_owned()))
        );
        assert!(resolve_source(Kind::Bluesky, "adsiz").is_err());
        assert!(resolve_source(Kind::Rss, "http://ornek.com/rss").is_err());
        assert!(resolve_source(Kind::Rss, "https://ornek.com/rss").is_ok());
        assert!(resolve_source(Kind::Podcast, "dosya.xml").is_err());
    }

    #[test]
    fn social_kinds_need_a_bridge() {
        assert_eq!(resolve_with_bridge(Kind::X, "@nasa", None), Err(BRIDGE_MISSING));
        assert_eq!(
            resolve_with_bridge(Kind::X, "https://x.com/NASA/", Some("http://rsshub:1200")),
            Ok(("http://rsshub:1200/twitter/user/NASA".to_owned(), "@NASA".to_owned()))
        );
        assert_eq!(
            resolve_with_bridge(Kind::Tiktok, "@ad.soyad", Some("https://kopru.ornek.com")).map(|r| r.0),
            Ok("https://kopru.ornek.com/tiktok/user/@ad.soyad".to_owned())
        );
        assert_eq!(resolve_with_bridge(Kind::Instagram, "nasa", Some("https://k")).map(|r| r.0), Ok("https://k/instagram/user/nasa".to_owned()));
        // Yol ya da sorgu eklemeye çalışan adlar reddedilir.
        for bad in ["a/../b", "a?x=1", "a b", "", "a#b"] {
            assert!(resolve_with_bridge(Kind::X, bad, Some("https://k")).is_err_and(|e| e != BRIDGE_MISSING) || bad == "a/../b", "{bad}");
        }
        assert!(is_under("http://rsshub:1200/twitter/user/a", "http://rsshub:1200"));
        assert!(!is_under("http://rsshub:1200.kotu.com/x", "http://rsshub:1200"));
        assert!(!is_under("http://rsshub:1200", "http://rsshub:1200"));
    }

    #[test]
    fn renders_template_with_role() {
        let mut feed = Feed {
            id: 1,
            kind: Kind::Reddit,
            url: String::new(),
            name: "r/test".to_owned(),
            channel: 1,
            role: Some(42),
            message: None,
            seen: Vec::new(),
        };
        let text = render(&feed, &item("a"));
        assert!(text.starts_with("<@&42> 🟠 **r/test** içinde yeni gönderi: **a**"), "{text}");
        assert!(text.ends_with("https://x/a"));
        feed.role = None;
        feed.message = Some("{kaynak} | {başlık} | {bağlantı}".to_owned());
        assert_eq!(render(&feed, &item("a")), "🟠 r/test | a | https://x/a");
    }

    #[test]
    fn kinds_round_trip() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(Kind::parse("faks"), None);
    }

    #[test]
    fn extract_social_handles_and_subs() {
        assert_eq!(extract_reddit_sub("https://www.reddit.com/r/rust/new/.rss"), Some("rust".to_owned()));
        assert_eq!(extract_reddit_sub("https://reddit.com/r/turkey"), Some("turkey".to_owned()));
        assert_eq!(extract_reddit_sub("https://other.com/r/test"), None);

        assert_eq!(extract_bluesky_handle("https://bsky.app/profile/user.bsky.social/rss"), Some("user.bsky.social".to_owned()));
        assert_eq!(extract_bluesky_handle("https://bsky.app/profile/handle.com"), Some("handle.com".to_owned()));
        assert_eq!(extract_bluesky_handle("https://other.com/profile/test"), None);

        assert_eq!(extract_tiktok_user("https://www.tiktok.com/@creator"), Some("creator".to_owned()));
        assert_eq!(extract_tiktok_user("https://tiktok.com/@cool.dancer"), Some("cool.dancer".to_owned()));
        assert_eq!(extract_tiktok_user("https://other.com/@creator"), None);
    }

    #[test]
    fn parses_reddit_json_with_nsfw_and_permalink() -> anyhow::Result<()> {
        let json = r#"{
            "data": {
                "children": [
                    {
                        "data": {
                            "id": "post123",
                            "title": "Clean Post",
                            "permalink": "/r/rust/comments/post123/clean_post/",
                            "over_18": false
                        }
                    },
                    {
                        "data": {
                            "id": "post456",
                            "title": "Spicy Post",
                            "permalink": "/r/rust/comments/post456/spicy_post/",
                            "over_18": true
                        }
                    }
                ]
            }
        }"#;

        let parsed = parse_reddit_json(json)?;
        assert_eq!(parsed.items.len(), 2);
        assert_eq!(parsed.items[0].id, "t3_post123");
        assert_eq!(parsed.items[0].title, "Clean Post");
        assert_eq!(parsed.items[0].link, "https://www.reddit.com/r/rust/comments/post123/clean_post/");

        assert_eq!(parsed.items[1].id, "t3_post456");
        assert_eq!(parsed.items[1].title, "[NSFW] Spicy Post");
        Ok(())
    }

    #[test]
    fn parses_bluesky_json_filters_replies() -> anyhow::Result<()> {
        let json = r#"{
            "feed": [
                {
                    "post": {
                        "uri": "at://did:plc:123/app.bsky.feed.post/3abcdef",
                        "cid": "bafyreicid1",
                        "author": { "handle": "author.bsky.social" },
                        "record": { "text": "First main post!", "createdAt": "2026-11-21T12:00:00Z" }
                    }
                },
                {
                    "post": {
                        "uri": "at://did:plc:123/app.bsky.feed.post/3reply",
                        "cid": "bafyreicid2",
                        "author": { "handle": "author.bsky.social" },
                        "record": { "text": "This is a reply", "createdAt": "2026-11-21T12:05:00Z" }
                    },
                    "reply": { "root": { "uri": "..." } }
                }
            ]
        }"#;

        let parsed = parse_bluesky_json(json)?;
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].id, "bafyreicid1");
        assert_eq!(parsed.items[0].title, "First main post!");
        assert_eq!(parsed.items[0].link, "https://bsky.app/profile/author.bsky.social/post/3abcdef");
        Ok(())
    }

    #[test]
    fn parses_tiktok_oembed_feed() -> anyhow::Result<()> {
        let json = r#"{
            "title": "Fun dance video #trending",
            "author_unique_id": "dancer",
            "author_name": "Dancer Pro"
        }"#;

        let parsed = parse_tiktok_oembed(json, "dancer")?;
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].title, "Fun dance video #trending");
        assert_eq!(parsed.items[0].link, "https://www.tiktok.com/@dancer");
        assert!(parsed.items[0].id.starts_with("tiktok_dancer_"));
        Ok(())
    }

    /// `cargo test live_feeds -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "gerçek sitelere istek atar"]
    async fn live_feeds() -> anyhow::Result<()> {
        for (kind, source) in [
            (Kind::Reddit, "rust"),
            (Kind::Bluesky, "bsky.app"),
            (Kind::Rss, "https://blog.rust-lang.org/feed.xml"),
            (Kind::Podcast, "https://feeds.npr.org/510289/podcast.xml"),
        ] {
            let started = std::time::Instant::now();
            match prepare(kind, source, 1, None, None, 1).await {
                Ok(feed) => println!("{} {source} → {} · {} paylaşım ({:.1} sn)", kind.label(), feed.name, feed.seen.len(), started.elapsed().as_secs_f64()),
                Err(e) => println!("{} {source} → HATA: {e}", kind.label()),
            }
        }
        Ok(())
    }
}
