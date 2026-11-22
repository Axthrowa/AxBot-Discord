//! RAM tabanlı ses önbelleği (sıfır disk yazmalı, seekable):
//! Takılma (stutter) ve ağ dalgalanmalarını önlemek için makul uzunluktaki (<= 30 dk, <= 40 MB)
//! parçaları RAM'de (`bytes::Bytes`) tutar ve doğrudan bellekten sıfır disk yazımıyla oynatır.

use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use dashmap::DashSet;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{debug, info};

/// Parçanın önbelleğe alınması için izin verilen en uzun süre (30 dakika).
pub const MAX_CACHEABLE_SECS: u64 = 30 * 60;

/// Tek bir parçanın RAM önbelleğinde tutulabileceği üst sınır (40 MB).
pub const MAX_TRACK_RAM_BYTES: usize = 40 * 1024 * 1024;

/// İndirme zaman aşımı süresi (60 saniye).
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// Bellek önbelleğinde saklanan parça kaydı.
#[derive(Clone)]
pub struct RamTrackEntry {
    pub bytes: Bytes,
    pub last_accessed: Instant,
}

/// Sıfır disk yazmalı LRU RAM Ses Önbelleği.
pub struct RamAudioCache {
    entries: HashMap<String, RamTrackEntry>,
    current_bytes: usize,
    max_bytes: usize,
}

