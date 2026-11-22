//! ffmpeg aşaması: ses filtreleri (bass boost, nightcore, 8D…) ve symphonia'nın çözemediği HLS akışları (SoundCloud).
//!
//! - Filtre şarkı BAŞLARKEN okunur: seçim çalan şarkıyı değiştirmez, sıradaki şarkıdan itibaren geçerlidir.
//!   Seçiliyse kaynağın baytları ffmpeg'e verilir, ffmpeg'in çıkardığı 48 kHz stereo f32 PCM çalınır.
//!   Filtreli şarkıda ileri/geri sarma çalışmaz (ffmpeg çıkışı geri sarılamaz).
//! - HLS akışı (`open_url`): ffmpeg adresi kendisi okur (parçalı MP4'ün başlangıç parçası dahil) ve PCM verir.
//!   Bu PCM'e filtre gerekirse songbird başlığı atılıp ham PCM olarak ikinci ffmpeg'e verilir.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

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
    /// ffmpeg `-af` zinciri. Hız/tempo değiştirenler önce zaman damgasını sabitler, sonra hızı uygular ve 48 kHz'e kilitler.
    pub fn chain(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Bass => Some("asetpts=PTS-STARTPTS,bass=g=10:f=110:w=0.6,aresample=48000:async=1:first_pts=0"),
            Self::Nightcore => Some("asetpts=PTS-STARTPTS,asetrate=60000,aresample=48000:async=1:first_pts=0"),
            Self::Vaporwave => Some("asetpts=PTS-STARTPTS,asetrate=38400,aresample=48000:async=1:first_pts=0"),
            Self::EightD => Some("asetpts=PTS-STARTPTS,apulsator=hz=0.125,aresample=48000:async=1:first_pts=0"),
        }
    }
}

/// Sunucu başına seçili filtre (bellekte; bot yeniden başlayınca kapanır).
pub type Filters = Arc<DashMap<GuildId, Filter>>;

static FFMPEG: OnceLock<String> = OnceLock::new();
static YTDLP: OnceLock<String> = OnceLock::new();

/// ffmpeg programının yolu (açılışta `Music::new` belirler; belirlenmediyse PATH'teki `ffmpeg`).
pub fn set_ffmpeg(path: String) {
    let _ = FFMPEG.set(path);
}

fn ffmpeg() -> &'static str {
    FFMPEG.get().map_or("ffmpeg", String::as_str)
}

/// yt-dlp programının yolu (açılışta `Music::new` belirler; belirlenmediyse PATH'teki `yt-dlp`).
pub fn set_ytdlp(path: String) {
    let _ = YTDLP.set(path);
}

fn ytdlp() -> &'static str {
    YTDLP.get().map_or("yt-dlp", String::as_str)
}

/// Sabit örnekleme hızı ve kanal sayısı (Discord ses standardı: 48 kHz stereo)
pub const TARGET_SAMPLE_RATE: u32 = 48_000;
pub const TARGET_CHANNELS: u32 = 2;

/// Kararlı zaman damgası normalizasyonu ve 48 kHz kilidi sağlayan temel resample filtresi.
pub const BASE_RESAMPLE_AF: &str = "asetpts=PTS-STARTPTS,aresample=48000:async=1:first_pts=0";

/// songbird'ün ham PCM başlığı (`RawAdapter`): imza + örnek hızı + kanal sayısı.
const RAW_MAGIC: &[u8; 8] = b"SbirdRaw";
const RAW_HEADER_LEN: usize = 16;
pub const PCM_OUT: [&str; 7] = ["-f", "f32le", "-ac", "2", "-ar", "48000", "pipe:1"];

