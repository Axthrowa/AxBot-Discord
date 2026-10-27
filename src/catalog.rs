//! `/tavsiye` veri kaynakları: film ve dizi TMDB'den (özet doğrudan Türkçe gelir), oyun RAWG'dan, IMDb puanı
//! IMDb'nin resmî günlük `title.ratings.tsv.gz` dosyasından (kişisel, ticari olmayan kullanım serbest).
//!
//! Anahtarlar `.env`'de: `TMDB_API_KEY` (v3 anahtarı ya da v4 "okuma" jetonu), `RAWG_API_KEY`.
//! Liste sonuçları bellekte 30 dk saklanır; IMDb puan dosyası günde bir kez arka planda indirilir.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde_json::Value;

const TMDB: &str = "https://api.themoviedb.org/3";
const RAWG: &str = "https://api.rawg.io/api";
const IMDB_RATINGS: &str = "https://datasets.imdbws.com/title.ratings.tsv.gz";
const CACHE_FOR: Duration = Duration::from_secs(30 * 60);
const TIMEOUT: Duration = Duration::from_secs(12);
/// Bellekte yalnızca bu kadar oy almış yapımların puanı tutulur (dosyanın çoğu birkaç oyluk kayıt).
const IMDB_MIN_VOTES: u32 = 100;
const IMDB_REFRESH: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Movie,
    Series,
    Game,
}

impl Kind {
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

    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            Self::Movie => ("🎬", "Film"),
            Self::Series => ("📺", "Dizi"),
            Self::Game => ("🎮", "Oyun"),
        }
    }

    /// Komutta seçilebilen türler: (kaynaktaki kimlik, Türkçe ad). TMDB'de sayı, RAWG'da kısa ad.
    pub fn genres(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Movie => &[
                ("28", "Aksiyon"), ("12", "Macera"), ("16", "Animasyon"), ("35", "Komedi"), ("80", "Suç"), ("99", "Belgesel"),
                ("18", "Dram"), ("10751", "Aile"), ("14", "Fantastik"), ("36", "Tarih"), ("27", "Korku"), ("10402", "Müzik"),
                ("9648", "Gizem"), ("10749", "Romantik"), ("878", "Bilim kurgu"), ("53", "Gerilim"), ("10752", "Savaş"), ("37", "Western"),
            ],
            Self::Series => &[
                ("10759", "Aksiyon & Macera"), ("16", "Animasyon"), ("35", "Komedi"), ("80", "Suç"), ("99", "Belgesel"), ("18", "Dram"),
                ("10751", "Aile"), ("10762", "Çocuk"), ("9648", "Gizem"), ("10764", "Reality"), ("10765", "Bilim kurgu & Fantastik"),
                ("10768", "Savaş & Politika"), ("37", "Western"),
            ],
            Self::Game => &[
                ("action", "Aksiyon"), ("adventure", "Macera"), ("role-playing-games-rpg", "Rol yapma (RPG)"), ("shooter", "Nişancı"),
                ("strategy", "Strateji"), ("simulation", "Simülasyon"), ("sports", "Spor"), ("racing", "Yarış"), ("fighting", "Dövüş"),
                ("platformer", "Platform"), ("puzzle", "Bulmaca"), ("indie", "Bağımsız"), ("arcade", "Arcade"), ("casual", "Gündelik"),
                ("massively-multiplayer", "Çok oyunculu (MMO)"), ("family", "Aile"), ("board-games", "Kutu oyunu"), ("card", "Kart"),
            ],
        }
    }

    pub fn genre_name(self, key: &str) -> Option<&'static str> {
        self.genres().iter().find(|(k, _)| *k == key).map(|(_, name)| *name)
    }

    /// Bu tür için gereken anahtar eksikse kullanıcıya gösterilecek açıklama.
    pub fn missing_key(self) -> Option<&'static str> {
        let keys = crate::integrations::get();
        match self {
            Self::Movie | Self::Series if keys.tmdb_api_key.is_none() => Some("Film ve dizi önerisi için `.env` dosyasına `TMDB_API_KEY` eklenmeli (themoviedb.org → Ayarlar → API)."),
            Self::Game if keys.rawg_api_key.is_none() => Some("Oyun önerisi için `.env` dosyasına `RAWG_API_KEY` eklenmeli (rawg.io/apidocs)."),
            _ => None,
        }
    }
}

