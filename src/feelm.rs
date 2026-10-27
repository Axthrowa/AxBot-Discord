//! feelm.org: `/tavsiye`'nin ilk kaynağı (ulaşılamazsa `crate::catalog` TMDB/RAWG'a geçer).
//!
//! feelm.org bir SvelteKit sitesi: her liste sayfasının verisi `…/__data.json` adresinden "devalue" biçiminde
//! gelir (düz bir dizi; nesneler değerlerin dizideki sırasını tutar). Ayrıntı sayfasının özeti HTML'deki
//! `<meta name="description">` etiketindedir. Sitenin IPv4 (Cloudflare) adresleri bu ağdan ulaşılamadığı için
//! istekler `net::dual_stack_client` ile gider (IPv4 takılırsa IPv6).

use std::time::Duration;

use serde_json::Value;

use crate::catalog::{Item, Kind, Query, Source};

pub const BASE: &str = "https://feelm.org";
const TIMEOUT: Duration = Duration::from_secs(12);
/// Detay sayfası en fazla bu kadar okunur (özet en baştaki meta etiketinde).
const MAX_PAGE_BYTES: usize = 1_500_000;

fn list_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Movie => "movies",
        Kind::Series => "series",
        Kind::Game => "games",
    }
}

pub fn detail_path(kind: Kind) -> &'static str {
    match kind {
        Kind::Movie => "movie",
        Kind::Series => "series",
        Kind::Game => "game",
    }
}

fn kind_of(raw: &str) -> Option<Kind> {
    match raw {
        "movie" => Some(Kind::Movie),
        "series" => Some(Kind::Series),
        "game" => Some(Kind::Game),
        _ => None,
    }
}

/// Komuttaki tür kimliğinin (TMDB sayısı / RAWG kısa adı) feelm karşılığı. `None`: feelm'de yok.
pub fn genre(kind: Kind, key: &str) -> Option<&'static str> {
    Some(match (kind, key) {
        (Kind::Movie, "28") => "action",
        (Kind::Movie, "12") => "adventure",
        (Kind::Movie | Kind::Series, "16") => "animation",
        (Kind::Movie | Kind::Series, "35") => "comedy",
        (Kind::Movie | Kind::Series, "80") => "crime",
        (Kind::Movie | Kind::Series, "99") => "documentary",
        (Kind::Movie | Kind::Series, "18") => "drama",
        (Kind::Movie | Kind::Series, "10751") => "family",
        (Kind::Movie, "14") => "fantasy",
        (Kind::Movie, "36") => "history",
        (Kind::Movie, "27") => "horror",
        (Kind::Movie, "10402") => "music",
        (Kind::Movie | Kind::Series, "9648") => "mystery",
        (Kind::Movie, "10749") => "romance",
        (Kind::Movie, "878") => "science-fiction",
        (Kind::Movie, "53") => "thriller",
        (Kind::Movie, "10752") => "war",
        (Kind::Movie | Kind::Series, "37") => "western",
        (Kind::Series, "10759") => "action-adventure",
        (Kind::Series, "10762") => "kids",
        (Kind::Series, "10764") => "reality",
        (Kind::Series, "10765") => "sci-fi-fantasy",
        (Kind::Series, "10768") => "war-politics",
        (Kind::Game, "adventure") => "adventure",
        (Kind::Game, "role-playing-games-rpg") => "role-playing-rpg",
        (Kind::Game, "shooter") => "shooter",
        (Kind::Game, "strategy") => "strategy",
        (Kind::Game, "simulation") => "simulator",
        (Kind::Game, "sports") => "sport",
        (Kind::Game, "racing") => "racing",
        (Kind::Game, "fighting") => "fighting",
        (Kind::Game, "platformer") => "platform",
        (Kind::Game, "puzzle") => "puzzle",
        (Kind::Game, "indie") => "indie",
        (Kind::Game, "arcade") => "arcade",
        _ => return None,
    })
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

