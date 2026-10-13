//! feelm.org'dan film, dizi ve oyun önerisi (`/tavsiye`).
//!
//! feelm.org bir SvelteKit sitesi: her liste sayfasının verisi `…/__data.json` adresinden "devalue" biçiminde
//! gelir (düz bir dizi; nesneler değerlerin dizideki sırasını tutar). Ayrıntı sayfasının özeti HTML'deki
//! `<meta name="description">` etiketindedir. Sonuçlar bellekte 30 dk saklanır (site gereksiz yere yorulmasın).

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde_json::Value;

pub const BASE: &str = "https://feelm.org";
const CACHE_FOR: Duration = Duration::from_secs(30 * 60);
const TIMEOUT: Duration = Duration::from_secs(12);
/// Detay sayfası en fazla bu kadar okunur (özet en baştaki meta etiketinde).
const MAX_PAGE_BYTES: usize = 1_500_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Movie,
    Series,
    Game,
}

impl Kind {
    fn list_path(self) -> &'static str {
        match self {
            Self::Movie => "movies",
            Self::Series => "series",
            Self::Game => "games",
        }
    }

    fn detail_path(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "series",
            Self::Game => "game",
        }
    }

    pub fn code(self) -> char {
        match self {
            Self::Movie => 'f',
            Self::Series => 'd',
            Self::Game => 'o',
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "f" => Some(Self::Movie),
            "d" => Some(Self::Series),
            "o" => Some(Self::Game),
            _ => None,
        }
    }

    fn from_site(raw: &str) -> Option<Self> {
        match raw {
            "movie" => Some(Self::Movie),
            "series" => Some(Self::Series),
            "game" => Some(Self::Game),
            _ => None,
        }
    }

    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            Self::Movie => ("🎬", "Film"),
            Self::Series => ("📺", "Dizi"),
            Self::Game => ("🎮", "Oyun"),
        }
    }

    /// Komutta seçilebilen türler: (sitedeki kısa ad, Türkçe ad).
    pub fn genres(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Movie => &[
                ("action", "Aksiyon"), ("adventure", "Macera"), ("animation", "Animasyon"), ("comedy", "Komedi"),
                ("crime", "Suç"), ("documentary", "Belgesel"), ("drama", "Dram"), ("family", "Aile"), ("fantasy", "Fantastik"),
                ("history", "Tarih"), ("horror", "Korku"), ("music", "Müzik"), ("mystery", "Gizem"), ("romance", "Romantik"),
                ("science-fiction", "Bilim kurgu"), ("thriller", "Gerilim"), ("war", "Savaş"), ("western", "Western"),
            ],
            Self::Series => &[
                ("action-adventure", "Aksiyon & Macera"), ("animation", "Animasyon"), ("comedy", "Komedi"), ("crime", "Suç"),
                ("documentary", "Belgesel"), ("drama", "Dram"), ("family", "Aile"), ("kids", "Çocuk"), ("mystery", "Gizem"),
                ("reality", "Reality"), ("sci-fi-fantasy", "Bilim kurgu & Fantastik"), ("war-politics", "Savaş & Politika"),
                ("western", "Western"),
            ],
            Self::Game => &[
                ("adventure", "Macera"), ("role-playing-rpg", "Rol yapma (RPG)"), ("shooter", "Nişancı"), ("strategy", "Strateji"),
                ("real-time-strategy-rts", "Gerçek zamanlı strateji"), ("turn-based-strategy-tbs", "Sıra tabanlı strateji"),
                ("simulator", "Simülasyon"), ("sport", "Spor"), ("racing", "Yarış"), ("fighting", "Dövüş"), ("platform", "Platform"),
                ("puzzle", "Bulmaca"), ("indie", "Bağımsız"), ("arcade", "Arcade"), ("hack-and-slash-beat-em-up", "Hack and slash"),
                ("horror", "Korku"), ("open-world", "Açık dünya"), ("survival", "Hayatta kalma"), ("stealth", "Gizlilik"),
                ("moba", "MOBA"), ("tactical", "Taktik"), ("visual-novel", "Görsel roman"), ("point-and-click", "Tıkla ve oyna"),
            ],
        }
    }

    pub fn genre_name(self, slug: &str) -> Option<&'static str> {
        self.genres().iter().find(|(s, _)| *s == slug).map(|(_, name)| *name)
    }
}