#[allow(dead_code)]
impl RamAudioCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            current_bytes: 0,
            max_bytes,
        }
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn current_bytes(&self) -> usize {
        self.current_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&mut self, key: &str) -> Option<Bytes> {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_accessed = Instant::now();
            Some(entry.bytes.clone())
        } else {
            None
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn insert(&mut self, key: String, bytes: Bytes) -> bool {
        let len = bytes.len();
        // 40 MB üstü veya toplam kapasiteden büyük veya çok küçük (< 1 KB) ise önbelleğe alma
        if len > MAX_TRACK_RAM_BYTES || len > self.max_bytes || len < 1024 {
            return false;
        }

        // Eğer aynı anahtar zaten varsa önce boyutunu çıkar
        if let Some(old) = self.entries.remove(&key) {
            self.current_bytes = self.current_bytes.saturating_sub(old.bytes.len());
        }

        // LRU tahliyesi: Yeterli yer açılana kadar en eski erişilen parçaları düşür
        while self.current_bytes + len > self.max_bytes && !self.entries.is_empty() {
            let mut oldest_key = None;
            let mut oldest_time = Instant::now();
            for (k, v) in &self.entries {
                if v.last_accessed <= oldest_time {
                    oldest_time = v.last_accessed;
                    oldest_key = Some(k.clone());
                }
            }
            if let Some(k) = oldest_key {
                if let Some(removed) = self.entries.remove(&k) {
                    self.current_bytes = self.current_bytes.saturating_sub(removed.bytes.len());
                    debug!(key = %k, freed = removed.bytes.len(), "LRU tahliyesi yapıldı");
                }
            } else {
                break;
            }
        }

        if self.current_bytes + len <= self.max_bytes {
            self.current_bytes += len;
            self.entries.insert(
                key,
                RamTrackEntry {
                    bytes,
                    last_accessed: Instant::now(),
                },
            );
            true
        } else {
            false
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Bytes> {
        if let Some(removed) = self.entries.remove(key) {
            self.current_bytes = self.current_bytes.saturating_sub(removed.bytes.len());
            Some(removed.bytes)
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.current_bytes = 0;
    }
}

/// Yapılandırılabilir toplam RAM önbellek sınırı: varsayılan 256 MB (64–1024 MB kelepçeli).
pub fn configured_ram_cache_limit() -> usize {
    let mb = std::env::var("MUSIC_RAM_CACHE_MB")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(256);
    mb.clamp(64, 1024) * 1024 * 1024
}

static RAM_CACHE: LazyLock<Mutex<RamAudioCache>> = LazyLock::new(|| {
    Mutex::new(RamAudioCache::new(configured_ram_cache_limit()))
});

/// URL veya şarkı kimliğinden deterministik bir anahtar üretir.
pub fn track_cache_key(identifier: &str) -> String {
    let canonical = super::meta_cache::canonical_identifier(identifier);
    let trimmed = canonical.trim();
    let mut hasher = DefaultHasher::new();
    trimmed.hash(&mut hasher);
    let h1 = hasher.finish();

    let mut hasher2 = DefaultHasher::new();
    trimmed.len().hash(&mut hasher2);
    trimmed.chars().rev().take(16).collect::<String>().hash(&mut hasher2);
    let h2 = hasher2.finish();

    format!("tr_{h1:016x}_{h2:016x}.webm")
}

/// Global RAM önbelleğinden parça verisini döner; varsa son erişim zamanını günceller.
pub fn get_cached_audio(identifier: &str) -> Option<Bytes> {
    let key = track_cache_key(identifier);
    let mut cache = RAM_CACHE.lock().ok()?;
    cache.get(&key)
}

/// Global RAM önbelleğinde parçanın olup olmadığını kontrol eder.
pub fn has_cached_audio(identifier: &str) -> bool {
    let key = track_cache_key(identifier);
    let Ok(cache) = RAM_CACHE.lock() else { return false; };
    cache.contains(&key)
}

/// Parçayı RAM önbelleğine kaydeder (LRU ile yer açar).
pub fn put_cached_audio(identifier: &str, bytes: Bytes) -> bool {
    let key = track_cache_key(identifier);
    let Ok(mut cache) = RAM_CACHE.lock() else { return false; };
    cache.insert(key, bytes)
}

/// Parçanın son erişim zamanını günceller (LRU için).
pub fn touch_cached_audio(identifier: &str) {
    let key = track_cache_key(identifier);
    if let Ok(mut cache) = RAM_CACHE.lock() {
        let _ = cache.get(&key);
    }
}

/// Eski disk önbelleği kalıntılarını tek seferlik temizler (hata verirse yoksayılır).
pub fn cleanup_legacy_disk_cache() {
    let dir = std::env::temp_dir().join("axbot_music_cache");
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Geriye dönük uyumluluk: disk temizlik işlevini RAM temizliği ve kalıntı silmeyle bağlar.
pub fn clear_orphaned_cache() {
    cleanup_legacy_disk_cache();
}

/// Geriye dönük uyumluluk için prune_cache (RAM önbelleği otomatik tahliye yapar).
pub fn prune_cache() -> std::io::Result<usize> {
    Ok(0)
}

/// URL'nin HLS akışı (parçalı / manifest) olup olmadığını denetler.
pub fn is_hls_stream(url: &str) -> bool {
    url.contains(".m3u8") || url.contains("/manifest/") || url.starts_with("hls")
}

/// Doğrudan HTTP bağlantısından veriyi RAM'e (Vec<u8> -> Bytes) indirir. 40 MB sınırını her chunk'ta denetler.
async fn try_direct_http_ram_download(
    client: &reqwest::Client,
    direct_url: &str,
    headers: Option<&std::collections::HashMap<String, String>>,
) -> Result<Bytes, String> {
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

    let mut resp = tokio::time::timeout(Duration::from_secs(20), req.send())
        .await
        .map_err(|_| "Doğrudan HTTP istek zaman aşımı (20 sn)".to_owned())?
        .map_err(|e| format!("Doğrudan HTTP isteği gönderilemedi: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Doğrudan HTTP yanıtı başarısız (kod: {})", resp.status()));
    }

    if let Some(content_len) = resp.content_length() {
        if content_len > MAX_TRACK_RAM_BYTES as u64 {
            return Err(format!("İçerik boyutu ({content_len} bayt) 40 MB RAM sınırını aşıyor"));
        }
    }

    let mut buf = Vec::with_capacity(2 * 1024 * 1024);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);

    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(15), resp.chunk())
        .await
        .map_err(|_| "HTTP akış okuma zaman aşımı (15 sn)".to_owned())?
        .map_err(|e| format!("HTTP chunk okunamadı: {e}"))?
    {
        if tokio::time::Instant::now() > deadline {
            return Err("HTTP akış toplam indirme zaman aşımı (45 sn)".to_owned());
        }
        if buf.len() + chunk.len() > MAX_TRACK_RAM_BYTES {
            return Err("Parça 40 MB RAM sınırını aştı; indirme durduruldu".to_owned());
        }
        buf.extend_from_slice(&chunk);
    }

    if buf.len() < 1024 {
        return Err("Doğrudan indirilen veri çok küçük (< 1 KB)".to_owned());
    }

    Ok(Bytes::from(buf))
}

/// yt-dlp ile stdout (`-o -`) üzerinden ses akışını doğrudan RAM'e okur (sıfır disk).
async fn try_ytdlp_ram_download(
    ytdlp: &str,
    user_args: &[String],
    target: &str,
) -> Result<Bytes, String> {
    let mut cmd = Command::new(ytdlp);
    cmd.args([
        "-q",
        "--no-warnings",
        "--no-playlist",
        "--buffer-size",
        "1024k",
        "-f",
        "bestaudio[ext=webm]/bestaudio[ext=m4a]/ba/b",
        "-o",
        "-",
    ]);
    cmd.args(user_args);
    cmd.arg(target);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());

    let mut child = cmd.spawn().map_err(|e| format!("yt-dlp çalıştırılamadı: {e}"))?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill().await;
        return Err("yt-dlp stdout alınamadı".to_owned());
    };

    let mut buf = Vec::with_capacity(2 * 1024 * 1024);
    let mut temp = [0u8; 64 * 1024];
    let deadline = tokio::time::Instant::now() + DOWNLOAD_TIMEOUT;

    loop {
        if tokio::time::Instant::now() > deadline {
            let _ = child.kill().await;
            return Err("yt-dlp indirme zaman aşımına uğradı (60 sn)".to_owned());
        }
        let read_fut = stdout.read(&mut temp);
        let n = match tokio::time::timeout(Duration::from_secs(15), read_fut).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                let _ = child.kill().await;
                return Err(format!("yt-dlp çıktısı okunamadı: {e}"));
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err("yt-dlp veri okuma zaman aşımı (15 sn)".to_owned());
            }
        };

        if n == 0 {
            break;
        }

        if buf.len() + n > MAX_TRACK_RAM_BYTES {
            let _ = child.kill().await;
            return Err("yt-dlp akışı 40 MB RAM sınırını aştı; indirme durduruldu".to_owned());
        }
        buf.extend_from_slice(&temp[..n]);
    }

    let _ = child.wait().await;

    if buf.len() < 1024 {
        return Err("İndirilen ses akışı çok küçük veya boş (< 1 KB)".to_owned());
    }

    Ok(Bytes::from(buf))
}

