//! yt-dlp sonucunu saklayan kaynak.
//!
//! songbird'ün `YoutubeDl` kaynağı şarkı bilgisini öğrenmek için (`aux_metadata`) yt-dlp'yi bir kez,
//! çalmaya başlarken (`create_async`) AYNI aramayı bir kez daha çalıştırır: `/çal` her şarkıda ~3 sn
//! fazladan beklerdi. Burada ilk çalıştırmanın sonucu (akış adresi dahil) saklanır ve çalarken
//! doğrudan kullanılır. Akış adresleri birkaç saat geçerli olduğundan, sırada uzun bekleyen
//! şarkılarda (veya adres artık açılmıyorsa) yt-dlp yeniden çalıştırılır.
//!
//! HLS akışları (SoundCloud) ffmpeg'e adresiyle verilir: songbird'ün HLS okuyucusu parçalı MP4'ün
//! başlangıç parçasını almadığı için symphonia bunları çözemez.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use songbird::input::metadata::YoutubeDlOutput;
use songbird::input::{AudioStream, AudioStreamError, AuxMetadata, Compose, Input, YoutubeDl};
use symphonia::core::io::MediaSource;

/// Saklanan akış adresinin güvenle kullanılabileceği süre (YouTube adresleri ~6 saat geçerli).
const FRESH_FOR: Duration = Duration::from_secs(2 * 60 * 60);
/// yt-dlp takılırsa komut sonsuza kadar beklemesin.
const QUERY_TIMEOUT: Duration = Duration::from_secs(45);

pub struct Prefetched {
    inner: YoutubeDl<'static>,
    result: Option<(YoutubeDlOutput, Instant)>,
}

async fn query(inner: &mut YoutubeDl<'static>) -> Result<YoutubeDlOutput, AudioStreamError> {
    let mut results = tokio::time::timeout(QUERY_TIMEOUT, inner.query(1))
        .await
        .map_err(|_| AudioStreamError::Fail("yt-dlp zaman aşımı (45 sn)".into()))??;
    if results.is_empty() {
        return Err(AudioStreamError::Fail("sonuç yok".into()));
    }
    Ok(results.swap_remove(0))
}

impl Prefetched {
    /// yt-dlp'yi bir kez çalıştırır; şarkı bilgisini ve çalmaya hazır kaynağı döner.
    pub async fn resolve(mut inner: YoutubeDl<'static>) -> Result<(Input, AuxMetadata), AudioStreamError> {
        let first = query(&mut inner).await?;
        let meta = first.as_aux_metadata();
        let source = Self { inner, result: Some((first, Instant::now())) };
        Ok((Input::Lazy(Box::new(source)), meta))
    }

    /// yt-dlp'yi çalmaya başlarken çalıştıran kaynak (çalma listesi parçaları gibi); HLS desteği için
    /// düz `YoutubeDl` yerine bu kullanılır.
    pub fn lazy(inner: YoutubeDl<'static>) -> Input {
        Input::Lazy(Box::new(Self { inner, result: None }))
    }

    async fn open(&self, result: &YoutubeDlOutput) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        if result.protocol.as_deref().is_some_and(|p| p.starts_with("m3u8")) {
            let headers = result.http_headers.clone().unwrap_or_default();
            let url = result.url.clone();
            return tokio::task::spawn_blocking(move || super::filters::open_url(&url, &headers))
                .await
                .map_err(|e| AudioStreamError::Fail(e.into()))?;
        }
        self.inner.get_stream(result).await
    }
}

#[async_trait]
impl Compose for Prefetched {
    fn create(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        Err(AudioStreamError::Unsupported)
    }

    async fn create_async(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        if let Some((result, fetched_at)) = self.result.take() {
            if fetched_at.elapsed() < FRESH_FOR {
                match self.open(&result).await {
                    Ok(stream) => return Ok(stream),
                    Err(e) => tracing::debug!(error = %e, "Saklanan akış adresi açılmadı, yt-dlp yeniden çalıştırılıyor"),
                }
            }
        }
        let result = query(&mut self.inner).await?;
        self.open(&result).await
    }

    fn should_create_async(&self) -> bool {
        true
    }

    async fn aux_metadata(&mut self) -> Result<AuxMetadata, AudioStreamError> {
        // `query` bilgiyi kaynağın içinde de saklar; yt-dlp tekrar çalışmaz.
        self.inner.aux_metadata().await
    }
}
