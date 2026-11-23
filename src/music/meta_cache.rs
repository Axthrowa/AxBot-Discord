//! Redis tabanlı sorgu + YouTube video_id müzik metaveri önbelleği.
//!
//! Amacı: Yerel disk önbelleğinde (`%TEMP%/axbot_music_cache`) ses dosyası hazır olan bir şarkı
//! tekrar arandığında veya bağlantısı verildiğinde `yt-dlp` ve Deno süreçlerini HİÇ başlatmadan
//! şarkıyı <500 ms (çoğunlukla <10 ms) içinde çalmaya başlatmak.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;
use std::time::Duration;

use redis::aio::ConnectionManager;
use tracing::{debug, warn};

use super::TrackInfo;

/// Redis TTL süresi (7 gün): arama metni → video bilgisi; sık çalınan şarkılar aramasız başlar.
pub const META_CACHE_TTL_SECS: u64 = 7 * 24 * 3600;

/// Redis istek zaman aşımı (250 ms): Redis yavaşlarsa veya yanıt vermezse müzik akışı bekletilmez (fail-open).
pub const REDIS_TIMEOUT: Duration = Duration::from_millis(250);

/// YouTube video URL'lerinden 11 karakterlik video_id çıkaran derlenmiş regex.
static YT_RE: LazyLock<Option<regex::Regex>> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?:(?:https?://)?(?:www\.|m\.|music\.)?(?:youtube\.com/(?:watch\?(?:[^&]*&)*v=|shorts/|embed/)|youtu\.be/))([A-Za-z0-9_-]{11})"
    ).ok()
});

/// Arama sorgusunu normalize eder:
/// - Baştaki/sondaki boşlukları temizler.
/// - Ardışık boşlukları tek boşluğa indirger.
/// - Türkçe karakter uyumlu küçük harfe çevirir (İ -> i, I -> ı, Ş -> ş vb.).
pub fn normalize_music_query(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len());
    let mut last_was_space = false;
    for c in raw.trim().chars() {
        if c.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            last_was_space = false;
            match c {
                'İ' => result.push('i'),
                'I' => result.push('ı'),
                'Ğ' => result.push('ğ'),
                'Ü' => result.push('ü'),
                'Ş' => result.push('ş'),
                'Ö' => result.push('ö'),
                'Ç' => result.push('ç'),
                other => {
                    for lc in other.to_lowercase() {
                        result.push(lc);
                    }
                }
            }
        }
    }
    result
}

/// YouTube bağlantısından 11 karakterlik geçerli video_id değerini ayıklar.
///
/// Desteklenen biçimler:
/// - `youtube.com/watch?v=...`
/// - `m.youtube.com/watch?v=...`
/// - `music.youtube.com/watch?v=...`
/// - `youtu.be/...`
/// - `youtube.com/shorts/...`
/// - `youtube.com/embed/...`
///
/// Eğer bağlantı doğrudan bir çalma listesi ise (`/playlist?list=...` ve `v=` yoksa) tekil önbellek atlanır (`None`).
pub fn extract_youtube_video_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if (trimmed.contains("/playlist?") || trimmed.contains("/playlist/")) && !trimmed.contains("v=") {
        return None;
    }
    let re = YT_RE.as_ref()?;
    let captures = re.captures(trimmed)?;
    let id_match = captures.get(1)?;
    let id = id_match.as_str();
    if id.len() == 11 {
        Some(id.to_owned())
    } else {
        None
    }
}

/// Video ID'den standart kanonik YouTube URL'si üretir.
pub fn canonical_youtube_url(video_id: &str) -> String {
    format!("https://www.youtube.com/watch?v={video_id}")
}

/// Girdiden kanonik hedef kimliğini döner: YouTube linki veya ID'si ise standart URL'ye çevirir.
pub fn canonical_identifier(raw: &str) -> String {
    if let Some(vid) = extract_youtube_video_id(raw) {
        canonical_youtube_url(&vid)
    } else {
        raw.trim().to_owned()
    }
}

/// Metin girdisi için deterministik hex karma döner.
pub fn hash_key(input: &str) -> String {
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Redis'te saklanan şarkı metaverisi.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct CachedTrackMeta {
    pub title: String,
    #[serde(default)]
    pub author: Option<String>,
    pub webpage_url: String,
    #[serde(default)]
    pub video_id: Option<String>,
    #[serde(default)]
    pub duration_secs: Option<u64>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub cache_key: String,
    #[serde(default)]
    pub cached_at: i64,
}