/// Aynı anda aynı parçanın çift indirilmesini önleyen küme.
static DOWNLOADING: LazyLock<DashSet<String>> = LazyLock::new(DashSet::new);

/// Parçayı diske yazmadan doğrudan RAM önbelleğine (`bytes::Bytes`) indirir.
pub async fn download_to_ram_cache(
    http: Option<&reqwest::Client>,
    ytdlp: &str,
    user_args: &[String],
    target: &str,
    direct_url: Option<&str>,
    headers: Option<&std::collections::HashMap<String, String>>,
    duration: Option<Duration>,
) -> Result<Bytes, String> {
    if let Some(dur) = duration {
        if dur.as_secs() > MAX_CACHEABLE_SECS {
            return Err(format!(
                "Parça süresi ({} sn) 30 dakikalık RAM önbellek sınırının üzerinde.",
                dur.as_secs()
            ));
        }
    }

    let key = track_cache_key(target);
    if let Some(cached) = get_cached_audio(target) {
        return Ok(cached);
    }

    // Eşzamanlı aynı anahtar indirme kilidi (in-flight deduplication)
    if !DOWNLOADING.insert(key.clone()) {
        let deadline = tokio::time::Instant::now() + DOWNLOAD_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Some(cached) = get_cached_audio(target) {
                return Ok(cached);
            }
            if !DOWNLOADING.contains(&key) {
                break;
            }
        }
        if let Some(cached) = get_cached_audio(target) {
            return Ok(cached);
        }
        DOWNLOADING.insert(key.clone());
    }

    struct Guard(String);
    impl Drop for Guard {
        fn drop(&mut self) {
            DOWNLOADING.remove(&self.0);
        }
    }
    let _guard = Guard(key);

    // 1. Yol: Doğrudan HTTP Akış İndirmesi
    if let (Some(client), Some(url)) = (http, direct_url) {
        if !is_hls_stream(url) {
            debug!(target, url, "Doğrudan HTTP RAM akış indirmesi deneniyor");
            match try_direct_http_ram_download(client, url, headers).await {
                Ok(bytes) => {
                    info!(target, bytes = bytes.len(), "Şarkı doğrudan HTTP ile RAM önbelleğine alındı");
                    put_cached_audio(target, bytes.clone());
                    return Ok(bytes);
                }
                Err(e) => {
                    debug!(target, error = %e, "Doğrudan HTTP indirmesi başarısız; yt-dlp stdout fallback'ine geçiliyor");
                }
            }
        }
    }

    // 2. Yol: yt-dlp stdout indirmesi
    debug!(target, "yt-dlp stdout ile RAM önbelleğe indirme başlatılıyor");
    let bytes = try_ytdlp_ram_download(ytdlp, user_args, target).await?;
    info!(target, bytes = bytes.len(), "Şarkı yt-dlp stdout ile RAM önbelleğine alındı");
    put_cached_audio(target, bytes.clone());
    Ok(bytes)
}

