//! YouTube araması doğrudan YouTube'un kendi web API'siyle (InnerTube): yt-dlp'nin her aramada Python
//! ve deno başlatması (2-5 sn) aradan çıkar, arama ~300 ms sürer. Ses adresi için yt-dlp kalır; o da
//! şarkı çalmaya başlarken arka planda çalışır.
//!
//! Resmî olmayan bir uç nokta olduğundan biçim değişebilir: sonuç okunamazsa çağıran yt-dlp aramasına döner.

use std::time::Duration;

use dashmap::DashMap;
use serde_json::{json, Value};

const SEARCH_URL: &str = "https://www.youtube.com/youtubei/v1/search?prettyPrint=false";
/// Web istemcisinin sürümü; YouTube eski sürümleri bir süre daha kabul eder.
const CLIENT_VERSION: &str = "2.20261101.00.00";
/// "Yalnızca videolar" süzgeci (oynatma listesi, kanal, kısa video kartları gelmez).
const VIDEOS_ONLY: &str = "EgIQAQ%3D%3D";
const TIMEOUT: Duration = Duration::from_secs(3);
/// Öneri listesinden seçilen şarkılar `/çal`'da yeniden aranmasın diye son sonuçlar tutulur.
const KNOWN_MAX: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration: Option<Duration>,
}

impl Hit {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }

    pub fn thumbnail(&self) -> String {
        format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", self.id)
    }
}

static KNOWN: std::sync::LazyLock<DashMap<String, Hit>> = std::sync::LazyLock::new(DashMap::new);

/// Daha önce aramada görülmüş bir video (kimliğiyle).
pub fn known(id: &str) -> Option<Hit> {
    KNOWN.get(id).map(|h| h.clone())
}

fn remember(hits: &[Hit]) {
    // shortcut: dolunca hepsi silinir; bir sunucu için yeterli, çok sunucuda LRU'ya geçilmeli.
    if KNOWN.len() + hits.len() > KNOWN_MAX {
        KNOWN.clear();
    }
    for hit in hits {
        KNOWN.insert(hit.id.clone(), hit.clone());
    }
}

/// YouTube'da video arar (canlı yayınlar hariç, en fazla `limit` sonuç).
pub async fn search(client: &reqwest::Client, query: &str, limit: usize) -> anyhow::Result<Vec<Hit>> {
    let body = json!({
        "context": { "client": { "clientName": "WEB", "clientVersion": CLIENT_VERSION, "hl": "tr", "gl": "TR" } },
        "query": query.trim(),
        "params": VIDEOS_ONLY.replace("%3D", "="),
    });
    let response: Value = client
        .post(SEARCH_URL)
        .json(&body)
        .timeout(TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let hits = parse(&response, limit);
    remember(&hits);
    Ok(hits)
}

/// Yanıttaki bütün `videoRenderer` nesnelerini sırayla toplar (sayfa düzeni değişse de bulunur).
fn parse(response: &Value, limit: usize) -> Vec<Hit> {
    let mut hits = Vec::new();
    collect(response, limit, &mut hits);
    hits
}

fn collect(value: &Value, limit: usize, hits: &mut Vec<Hit>) {
    if hits.len() >= limit {
        return;
    }
    match value {
        Value::Object(map) => {
            if let Some(hit) = map.get("videoRenderer").and_then(video) {
                hits.push(hit);
                return;
            }
            for child in map.values() {
                collect(child, limit, hits);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect(child, limit, hits);
            }
        }
        _ => {}
    }
}

fn video(renderer: &Value) -> Option<Hit> {
    let id = renderer.get("videoId")?.as_str()?;
    if id.len() != 11 {
        return None;
    }
    let text = |key: &str| {
        renderer
            .pointer(&format!("/{key}/runs/0/text"))
            .or_else(|| renderer.pointer(&format!("/{key}/simpleText")))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    // Süresi olmayanlar canlı yayındır; müzik kuyruğuna uygun değil.
    let duration = parse_length(renderer.pointer("/lengthText/simpleText")?.as_str()?)?;
    Some(Hit {
        id: id.to_owned(),
        title: text("title")?,
        channel: text("ownerText"),
        duration: Some(duration),
    })
}

/// "3:45" / "1:02:03" → süre.
fn parse_length(text: &str) -> Option<Duration> {
    let mut secs = 0u64;
    for part in text.trim().split(':') {
        secs = secs.checked_mul(60)?.checked_add(part.parse::<u64>().ok()?)?;
    }
    Some(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_video_renderers_anywhere_and_skips_live() {
        let response = json!({
            "contents": { "twoColumnSearchResultsRenderer": { "primaryContents": { "sectionListRenderer": { "contents": [
                { "itemSectionRenderer": { "contents": [
                    { "videoRenderer": { "videoId": "dQw4w9WgXcQ", "title": { "runs": [{ "text": "Never Gonna Give You Up" }] },
                        "ownerText": { "runs": [{ "text": "Rick Astley" }] }, "lengthText": { "simpleText": "3:33" } } },
                    { "videoRenderer": { "videoId": "liveliveliv", "title": { "runs": [{ "text": "Canlı" }] } } },
                    { "shelfRenderer": { "content": { "videoRenderer": { "videoId": "abcdefghijk",
                        "title": { "simpleText": "İkinci" }, "lengthText": { "simpleText": "1:02:03" } } } } }
                ] } }
            ] } } } }
        });
        let hits = parse(&response, 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "dQw4w9WgXcQ");
        assert_eq!(hits[0].channel.as_deref(), Some("Rick Astley"));
        assert_eq!(hits[0].duration, Some(Duration::from_secs(213)));
        assert_eq!(hits[1].title, "İkinci");
        assert_eq!(hits[1].duration, Some(Duration::from_secs(3723)));
        assert_eq!(parse(&response, 1).len(), 1);
    }

    #[test]
    fn rejects_bad_lengths() {
        assert_eq!(parse_length("abc"), None);
        assert_eq!(parse_length("4:05"), Some(Duration::from_secs(245)));
    }
}

#[cfg(test)]
mod live_tests {
    #[tokio::test]
    #[ignore = "gerçek YouTube'a bağlanır"]
    async fn live_innertube_search() -> anyhow::Result<()> {
        let client = crate::net::http_client()?;
        let started = std::time::Instant::now();
        let hits = super::search(&client, "tarkan şımarık", 5).await?;
        println!("{} sonuç, {:?}: {:?}", hits.len(), started.elapsed(), hits.first());
        assert!(!hits.is_empty());
        Ok(())
    }
}
