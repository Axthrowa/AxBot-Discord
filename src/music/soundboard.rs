//! Ses panosu (`/müzik efekt`): sunucunun Discord ses panosu sesleri + Discord'un hazır sesleri,
//! çalan müziğin ÜSTÜNE karışarak çalınır (kuyruğa girmez).
//!
//! Discord'un "ses panosu sesi gönder" ucu sağırlaştırılmış kullanıcıda çalışmaz (bot kendini sağırlaştırır);
//! bu yüzden sesin dosyası CDN'den alınıp ffmpeg ile açılır.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use poise::serenity_prelude::{GuildId, Http};
use songbird::input::{AudioStream, AudioStreamError, Compose, Input};
use symphonia::core::io::MediaSource;

/// Ses listesinin önbellekte kalma süresi (otomatik tamamlama her tuşta sorar).
const CACHE_FOR: Duration = Duration::from_secs(5 * 60);
/// Sunucu başına iki efekt arasında en az bu kadar beklenir.
const COOLDOWN: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct Sound {
    pub id: u64,
    pub name: String,
    pub volume: f32,
}

type Cached = (Instant, Arc<Vec<Sound>>);
static CACHE: LazyLock<DashMap<GuildId, Cached>> = LazyLock::new(DashMap::new);
static LAST: LazyLock<DashMap<GuildId, Instant>> = LazyLock::new(DashMap::new);

/// Sunucunun kullanılabilir sesleri ve Discord'un hazır sesleri (önce sunucununkiler).
pub async fn sounds(http: &Http, guild: GuildId) -> Arc<Vec<Sound>> {
    if let Some(entry) = CACHE.get(&guild) {
        if entry.0.elapsed() < CACHE_FOR {
            return Arc::clone(&entry.1);
        }
    }
    let (own, default) = tokio::join!(http.get_guild_soundboards(guild), http.list_default_soundboard_sounds());
    let mut list = Vec::new();
    for (result, kind) in [(own, "sunucu"), (default, "hazır")] {
        match result {
            Ok(items) => list.extend(items.into_iter().filter(|s| s.available).map(|s| Sound {
                id: s.id.get(),
                name: s.name,
                volume: s.volume.clamp(0.0, 1.0) as f32,
            })),
            Err(e) => tracing::warn!(guild = %guild, kind, error = %e, "Ses panosu sesleri alınamadı"),
        }
    }
    let list = Arc::new(list);
    CACHE.insert(guild, (Instant::now(), Arc::clone(&list)));
    list
}

/// Ada (büyük/küçük harf önemsiz) ya da kimliğe göre ses.
pub fn find<'a>(sounds: &'a [Sound], query: &str) -> Option<&'a Sound> {
    let query = query.trim();
    let lower = query.to_lowercase();
    sounds.iter().find(|s| s.name.to_lowercase() == lower).or_else(|| sounds.iter().find(|s| s.id.to_string() == query))
}

/// Bekleme süresi dolduysa sunucu için yeni efekt hakkı alır.
pub fn claim(guild: GuildId) -> bool {
    let now = Instant::now();
    let mut ok = false;
    LAST.entry(guild)
        .and_modify(|last| {
            if now.duration_since(*last) >= COOLDOWN {
                *last = now;
                ok = true;
            }
        })
        .or_insert_with(|| {
            ok = true;
            now
        });
    ok
}

struct CdnSound(String);

#[async_trait]
impl Compose for CdnSound {
    fn create(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        super::filters::open_url(&self.0, &Default::default())
    }

    async fn create_async(&mut self) -> Result<AudioStream<Box<dyn MediaSource>>, AudioStreamError> {
        let url = self.0.clone();
        tokio::task::spawn_blocking(move || super::filters::open_url(&url, &Default::default()))
            .await
            .map_err(|e| AudioStreamError::Fail(e.into()))?
    }

    fn should_create_async(&self) -> bool {
        true
    }
}

/// Sesin çalınabilir kaynağı (Discord CDN'inden; kimlik sayısal olduğundan adres güvenle kurulur).
pub fn input(sound: &Sound) -> Input {
    Input::Lazy(Box::new(CdnSound(format!("https://cdn.discordapp.com/soundboard-sounds/{}", sound.id))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_name_or_id_and_rate_limits() {
        let list = vec![Sound { id: 7, name: "Kurbağa".into(), volume: 1.0 }, Sound { id: 9, name: "ba dum tss".into(), volume: 0.5 }];
        assert_eq!(find(&list, " kurbağa ").map(|s| s.id), Some(7));
        assert_eq!(find(&list, "9").map(|s| s.id), Some(9));
        assert!(find(&list, "yok").is_none());
        let guild = GuildId::new(424_242);
        assert!(claim(guild));
        assert!(!claim(guild));
    }

    /// Discord'un hazır "quack" sesi (kimlik 1) CDN'den ffmpeg ile açılıp çözülüyor.
    #[tokio::test]
    #[ignore = "ffmpeg ve Discord CDN'ine istek atar"]
    async fn live_default_sound_decodes() -> anyhow::Result<()> {
        let input = input(&Sound { id: 1, name: "quack".into(), volume: 1.0 });
        let input = input.make_playable_async(songbird::input::codecs::get_codec_registry(), songbird::input::codecs::get_probe()).await?;
        let Input::Live(songbird::input::LiveInput::Parsed(mut parsed), _) = input else {
            anyhow::bail!("çözülemedi");
        };
        let mut decoded = 0;
        while let Ok(packet) = parsed.format.next_packet() {
            if packet.track_id() == parsed.track_id {
                parsed.decoder.decode(&packet)?;
                decoded += 1;
            }
        }
        anyhow::ensure!(decoded > 5, "yalnızca {decoded} paket");
        Ok(())
    }
}