/// Öneri süzgeci.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub genre: Option<String>,
    pub year_from: Option<u16>,
    /// Filmde/dizide IMDb (1-10), oyunda IGDB puanı onda biri (8 → 80).
    pub min_score: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    pub kind: Option<Kind>,
    pub title: String,
    pub year: Option<i64>,
    pub slug: String,
    pub poster: Option<String>,
    pub color: Option<u32>,
    pub imdb: Option<f64>,
    pub tmdb: Option<f64>,
    pub igdb: Option<f64>,
    pub runtime: Option<i64>,
    pub certification: Option<String>,
    pub release: Option<String>,
    pub seasons: Option<i64>,
    pub episodes: Option<i64>,
    pub developers: Vec<String>,
}

impl Item {
    pub fn url(&self) -> String {
        let path = self.kind.map_or("movie", Kind::detail_path);
        format!("{BASE}/{path}/{}", self.slug)
    }

    /// Yetişkin içerik olarak işaretlenmiş mi? (sohbette önerilmez)
    pub fn is_adult(&self) -> bool {
        self.certification.as_deref().is_some_and(|c| matches!(c.trim().to_ascii_uppercase().as_str(), "NC-17" | "X" | "18+" | "AO" | "TV-MA-L"))
    }
}

/// SvelteKit devalue biçimini düz JSON'a çevirir. `-1` vb. negatif sıralar "tanımsız" demektir.
pub fn devalue(table: &[Value]) -> Value {
    fn at(table: &[Value], index: &Value, depth: u8) -> Value {
        let Some(i) = index.as_i64() else { return Value::Null };
        if i < 0 || depth > 48 {
            return Value::Null;
        }
        let Some(raw) = usize::try_from(i).ok().and_then(|i| table.get(i)) else { return Value::Null };
        match raw {
            Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), at(table, v, depth + 1))).collect()),
            // ["Date", "..."] gibi özel değerler olduğu gibi bırakılır.
            Value::Array(list) if list.first().and_then(Value::as_str).is_some_and(|t| matches!(t, "Date" | "Set" | "Map" | "BigInt" | "RegExp")) => raw.clone(),
            Value::Array(list) => Value::Array(list.iter().map(|v| at(table, v, depth + 1)).collect()),
            other => other.clone(),
        }
    }
    at(table, &Value::from(0), 0)
}

/// `__data.json` yanıtından sayfa verisini (son veri düğümü) çıkarır.
fn page_data(body: &Value) -> Option<Value> {
    body.get("nodes")?
        .as_array()?
        .iter()
        .rev()
        .filter(|n| n.get("type").and_then(Value::as_str) == Some("data"))
        .find_map(|n| n.get("data")?.as_array().map(|t| devalue(t)).filter(|d| d.get("results").is_some()))
}

fn rating(value: &Value, site: &str) -> Option<f64> {
    value.get("ratings")?.get(site)?.get("rating")?.as_f64()
}