/// yt-dlp ile ses akışını stdout'a indirip ffmpeg'e pipe eder; 48 kHz stereo f32le PCM verir.
pub fn open_ytdlp_pipeline(
    target: &str,
    user_args: &[String],
    seek: Option<Duration>,
) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
    let mut ytdl_cmd = Command::new(ytdlp());
    ytdl_cmd
        .args(["-q", "--no-warnings", "--no-playlist", "--buffer-size", "1024k"])
        .args(user_args)
        .args(["-f", "ba/b[acodec!=none]/ba/b", "-o", "-", target])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut ytdl_child = ytdl_cmd
        .spawn()
        .map_err(|e| AudioStreamError::Fail(format!("yt-dlp çalıştırılamadı: {e}").into()))?;

    let Some(ytdl_stdout) = ytdl_child.stdout.take() else {
        let _ = ytdl_child.kill();
        return Err(AudioStreamError::Fail("yt-dlp çıktısı alınamadı".into()));
    };

    let mut ff_cmd = Command::new(ffmpeg());
    ff_cmd.args(["-hide_banner", "-loglevel", "error"]);
    // Kararlı akış analizi için güvenli probe/analiz süreleri (1MB / 2 sn)
    ff_cmd.args(["-probesize", "1048576", "-analyzeduration", "2000000"]);
    let seek_str;
    if let Some(seek) = seek {
        if seek.as_secs() > 0 {
            seek_str = seek.as_secs_f64().to_string();
            ff_cmd.args(["-ss", &seek_str]);
        }
    }
    ff_cmd.args(["-i", "pipe:0", "-vn"]);
    ff_cmd.args(["-af", BASE_RESAMPLE_AF]);
    ff_cmd.args(PCM_OUT);
    ff_cmd.stdin(Stdio::from(ytdl_stdout));
    ff_cmd.stdout(Stdio::piped());
    ff_cmd.stderr(Stdio::null());

    let ff_child = match ff_cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = ytdl_child.kill();
            return Err(AudioStreamError::Fail(format!("ffmpeg çalıştırılamadı: {e}").into()));
        }
    };

    let container = ChildContainer::from(vec![ytdl_child, ff_child]);
    let pcm = ReadOnlySource::new(container);
    Ok(AudioStream {
        input: Box::new(RawAdapter::new(pcm, TARGET_SAMPLE_RATE, TARGET_CHANNELS)) as Box<dyn MediaSource>,
    })
}

fn spawn(args: &[&str], stdin: Stdio) -> Result<std::process::Child, AudioStreamError> {
    Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error"])
        .args(args)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AudioStreamError::Fail(format!("ffmpeg çalıştırılamadı: {e}").into()))
}

fn pcm_stream(child: std::process::Child) -> AudioStream<Box<dyn MediaSource>> {
    let pcm = ReadOnlySource::new(ChildContainer::from(child));
    AudioStream { input: Box::new(RawAdapter::new(pcm, 48_000, 2)) as Box<dyn MediaSource> }
}

/// HLS (m3u8) adresini ffmpeg ile açar. Engelleyici; `spawn_blocking` içinde çağrılmalı.
pub fn open_url(url: &str, headers: &HashMap<String, String>) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
    open_url_with_seek(url, headers, Duration::ZERO)
}

/// URL'yi belirli bir saniyeden itibaren ffmpeg ile açar. Engelleyici; `spawn_blocking` içinde çağrılmalı.
pub fn open_url_with_seek(url: &str, headers: &HashMap<String, String>, seek: Duration) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
    let headers: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
    let mut args = Vec::new();
    // Kararlı akış analizi ve HLS/HTTP başlıklarının doğru zaman tabanıyla okunması için güvenli probe
    args.extend(["-probesize", "1048576", "-analyzeduration", "2000000"]);
    if !headers.is_empty() {
        args.extend(["-headers", headers.as_str()]);
    }
    let seek_str;
    if seek.as_secs() > 0 {
        seek_str = seek.as_secs_f64().to_string();
        args.extend(["-ss", &seek_str]);
    }
    args.extend(["-i", url, "-vn"]);
    args.extend(["-af", BASE_RESAMPLE_AF]);
    args.extend(PCM_OUT);
    Ok(pcm_stream(spawn(&args, Stdio::null())?))
}