/// `std::io::Cursor<bytes::Bytes>` tabanlı seekable MediaSource (sıfır kopyalama, tam seekable).
pub struct MemoryMediaSource {
    inner: std::io::Cursor<Bytes>,
}

#[allow(dead_code)]
impl MemoryMediaSource {
    pub fn new(bytes: Bytes) -> Self {
        Self {
            inner: std::io::Cursor::new(bytes),
        }
    }

    pub fn into_inner(self) -> Bytes {
        self.inner.into_inner()
    }
}

impl std::io::Read for MemoryMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::io::Read::read(&mut self.inner, buf)
    }
}

impl std::io::Seek for MemoryMediaSource {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl symphonia::core::io::MediaSource for MemoryMediaSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.inner.get_ref().len() as u64)
    }
}

/// RAM'deki ses verisini Songbird Input olarak temsil eden Lazy Compose yapısı.
#[derive(Clone)]
pub struct MemoryAudio(pub Bytes);

#[async_trait::async_trait]
impl songbird::input::Compose for MemoryAudio {
    fn create(&mut self) -> Result<songbird::input::AudioStream<Box<dyn symphonia::core::io::MediaSource>>, songbird::input::AudioStreamError> {
        let source = MemoryMediaSource::new(self.0.clone());
        Ok(songbird::input::AudioStream {
            input: Box::new(source),
        })
    }

    async fn create_async(&mut self) -> Result<songbird::input::AudioStream<Box<dyn symphonia::core::io::MediaSource>>, songbird::input::AudioStreamError> {
        self.create()
    }

    fn should_create_async(&self) -> bool {
        false
    }
}