fn parse_item(value: &Value) -> Option<Item> {
    let details = value.get("details");
    let detail = |key: &str| details.and_then(|d| d.get(key));
    Some(Item {
        kind: value.get("type").and_then(Value::as_str).and_then(Kind::from_site),
        title: value.get("title")?.as_str()?.to_owned(),
        year: value.get("year").and_then(Value::as_i64),
        slug: value.get("slug")?.as_str()?.to_owned(),
        poster: value.get("poster").and_then(Value::as_str).filter(|p| p.starts_with("https://")).map(str::to_owned),
        color: value.get("color").and_then(Value::as_str).and_then(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok()),
        imdb: rating(value, "imdb"),
        tmdb: rating(value, "tmdb"),
        igdb: rating(value, "igdb"),
        runtime: detail("runtime").and_then(Value::as_i64),
        certification: detail("certification").and_then(Value::as_str).map(str::to_owned),
        release: detail("releaseDate").and_then(Value::as_str).map(str::to_owned),
        seasons: detail("seasonCount").and_then(Value::as_i64),
        episodes: detail("episodeCount").and_then(Value::as_i64),
        developers: detail("developers").and_then(Value::as_array).map(|d| d.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default(),
    })
}

/// Sayfa verisinden öğeler ve "daha fazla var mı".
pub fn parse_results(body: &Value) -> (Vec<Item>, bool) {
    let Some(data) = page_data(body) else { return (Vec::new(), false) };
    let results = &data["results"];
    let items = results.get("items").and_then(Value::as_array).map(|list| list.iter().filter_map(parse_item).collect()).unwrap_or_default();
    (items, results.get("hasMore").and_then(Value::as_bool).unwrap_or(false))
}

/// Liste adresi (`__data.json`).
pub fn list_url(kind: Kind, query: &Query, page: u32) -> String {
    let mut url = format!("{BASE}/{}/__data.json?sort=popularity&page={page}", kind.list_path());
    if let Some(genre) = query.genre.as_deref().filter(|g| kind.genre_name(g).is_some()) {
        url.push_str(&format!("&genre={genre}"));
    }
    if let Some(year) = query.year_from {
        url.push_str(&format!("&yearFrom={year}"));
    }
    match (kind, query.min_score) {
        (Kind::Game, Some(score)) => url.push_str(&format!("&scoreMin={}", u32::from(score) * 10)),
        (_, Some(score)) => url.push_str(&format!("&imdbMin={score}")),
        _ => {}
    }
    url
}

type Cached = (Instant, Arc<(Vec<Item>, bool)>);
static CACHE: LazyLock<DashMap<String, Cached>> = LazyLock::new(DashMap::new);

async fn get_json(http: &reqwest::Client, url: &str) -> anyhow::Result<Arc<(Vec<Item>, bool)>> {
    if let Some(hit) = CACHE.get(url).filter(|e| e.0.elapsed() < CACHE_FOR) {
        return Ok(Arc::clone(&hit.1));
    }
    let body: Value = http.get(url).timeout(TIMEOUT).send().await?.error_for_status()?.json().await?;
    let parsed = Arc::new(parse_results(&body));
    if CACHE.len() > 300 {
        CACHE.retain(|_, e| e.0.elapsed() < CACHE_FOR);
    }
    CACHE.insert(url.to_owned(), (Instant::now(), Arc::clone(&parsed)));
    Ok(parsed)
}

/// Süzgece uyan bir liste sayfası (popülerliğe göre, sayfa başına 70).
pub async fn list(http: &reqwest::Client, kind: Kind, query: &Query, page: u32) -> anyhow::Result<Arc<(Vec<Item>, bool)>> {
    get_json(http, &list_url(kind, query, page.max(1))).await
}

/// Film, dizi ve oyunlarda arama.
pub async fn search(http: &reqwest::Client, text: &str) -> anyhow::Result<Arc<(Vec<Item>, bool)>> {
    let url = reqwest::Url::parse_with_params(&format!("{BASE}/search/__data.json"), &[("q", text.trim())])?;
    get_json(http, url.as_str()).await
}

/// Ayrıntı sayfasındaki özet (İngilizce).
pub async fn overview(http: &reqwest::Client, item: &Item) -> Option<String> {
    let response = http.get(item.url()).timeout(TIMEOUT).send().await.ok()?.error_for_status().ok()?;
    let bytes = response.bytes().await.ok()?;
    let html = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_PAGE_BYTES)]);
    description(&html)
}