/// Redis'te verilen sorgu veya bağlantı için `CachedTrackMeta` arar.
///
/// Fail-open: Redis kapalıysa veya zaman aşımına uğrarsa `None` döner.
pub async fn get_cached_meta(
    redis: &ConnectionManager,
    query_or_url: &str,
) -> Option<CachedTrackMeta> {
    let trimmed = query_or_url.trim();
    let mut conn = redis.clone();

    // 1. Önce doğrudan video_id veya kanonik URL üzerinden anahtarları hazırla
    let mut candidate_keys = Vec::new();
    if let Some(vid) = extract_youtube_video_id(trimmed) {
        candidate_keys.push(format!("axbot:music:meta:vid:{vid}"));
        candidate_keys.push(format!("axbot:music:meta:url:{}", hash_key(&canonical_youtube_url(&vid))));
    } else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        candidate_keys.push(format!("axbot:music:meta:url:{}", hash_key(trimmed)));
    } else {
        let norm = normalize_music_query(trimmed);
        candidate_keys.push(format!("axbot:music:meta:q:{}", hash_key(&norm)));
    }

    for key in candidate_keys {
        let fetch_fut = async {
            let res: Option<String> = redis::cmd("GET").arg(&key).query_async(&mut conn).await.ok().flatten();
            res
        };

        if let Ok(Some(json_str)) = tokio::time::timeout(REDIS_TIMEOUT, fetch_fut).await {
            match serde_json::from_str::<CachedTrackMeta>(&json_str) {
                Ok(meta) => {
                    debug!(key = %key, title = %meta.title, "Redis müzik metaveri isabeti");
                    return Some(meta);
                }
                Err(e) => {
                    warn!(key = %key, error = %e, "Redis müzik metaveri JSON ayrıştırma hatası");
                }
            }
        }
    }

    None
}

