//! Şarkı sözü sağlayıcıları ve önbellek soyutlaması.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CACHE_TTL: Duration = Duration::from_secs(30 * 60); // 30 dakika

/// Çözümlenmiş şarkı sözü bilgisi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lyrics {
    pub title: String,
    pub artist: String,
    pub text: String,
    #[serde(default)]
    pub synced: Option<String>,
}

/// Şarkı sözü servisi soyutlaması.
#[async_trait::async_trait]
pub trait LyricsProvider: Send + Sync {

    /// Şarkı ve opsiyonel sanatçı adı ile söz arar.
    async fn fetch_lyrics(&self, client: &reqwest::Client, query: &str) -> anyhow::Result<Option<Lyrics>>;
}

/// lrclib.net (ücretsiz, anahtar gerektirmez) sağlayıcısı.
#[derive(Default)]
pub struct LrclibProvider;

#[async_trait::async_trait]
impl LyricsProvider for LrclibProvider {

    async fn fetch_lyrics(&self, client: &reqwest::Client, query: &str) -> anyhow::Result<Option<Lyrics>> {
        let resp = client
            .get("https://lrclib.net/api/search")
            .query(&[("q", query)])
            .header(reqwest::header::USER_AGENT, concat!("AxBot/", env!("CARGO_PKG_VERSION")))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("lrclib.net servisine ulaşılamadı")?
            .error_for_status()
            .context("lrclib.net arama hatası")?
            .text()
            .await?;

        let results: Vec<serde_json::Value> = serde_json::from_str(&resp).unwrap_or_default();
        Ok(parse_lrclib_results(&results))
    }
}

/// lrclib.net JSON yanıtından ilk geçerli şarkı sözünü ayıklar.
pub fn parse_lrclib_results(results: &[serde_json::Value]) -> Option<Lyrics> {
    results.iter().find_map(|r| {
        let text = r.get("plainLyrics")?.as_str()?.trim();
        let synced = r.get("syncedLyrics").and_then(serde_json::Value::as_str).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        (!text.is_empty()).then(|| Lyrics {
            title: r.get("trackName").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned(),
            artist: r.get("artistName").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned(),
            text: text.to_owned(),
            synced,
        })
    })
}

static LYRICS_CACHE: LazyLock<DashMap<String, (Option<Lyrics>, Instant)>> = LazyLock::new(DashMap::new);

/// Önbellek destekli şarkı sözü arar.
pub async fn search_lyrics(client: &reqwest::Client, query: &str) -> anyhow::Result<Option<Lyrics>> {
    let clean_key = query.trim().to_lowercase();
    if clean_key.is_empty() {
        return Ok(None);
    }

    // Önbellek kontrolü (30 dk TTL)
    if let Some(entry) = LYRICS_CACHE.get(&clean_key) {
        let (cached_lyrics, timestamp) = entry.value();
        if timestamp.elapsed() < CACHE_TTL {
            return Ok(cached_lyrics.clone());
        }
    }

    let provider = LrclibProvider;
    let res = provider.fetch_lyrics(client, query).await;
    match res {
        Ok(opt) => {
            LYRICS_CACHE.insert(clean_key, (opt.clone(), Instant::now()));
            Ok(opt)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lrclib_valid_response() {
        let json = serde_json::json!([
            {
                "trackName": "Dönence",
                "artistName": "Barış Manço",
                "plainLyrics": "Simsiyah gecenin koynundayım yapayalnız",
                "syncedLyrics": "[00:15.00] Simsiyah gecenin koynundayım yapayalnız"
            }
        ]);
        let items = json.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
        let parsed = parse_lrclib_results(items);
        assert!(parsed.is_some());
        if let Some(l) = parsed {
            assert_eq!(l.title, "Dönence");
            assert_eq!(l.artist, "Barış Manço");
            assert!(l.text.contains("Simsiyah"));
            assert!(l.synced.is_some_and(|s| s.starts_with("[00:15.00]")));
        }
    }

    #[test]
    fn parse_lrclib_empty_and_missing_plain_lyrics() {
        let json = serde_json::json!([
            {
                "trackName": "Enstrümantal",
                "artistName": "Grup",
                "plainLyrics": ""
            },
            {
                "trackName": "Eksik",
                "artistName": "Grup"
            }
        ]);
        let items = json.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
        let parsed = parse_lrclib_results(items);
        assert!(parsed.is_none());
    }
}
