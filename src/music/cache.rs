//! Yerel müzik önbelleği: takılma (stutter) ve ağ dalgalanmalarını önlemek için
//! makul uzunluktaki (<= 30 dk) parçaları geçici dizine indirir ve doğrudan yerel dosyadan oynatır.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use dashmap::DashSet;
use tokio::process::Command;
use tracing::{debug, info};

/// Parçanın önbelleğe alınması için izin verilen en uzun süre (30 dakika).
pub const MAX_CACHEABLE_SECS: u64 = 30 * 60;
/// Önbellek dizininin üst boyutu (512 MB).
pub const MAX_CACHE_BYTES: u64 = 512 * 1024 * 1024;
/// Temizlik sonrasında hedeflenen üst boyut (384 MB = %75).
pub const TARGET_CACHE_BYTES: u64 = 384 * 1024 * 1024;
/// İndirme zaman aşımı süresi (90 saniye).
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(90);

/// Aynı anda aynı parçanın çift indirilmesini önleyen küme.
static DOWNLOADING: LazyLock<DashSet<String>> = LazyLock::new(DashSet::new);

/// Yerel önbellek dizini: `%TEMP%/axbot_music_cache`.
pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("axbot_music_cache")
}

/// Önbellek dizinini oluşturur.
pub fn ensure_cache_dir() -> std::io::Result<PathBuf> {
    let dir = cache_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    }
    Ok(dir)
}

/// URL veya şarkı kimliğinden güvenli, deterministik bir önbellek dosya adı üretir.
pub fn track_cache_key(identifier: &str) -> String {
    let trimmed = identifier.trim();
    let mut hasher = DefaultHasher::new();
    trimmed.hash(&mut hasher);
    let h1 = hasher.finish();

    let mut hasher2 = DefaultHasher::new();
    trimmed.len().hash(&mut hasher2);
    trimmed.chars().rev().take(16).collect::<String>().hash(&mut hasher2);
    let h2 = hasher2.finish();

    format!("tr_{h1:016x}_{h2:016x}.webm")
}

/// Parça yerel diskte hazır ve geçerli boyutta (>= 1 KB) ise yolunu döner.
pub fn get_cached_track(identifier: &str) -> Option<PathBuf> {
    let key = track_cache_key(identifier);
    let path = cache_dir().join(key);
    if path.is_file() {
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() >= 1024 {
                return Some(path);
            }
        }
    }
    None
}

/// URL'nin HLS akışı (parçalı / manifest) olup olmadığını denetler.
pub fn is_hls_stream(url: &str) -> bool {
    url.contains(".m3u8") || url.contains("/manifest/") || url.starts_with("hls")
}

async fn try_direct_http_download(
    client: &reqwest::Client,
    direct_url: &str,
    headers: Option<&std::collections::HashMap<String, String>>,
    part_path: &std::path::Path,
) -> Result<u64, String> {
    let mut req = client.get(direct_url);
    if let Some(hdrs) = headers {
        for (k, v) in hdrs {
            if let (Ok(hname), Ok(hval)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                req = req.header(hname, hval);
            }
        }
    }

    let resp = tokio::time::timeout(Duration::from_secs(30), req.send())
        .await
        .map_err(|_| "Doğrudan HTTP istek zaman aşımı (30 sn)".to_owned())?
        .map_err(|e| format!("Doğrudan HTTP isteği gönderilemedi: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Doğrudan HTTP yanıtı başarısız (kod: {})", resp.status()));
    }

    let bytes = tokio::time::timeout(Duration::from_secs(45), resp.bytes())
        .await
        .map_err(|_| "Doğrudan HTTP veri alma zaman aşımı (45 sn)".to_owned())?
        .map_err(|e| format!("Doğrudan HTTP verisi okunamadı: {e}"))?;

    let len = bytes.len() as u64;
    if len < 1024 {
        return Err(format!("Doğrudan indirilen dosya çok küçük ({len} bayt)"));
    }

    std::fs::write(part_path, &bytes).map_err(|e| format!("Diske yazılamadı: {e}"))?;
    Ok(len)
}