/// Yetişkin içerik olarak işaretlenmiş mi? (sohbette önerilmez)
fn is_adult(certification: Option<&str>) -> bool {
    certification.is_some_and(|c| matches!(c.trim().to_ascii_uppercase().as_str(), "NC-17" | "X" | "18+" | "AO" | "TV-MA-L"))
}

fn parse_item(value: &Value) -> Option<Item> {
    let kind = kind_of(value.get("type")?.as_str()?)?;
    let details = value.get("details");
    let detail = |key: &str| details.and_then(|d| d.get(key));
    let mut item = Item::new(Source::Feelm, kind, 0, value.get("title")?.as_str()?.to_owned());
    item.slug = value.get("slug")?.as_str()?.to_owned();
    item.year = value.get("year").and_then(Value::as_i64);
    item.poster = value.get("poster").and_then(Value::as_str).filter(|p| p.starts_with("https://")).map(str::to_owned);
    item.imdb_score = rating(value, "imdb");
    // Oyunda IGDB (/100), film/dizide TMDB (yüzde → /10).
    item.score = if kind == Kind::Game { rating(value, "igdb") } else { rating(value, "tmdb").map(|t| if t > 10.0 { t / 10.0 } else { t }) };
    item.runtime = detail("runtime").and_then(Value::as_i64);
    item.release = detail("releaseDate").and_then(Value::as_str).map(str::to_owned);
    item.seasons = detail("seasonCount").and_then(Value::as_i64);
    item.episodes = detail("episodeCount").and_then(Value::as_i64);
    item.makers = detail("developers").and_then(Value::as_array).map(|d| d.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
    item.adult = is_adult(detail("certification").and_then(Value::as_str));
    Some(item)
}

/// Sayfa verisindeki öğeler (yetişkin içerik atılır).
pub fn parse_results(body: &Value) -> Vec<Item> {
    let Some(data) = page_data(body) else { return Vec::new() };
    let list = data["results"].get("items").and_then(Value::as_array);
    list.map(|list| list.iter().filter_map(parse_item).filter(|i| !i.adult).collect()).unwrap_or_default()
}

/// Liste adresi (`__data.json`). Türün feelm karşılığı yoksa `None` (o istek TMDB/RAWG'a gider).
pub fn list_url(kind: Kind, query: &Query, page: u32) -> Option<String> {
    let mut url = format!("{BASE}/{}/__data.json?sort=popularity&page={}", list_path(kind), page.max(1));
    if let Some(key) = query.genre.as_deref() {
        url.push_str(&format!("&genre={}", genre(kind, key)?));
    }
    if let Some(year) = query.year_from {
        url.push_str(&format!("&yearFrom={year}"));
    }
    match (kind, query.min_score) {
        (Kind::Game, Some(score)) => url.push_str(&format!("&scoreMin={}", u32::from(score) * 10)),
        (_, Some(score)) => url.push_str(&format!("&imdbMin={score}")),
        _ => {}
    }
    Some(url)
}

async fn get_json(url: &str) -> anyhow::Result<Vec<Item>> {
    let body: Value = crate::net::dual_stack_client()?.get(url).timeout(TIMEOUT).send().await?.error_for_status()?.json().await?;
    Ok(parse_results(&body))
}

/// Süzgece uyan liste sayfası (popülerliğe göre). `Ok(None)`: süzgeç feelm'de yok.
pub async fn list(kind: Kind, query: &Query, page: u32) -> anyhow::Result<Option<Vec<Item>>> {
    let Some(url) = list_url(kind, query, page) else { return Ok(None) };
    get_json(&url).await.map(Some)
}

/// Film, dizi ve oyunlarda arama.
pub async fn search(text: &str) -> anyhow::Result<Vec<Item>> {
    let url = reqwest::Url::parse_with_params(&format!("{BASE}/search/__data.json"), &[("q", text.trim())])?;
    get_json(url.as_str()).await
}

/// Ayrıntı sayfasındaki özet (İngilizce).
pub async fn overview(item: &Item) -> anyhow::Result<Option<String>> {
    let response = crate::net::dual_stack_client()?.get(item.url()).timeout(TIMEOUT).send().await?.error_for_status()?;
    let bytes = response.bytes().await?;
    let html = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_PAGE_BYTES)]);
    Ok(description(&html))
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
        let items = parse_results(&body);
        let item = items.first().ok_or_else(|| anyhow::anyhow!("öğe yok"))?;
        assert_eq!((item.title.as_str(), item.kind, item.year), ("The Odyssey", Kind::Movie, Some(2026)));
        assert_eq!((item.imdb_score, item.runtime, item.adult), (Some(8.5), Some(173), false));
        assert_eq!(item.url(), "https://feelm.org/movie/the-odyssey-2026");
        Ok(())
    }

    #[test]
    fn builds_list_urls_with_mapped_genres_only() {
        let query = Query { genre: Some("27".to_owned()), year_from: Some(2010), min_score: Some(7) };
        assert_eq!(
            list_url(Kind::Movie, &query, 2).as_deref(),
            Some("https://feelm.org/movies/__data.json?sort=popularity&page=2&genre=horror&yearFrom=2010&imdbMin=7")
        );
        let game = Query { genre: Some("role-playing-games-rpg".to_owned()), min_score: Some(8), ..Query::default() };
        assert_eq!(list_url(Kind::Game, &game, 1).as_deref(), Some("https://feelm.org/games/__data.json?sort=popularity&page=1&genre=role-playing-rpg&scoreMin=80"));
        // feelm'de karşılığı olmayan tür: istek TMDB/RAWG'a gider.
        assert_eq!(list_url(Kind::Game, &Query { genre: Some("casual".to_owned()), ..Query::default() }, 1), None);
        // Komuttaki her tür ya eşleşir ya da bilerek boş kalır (yazım hatası yakalanır).
        let unmapped: Vec<&str> = [Kind::Movie, Kind::Series].iter().flat_map(|k| k.genres().iter().filter(|(g, _)| genre(*k, g).is_none()).map(|(g, _)| *g)).collect();
        assert!(unmapped.is_empty(), "{unmapped:?}");
    }

    #[test]
    fn reads_item_description_not_site_tagline() {
        let html = r#"<meta name="description" content="Feelm — a shared log of the films people finished."/>
            <meta name="description" content="Odysseus, the King of Ithaca, embarks on a &quot;long&quot; journey home."/>"#;
        assert_eq!(description(html).as_deref(), Some("Odysseus, the King of Ithaca, embarks on a \"long\" journey home."));
        assert!(is_adult(Some("NC-17")));
        assert!(!is_adult(Some("PG-13")));
    }

    /// `cargo test live_feelm -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "gerçek feelm.org'a istek atar"]
    async fn live_feelm() -> anyhow::Result<()> {
        let found = list(Kind::Game, &Query { genre: Some("role-playing-games-rpg".to_owned()), min_score: Some(8), ..Query::default() }, 1)
            .await?
            .ok_or_else(|| anyhow::anyhow!("tür eşleşmedi"))?;
        let first = found.first().ok_or_else(|| anyhow::anyhow!("oyun yok"))?;
        println!("{} öğe, ilk: {} ({:?}) puan {:?} · {:?}", found.len(), first.title, first.year, first.score, first.makers);
        println!("özet: {:?}", overview(first).await?);
        let movies = list(Kind::Movie, &Query::default(), 1).await?.unwrap_or_default();
        println!("film: {:?}", movies.iter().take(3).map(|i| (&i.title, i.imdb_score, i.score)).collect::<Vec<_>>());
        let hits = search("batman").await?;
        println!("arama: {:?}", hits.iter().take(3).map(|i| &i.title).collect::<Vec<_>>());
        assert!(!hits.is_empty());
        Ok(())
    }
}