/// Bytes verisinden seekable Songbird Input üretir.
pub fn memory_input(bytes: Bytes) -> songbird::input::Input {
    songbird::input::Input::Lazy(Box::new(MemoryAudio(bytes)))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom};
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
    fn cache_key_unifies_youtube_urls() {
        let k1 = track_cache_key("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        let k2 = track_cache_key("https://youtu.be/dQw4w9WgXcQ");
        let k3 = track_cache_key("https://music.youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(k1, k2);
        assert_eq!(k2, k3);
    }

    #[test]
    fn ram_cache_insert_and_get() {
        let mut cache = RamAudioCache::new(10 * 1024 * 1024);
        let data = Bytes::from(vec![42u8; 2048]);
        assert!(cache.insert("key1".to_owned(), data.clone()));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.current_bytes(), 2048);

        let retrieved = cache.get("key1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.as_ref().map(|b| b.len()), Some(2048));
    }

    #[test]
    fn ram_cache_eviction_lru() {
        let mut cache = RamAudioCache::new(5000);
        let b1 = Bytes::from(vec![1u8; 2000]);
        let b2 = Bytes::from(vec![2u8; 2000]);
        let b3 = Bytes::from(vec![3u8; 2000]);

        assert!(cache.insert("k1".to_owned(), b1));
        assert!(cache.insert("k2".to_owned(), b2));
        assert_eq!(cache.current_bytes(), 4000);

        // k1'e eriş: k2 en eski erişilen durumuna gelsin
        assert!(cache.get("k1").is_some());

        // k3 ekle: 4000 + 2000 > 5000 -> k2 evict edilmeli
        assert!(cache.insert("k3".to_owned(), b3));
        assert!(cache.current_bytes() <= 5000);
        assert!(cache.get("k2").is_none());
        assert!(cache.get("k1").is_some());
        assert!(cache.get("k3").is_some());
    }

    #[test]
    fn ram_cache_rejects_over_40mb() {
        let mut cache = RamAudioCache::new(100 * 1024 * 1024);
        // MAX_TRACK_RAM_BYTES = 40 MB
        let huge = Bytes::from(vec![0u8; MAX_TRACK_RAM_BYTES + 1]);
        assert!(!cache.insert("huge".to_owned(), huge));
        assert!(cache.is_empty());
    }

    #[test]
    fn memory_media_source_seekable() {
        use symphonia::core::io::MediaSource;
        let data = Bytes::from_static(b"0123456789abcdefghij");
        let mut source = MemoryMediaSource::new(data);

        assert!(source.is_seekable());
        assert_eq!(source.byte_len(), Some(20));

        let mut buf5 = [0u8; 5];
        let res_seek = source.seek(SeekFrom::Start(5));
        assert_eq!(res_seek.ok(), Some(5));
        let res_read = source.read(&mut buf5);
        assert_eq!(res_read.ok(), Some(5));
        assert_eq!(&buf5, b"56789");

        let res_seek2 = source.seek(SeekFrom::Current(2));
        assert_eq!(res_seek2.ok(), Some(12));
        let mut buf3 = [0u8; 3];
        let res_read2 = source.read(&mut buf3);
        assert_eq!(res_read2.ok(), Some(3));
        assert_eq!(&buf3, b"cde");

        let res_seek3 = source.seek(SeekFrom::End(-4));
        assert_eq!(res_seek3.ok(), Some(16));
        let mut buf4 = [0u8; 4];
        let res_read3 = source.read(&mut buf4);
        assert_eq!(res_read3.ok(), Some(4));
        assert_eq!(&buf4, b"ghij");
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

    #[tokio::test]
    async fn direct_download_fallback_on_invalid_url() {
        let client = reqwest::Client::new();
        let target = "test_nonexistent_track_for_fallback_test";
        let bad_url = "http://127.0.0.1:1/invalid_stream.opus";
        let res = download_to_ram_cache(
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
    fn ram_cache_methods_and_into_inner() {
        let mut cache = RamAudioCache::new(50_000);
        assert_eq!(cache.max_bytes(), 50_000);
        let b = Bytes::from(vec![9u8; 1500]);
        assert!(cache.insert("del".to_owned(), b.clone()));
        assert_eq!(cache.len(), 1);

        let removed = cache.remove("del");
        assert_eq!(removed, Some(b.clone()));
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.current_bytes(), 0);

        assert!(cache.insert("del2".to_owned(), b.clone()));
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.current_bytes(), 0);

        let source = MemoryMediaSource::new(b.clone());
        let inner_bytes = source.into_inner();
        assert_eq!(inner_bytes, b);
    }
}