/// Çözümlenen parça bilgisini Redis'e ilgili tüm anahtarlarla (`q`, `vid`, `url`) kaydeder.
pub async fn save_track_info(
    redis: &ConnectionManager,
    original_query: &str,
    info: &TrackInfo,
) {
    let Some(ref url) = info.url else { return };
    let video_id = extract_youtube_video_id(url);
    let canonical_url = video_id
        .as_deref()
        .map(canonical_youtube_url)
        .unwrap_or_else(|| url.clone());

    let cache_key = super::cache::track_cache_key(&canonical_url);
    let meta = CachedTrackMeta {
        title: info.title.clone(),
        author: None,
        webpage_url: canonical_url.clone(),
        video_id: video_id.clone(),
        duration_secs: info.duration.map(|d| d.as_secs()),
        thumbnail: info.thumbnail.clone(),
        cache_key,
        cached_at: chrono::Utc::now().timestamp(),
    };

    let Ok(json_str) = serde_json::to_string(&meta) else {
        return;
    };

    let mut keys = Vec::new();
    let norm_q = normalize_music_query(original_query);
    keys.push(format!("axbot:music:meta:q:{}", hash_key(&norm_q)));
    keys.push(format!("axbot:music:meta:url:{}", hash_key(&canonical_url)));
    if let Some(ref vid) = video_id {
        keys.push(format!("axbot:music:meta:vid:{vid}"));
    }

    let mut conn = redis.clone();
    let write_fut = async move {
        let mut pipe = redis::pipe();
        for key in keys {
            pipe.cmd("SET").arg(key).arg(&json_str).arg("EX").arg(META_CACHE_TTL_SECS).ignore();
        }
        let _: redis::RedisResult<()> = pipe.query_async(&mut conn).await;
    };

    let _ = tokio::time::timeout(REDIS_TIMEOUT, write_fut).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_music_query_turkish_and_spaces() {
        assert_eq!(normalize_music_query("  BLOK3   GİT  "), "blok3 git");
        assert_eq!(normalize_music_query("IĞDIR   İZMİR"), "ığdır izmir");
        assert_eq!(normalize_music_query("ŞARKI   ÇÖZÜMÜ   ÖZEL"), "şarkı çözümü özel");
        assert_eq!(normalize_music_query("hello\tworld\n\nagain"), "hello world again");
    }

    #[test]
    fn extract_youtube_video_ids_various_formats() {
        let v1 = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        let v2 = "https://m.youtube.com/watch?v=dQw4w9WgXcQ&feature=shared";
        let v3 = "https://music.youtube.com/watch?v=dQw4w9WgXcQ";
        let v4 = "https://youtu.be/dQw4w9WgXcQ?t=42";
        let v5 = "https://www.youtube.com/shorts/dQw4w9WgXcQ";
        let v6 = "https://www.youtube.com/embed/dQw4w9WgXcQ";

        assert_eq!(extract_youtube_video_id(v1), Some("dQw4w9WgXcQ".to_owned()));
        assert_eq!(extract_youtube_video_id(v2), Some("dQw4w9WgXcQ".to_owned()));
        assert_eq!(extract_youtube_video_id(v3), Some("dQw4w9WgXcQ".to_owned()));
        assert_eq!(extract_youtube_video_id(v4), Some("dQw4w9WgXcQ".to_owned()));
        assert_eq!(extract_youtube_video_id(v5), Some("dQw4w9WgXcQ".to_owned()));
        assert_eq!(extract_youtube_video_id(v6), Some("dQw4w9WgXcQ".to_owned()));
    }

    #[test]
    fn rejects_playlists_and_invalid_ids() {
        let p1 = "https://www.youtube.com/playlist?list=PL1234567890abcdef";
        let p2 = "https://m.youtube.com/playlist?list=PL1234567890abcdef";
        let invalid = "https://example.com/watch?v=dQw4w9WgXcQ";
        let too_short = "https://youtu.be/abc";

        assert_eq!(extract_youtube_video_id(p1), None);
        assert_eq!(extract_youtube_video_id(p2), None);
        assert_eq!(extract_youtube_video_id(invalid), None);
        assert_eq!(extract_youtube_video_id(too_short), None);
    }

    #[test]
    fn canonical_identifier_unifies_links() {
        let l1 = "https://youtu.be/dQw4w9WgXcQ";
        let l2 = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        assert_eq!(canonical_identifier(l1), canonical_identifier(l2));
        assert_eq!(canonical_identifier(l1), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    }

    #[test]
    fn cached_track_meta_serde_roundtrip() {
        let meta = CachedTrackMeta {
            title: "Test Track".to_owned(),
            author: Some("Artist".to_owned()),
            webpage_url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_owned(),
            video_id: Some("dQw4w9WgXcQ".to_owned()),
            duration_secs: Some(210),
            thumbnail: Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg".to_owned()),
            cache_key: "tr_test.webm".to_owned(),
            cached_at: 1700000000,
        };

        let json = serde_json::to_string(&meta).unwrap_or_default();
        let parsed: Result<CachedTrackMeta, _> = serde_json::from_str(&json);
        assert!(parsed.is_ok());
        let res = parsed.unwrap_or_default();
        assert_eq!(res.title, "Test Track");
        assert_eq!(res.video_id.as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(res.duration_secs, Some(210));
    }

    #[test]
    fn hash_key_is_deterministic_and_hex() {
        let h1 = hash_key("test query 123");
        let h2 = hash_key("test query 123");
        let h3 = hash_key("test query 124");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 16);
    }

    #[test]
    fn normalize_music_query_preserves_alphanumeric() {
        assert_eq!(normalize_music_query("Queen - Bohemian Rhapsody 1975"), "queen - bohemian rhapsody 1975");
        assert_eq!(normalize_music_query(""), "");
    }

    #[test]
    fn extract_youtube_video_id_with_complex_params() {
        let u1 = "https://www.youtube.com/watch?feature=shared&v=dQw4w9WgXcQ&t=42s";
        let u2 = "https://youtu.be/dQw4w9WgXcQ?si=abcdef123456";
        assert_eq!(extract_youtube_video_id(u1), Some("dQw4w9WgXcQ".to_owned()));
        assert_eq!(extract_youtube_video_id(u2), Some("dQw4w9WgXcQ".to_owned()));
    }

    #[test]
    fn canonical_identifier_leaves_plain_text_untouched() {
        let query = "Metallica Enter Sandman";
        assert_eq!(canonical_identifier(query), query);
    }
}