/// Öneri süzgeci.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub genre: Option<String>,
    pub year_from: Option<u16>,
    /// Film/dizide TMDB puanı (1-10), oyunda Metacritic'in onda biri (8 → 80).
    pub min_score: Option<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: Kind,
    /// TMDB ya da RAWG kimliği.
    pub id: i64,
    /// RAWG adresindeki kısa ad (TMDB'de boş).
    pub slug: String,
    pub title: String,
    pub year: Option<i64>,
    pub poster: Option<String>,
    /// TMDB'de Türkçe özet (yoksa boş); RAWG listede özet vermez.
    pub overview: String,
    /// TMDB puanı (/10) ya da Metacritic (/100).
    pub score: Option<f64>,
    pub release: Option<String>,
    pub adult: bool,
    // Ayrıntı isteğiyle dolanlar (`enrich`).
    pub english_overview: Option<String>,
    pub imdb_id: Option<String>,
    pub runtime: Option<i64>,
    pub seasons: Option<i64>,
    pub episodes: Option<i64>,
    pub genres: Vec<String>,
    pub makers: Vec<String>,
    pub platforms: Vec<String>,
}

impl Item {
    fn new(kind: Kind, id: i64, title: String) -> Self {
        Self {
            kind,
            id,
            slug: String::new(),
            title,
            year: None,
            poster: None,
            overview: String::new(),
            score: None,
            release: None,
            adult: false,
            english_overview: None,
            imdb_id: None,
            runtime: None,
            seasons: None,
            episodes: None,
            genres: Vec::new(),
            makers: Vec::new(),
            platforms: Vec::new(),
        }
    }

    /// Kanal içi "yakında önerildi" listesi için anahtar.
    pub fn key(&self) -> String {
        format!("{}{}", self.kind.code(), self.id)
    }

    pub fn url(&self) -> String {
        match self.kind {
            Kind::Movie => format!("https://www.themoviedb.org/movie/{}", self.id),
            Kind::Series => format!("https://www.themoviedb.org/tv/{}", self.id),
            Kind::Game => format!("https://rawg.io/games/{}", if self.slug.is_empty() { self.id.to_string() } else { self.slug.clone() }),
        }
    }

    pub fn imdb_url(&self) -> Option<String> {
        self.imdb_id.as_ref().map(|id| format!("https://www.imdb.com/title/{id}/"))
    }
}