/// Kaynağı filtre zincirinden geçirir. `raw_pcm`: kaynak başlıksız 48 kHz stereo f32 PCM.
fn through_ffmpeg(chain: &str, source: Box<dyn MediaSource>, raw_pcm: bool) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
    let mut args = Vec::new();
    if raw_pcm {
        args.extend(["-f", "f32le", "-ar", "48000", "-ac", "2"]);
    }
    let full_chain = if chain.contains("aresample=48000") {
        if chain.contains("asetpts=PTS-STARTPTS") {
            chain.to_owned()
        } else {
            format!("asetpts=PTS-STARTPTS,{chain}")
        }
    } else if chain.contains("asetpts=PTS-STARTPTS") {
        format!("{chain},aresample=48000:async=1:first_pts=0")
    } else {
        format!("asetpts=PTS-STARTPTS,{chain},aresample=48000:async=1:first_pts=0")
    };
    args.extend(["-i", "pipe:0", "-af", &full_chain]);
    args.extend(PCM_OUT);
    let mut child = spawn(&args, Stdio::piped())?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err(AudioStreamError::Fail("ffmpeg girişi açılamadı".into()));
    };
    // Kaynak eşzamanlı okunur; 32KB tampon ile aktarılır, ffmpeg kapanınca (şarkı bitti/atlandı) yazma hata verir ve iş parçacığı biter.
    std::thread::spawn(move || {
        let mut source = source;
        let mut buf = [0u8; 32768];
        loop {
            match source.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdin.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = stdin.flush();
    });
    Ok(pcm_stream(child))
}

/// Filtre seçiliyse akışı ffmpeg'den geçirir. Engelleyici okuma yapabilir.
fn prepare(stream: AudioStream<Box<dyn MediaSource>>, chain: Option<&str>) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
    let Some(chain) = chain else { return Ok(stream) };
    if stream.input.is_seekable() {
        return through_ffmpeg(chain, stream.input, false);
    }
    // Geri sarılamayan akış `open_url`'nin PCM'i olabilir: başlığına bakılır.
    let mut source = stream.input;
    let mut head = Vec::with_capacity(RAW_HEADER_LEN);
    while head.len() < RAW_HEADER_LEN {
        let mut buf = [0u8; RAW_HEADER_LEN];
        match source.read(&mut buf[..RAW_HEADER_LEN - head.len()]) {
            Ok(0) => break,
            Ok(n) => head.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(AudioStreamError::Fail(e.into())),
        }
    }
    if head.starts_with(RAW_MAGIC) && head.len() == RAW_HEADER_LEN {
        return through_ffmpeg(chain, source, true);
    }
    // Okunan baytlar akışın önüne geri eklenir.
    let joined: Box<dyn MediaSource> = Box::new(ReadOnlySource::new(std::io::Cursor::new(head).chain(source)));
    through_ffmpeg(chain, joined, false)
}

/// Bir kaynağı sarar; çalmaya başlarken sunucunun filtresine bakar.
pub struct Filtered {
    inner: Box<dyn Compose>,
    guild: GuildId,
    filters: Filters,
}

/// Tembel (henüz açılmamış) kaynakları filtre katmanıyla sarar; diğerlerine dokunmaz.
pub fn wrap(input: Input, guild: GuildId, filters: &Filters) -> Input {
    match input {
        Input::Lazy(inner) => Input::Lazy(Box::new(Filtered { inner, guild, filters: Arc::clone(filters) })),
        other => other,
    }
}

impl Filtered {
    fn chain(&self) -> Option<&'static str> {
        self.filters.get(&self.guild).and_then(|f| f.chain())
    }
}

#[async_trait]
impl Compose for Filtered {
    fn create(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        let stream = self.inner.create()?;
        prepare(stream, self.chain())
    }

    async fn create_async(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        let stream = self.inner.create_async().await?;
        let chain = self.chain();
        if chain.is_none() {
            return Ok(stream);
        }
        // Başlığı okumak (ağ beklemesi) çalışma zamanını bloklamasın.
        tokio::task::spawn_blocking(move || prepare(stream, chain))
            .await
            .map_err(|e| AudioStreamError::Fail(e.into()))?
    }

    fn should_create_async(&self) -> bool {
        self.inner.should_create_async()
    }

