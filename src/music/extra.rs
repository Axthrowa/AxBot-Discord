//! Müzik eklentileri: radyo istasyonları, Spotify bağlantıları, şarkı sözleri,
//! otomatik çalma için benzer şarkı bulma ve ileri sarma için süre okuma.

use std::time::Duration;

use anyhow::{anyhow, Context as _};
use regex::Regex;
use tokio::process::Command;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
const MIX_TIMEOUT: Duration = Duration::from_secs(40);

/// GET isteği; bağlantı hatasında bir kez daha dener (ağda ara sıra TLS kopmaları oluyor).
async fn get(request: impl Fn() -> reqwest::RequestBuilder) -> reqwest::Result<reqwest::Response> {
    match request().send().await {
        Ok(response) => Ok(response),
        Err(e) if e.is_connect() || e.is_request() || e.is_timeout() => {
            tracing::debug!(error = %e, "İstek başarısız, tekrar deneniyor");
            request().send().await
        }
        Err(e) => Err(e),
    }
}

// ===================== Radyo =====================

pub struct Station {
    pub name: &'static str,
    pub url: &'static str,
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum RadioChoice {
    #[name = "Süper FM"]
    SuperFm,
    #[name = "Metro FM"]
    MetroFm,
    #[name = "Joy FM"]
    JoyFm,
    #[name = "Joy Türk"]
    JoyTurk,
    #[name = "Joy Jazz"]
    JoyJazz,
    #[name = "Virgin Radio"]
    Virgin,
    #[name = "Lofi / Chillhop"]
    Lofi,
}

/// Türk radyolarının (Doğuş/Süper grubu) yayın adresleri.
macro_rules! stw {
    ($mount:literal) => {
        concat!("https://playerservices.streamtheworld.com/api/livestream-redirect/", $mount, ".mp3")
    };
}

impl RadioChoice {
    pub fn station(self) -> Station {
        let (name, url) = match self {
            Self::SuperFm => ("Süper FM", stw!("SUPER_FM")),
            Self::MetroFm => ("Metro FM", stw!("METRO_FM")),
            Self::JoyFm => ("Joy FM", stw!("JOY_FM")),
            Self::JoyTurk => ("Joy Türk", stw!("JOY_TURK")),
            Self::JoyJazz => ("Joy Jazz", stw!("JOY_JAZZ")),
            Self::Virgin => ("Virgin Radio", stw!("VIRGIN_RADIO")),
            Self::Lofi => ("I♥Chillhop (lofi)", "https://streams.ilovemusic.de/iloveradio17.mp3"),
        };
        Station { name, url }
    }
}

// ===================== Otomatik çalma =====================

/// YouTube bağlantısından video kimliği.
pub fn youtube_video_id(url: &str) -> Option<String> {
    let re = Regex::new(r"(?:v=|youtu\.be/|/shorts/)([A-Za-z0-9_-]{11})").ok()?;
    re.captures(url).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned())
}

/// YouTube video kapağı (her videoda bulunan orta boy görsel).
pub fn youtube_cover(video_id: &str) -> String {
    format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg")
}

/// Spotify şarkısının kapak görseli (oEmbed; anahtar gerekmez).
pub async fn spotify_cover(client: &reqwest::Client, track_url: &str) -> Option<String> {
    let json: serde_json::Value = get(|| {
        client
            .get("https://open.spotify.com/oembed")
            .query(&[("url", track_url)])
            .timeout(Duration::from_secs(8))
    })
    .await
    .ok()?
    .json()
    .await
    .ok()?;
    json.get("thumbnail_url")?.as_str().map(str::to_owned)
}

pub struct Related {
    pub url: String,
    pub title: String,
    pub duration: Option<Duration>,
}