fn year_of(date: Option<&str>) -> Option<i64> {
    date.and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn names(value: &Value, key: &str) -> Vec<String> {
    value.get(key).and_then(Value::as_array).map(|list| list.iter().filter_map(|v| text(v, "name")).collect()).unwrap_or_default()
}

/// TMDB liste/arama sonucundaki bir öğe. `kind` verilmezse `media_type`'tan okunur (arama).
pub fn parse_tmdb(value: &Value, kind: Option<Kind>) -> Option<Item> {
    let kind = match kind {
        Some(kind) => kind,
        None => match value.get("media_type")?.as_str()? {
            "movie" => Kind::Movie,
            "tv" => Kind::Series,
            _ => return None,
        },
    };
    let title = text(value, "title").or_else(|| text(value, "name"))?;
    let release = text(value, "release_date").or_else(|| text(value, "first_air_date"));
    let mut item = Item::new(kind, value.get("id")?.as_i64()?, title);
    item.year = year_of(release.as_deref());
    item.release = release;
    item.poster = text(value, "poster_path").map(|p| format!("https://image.tmdb.org/t/p/w342{p}"));
    item.overview = text(value, "overview").unwrap_or_default();
    item.score = value.get("vote_average").and_then(Value::as_f64).filter(|s| *s > 0.0);
    item.adult = value.get("adult").and_then(Value::as_bool).unwrap_or(false);
    Some(item)
}

/// RAWG liste/arama sonucundaki bir oyun.
pub fn parse_rawg(value: &Value) -> Option<Item> {
    let mut item = Item::new(Kind::Game, value.get("id")?.as_i64()?, text(value, "name")?);
    item.slug = text(value, "slug").unwrap_or_default();
    item.release = text(value, "released");
    item.year = year_of(item.release.as_deref());
    item.poster = text(value, "background_image").filter(|u| u.starts_with("https://"));
    item.score = value.get("metacritic").and_then(Value::as_f64).filter(|s| *s > 0.0);
    item.adult = value.pointer("/esrb_rating/slug").and_then(Value::as_str) == Some("adults-only");
    item.genres = names(value, "genres");
    Some(item)
}

fn tmdb_request(http: &reqwest::Client, path: &str) -> anyhow::Result<reqwest::RequestBuilder> {
    let key = crate::integrations::get().tmdb_api_key.as_deref().ok_or_else(|| anyhow::anyhow!("TMDB_API_KEY yok"))?;
    let request = http.get(format!("{TMDB}{path}")).timeout(TIMEOUT);
    // v4 "okuma jetonu" (JWT, "eyJ" ile başlar) başlıkta, v3 anahtarı adreste gider.
    Ok(if key.starts_with("eyJ") { request.bearer_auth(key) } else { request.query(&[("api_key", key)]) })
}

fn rawg_request(http: &reqwest::Client, path: &str) -> anyhow::Result<reqwest::RequestBuilder> {
    let key = crate::integrations::get().rawg_api_key.as_deref().ok_or_else(|| anyhow::anyhow!("RAWG_API_KEY yok"))?;
    Ok(http.get(format!("{RAWG}{path}")).timeout(TIMEOUT).query(&[("key", key)]))
}

async fn send_json(request: reqwest::RequestBuilder) -> anyhow::Result<Value> {
    Ok(request.send().await?.error_for_status()?.json().await?)
}

/// Liste isteğinin adres parametreleri (anahtar hariç).
pub fn list_params(kind: Kind, query: &Query, page: u32) -> Vec<(&'static str, String)> {
    let genre = query.genre.as_deref().filter(|g| kind.genre_name(g).is_some());
    let mut params = vec![("page", page.max(1).to_string())];
    match kind {
        Kind::Movie | Kind::Series => {
            params.extend([
                ("language", "tr-TR".to_owned()),
                ("sort_by", "popularity.desc".to_owned()),
                ("include_adult", "false".to_owned()),
                // Az oylu (bilinmeyen) yapımlar atlanır.
                ("vote_count.gte", if kind == Kind::Movie { "200" } else { "100" }.to_owned()),
            ]);
            if let Some(genre) = genre {
                params.push(("with_genres", genre.to_owned()));
            }
            if let Some(year) = query.year_from {
                let key = if kind == Kind::Movie { "primary_release_date.gte" } else { "first_air_date.gte" };
                params.push((key, format!("{year}-01-01")));
            }
            if let Some(score) = query.min_score {
                params.push(("vote_average.gte", score.to_string()));
            }
        }
        Kind::Game => {
            params.extend([("page_size", "40".to_owned()), ("ordering", "-added".to_owned()), ("exclude_additions", "true".to_owned())]);
            if let Some(genre) = genre {
                params.push(("genres", genre.to_owned()));
            }
            if let Some(year) = query.year_from {
                params.push(("dates", format!("{year}-01-01,2100-12-31")));
            }
            if let Some(score) = query.min_score {
                params.push(("metacritic", format!("{},100", u32::from(score) * 10)));
            }
        }
    }
    params
}

type Cached = (Instant, Arc<Vec<Item>>);
static CACHE: LazyLock<DashMap<String, Cached>> = LazyLock::new(DashMap::new);

/// Süzgece uyan popüler yapımlardan bir sayfa (TMDB 20, RAWG 40 öğe).
pub async fn list(http: &reqwest::Client, kind: Kind, query: &Query, page: u32) -> anyhow::Result<Arc<Vec<Item>>> {
    warm_imdb(http);
    let params = list_params(kind, query, page);
    let cache_key = format!("{}:{params:?}", kind.code());
    if let Some(hit) = CACHE.get(&cache_key).filter(|e| e.0.elapsed() < CACHE_FOR) {
        return Ok(Arc::clone(&hit.1));
    }
    let items: Vec<Item> = match kind {
        Kind::Movie | Kind::Series => {
            let path = if kind == Kind::Movie { "/discover/movie" } else { "/discover/tv" };
            let body = send_json(tmdb_request(http, path)?.query(&params)).await?;
            body["results"].as_array().map(|list| list.iter().filter_map(|v| parse_tmdb(v, Some(kind))).collect()).unwrap_or_default()
        }
        Kind::Game => {
            let body = send_json(rawg_request(http, "/games")?.query(&params)).await?;
            body["results"].as_array().map(|list| list.iter().filter_map(parse_rawg).collect()).unwrap_or_default()
        }
    };
    let items = Arc::new(items.into_iter().filter(|i| !i.adult).collect::<Vec<_>>());
    if CACHE.len() > 300 {
        CACHE.retain(|_, e| e.0.elapsed() < CACHE_FOR);
    }
    CACHE.insert(cache_key, (Instant::now(), Arc::clone(&items)));
    Ok(items)
}

/// Film, dizi (TMDB) ve oyunlarda (RAWG) arama; anahtarı olmayan kaynak atlanır. Sıra: film/dizi, sonra oyun.
pub async fn search(http: &reqwest::Client, text: &str) -> anyhow::Result<Vec<Item>> {
    warm_imdb(http);
    let text = text.trim();
    let keys = crate::integrations::get();
    let tmdb = async {
        if keys.tmdb_api_key.is_none() {
            return Ok(Vec::new());
        }
        let request = tmdb_request(http, "/search/multi")?.query(&[("query", text), ("language", "tr-TR"), ("include_adult", "false")]);
        let body = send_json(request).await?;
        anyhow::Ok(body["results"].as_array().map(|list| list.iter().filter_map(|v| parse_tmdb(v, None)).collect::<Vec<_>>()).unwrap_or_default())
    };
    let rawg = async {
        if keys.rawg_api_key.is_none() {
            return Ok(Vec::new());
        }
        let body = send_json(rawg_request(http, "/games")?.query(&[("search", text), ("page_size", "5")])).await?;
        anyhow::Ok(body["results"].as_array().map(|list| list.iter().filter_map(parse_rawg).collect::<Vec<_>>()).unwrap_or_default())
    };
    let (tmdb, rawg) = tokio::join!(tmdb, rawg);
    if let (Err(e), Err(_)) = (&tmdb, &rawg) {
        anyhow::bail!("arama yapılamadı: {e}");
    }
    let mut found: Vec<Item> = tmdb.unwrap_or_default();
    found.extend(rawg.unwrap_or_default());
    found.retain(|i| !i.adult);
    Ok(found)
}

/// Ayrıntı sayfasıyla eksikleri doldurur (süre, sezon, tür, IMDb kimliği, yapımcı, İngilizce özet…).
/// Hata olursa öğe listedeki hâliyle kalır.
pub async fn enrich(http: &reqwest::Client, item: &mut Item) {
    if let Err(e) = try_enrich(http, item).await {
        tracing::debug!(item = %item.key(), error = %e, "Tavsiye ayrıntısı alınamadı");
    }
}

async fn try_enrich(http: &reqwest::Client, item: &mut Item) -> anyhow::Result<()> {
    match item.kind {
        Kind::Movie | Kind::Series => {
            let base = if item.kind == Kind::Movie { "movie" } else { "tv" };
            let path = format!("/{base}/{}", item.id);
            let body = send_json(tmdb_request(http, &path)?.query(&[("language", "tr-TR"), ("append_to_response", "external_ids")])).await?;
            item.imdb_id = text(&body, "imdb_id").or_else(|| body.get("external_ids").and_then(|e| text(e, "imdb_id")));
            item.runtime = body.get("runtime").and_then(Value::as_i64).or_else(|| body.pointer("/episode_run_time/0").and_then(Value::as_i64));
            item.seasons = body.get("number_of_seasons").and_then(Value::as_i64);
            item.episodes = body.get("number_of_episodes").and_then(Value::as_i64);
            item.genres = names(&body, "genres");
            item.makers = if item.kind == Kind::Series { names(&body, "networks") } else { names(&body, "production_companies") };
            item.makers.truncate(3);
            if let Some(overview) = text(&body, "overview") {
                item.overview = overview;
            }
            if item.overview.is_empty() {
                let english = send_json(tmdb_request(http, &path)?.query(&[("language", "en-US")])).await?;
                item.english_overview = text(&english, "overview");
            }
        }
        Kind::Game => {
            let body = send_json(rawg_request(http, &format!("/games/{}", item.id))?).await?;
            item.english_overview = text(&body, "description_raw");
            item.makers = names(&body, "developers");
            item.makers.truncate(3);
            item.genres = names(&body, "genres");
            item.platforms = body
                .get("parent_platforms")
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(|p| p.get("platform").and_then(|p| text(p, "name"))).collect())
                .unwrap_or_default();
        }
    }
    Ok(())
}

