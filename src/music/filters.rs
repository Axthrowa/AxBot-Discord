//! Ses filtreleri (bass boost, nightcore, 8D…): şarkı çalmaya başlarken sunucuda bir filtre seçiliyse
//! kaynağın ham baytları ffmpeg'e verilir, ffmpeg'in çıkardığı 48 kHz stereo f32 PCM çalınır.
//!
//! Filtre şarkı BAŞLARKEN okunur: seçim çalan şarkıyı değiştirmez, sıradaki şarkıdan itibaren geçerlidir.
//! Filtreli şarkıda ileri/geri sarma çalışmaz (ffmpeg çıkışı geri sarılamaz).

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use poise::serenity_prelude::GuildId;
use songbird::input::{AudioStream, AudioStreamError, AuxMetadata, ChildContainer, Compose, Input, RawAdapter};
use symphonia::core::io::{MediaSource, ReadOnlySource};

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum Filter {
    #[name = "Kapalı"]
    Off,
    #[name = "Bass boost"]
    Bass,
    #[name = "Nightcore"]
    Nightcore,
    #[name = "Vaporwave"]
    Vaporwave,
    #[name = "8D"]
    EightD,
}

impl Filter {
    /// ffmpeg `-af` zinciri. Hız değiştirenler önce 48 kHz'e çevirir, sonra örnek hızını oynatır.
    pub fn chain(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Bass => Some("bass=g=10:f=110:w=0.6"),
            Self::Nightcore => Some("aresample=48000,asetrate=60000,aresample=48000"),
            Self::Vaporwave => Some("aresample=48000,asetrate=38400,aresample=48000"),
            Self::EightD => Some("apulsator=hz=0.125"),
        }
    }
}

/// Sunucu başına seçili filtre (bellekte; bot yeniden başlayınca kapanır).
pub type Filters = Arc<DashMap<GuildId, Filter>>;

/// Bir kaynağı sarar; çalmaya başlarken sunucunun filtresine bakar.
pub struct Filtered {
    inner: Box<dyn Compose>,
    guild: GuildId,
    filters: Filters,
    ffmpeg: &'static str,
}

/// Tembel (henüz açılmamış) kaynakları filtre katmanıyla sarar; diğerlerine dokunmaz.
pub fn wrap(input: Input, guild: GuildId, filters: &Filters, ffmpeg: &'static str) -> Input {
    match input {
        Input::Lazy(inner) => Input::Lazy(Box::new(Filtered { inner, guild, filters: Arc::clone(filters), ffmpeg })),
        other => other,
    }
}

fn through_ffmpeg(ffmpeg: &str, chain: &str, source: Box<dyn MediaSource>) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
    let mut child = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i", "pipe:0", "-af", chain, "-f", "f32le", "-ac", "2", "-ar", "48000", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AudioStreamError::Fail(format!("ffmpeg çalıştırılamadı: {e}").into()))?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err(AudioStreamError::Fail("ffmpeg girişi açılamadı".into()));
    };
    // Kaynak eşzamanlı okunur; ffmpeg kapanınca (şarkı bitti/atlandı) yazma hata verir ve iş parçacığı biter.
    std::thread::spawn(move || {
        let mut source = source;
        let _ = std::io::copy(&mut source, &mut stdin);
        let _ = stdin.flush();
    });
    let pcm = ReadOnlySource::new(ChildContainer::from(child));
    Ok(AudioStream { input: Box::new(RawAdapter::new(pcm, 48_000, 2)) as Box<dyn MediaSource> })
}

#[async_trait]
impl Compose for Filtered {
    fn create(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        let stream = self.inner.create()?;
        self.apply(stream)
    }

    async fn create_async(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        let stream = self.inner.create_async().await?;
        self.apply(stream)
    }

    fn should_create_async(&self) -> bool {
        self.inner.should_create_async()
    }

    async fn aux_metadata(&mut self) -> Result<AuxMetadata, AudioStreamError> {
        self.inner.aux_metadata().await
    }
}

impl Filtered {
    fn apply(&self, stream: AudioStream<Box<dyn MediaSource>>) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        let filter = self.filters.get(&self.guild).map_or(Filter::Off, |f| *f);
        match filter.chain() {
            Some(chain) => through_ffmpeg(self.ffmpeg, chain, stream.input),
            None => Ok(stream),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gerçek ffmpeg ile: 1 sn'lik WAV sesi her filtreden geçer ve PCM çıkar.
    #[tokio::test]
    #[ignore = "ffmpeg gerektirir"]
    async fn live_ffmpeg_filters_produce_pcm() -> Result<(), String> {
        use std::io::Read;
        // 48 kHz mono 16 bit 1 sn sessiz WAV.
        let samples = 48_000u32;
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + samples * 2).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(48_000u32.to_le_bytes());
        wav.extend(96_000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((samples * 2).to_le_bytes());
        wav.resize(wav.len() + (samples * 2) as usize, 0);
        for filter in [Filter::Bass, Filter::Nightcore, Filter::Vaporwave, Filter::EightD] {
            let chain = filter.chain().ok_or("zincir yok")?;
            let source: Box<dyn MediaSource> = Box::new(ReadOnlySource::new(std::io::Cursor::new(wav.clone())));
            let mut stream = through_ffmpeg("ffmpeg", chain, source).map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            stream.input.read_to_end(&mut out).map_err(|e| e.to_string())?;
            assert!(out.len() > 100_000, "{filter:?}: {} bayt", out.len());
        }

        // Asıl yol: sarılmış kaynak songbird'ün çözücüsünden geçebiliyor mu?
        struct Wav(Vec<u8>);
        #[async_trait]
        impl Compose for Wav {
            fn create(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
                Ok(AudioStream { input: Box::new(std::io::Cursor::new(self.0.clone())) as Box<dyn MediaSource> })
            }
            async fn create_async(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
                self.create()
            }
            fn should_create_async(&self) -> bool {
                false
            }
        }
        let filters: Filters = Arc::new(DashMap::new());
        filters.insert(GuildId::new(1), Filter::Nightcore);
        let input = wrap(Input::Lazy(Box::new(Wav(wav))), GuildId::new(1), &filters, "ffmpeg");
        let playable = input
            .make_playable_async(songbird::input::codecs::get_codec_registry(), songbird::input::codecs::get_probe())
            .await
            .map_err(|e| e.to_string())?;
        assert!(playable.is_playable());
        Ok(())
    }
}
