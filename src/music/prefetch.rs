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
    target: String,
    inner: Option<YoutubeDl<'static>>,
    args: Vec<String>,
    result: Option<(YoutubeDlOutput, Instant)>,
    seek_to: Option<Duration>,
    http: Option<reqwest::Client>,
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
    pub async fn resolve(
        mut inner: YoutubeDl<'static>,
        args: Vec<String>,
        http: Option<reqwest::Client>,
    ) -> Result<(Input, AuxMetadata), AudioStreamError> {
        let first = query(&mut inner).await?;
        let meta = first.as_aux_metadata();
        let target = first.webpage_url.clone().unwrap_or_else(|| first.url.clone());
        let source = Self {
            target,
            inner: Some(inner),
            args,
            result: Some((first, Instant::now())),
            seek_to: None,
            http,
        };
        Ok((Input::Lazy(Box::new(source)), meta))
    }

    /// Belirli bir saniyeden itibaren başlayan kaynak.
    #[allow(dead_code)]
    pub fn seek_from(
        target: String,
        inner: YoutubeDl<'static>,
        args: Vec<String>,
        seek: Duration,
        http: Option<reqwest::Client>,
    ) -> Input {
        Input::Lazy(Box::new(Self {
            target,
            inner: Some(inner),
            args,
            result: None,
            seek_to: Some(seek),
            http,
        }))
    }

    /// yt-dlp'yi çalmaya başlarken çalıştıran kaynak (çalma listesi parçaları gibi); HLS desteği için
    /// düz `YoutubeDl` yerine bu kullanılır.
    pub fn lazy(
        target: String,
        inner: YoutubeDl<'static>,
        args: Vec<String>,
        http: Option<reqwest::Client>,
    ) -> Input {
        Input::Lazy(Box::new(Self {
            target,
            inner: Some(inner),
            args,
            result: None,
            seek_to: None,
            http,
        }))
    }

    /// Bellek önbelleğindeki meta veri ile hazır kaynak oluşturur; yt-dlp araması yapmaz.
    pub fn from_cached(
        target: String,
        inner: YoutubeDl<'static>,
        args: Vec<String>,
        meta: AuxMetadata,
        http: Option<reqwest::Client>,
    ) -> (Input, AuxMetadata) {
        let source = Self {
            target,
            inner: Some(inner),
            args,
            result: None,
            seek_to: None,
            http,
        };
        (Input::Lazy(Box::new(source)), meta)
    }

    async fn open(&self, result: &YoutubeDlOutput) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        let target = result.webpage_url.as_deref().unwrap_or(&result.url).to_owned();
        let args = self.args.clone();
        let seek = self.seek_to;
        match tokio::task::spawn_blocking(move || super::filters::open_ytdlp_pipeline(&target, &args, seek))
            .await
            .map_err(|e| AudioStreamError::Fail(e.into()))?
        {
            Ok(stream) => Ok(stream),
            Err(e) => {
                tracing::debug!(error = %e, "yt-dlp pipeline açılamadı, doğrudan akış deneniyor");
                let headers = result.http_headers.clone().unwrap_or_default();
                let url = result.url.clone();
                if let Some(seek) = seek {
                    if seek.as_secs() > 0 {
                        return tokio::task::spawn_blocking(move || super::filters::open_url_with_seek(&url, &headers, seek))
                            .await
                            .map_err(|e| AudioStreamError::Fail(e.into()))?;
                    }
                }
                tokio::task::spawn_blocking(move || super::filters::open_url(&url, &headers))
                    .await
                    .map_err(|e| AudioStreamError::Fail(e.into()))?
            }
        }
    }
}

#[async_trait]
impl Compose for Prefetched {
    fn create(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        Err(AudioStreamError::Unsupported)
    }

    async fn create_async(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        // 1. RAM önbelleğinde hazır veri var mı kontrol et. Varsa sıfır disk ve sıfır gecikmeyle aç.
        if !self.target.is_empty() {
            if let Some(bytes) = super::cache::get_cached_audio(&self.target) {
                if let Some(seek) = self.seek_to {
                    if seek.as_secs() > 0 || seek.subsec_millis() > 0 {
                        return super::filters::open_ram_seek_stream(bytes, seek, super::filters::Filter::Off);
                    }
                }
                let source = super::cache::MemoryMediaSource::new(bytes);
                return Ok(AudioStream {
                    input: Box::new(source),
                });
            }
        }

        // 2. RAM önbelleğinde yoksa ve arama/sarma yoksa doğrudan RAM önbelleğine indir.
        if !self.target.is_empty() && self.seek_to.is_none() {
            let ytdlp_bin = super::filters::ytdlp();
            let (direct_url, headers, duration) = self
                .result
                .as_ref()
                .map(|(r, _)| {
                    (
                        Some(r.url.as_str()),
                        r.http_headers.as_ref(),
                        r.duration.map(Duration::from_secs_f64),
                    )
                })
                .unwrap_or((None, None, None));

            if let Ok(bytes) = super::cache::download_to_ram_cache(
                self.http.as_ref(),
                ytdlp_bin,
                &self.args,
                &self.target,
                direct_url,
                headers,
                duration,
            )
            .await
            {
                let source = super::cache::MemoryMediaSource::new(bytes);
                return Ok(AudioStream {
                    input: Box::new(source),
                });
            }
        }

        // 3. İndirme yapılamadıysa veya canlı akış ise pipeline/doğrudan akış ile devam et.
        let mut had_cached_result = false;
        if let Some((result, fetched_at)) = self.result.take() {
            had_cached_result = true;
            if fetched_at.elapsed() < FRESH_FOR {
                match self.open(&result).await {
                    Ok(stream) => return Ok(stream),
                    Err(e) => tracing::debug!(error = %e, "Saklanan akış adresi açılamadı"),
                }
            }
        }
        // Eğer önceden saklanan sonuç yoksa doğrudan hedef üzerinden tek sefer açmayı dene.
        // Önceden saklanan sonuç open() tarafından denenmiş ve başarısız olmuşsa aynı hedefi çift açma.
        if !had_cached_result && !self.target.is_empty() {
            let target = self.target.clone();
            let args = self.args.clone();
            let seek = self.seek_to;
            match tokio::task::spawn_blocking(move || super::filters::open_ytdlp_pipeline(&target, &args, seek))
                .await
                .map_err(|e| AudioStreamError::Fail(e.into()))?
            {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    tracing::debug!(error = %e, "Doğrudan işlem hattı açılamadı, yedek sorgu deneniyor");
                }
            }
        }
        if let Some(ref mut inner) = self.inner {
            let result = query(inner).await?;
            self.open(&result).await
        } else {
            Err(AudioStreamError::Fail("Şarkı akışı başlatılamadı".into()))
        }
    }

    fn should_create_async(&self) -> bool {
        true
    }

    async fn aux_metadata(&mut self) -> Result<AuxMetadata, AudioStreamError> {
        if let Some((result, _)) = &self.result {
            return Ok(result.as_aux_metadata());
        }
        if let Some(ref mut inner) = self.inner {
            inner.aux_metadata().await
        } else {
            Ok(AuxMetadata::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefetch_timeouts_and_constants() {
        assert_eq!(FRESH_FOR, Duration::from_secs(2 * 60 * 60));
        assert_eq!(QUERY_TIMEOUT, Duration::from_secs(45));
    }
}