// ===================== IMDb puanları =====================

/// "tt0468569" → 468569
fn imdb_number(id: &str) -> Option<u32> {
    id.strip_prefix("tt")?.parse().ok()
}

struct Ratings {
    loaded: Instant,
    /// IMDb numarası → (puan, oy sayısı)
    map: HashMap<u32, (f32, u32)>,
}

static RATINGS: LazyLock<RwLock<Option<Arc<Ratings>>>> = LazyLock::new(|| RwLock::new(None));
static LOADING: AtomicBool = AtomicBool::new(false);

fn ratings() -> Option<Arc<Ratings>> {
    RATINGS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// Puan dosyası yoksa ya da bir günden eskiyse arka planda indirmeye başlar (aynı anda tek indirme).
fn warm_imdb(http: &reqwest::Client) {
    if ratings().is_some_and(|r| r.loaded.elapsed() < IMDB_REFRESH) || LOADING.swap(true, Ordering::AcqRel) {
        return;
    }
    let http = http.clone();
    tokio::spawn(async move {
        match load_ratings(&http).await {
            Ok(map) => {
                tracing::info!(titles = map.len(), "IMDb puanları yüklendi");
                *RATINGS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(Ratings { loaded: Instant::now(), map }));
            }
            Err(e) => tracing::warn!(error = ?e, "IMDb puan dosyası indirilemedi"),
        }
        LOADING.store(false, Ordering::Release);
    });
}