/// YouTube'un "Mix" listesinden son çalınanlarda olmayan ilk şarkı.
pub async fn related_track(ytdlp: &str, args: &[String], seed: &str, recent: &[String]) -> anyhow::Result<Option<Related>> {
    let url = format!("https://www.youtube.com/watch?v={seed}&list=RD{seed}");
    let output = tokio::time::timeout(
        MIX_TIMEOUT,
        Command::new(ytdlp)
            .args(args)
            .args(["--flat-playlist", "-J", "--no-warnings", "--playlist-end", "25"])
            .arg(&url)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| anyhow!("benzer şarkı listesi zaman aşımına uğradı"))??;
    if !output.status.success() {
        return Err(anyhow!("yt-dlp: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(pick_related(&json, seed, recent))
}

fn pick_related(json: &serde_json::Value, seed: &str, recent: &[String]) -> Option<Related> {
    json.get("entries")?.as_array()?.iter().find_map(|e| {
        let id = e.get("id")?.as_str()?;
        let title = e.get("title")?.as_str()?;
        let duration = e.get("duration").and_then(serde_json::Value::as_f64);
        // Aynı şarkı, son çalınanlar, canlı yayınlar (süresiz) ve 10 dakikadan uzunlar atlanır.
        let usable = id != seed
            && !recent.iter().any(|r| r == id)
            && duration.is_some_and(|d| d > 30.0 && d < 600.0)
            && !title.starts_with('[');
        usable.then(|| Related {
            url: format!("https://www.youtube.com/watch?v={id}"),
            title: title.to_owned(),
            duration: duration.map(Duration::from_secs_f64),
        })
    })
}

// ===================== Spotify =====================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpotifyKind {
    Track,
    Album,
    Playlist,
}

/// open.spotify.com/(intl-tr/)track|album|playlist/<id>
pub fn spotify_link(url: &str) -> Option<(SpotifyKind, String)> {
    let re = Regex::new(r"open\.spotify\.com/(?:intl-[a-z]{2}/)?(track|album|playlist)/([A-Za-z0-9]{22})").ok()?;
    let caps = re.captures(url)?;
    let kind = match caps.get(1)?.as_str() {
        "track" => SpotifyKind::Track,
        "album" => SpotifyKind::Album,
        _ => SpotifyKind::Playlist,
    };
    Some((kind, caps.get(2)?.as_str().to_owned()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotifyTrack {
    pub title: String,
    pub artists: String,
    pub duration: Option<Duration>,
    /// open.spotify.com/track/… bağlantısı.
    pub url: Option<String>,
    /// Kapak görseli (sadece tek şarkı bağlantılarında bilinir).
    pub cover: Option<String>,
}

/// Spotify Web API erişim anahtarı (client credentials; ~1 saat geçerli, önbellekte).
async fn spotify_token(client: &reqwest::Client, id: &str, secret: &str) -> anyhow::Result<String> {
    static CACHE: tokio::sync::Mutex<Option<(String, std::time::Instant)>> = tokio::sync::Mutex::const_new(None);
    let mut cache = CACHE.lock().await;
    if let Some((token, until)) = cache.as_ref() {
        if std::time::Instant::now() < *until {
            return Ok(token.clone());
        }
    }
    #[derive(serde::Deserialize)]
    struct Token {
        access_token: String,
        expires_in: u64,
    }
    let token: Token = client
        .post("https://accounts.spotify.com/api/token")
        .basic_auth(id, Some(secret))
        .form(&[("grant_type", "client_credentials")])
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .context("Spotify'a ulaşılamadı")?
        .error_for_status()
        .context("Spotify anahtarı reddedildi (SPOTIFY_CLIENT_ID / SECRET)")?
        .json()
        .await?;
    let until = std::time::Instant::now() + Duration::from_secs(token.expires_in.saturating_sub(60));
    *cache = Some((token.access_token.clone(), until));
    Ok(token.access_token)
}

/// Şarkı adını Spotify'da arar (Türkiye kataloğu). Anahtar yoksa `Ok(None)`.
pub async fn spotify_search(client: &reqwest::Client, query: &str) -> anyhow::Result<Option<SpotifyTrack>> {
    let Some((id, secret)) = crate::integrations::get().spotify.as_ref() else {
        return Ok(None);
    };
    let token = spotify_token(client, id, secret).await?;
    let body: serde_json::Value = client
        .get("https://api.spotify.com/v1/search")
        .bearer_auth(token)
        .query(&[("q", query), ("type", "track"), ("limit", "1"), ("market", "TR")])
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(body["tracks"]["items"].get(0).and_then(search_item))
}

/// Spotify arama sonucundaki şarkı.
fn search_item(t: &serde_json::Value) -> Option<SpotifyTrack> {
    let title = t["name"].as_str()?.trim().to_owned();
    if title.is_empty() {
        return None;
    }
    let artists = t["artists"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    Some(SpotifyTrack {
        title,
        artists,
        duration: t["duration_ms"].as_u64().map(Duration::from_millis),
        url: t["external_urls"]["spotify"].as_str().map(str::to_owned),
        cover: t["album"]["images"].get(0).and_then(|i| i["url"].as_str()).map(str::to_owned),
    })
}

/// "spotify:track:ID" → "https://open.spotify.com/track/ID"
fn spotify_track_url(v: &serde_json::Value) -> Option<String> {
    let uri = v.get("uri")?.as_str()?;
    let id = uri.strip_prefix("spotify:track:")?;
    Some(format!("https://open.spotify.com/track/{id}"))
}

pub struct SpotifyList {
    pub name: String,
    pub tracks: Vec<SpotifyTrack>,
}

/// Spotify'ın herkese açık gömme (embed) sayfasından şarkı bilgilerini okur (anahtar gerekmez).
pub async fn spotify(client: &reqwest::Client, kind: &SpotifyKind, id: &str) -> anyhow::Result<SpotifyList> {
    let path = match kind {
        SpotifyKind::Track => "track",
        SpotifyKind::Album => "album",
        SpotifyKind::Playlist => "playlist",
    };
    let url = format!("https://open.spotify.com/embed/{path}/{id}");
    let html = get(|| client.get(&url).timeout(REQUEST_TIMEOUT))
        .await
        .context("Spotify'a ulaşılamadı")?
        .error_for_status()
        .context("Spotify bağlantısı bulunamadı (özel liste olabilir)")?
        .text()
        .await?;
    parse_spotify_embed(&html)
}

fn parse_spotify_embed(html: &str) -> anyhow::Result<SpotifyList> {
    let re = Regex::new(r#"(?s)<script id="__NEXT_DATA__" type="application/json">(.*?)</script>"#)?;
    let raw = re
        .captures(html)
        .and_then(|c| c.get(1))
        .ok_or_else(|| anyhow!("Spotify sayfası beklenen biçimde değil"))?;
    let json: serde_json::Value = serde_json::from_str(raw.as_str())?;
    let entity = json
        .pointer("/props/pageProps/state/data/entity")
        .ok_or_else(|| anyhow!("Spotify verisi bulunamadı"))?;
    let text = |v: &serde_json::Value, key: &str| v.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    let name = text(entity, "name");
    let name = if name.is_empty() { text(entity, "title") } else { name };
    let ms = |v: &serde_json::Value| v.get("duration").and_then(serde_json::Value::as_u64).filter(|d| *d > 0).map(Duration::from_millis);

    let tracks = match entity.get("trackList").and_then(serde_json::Value::as_array) {
        // Liste/albüm: her şarkının başlığı ve sanatçıları (subtitle).
        Some(list) => list
            .iter()
            .filter_map(|t| {
                let title = text(t, "title");
                (!title.is_empty()).then(|| SpotifyTrack {
                    title,
                    artists: text(t, "subtitle").replace('\u{a0}', " "),
                    duration: ms(t),
                    url: spotify_track_url(t),
                    cover: None,
                })
            })
            .collect(),
        // Tek şarkı.
        None => {
            let artists = entity
                .get("artists")
                .and_then(serde_json::Value::as_array)
                .map(|a| a.iter().map(|x| text(x, "name")).filter(|n| !n.is_empty()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            // En büyük kapak görseli.
            let cover = entity
                .pointer("/coverArt/sources")
                .and_then(serde_json::Value::as_array)
                .and_then(|s| s.iter().max_by_key(|c| c.get("width").and_then(serde_json::Value::as_u64).unwrap_or(0)))
                .and_then(|c| c.get("url"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            vec![SpotifyTrack { title: name.clone(), artists, duration: ms(entity), url: spotify_track_url(entity), cover }]
        }
    };
    if tracks.is_empty() {
        return Err(anyhow!("Spotify'da çalınabilir şarkı bulunamadı"));
    }
    Ok(SpotifyList { name, tracks })
}

// ===================== Şarkı sözleri =====================

/// "TARKAN - Şımarık (Official Music Video)" → "TARKAN - Şımarık"
pub fn clean_title(title: &str) -> String {
    let brackets = Regex::new(r"[\(\[\{][^\)\]\}]*[\)\]\}]").map(|re| re.replace_all(title, "").into_owned()).unwrap_or_else(|_| title.to_owned());
    let noise = ["official video", "official music video", "official audio", "lyrics", "lyric video", "4k", "hd", "klip", "video klip"];
    let mut cleaned = brackets;
    for word in noise {
        let lower = cleaned.to_lowercase();
        if let Some(pos) = lower.find(word) {
            cleaned.replace_range(pos..pos + word.len(), "");
        }
    }
    cleaned.split('|').next().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ").trim_matches(['-', ' ']).to_owned()
}

// ===================== İleri sarma =====================

/// "90", "1:30", "1:02:03" → süre.
pub fn parse_position(input: &str) -> Option<Duration> {
    let parts: Vec<u64> = input.trim().split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [s] => *s,
        [m, s] if *s < 60 => m * 60 + s,
        [h, m, s] if *m < 60 && *s < 60 => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

// ===================== Bağlantı güvenliği =====================

/// İnternette herkese açık bir adres mi? Yerel ağ / bu bilgisayar (veritabanı, Redis, web formu)
/// adresleri reddedilir.
fn is_public_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || a == 0
                || (a == 100 && (64..128).contains(&b)))
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public_ip(IpAddr::V4(v4)),
            None => {
                let first = v6.segments()[0];
                !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast()
                    || (first & 0xfe00) == 0xfc00
                    || (first & 0xffc0) == 0xfe80)
            }
        },
    }
}

/// `/çal` bağlantılarında kabul edilen siteler (alt alan adları dahil).
/// yt-dlp yönlendirmeleri kendisi izler; rastgele bir adres herkese açık görünüp sonra iç ağa
/// (ör. bulut sunucusunun 169.254.169.254 bilgi servisine) yönlendirebilir. Bu yüzden yalnızca bilinen müzik/video siteleri.
pub const MEDIA_HOSTS: &[&str] = &[
    "youtube.com", "youtu.be", "youtube-nocookie.com", "soundcloud.com", "spotify.com", "spotify.link", "bandcamp.com",
    "vimeo.com", "twitch.tv", "kick.com", "mixcloud.com", "dailymotion.com", "deezer.com", "audiomack.com",
    "tiktok.com", "x.com", "twitter.com", "instagram.com", "reddit.com", "archive.org", "bilibili.com", "nicovideo.jp",
];

/// Bağlantı `MEDIA_HOSTS` listesindeki bir siteye mi ait?
pub fn is_media_site(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url.trim()) else {
        return false;
    };
    let Some(url::Host::Domain(host)) = parsed.host() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    MEDIA_HOSTS.iter().any(|known| host == *known || host.ends_with(&format!(".{known}")))
}

/// Kullanıcının verdiği bağlantı yt-dlp'ye verilebilir mi? Sadece http(s) ve sadece herkese açık
/// adresler: aksi halde biri `/çal http://127.0.0.1:...` ile botun iç servislerine istek attırabilir.
pub async fn is_public_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url.trim()) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    match parsed.host() {
        Some(url::Host::Ipv4(ip)) => is_public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => is_public_ip(ip.into()),
        Some(url::Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            if domain == "localhost" || domain.ends_with(".localhost") || domain.ends_with(".local") {
                return false;
            }
            // Alan adı yerel bir adrese çözülüyorsa (ör. 127.0.0.1.nip.io) yine reddedilir.
            match crate::net::lookup_ips(&domain).await {
                Ok(ips) => !ips.is_empty() && ips.into_iter().all(is_public_ip),
                Err(_) => false,
            }
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_known_media_sites() {
        for good in [
            "https://www.youtube.com/watch?v=abc",
            "https://youtu.be/abc",
            "https://music.youtube.com/watch?v=abc",
            "https://open.spotify.com/track/1",
            "https://soundcloud.com/a/b",
            "https://artist.bandcamp.com/track/x",
        ] {
            assert!(is_media_site(good), "{good} kabul edilmeliydi");
        }
        for bad in [
            "https://evil.example/redirect",
            "https://youtube.com.evil.example/",
            "https://notyoutube.com/x",
            "http://169.254.169.254/latest",
            "https://127.0.0.1/a",
            "çal bunu",
        ] {
            assert!(!is_media_site(bad), "{bad} reddedilmeliydi");
        }
    }

    #[tokio::test]
    async fn rejects_private_urls() {
        for bad in [
            "http://127.0.0.1:6379",
            "http://localhost:8080/itiraz",
            "http://10.0.0.5/a",
            "http://192.168.1.1",
            "http://[::1]:5432",
            "http://[::ffff:127.0.0.1]/",
            "http://169.254.169.254/latest/meta-data",
            "file:///C:/Windows/win.ini",
            "ftp://example.com/a.mp3",
            "http://0.0.0.0:8888",
        ] {
            assert!(!is_public_url(bad).await, "{bad} reddedilmeliydi");
        }
        assert!(is_public_url("http://8.8.8.8/x").await);
    }

    #[test]
    fn extracts_youtube_ids() {
        assert_eq!(youtube_video_id("https://www.youtube.com/watch?v=cpp69ghR1IM"), Some("cpp69ghR1IM".to_owned()));
        assert_eq!(youtube_video_id("https://youtu.be/cpp69ghR1IM?t=3"), Some("cpp69ghR1IM".to_owned()));
        assert_eq!(youtube_video_id("https://soundcloud.com/x/y"), None);
    }

    #[test]
    fn picks_related_skipping_recent_and_long() {
        let json = serde_json::json!({ "entries": [
            { "id": "seedseedsee", "title": "Aynı", "duration": 200.0 },
            { "id": "recentrecen", "title": "Yakın zamanda", "duration": 200.0 },
            { "id": "longlonglon", "title": "Uzun mix", "duration": 3600.0 },
            { "id": "livelivelive", "title": "Canlı", "duration": null },
            { "id": "goodgoodgoo", "title": "Bu", "duration": 210.0 }
        ]});
        let picked = pick_related(&json, "seedseedsee", &["recentrecen".to_owned()]);
        assert_eq!(picked.map(|r| r.title), Some("Bu".to_owned()));
    }

    #[test]
    fn reads_spotify_search_item() -> anyhow::Result<()> {
        let item = serde_json::json!({
            "name": "Şımarık",
            "artists": [{"name": "Tarkan"}],
            "duration_ms": 213000,
            "external_urls": {"spotify": "https://open.spotify.com/track/abc"},
            "album": {"images": [{"url": "https://i.scdn.co/image/x"}]}
        });
        let track = search_item(&item).ok_or_else(|| anyhow!("okunamadı"))?;
        assert_eq!((track.title.as_str(), track.artists.as_str()), ("Şımarık", "Tarkan"));
        assert_eq!(track.duration, Some(Duration::from_secs(213)));
        assert!(track.url.is_some() && track.cover.is_some());
        Ok(())
    }

    #[test]
    fn recognises_spotify_links() {
        assert_eq!(
            spotify_link("https://open.spotify.com/intl-tr/track/4uLU6hMCjMI75M1A2tKUQC?si=x"),
            Some((SpotifyKind::Track, "4uLU6hMCjMI75M1A2tKUQC".to_owned()))
        );
        assert_eq!(
            spotify_link("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"),
            Some((SpotifyKind::Playlist, "37i9dQZF1DXcBWIGoYBM5M".to_owned()))
        );
        assert_eq!(spotify_link("https://www.youtube.com/watch?v=x"), None);
    }

    #[test]
    fn parses_spotify_embed() -> anyhow::Result<()> {
        let html = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"data":{"entity":
            {"type":"playlist","name":"Liste","trackList":[{"title":"Şımarık","subtitle":"Tarkan","duration":192000},{"title":"Dudu","subtitle":"Tarkan"}]}}}}}}</script>"#;
        let list = parse_spotify_embed(html)?;
        assert_eq!(list.name, "Liste");
        assert_eq!(list.tracks.len(), 2);
        assert_eq!(list.tracks.first().map(|t| (t.artists.as_str(), t.duration)), Some(("Tarkan", Some(Duration::from_secs(192)))));
        Ok(())
    }

    #[test]
    fn cleans_titles_for_lyrics() {
        assert_eq!(clean_title("TARKAN - Şımarık (Official Music Video)"), "TARKAN - Şımarık");
        assert_eq!(clean_title("Sezen Aksu - Firuze [HD] | Klip"), "Sezen Aksu - Firuze");
        assert_eq!(clean_title("Mabel Matiz - Umrumdışı"), "Mabel Matiz - Umrumdışı");
    }

    #[test]
    fn parses_seek_positions() {
        assert_eq!(parse_position("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_position("1:30"), Some(Duration::from_secs(90)));
        assert_eq!(parse_position("1:02:03"), Some(Duration::from_secs(3723)));
        assert_eq!(parse_position("1:75"), None);
        assert_eq!(parse_position("abc"), None);
    }

    #[test]
    fn stations_point_to_streams() {
        for choice in [RadioChoice::SuperFm, RadioChoice::JoyTurk, RadioChoice::Virgin, RadioChoice::Lofi] {
            assert!(choice.station().url.starts_with("https://"));
        }
    }
}

/// Gerçek ağ testleri. Elle: `cargo test live_music_extras -- --ignored --nocapture`
#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::music::lyrics::lyrics;
    use songbird::input::codecs::{get_codec_registry, get_probe};
    use songbird::input::{HttpRequest, Input};

    #[tokio::test]
    #[ignore = "gerçek Spotify, lrclib ve radyo yayınlarına bağlanır"]
    async fn live_music_extras() -> anyhow::Result<()> {
        let client = crate::net::http_client()?;

        let track = spotify(&client, &SpotifyKind::Track, "4uLU6hMCjMI75M1A2tKUQC").await?;
        println!("Spotify şarkı: {:?}", track.tracks.first());
        let list = spotify(&client, &SpotifyKind::Playlist, "37i9dQZF1DXcBWIGoYBM5M").await?;
        println!("Spotify liste: {} · {} şarkı · ilk: {:?}", list.name, list.tracks.len(), list.tracks.first());
        assert!(!list.tracks.is_empty());

        let found = lyrics(&client, &clean_title("TARKAN - Şımarık (Official Music Video)"))
            .await
            .map_err(|e| anyhow!("{e:#}"))?;
        let found = found.ok_or_else(|| anyhow!("şarkı sözü bulunamadı"))?;
        println!("Sözler: {} - {} ({} karakter)", found.artist, found.title, found.text.len());

        let related = related_track("yt-dlp", &[], "cpp69ghR1IM", &["-CxauCeQ_SQ".to_owned()]).await?;
        let related = related.ok_or_else(|| anyhow!("benzer şarkı bulunamadı"))?;
        println!("Otomatik çalma (Şımarık'tan sonra): {} {}", related.title, related.url);
        assert!(!related.url.contains("cpp69ghR1IM") && !related.url.contains("-CxauCeQ_SQ"));

        for choice in [RadioChoice::SuperFm, RadioChoice::MetroFm, RadioChoice::JoyFm, RadioChoice::JoyTurk, RadioChoice::JoyJazz, RadioChoice::Virgin, RadioChoice::Lofi] {
            let station = choice.station();
            let started = std::time::Instant::now();
            let input: Input = HttpRequest::new(client.clone(), station.url.to_owned()).into();
            let playable = input.make_playable_async(get_codec_registry(), get_probe()).await?;
            assert!(playable.is_playable(), "{} çözülemedi", station.name);
            println!("📻 {} çalınabilir ({:.1} sn)", station.name, started.elapsed().as_secs_f64());
        }
        Ok(())
    }
}