    async fn aux_metadata(&mut self) -> Result<AuxMetadata, AudioStreamError> {
        self.inner.aux_metadata().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 48 kHz mono 16 bit 1 sn sessiz WAV.
    fn silent_wav() -> Vec<u8> {
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
        wav
    }

    struct Bytes(Vec<u8>, bool);
    #[async_trait]
    impl Compose for Bytes {
        fn create(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
            let input: Box<dyn MediaSource> = if self.1 {
                Box::new(std::io::Cursor::new(self.0.clone()))
            } else {
                Box::new(ReadOnlySource::new(std::io::Cursor::new(self.0.clone())))
            };
            Ok(AudioStream { input })
        }
        async fn create_async(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
            self.create()
        }
        fn should_create_async(&self) -> bool {
            false
        }
    }

    async fn decoded_packets(input: Input) -> anyhow::Result<usize> {
        let input = input.make_playable_async(songbird::input::codecs::get_codec_registry(), songbird::input::codecs::get_probe()).await?;
        let Input::Live(songbird::input::LiveInput::Parsed(mut parsed), _) = input else {
            anyhow::bail!("çözülemedi");
        };
        let mut decoded = 0;
        while decoded < 40 {
            let Ok(packet) = parsed.format.next_packet() else { break };
            if packet.track_id() == parsed.track_id {
                parsed.decoder.decode(&packet)?;
                decoded += 1;
            }
        }
        Ok(decoded)
    }

    /// Gerçek ffmpeg ile: her filtre (geri sarılabilen, sarılamayan ve ham PCM kaynakta) çözülebilir ses verir.
    #[tokio::test]
    #[ignore = "ffmpeg gerektirir"]
    async fn live_ffmpeg_filters_produce_pcm() -> anyhow::Result<()> {
        let wav = silent_wav();
        // songbird ham PCM'i: başlık + 0,5 sn sessizlik.
        let mut raw = Vec::new();
        RawAdapter::new(ReadOnlySource::new(std::io::Cursor::new(vec![0u8; 48_000 * 8 / 2])), 48_000, 2).read_to_end(&mut raw)?;
        for filter in [Filter::Bass, Filter::Nightcore, Filter::Vaporwave, Filter::EightD] {
            for (bytes, seekable) in [(wav.clone(), true), (wav.clone(), false), (raw.clone(), false)] {
                let filters: Filters = Arc::new(DashMap::new());
                filters.insert(GuildId::new(1), filter);
                let input = wrap(Input::Lazy(Box::new(Bytes(bytes, seekable))), GuildId::new(1), &filters);
                let n = decoded_packets(input).await?;
                anyhow::ensure!(n > 5, "{filter:?} seekable={seekable}: {n} paket");
            }
        }
        Ok(())
    }

    /// Gerçek SoundCloud şarkısı (HLS) çalınabilir hâle geliyor, filtreli ve filtresiz ses paketleri çözülüyor.
    #[tokio::test]
    #[ignore = "gerçek yt-dlp, ffmpeg ve SoundCloud'a istek atar"]
    async fn live_soundcloud_plays() -> anyhow::Result<()> {
        use poise::serenity_prelude::UserId;
        let music = super::super::Music::new(songbird::Songbird::serenity(), reqwest::Client::new());
        for filter in [Filter::Off, Filter::Nightcore] {
            let (input, info) = music
                .resolve("https://soundcloud.com/forss/flickermood", UserId::new(1), GuildId::new(1))
                .await
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("{} {:?} {filter:?}", info.title, info.duration);
            let filters: Filters = Arc::new(DashMap::new());
            filters.insert(GuildId::new(1), filter);
            let n = decoded_packets(wrap(input, GuildId::new(1), &filters)).await?;
            anyhow::ensure!(n >= 40, "{filter:?}: yalnızca {n} paket");
        }
        Ok(())
    }

    #[test]
    fn test_filter_chain_always_contains_48000_resample() {
        assert_eq!(Filter::Off.chain(), None);
        for filter in [Filter::Bass, Filter::Nightcore, Filter::Vaporwave, Filter::EightD] {
            let chain = filter.chain().unwrap_or("");
            assert!(
                chain.contains("aresample=48000"),
                "{filter:?} filtresi 48000 Hz resample içermiyor: {chain}"
            );
            assert!(
                chain.contains("async=1"),
                "{filter:?} filtresi kararlı async=1 kilidi içermiyor: {chain}"
            );
            assert!(
                chain.contains("asetpts=PTS-STARTPTS"),
                "{filter:?} filtresi kararlı PTS normalizasyonu içermiyor: {chain}"
            );
        }
    }

    #[test]
    fn test_pcm_and_stream_target_constants() {
        assert_eq!(TARGET_SAMPLE_RATE, 48_000);
        assert_eq!(TARGET_CHANNELS, 2);
        assert!(BASE_RESAMPLE_AF.contains("asetpts=PTS-STARTPTS"));
        assert!(BASE_RESAMPLE_AF.contains("aresample=48000:async=1:first_pts=0"));
        assert_eq!(PCM_OUT, ["-f", "f32le", "-ac", "2", "-ar", "48000", "pipe:1"]);
    }
}