/// Parçayı önce doğrudan ses akış URL'si (direct_url) üzerinden hızlı HTTP akışıyla dener;
/// başarısız olursa yt-dlp ile geçici `.part` dosyasına indirip atomik olarak son dosyaya taşır.
pub async fn download_to_cache(
    http: Option<&reqwest::Client>,
    ytdlp: &str,
    user_args: &[String],
    target: &str,
    direct_url: Option<&str>,
    headers: Option<&std::collections::HashMap<String, String>>,
    duration: Option<Duration>,
) -> Result<PathBuf, String> {
    if let Some(dur) = duration {
        if dur.as_secs() > MAX_CACHEABLE_SECS {
            return Err(format!(
                "Parça süresi ({} sn) 30 dakikalık önbellek sınırının üzerinde.",
                dur.as_secs()
            ));
        }
    }

    let _ = ensure_cache_dir().map_err(|e| format!("Önbellek dizini açılamadı: {e}"))?;
    let key = track_cache_key(target);
    let final_path = cache_dir().join(&key);

    if let Some(cached) = get_cached_track(target) {
        return Ok(cached);
    }

    // Başka bir görev şu an bu parçayı indiriyorsa tamamlanmasını bekle.
    if !DOWNLOADING.insert(key.clone()) {
        let deadline = tokio::time::Instant::now() + DOWNLOAD_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Some(cached) = get_cached_track(target) {
                return Ok(cached);
            }
            if !DOWNLOADING.contains(&key) {
                break;
            }
        }
        if let Some(cached) = get_cached_track(target) {
            return Ok(cached);
        }
        // İndirme başarısız olmuş olabilir, tek sefer kendimiz deneyelim
        DOWNLOADING.insert(key.clone());
    }

    struct Guard(String);
    impl Drop for Guard {
        fn drop(&mut self) {
            DOWNLOADING.remove(&self.0);
        }
    }
    let _guard = Guard(key);

    let part_name = format!("dl_{}_{}.part", uuid::Uuid::new_v4(), std::process::id());
    let part_path = cache_dir().join(&part_name);

    // 1. Yol: Doğrudan HTTP Akış İndirmesi (çift yt-dlp başlatmadan hızlıca indirme)
    if let (Some(client), Some(url)) = (http, direct_url) {
        if !is_hls_stream(url) {
            debug!(target, url, "Doğrudan HTTP akış indirmesi deneniyor");
            match try_direct_http_download(client, url, headers, &part_path).await {
                Ok(bytes) => {
                    if final_path.exists() {
                        let _ = std::fs::remove_file(&final_path);
                    }
                    if std::fs::rename(&part_path, &final_path).is_ok() {
                        info!(path = ?final_path, bytes, "Şarkı doğrudan HTTP akışıyla yerel önbelleğe alındı");
                        let _ = prune_cache();
                        return Ok(final_path);
                    }
                }
                Err(e) => {
                    debug!(target, error = %e, "Doğrudan HTTP indirmesi başarısız; yt-dlp fallback'ine geçiliyor");
                    let _ = std::fs::remove_file(&part_path);
                }
            }
        }
    }

    // 2. Yol (Fallback): yt-dlp ile indirme
    let mut cmd = Command::new(ytdlp);
    cmd.args([
        "-q",
        "--no-warnings",
        "--no-playlist",
        "--concurrent-fragments",
        "4",
    ]);
    cmd.args(user_args);
    cmd.args([
        "-f",
        "bestaudio[ext=webm]/bestaudio[ext=m4a]/ba/b",
        "-o",
    ]);
    cmd.arg(&part_path);
    cmd.arg(target);

    debug!(target, part = ?part_path, "yt-dlp ile yerel önbelleğe indirme başlatılıyor");

    let status = match tokio::time::timeout(DOWNLOAD_TIMEOUT, cmd.status()).await {
        Ok(Ok(st)) => st,
        Ok(Err(e)) => {
            let _ = std::fs::remove_file(&part_path);
            return Err(format!("yt-dlp çalıştırılamadı: {e}"));
        }
        Err(_) => {
            let _ = std::fs::remove_file(&part_path);
            return Err("yt-dlp indirme zaman aşımına uğradı (90 sn)".to_owned());
        }
    };

    if !status.success() {
        let _ = std::fs::remove_file(&part_path);
        return Err(format!("yt-dlp indirmesi başarısız oldu (çıkış kodu: {status})"));
    }

    let meta = std::fs::metadata(&part_path).map_err(|e| {
        let _ = std::fs::remove_file(&part_path);
        format!("İndirilen dosya okunamadı: {e}")
    })?;

    if meta.len() < 1024 {
        let _ = std::fs::remove_file(&part_path);
        return Err("İndirilen dosya çok küçük veya boş.".to_owned());
    }

    // Atomik yeniden adlandırma (varsa eskisinin üzerine yazmak için önce kaldır)
    if final_path.exists() {
        let _ = std::fs::remove_file(&final_path);
    }
    std::fs::rename(&part_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&part_path);
        format!("Önbellek dosyası taşınamadı: {e}")
    })?;

    info!(path = ?final_path, bytes = meta.len(), "Şarkı yerel önbelleğe alındı");
    let _ = prune_cache();
    Ok(final_path)
}