/// HTML'deki `<meta name="description" content="…">` etiketlerinden sitenin genel tanıtımı olmayanı.
pub fn description(html: &str) -> Option<String> {
    let re = regex::Regex::new(r#"(?s)<meta\s+name="description"\s+content="([^"]*)""#).ok()?;
    let found = re
        .captures_iter(html)
        .filter_map(|c| c.get(1).map(|m| unescape(m.as_str())))
        .find(|text| !text.starts_with("Feelm") && text.chars().count() > 20);
    found
}

fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"").replace("&#39;", "'").replace("&#x27;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_devalue_and_items() -> anyhow::Result<()> {
        let body: Value = serde_json::from_str(
            r##"{"type":"data","nodes":[{"type":"data","data":[{"locale":1},"en"]},{"type":"data","data":[
                {"type":1,"results":2},"movie",{"items":3,"hasMore":12},[4],
                {"type":1,"slug":5,"title":6,"year":7,"ratings":8,"details":10,"color":13},
                "the-odyssey-2026","The Odyssey",2026,{"imdb":9},{"rating":14},{"runtime":11,"certification":-1},173,true,"#d5b98e",8.5]}]}"##,
        )?;
        let (items, more) = parse_results(&body);
        assert!(more);
        let item = items.first().ok_or_else(|| anyhow::anyhow!("öğe yok"))?;
        assert_eq!(item.title, "The Odyssey");
        assert_eq!(item.kind, Some(Kind::Movie));
        assert_eq!(item.year, Some(2026));
        assert_eq!(item.imdb, Some(8.5));
        assert_eq!(item.runtime, Some(173));
        assert_eq!(item.certification, None);
        assert_eq!(item.color, Some(0xd5b98e));
        assert_eq!(item.url(), "https://feelm.org/movie/the-odyssey-2026");
        Ok(())
    }

    #[test]
    fn builds_list_urls_with_valid_filters_only() {
        let query = Query { genre: Some("horror".to_owned()), year_from: Some(2010), min_score: Some(7) };
        assert_eq!(list_url(Kind::Movie, &query, 2), "https://feelm.org/movies/__data.json?sort=popularity&page=2&genre=horror&yearFrom=2010&imdbMin=7");
        assert_eq!(list_url(Kind::Game, &Query { min_score: Some(8), ..Query::default() }, 1), "https://feelm.org/games/__data.json?sort=popularity&page=1&scoreMin=80");
        // Listede olmayan tür (ör. yetişkin) adrese hiç girmez.
        assert_eq!(list_url(Kind::Series, &Query { genre: Some("erotic".to_owned()), ..Query::default() }, 1), "https://feelm.org/series/__data.json?sort=popularity&page=1");
    }

    #[test]
    fn reads_item_description_not_site_tagline() {
        let html = r#"<meta name="description" content="Feelm — a shared log of the films people finished."/>
            <meta name="description" content="Odysseus, the King of Ithaca, embarks on a &quot;long&quot; journey home."/>"#;
        assert_eq!(description(html).as_deref(), Some("Odysseus, the King of Ithaca, embarks on a \"long\" journey home."));
    }

    #[test]
    fn hides_adult_titles() {
        assert!(Item { certification: Some("NC-17".to_owned()), ..Item::default() }.is_adult());
        assert!(!Item { certification: Some("PG-13".to_owned()), ..Item::default() }.is_adult());
    }

    /// `cargo test live_feelm -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "gerçek feelm.org'a istek atar"]
    async fn live_feelm() -> anyhow::Result<()> {
        let http = reqwest::Client::builder().user_agent("AxBot").build()?;
        let found = list(&http, Kind::Game, &Query { genre: Some("role-playing-rpg".to_owned()), min_score: Some(8), ..Query::default() }, 1).await?;
        let first = found.0.first().ok_or_else(|| anyhow::anyhow!("oyun yok"))?;
        println!("{} öğe, ilk: {} ({:?}) IGDB {:?} · {:?}", found.0.len(), first.title, first.year, first.igdb, first.developers);
        println!("özet: {:?}", overview(&http, first).await);
        let hits = search(&http, "batman").await?;
        println!("arama: {:?}", hits.0.iter().take(3).map(|i| &i.title).collect::<Vec<_>>());
        assert!(!hits.0.is_empty());
        Ok(())
    }
}