async fn load_ratings(http: &reqwest::Client) -> anyhow::Result<HashMap<u32, (f32, u32)>> {
    let bytes = http.get(IMDB_RATINGS).timeout(Duration::from_secs(120)).send().await?.error_for_status()?.bytes().await?;
    Ok(tokio::task::spawn_blocking(move || parse_ratings(BufReader::new(flate2::read::GzDecoder::new(&bytes[..])))).await?)
}

/// `tconst<TAB>averageRating<TAB>numVotes` satırları; az oylu ya da bozuk satırlar atlanır.
fn parse_ratings(reader: impl BufRead) -> HashMap<u32, (f32, u32)> {
    reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let id = imdb_number(parts.next()?)?;
            let rating: f32 = parts.next()?.parse().ok()?;
            let votes: u32 = parts.next()?.parse().ok()?;
            (votes >= IMDB_MIN_VOTES).then_some((id, (rating, votes)))
        })
        .collect()
}

/// IMDb puanı ve oy sayısı (dosya henüz yüklenmediyse `None`).
pub fn imdb_rating(id: &str) -> Option<(f32, u32)> {
    ratings()?.map.get(&imdb_number(id)?).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tmdb_and_rawg_items() -> anyhow::Result<()> {
        let movie: Value = serde_json::from_str(
            r#"{"id":155,"title":"Kara Şövalye","overview":"Batman, Joker'e karşı.","poster_path":"/a.jpg","release_date":"2008-07-16","vote_average":8.5,"adult":false}"#,
        )?;
        let item = parse_tmdb(&movie, Some(Kind::Movie)).ok_or_else(|| anyhow::anyhow!("film yok"))?;
        assert_eq!((item.title.as_str(), item.year, item.score), ("Kara Şövalye", Some(2008), Some(8.5)));
        assert_eq!(item.poster.as_deref(), Some("https://image.tmdb.org/t/p/w342/a.jpg"));
        assert_eq!(item.url(), "https://www.themoviedb.org/movie/155");

        let person: Value = serde_json::from_str(r#"{"id":1,"name":"Biri","media_type":"person"}"#)?;
        assert!(parse_tmdb(&person, None).is_none());
        let series: Value = serde_json::from_str(r#"{"id":1399,"name":"Taht Oyunları","media_type":"tv","first_air_date":"2011-04-17"}"#)?;
        assert_eq!(parse_tmdb(&series, None).map(|i| (i.kind, i.year)), Some((Kind::Series, Some(2011))));

        let game: Value = serde_json::from_str(
            r#"{"id":3328,"slug":"the-witcher-3-wild-hunt","name":"The Witcher 3","released":"2015-05-18","metacritic":92,"background_image":"https://media.rawg.io/x.jpg","esrb_rating":{"slug":"mature"}}"#,
        )?;
        let item = parse_rawg(&game).ok_or_else(|| anyhow::anyhow!("oyun yok"))?;
        assert_eq!((item.year, item.score, item.adult), (Some(2015), Some(92.0), false));
        assert_eq!(item.url(), "https://rawg.io/games/the-witcher-3-wild-hunt");
        let adult: Value = serde_json::from_str(r#"{"id":1,"name":"x","esrb_rating":{"slug":"adults-only"}}"#)?;
        assert!(parse_rawg(&adult).is_some_and(|i| i.adult));
        Ok(())
    }

    #[test]
    fn builds_filters_with_valid_values_only() {
        let query = Query { genre: Some("27".to_owned()), year_from: Some(2010), min_score: Some(7) };
        let params = list_params(Kind::Movie, &query, 2);
        assert!(params.contains(&("with_genres", "27".to_owned())));
        assert!(params.contains(&("primary_release_date.gte", "2010-01-01".to_owned())));
        assert!(params.contains(&("vote_average.gte", "7".to_owned())));
        let game = list_params(Kind::Game, &Query { genre: Some("bilinmeyen".to_owned()), min_score: Some(8), ..Query::default() }, 1);
        assert!(game.contains(&("metacritic", "80,100".to_owned())));
        assert!(!game.iter().any(|(k, _)| *k == "genres"));
    }

    #[test]
    fn parses_imdb_ratings_file() {
        let file = "tconst\taverageRating\tnumVotes\ntt0468569\t9.0\t3100000\ntt0000001\t5.7\t12\nbozuk satır\n";
        let map = parse_ratings(BufReader::new(file.as_bytes()));
        assert_eq!(map.get(&468_569), Some(&(9.0, 3_100_000)));
        assert_eq!(map.len(), 1); // az oylu ve başlık satırı atlanır
        assert_eq!(imdb_number("tt0468569"), Some(468_569));
        assert_eq!(imdb_number("nm123"), None);
    }

    /// `cargo test live_catalog -- --ignored --nocapture` (`.env`'de TMDB_API_KEY ve RAWG_API_KEY gerekir)
    #[tokio::test]
    #[ignore = "gerçek TMDB / RAWG / IMDb'ye istek atar"]
    async fn live_catalog() -> anyhow::Result<()> {
        let _ = dotenvy::dotenv();
        crate::integrations::init();
        let http = reqwest::Client::builder().build()?;
        for kind in [Kind::Movie, Kind::Series, Kind::Game] {
            let found = list(&http, kind, &Query { min_score: Some(7), ..Query::default() }, 1).await?;
            let mut first = found.first().cloned().ok_or_else(|| anyhow::anyhow!("{kind:?} boş"))?;
            enrich(&http, &mut first).await;
            println!("{kind:?}: {} öğe · {} ({:?}) puan {:?} imdb {:?} · {:?}", found.len(), first.title, first.year, first.score, first.imdb_id, first.genres);
            println!("  özet: {}", if first.overview.is_empty() { first.english_overview.clone().unwrap_or_default() } else { first.overview.clone() }.chars().take(150).collect::<String>());
        }
        let hits = search(&http, "witcher").await?;
        println!("arama: {:?}", hits.iter().take(6).map(|i| (i.kind, i.title.clone())).collect::<Vec<_>>());
        let ratings = load_ratings(&http).await?;
        println!("IMDb puanı olan yapım: {} · Kara Şövalye: {:?}", ratings.len(), ratings.get(&468_569));
        assert!(!hits.is_empty());
        Ok(())
    }
}