/// Önbellek boyutunu denetler; 512 MB'ı aşmışsa en eski dosyalardan başlayarak temizler.
pub fn prune_cache() -> std::io::Result<usize> {
    let dir = cache_dir();
    if !dir.is_dir() {
        return Ok(0);
    }

    let mut entries = Vec::new();
    let mut total_bytes = 0u64;
    let now = SystemTime::now();

    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };

        if !meta.is_file() {
            continue;
        }

        // Yarım kalmış .part dosyaları 10 dakikadan eskiyse sil
        if path.extension().is_some_and(|ext| ext == "part") {
            let modified = meta.modified().unwrap_or(now);
            if now.duration_since(modified).unwrap_or(Duration::ZERO) > Duration::from_secs(600) {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }

        let len = meta.len();
        total_bytes = total_bytes.saturating_add(len);
        let modified = meta.modified().unwrap_or(now);
        entries.push((path, len, modified));
    }

    if total_bytes <= MAX_CACHE_BYTES {
        return Ok(0);
    }

    // En eski erişilen/değiştirilen önce
    entries.sort_by_key(|(_, _, modified)| *modified);

    let mut removed_count = 0;
    for (path, len, _) in entries {
        if total_bytes <= TARGET_CACHE_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total_bytes = total_bytes.saturating_sub(len);
            removed_count += 1;
        }
    }

    if removed_count > 0 {
        info!(removed_count, remaining_bytes = total_bytes, "Önbellek LRU temizliği tamamlandı");
    }

    Ok(removed_count)
}

/// Bot açılışında kalan yarım indirmeleri (.part) ve çok eski dosyaları temizler.
pub fn clear_orphaned_cache() {
    let dir = cache_dir();
    if !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "part") {
            let _ = std::fs::remove_file(path);
        }
    }
    let _ = prune_cache();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_deterministic_and_safe() {
        let k1 = track_cache_key("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        let k2 = track_cache_key("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(k1, k2);
        assert!(k1.ends_with(".webm"));
        assert!(!k1.contains('/'));
        assert!(!k1.contains('\\'));
        assert!(!k1.contains(':'));
    }

    #[test]
    fn limits_are_consistent() {
        const { assert!(TARGET_CACHE_BYTES < MAX_CACHE_BYTES) };
        assert_eq!(MAX_CACHEABLE_SECS, 1800);
    }

    #[tokio::test]
    async fn direct_download_fallback_on_invalid_url() {
        let client = reqwest::Client::new();
        let target = "test_nonexistent_track_for_fallback_test";
        let bad_url = "http://127.0.0.1:1/invalid_stream.opus";
        let res = download_to_cache(
            Some(&client),
            "invalid_ytdlp_binary_nonexistent",
            &[],
            target,
            Some(bad_url),
            None,
            Some(Duration::from_secs(120)),
        )
        .await;
        // Doğrudan HTTP başarısız olmalı ve yt-dlp fallback'ine geçip paniklemeden hata dönmeli
        assert!(res.is_err());
        assert!(!res.as_ref().err().map_or("", |e| e.as_str()).is_empty());
    }

    #[test]
    fn hls_streams_skip_direct_download() {
        let hls1 = "https://example.com/stream/playlist.m3u8";
        let hls2 = "https://example.com/manifest/hls/audio";
        let regular = "https://rr1---sn-abc.googlevideo.com/videoplayback?expire=123";

        assert!(is_hls_stream(hls1));
        assert!(is_hls_stream(hls2));
        assert!(!is_hls_stream(regular));
    }
}
