//! Ses kanalında müzik: YouTube araması/bağlantısı, SoundCloud, çalma listeleri ve doğrudan ses bağlantıları.
//!
//! - Ses bağlantısı: songbird (Discord'un zorunlu uçtan uca ses şifrelemesi DAVE dahil).
//! - Kaynak çözme: `yt-dlp` (proje içindeki `tools/` klasöründen ya da PATH'ten).
//! - Ses çözme: symphonia (saf Rust). ffmpeg yalnızca ses filtresinde ve HLS akışlarında (SoundCloud) kullanılır (`filters`).
//!
//! Şarkı bilgileri songbird'ün tip-güvensiz `TrackHandle::data` alanı yerine burada,
//! parça UUID'si ile anahtarlanan bir tabloda tutulur.
//!
//! Paylaşılan durum (`Shared`) `Arc` içindedir: olay dinleyicileri (otomatik çalma) de kuyruğa
//! şarkı ekleyebilsin diye.

pub mod card;
pub mod driver;
pub mod lyrics;
pub mod render;
mod events;
pub mod extra;
pub mod filters;
pub mod icons;
pub mod playlists;
pub mod soundboard;
mod prefetch;
pub mod cache;

use std::collections::VecDeque;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use poise::serenity_prelude::{self as serenity, ChannelId, GuildId, Http, UserId};
use songbird::input::{Input, YoutubeDl};
use songbird::tracks::Track;
use tokio::sync::Mutex;
use songbird::{Call, Event, Songbird, TrackEvent};
use sqlx::PgPool;
use tokio::process::Command;
use tracing::{info, warn};
use uuid::Uuid;

use crate::db::snowflake;
use crate::guild_config::to_id;

use self::events::{IdleLeaver, TrackNotice, TrackNotifier};
pub use self::events::track_line;

/// Bir sunucunun kuyruğundaki en fazla şarkı.
pub const MAX_QUEUE: usize = 100;
/// Varsayılan ses seviyesi (1.0 = %100).
const DEFAULT_VOLUME: f32 = 0.6;
/// Çalma listesi okuma süresi sınırı.
const PLAYLIST_TIMEOUT: Duration = Duration::from_secs(60);
/// Seslendirilecek metnin en fazla uzunluğu.
pub const TTS_MAX_CHARS: usize = 200;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(25);
/// Şarkı kartının (ilerleme çubuğu) yenilenme aralığı. Discord REST ve CPU yükünü dengeler.
const CARD_REFRESH: Duration = Duration::from_secs(12);
/// Kart görselinin dosya adı (her düzenlemede değiştirilir).
const CARD_FILE: &str = "kart.jpg";
/// Otomatik çalmada tekrar etmesin diye hatırlanan son şarkı sayısı.
const HISTORY_LEN: usize = 30;

/// Çözümlenen şarkı metadata ve akış URL önbelleği (15 dk TTL, maks 500 kayıt).
static RESOLVE_CACHE: std::sync::LazyLock<dashmap::DashMap<String, (songbird::input::AuxMetadata, String, std::time::Instant)>> =
    std::sync::LazyLock::new(dashmap::DashMap::new);
const RESOLVE_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const RESOLVE_CACHE_MAX: usize = 500;

/// Sunucu başına birleşik müzik ayarları (veritabanında kalıcı).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MusicSettings {
    /// 7/24: kanal boşalsa da kuyruk bitse de ayrılma.
    pub always_on: bool,
    /// Kuyruk bitince benzer şarkılarla devam et.
    pub autoplay: bool,
    /// Boşta kalınca ayrılma süresi (saniye; varsayılan 300 = 5 dk, 0 = anında).
    pub idle_disconnect_secs: u32,
    /// Tek bir ses oturumunda kalma üst süresi (dakika; 0 = sınırsız).
    pub max_session_mins: u32,
    /// Varsayılan ses seviyesi (1-100; varsayılan 60).
    pub default_volume: u16,
    /// Maksimum parça süresi (saniye; 0 = sınırsız).
    pub max_length_secs: u32,
    /// DJ rolü (varsa yalnızca bu rol ve yöneticiler yönetir).
    pub dj_role: Option<u64>,
}

impl Default for MusicSettings {
    fn default() -> Self {
        Self {
            always_on: false,
            autoplay: false,
            idle_disconnect_secs: 300,
            max_session_mins: 0,
            default_volume: 60,
            max_length_secs: 0,
            dj_role: None,
        }
    }
}

#[allow(dead_code)]
pub type MusicConfig = MusicSettings;

impl crate::settings::PluginConfig for MusicSettings {
    const KEY: &'static str = "music";
}

#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub guild_id: GuildId,
    pub title: String,
    pub url: Option<String>,
    pub duration: Option<Duration>,
    pub thumbnail: Option<String>,
    pub requester: UserId,
    /// Sıraya eklenen şarkılar çalmaya başlayınca kanala duyurulur
    /// (hemen çalanlar için `/çal` yanıtı yeterli).
    pub announce: bool,
    /// Şarkı Spotify'dan geldiyse orijinal bağlantısı ("Spotify'da aç" butonu).
    pub spotify_url: Option<String>,
}

pub struct Shared {
    pub(crate) songbird: Arc<Songbird>,
    http: reqwest::Client,
    /// yt-dlp programı; `YoutubeDl` 'static yaşam süresi istediği için açılışta sabitlenir.
    ytdlp: &'static str,
    ytdlp_args: Vec<String>,
    pub(crate) tracks: Arc<DashMap<Uuid, TrackInfo>>,
    volumes: DashMap<GuildId, f32>,
    /// Duyuruların yapılacağı metin kanalı (son `/çal` kullanılan kanal).
    pub(crate) text_channels: Arc<DashMap<GuildId, ChannelId>>,
    pub(crate) settings: Arc<DashMap<GuildId, MusicSettings>>,
    pub(crate) session_started: Arc<DashMap<GuildId, std::time::Instant>>,
    driver: Arc<dyn driver::AudioDriver>,
    /// Son çalınan YouTube video kimlikleri (otomatik çalma tekrar etmesin).
    history: DashMap<GuildId, VecDeque<String>>,
    /// Sunucudaki son "şimdi çalıyor" kartı; yenisi gelince eskisinin butonları kaldırılır.
    cards: DashMap<GuildId, (ChannelId, serenity::MessageId)>,
    /// Kartını canlı güncelleyen görevi çalışan sunucular (sunucu başına tek görev).
    card_tasks: dashmap::DashSet<GuildId>,
    /// Otomatik çalma sorgusu çalışan sunucular (çift istek/çakışma engeli).
    pub(crate) autoplay_tasks: Arc<dashmap::DashSet<GuildId>>,
    /// Ses kanalında dinleyici kalmadığında geçen süreyi takip eder.
    pub(crate) empty_channels: Arc<DashMap<GuildId, std::time::Instant>>,
    /// Çözümlenmiş kapak görselleri (adres → görsel; bulunamadıysa `None`).
    covers: DashMap<String, Option<Arc<image::RgbaImage>>>,
    /// Kartta yazan "isteyen" adları.
    names: DashMap<UserId, String>,
    /// Kartın altındaki sunucu adı (her çizimde Discord'a sorulmasın).
    guild_names: DashMap<GuildId, String>,
    /// Sunucu başına ses filtresi (`/müzik filtre`).
    filters: filters::Filters,
    /// seek_to nedeniyle elle durdurulan parçaların UUID'leri (TrackEvent::End bastırma).
    pub(crate) suppressed_ends: Arc<dashmap::DashSet<Uuid>>,
    /// seek_to ile atlanan saniye ofseti (kart ve panel senkronizasyonu).
    pub(crate) position_offsets: Arc<DashMap<GuildId, Duration>>,
}

/// Müzik sistemi; ucuzca klonlanır, alanlarına `Deref` ile erişilir.
#[derive(Clone)]
pub struct Music(Arc<Shared>);

impl Shared {
    /// yt-dlp'ye verilen ortak argümanlar + (bulunduysa) engeli aşan YouTube istemcisi.
    fn args(&self) -> Vec<String> {
        let mut args = self.ytdlp_args.clone();
        args.extend(client_args(preferred_client().as_deref()));
        args
    }

    /// Sunucu için geçerli ses seviyesini döner (ayarlanmış seviye veya sunucu varsayılanı).
    pub fn volume_of(&self, guild_id: GuildId) -> f32 {
        self.volumes.get(&guild_id).map_or_else(
            || {
                let def = self.settings.get(&guild_id).map(|s| s.default_volume).unwrap_or(60);
                (f32::from(def)) / 100.0
            },
            |v| *v,
        )
    }

    /// Sunucu için aktif parça sarma (seek_to) saniye ofseti.
    pub fn position_offset_of(&self, guild_id: GuildId) -> Duration {
        self.position_offsets.get(&guild_id).map(|o| *o).unwrap_or(Duration::ZERO)
    }

    /// yt-dlp -g ile taze doğrudan medya URL'sini çözümler (FFmpeg'e web sayfası linki vermemek için).
    pub async fn resolve_direct_stream_url(&self, target: &str) -> Option<String> {
        let mut cmd = Command::new(self.ytdlp);
        cmd.args(["-g", "-f", "bestaudio[ext=webm]/bestaudio[ext=m4a]/ba/b", "--no-warnings", "--no-playlist"]);
        cmd.args(self.args());
        cmd.arg(target);
        let output = tokio::time::timeout(Duration::from_secs(15), cmd.output()).await.ok()?.ok()?;
        if output.status.success() {
            let url = String::from_utf8_lossy(&output.stdout).lines().next()?.trim().to_owned();
            if !url.is_empty() {
                return Some(url);
            }
        }
        None
    }
}

impl Deref for Music {
    type Target = Shared;
    fn deref(&self) -> &Shared {
        &self.0
    }
}

pub struct Enqueued {
    /// İlk eklenen parçanın yeri: 0 = hemen çalıyor, 1+ = kuyruktaki sırası.
    pub position: usize,
}

/// Çalma listesindeki parçalar (henüz çözülmemiş; sırası gelince yt-dlp ile çözülür).
pub struct Playlist {
    pub title: String,
    pub tracks: Vec<(Input, TrackInfo)>,
    /// Listede olup okunamayan (özel/silinmiş) parçalar.
    pub skipped: usize,
    /// Liste `limit`'ten uzundu; fazlası eklenmedi.
    pub truncated: bool,
}

/// YouTube çalma listesi (`list=`) veya SoundCloud seti bağlantısı mı?
pub fn is_playlist_url(query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if !(q.starts_with("http://") || q.starts_with("https://")) {
        return false;
    }
    let youtube = q.contains("youtube.com/") || q.contains("youtu.be/");
    (youtube && q.contains("list=")) || (q.contains("soundcloud.com/") && q.contains("/sets/"))
}

/// "Bot musun" engelinde sırayla denenen YouTube istemcileri. yt-dlp'nin PO Token rehberi `mweb` + PO token
/// önerir; diğerleri farklı uçlar kullandığı için bazı veri merkezi IP'lerinde engele takılmaz.
const FALLBACK_CLIENTS: [&str; 4] = ["mweb", "tv_simply", "web_embedded", "android_vr"];

/// İşe yarayan istemci (bulunduysa); bütün yt-dlp çağrılarında kullanılır. `YTDLP_YOUTUBE_CLIENT` ile başlangıç değeri verilebilir.
static PREFERRED_CLIENT: std::sync::LazyLock<std::sync::RwLock<Option<String>>> = std::sync::LazyLock::new(|| {
    std::sync::RwLock::new(std::env::var("YTDLP_YOUTUBE_CLIENT").ok().map(|c| c.trim().to_owned()).filter(|c| !c.is_empty()))
});

fn preferred_client() -> Option<String> {
    PREFERRED_CLIENT.read().ok().and_then(|c| c.clone())
}

/// Bütün istemciler engellenince yedekler bu zamana (Unix sn) kadar atlanır.
static ALL_BLOCKED_UNTIL: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
const ALL_BLOCKED_PAUSE_SECS: i64 = 10 * 60;

fn set_preferred_client(client: &str) {
    if let Ok(mut current) = PREFERRED_CLIENT.write() {
        *current = Some(client.to_owned());
    }
}

fn client_args(client: Option<&str>) -> Vec<String> {
    client.map(|c| vec!["--extractor-args".to_owned(), format!("youtube:player_client={c}")]).unwrap_or_default()
}

/// yt-dlp hatası YouTube'un bot engeli mi? (veri merkezi IP'lerinde sık görülür)
fn is_bot_block(error: &str) -> bool {
    error.contains("Sign in to confirm") || error.contains("not a bot")
}

/// Sunucu kurulumundaki yt-dlp ayarları (ortam değişkenleriyle; ev bilgisayarında hiçbiri gerekmez):
/// - `YTDLP_PLUGIN_DIRS`: eklenti klasörü (PO token sağlayıcısı bgutil burada),
/// - `YTDLP_POT_URL`: PO token sunucusu (ör. `http://pot:4416`),
/// - `YTDLP_COOKIES`: isteğe bağlı YouTube çerez dosyası (Netscape biçimi). yt-dlp çerezleri geri yazdığı için
///   yazılabilir bir kopyası kullanılır.
fn server_ytdlp_args() -> Vec<String> {
    let mut args = Vec::new();
    let var = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    if let Some(dirs) = var("YTDLP_PLUGIN_DIRS") {
        args.extend(["--plugin-dirs".to_owned(), dirs]);
    }
    if let Some(url) = var("YTDLP_POT_URL") {
        args.extend(["--extractor-args".to_owned(), format!("youtubepot-bgutilhttp:base_url={url}")]);
    }
    // Sunucunun IPv6 adresi engellenmemişse YouTube'a IPv6 ile gidilir (kapsayıcıda IPv6 açık olmalı).
    if var("YTDLP_FORCE_IPV6").is_some_and(|v| v == "1") {
        args.push("--force-ipv6".to_owned());
    }
    // Dosya konmadıysa sessizce geçilir (çerez isteğe bağlı).
    if let Some(path) = var("YTDLP_COOKIES").filter(|p| Path::new(p).is_file()) {
        let copy = std::env::temp_dir().join("axbot-yt-cookies.txt");
        match std::fs::copy(&path, &copy) {
            Ok(_) => {
                info!("YouTube çerez dosyası kullanılıyor");
                args.extend(["--cookies".to_owned(), copy.to_string_lossy().into_owned()]);
            }
            Err(e) => warn!(path, error = %e, "YTDLP_COOKIES dosyası okunamadı; çerezsiz devam ediliyor"),
        }
    }
    args
}

impl Music {
    pub fn new(songbird: Arc<Songbird>, http: reqwest::Client) -> Self {
        let tools = Path::new("tools");
        let ytdlp = find_tool(tools, "yt-dlp").unwrap_or_else(|| "yt-dlp".to_owned());
        filters::set_ytdlp(ytdlp.clone());
        filters::set_ffmpeg(find_tool(tools, "ffmpeg").unwrap_or_else(|| "ffmpeg".to_owned()));

        // YouTube'un bot korumasını çözmek için yt-dlp bir JavaScript motoru (deno) kullanır.
        let mut ytdlp_args = Vec::new();
        ytdlp_args.push("--no-warnings".to_owned());
        if let Some(deno) = find_tool(tools, "deno") {
            ytdlp_args.push("--js-runtimes".to_owned());
            ytdlp_args.push(format!("deno:{deno}"));
        }
        ytdlp_args.extend(server_ytdlp_args());

        let driver = driver::resolve_driver(&http);
        cache::clear_orphaned_cache();
        Self(Arc::new(Shared {
            songbird,
            http,
            ytdlp: Box::leak(ytdlp.into_boxed_str()),
            ytdlp_args,
            tracks: Arc::new(DashMap::new()),
            volumes: DashMap::new(),
            text_channels: Arc::new(DashMap::new()),
            settings: Arc::new(DashMap::new()),
            session_started: Arc::new(DashMap::new()),
            driver,
            history: DashMap::new(),
            cards: DashMap::new(),
            card_tasks: dashmap::DashSet::new(),
            autoplay_tasks: Arc::new(dashmap::DashSet::new()),
            empty_channels: Arc::new(DashMap::new()),
            covers: DashMap::new(),
            names: DashMap::new(),
            guild_names: DashMap::new(),
            filters: Arc::new(DashMap::new()),
            suppressed_ends: Arc::new(dashmap::DashSet::new()),
            position_offsets: Arc::new(DashMap::new()),
        }))
    }

    /// Sunucunun ses filtresi; sıradaki şarkıdan itibaren geçerli olur.
    pub fn set_filter(&self, guild_id: GuildId, filter: filters::Filter) {
        if filter == filters::Filter::Off {
            self.filters.remove(&guild_id);
        } else {
            self.filters.insert(guild_id, filter);
        }
    }

    /// Aktif ses sürücüsü (NativeSongbird veya Lavalink v4).
    pub fn driver(&self) -> &dyn driver::AudioDriver {
        self.driver.as_ref()
    }

    /// Müzik yönetim yetkisi denetimi:
    /// - DJ rolü ayarlanmamışsa herkes yönetebilir.
    /// - Sunucu sahibi veya Yönetici/Sunucuyu Yönet iznine sahip olanlar.
    /// - DJ rolüne sahip üyeler.
    /// - Çalan parçayı veya kuyruktaki ilgili parçayı ekleyen üye (requester).
    /// - Kanalda bot dışında yalnızca 1 dinleyici varsa (baş başa kalındıysa).
    pub fn can_manage(
        &self,
        cache: &serenity::Cache,
        guild_id: GuildId,
        user_id: UserId,
        channel_id: ChannelId,
        requester: Option<UserId>,
    ) -> bool {
        let settings = self.settings(guild_id);
        let Some(dj_role) = settings.dj_role else {
            return true;
        };

        if Some(user_id) == requester {
            return true;
        }

        cache.guild(guild_id).is_none_or(|guild| {
            let Some(member) = guild.members.get(&user_id) else {
                return false;
            };
            let manages = guild.owner_id == user_id
                || guild.member_permissions(member).intersects(
                    serenity::Permissions::ADMINISTRATOR | serenity::Permissions::MANAGE_GUILD,
                );
            let listeners = guild
                .voice_states
                .values()
                .filter(|v| v.channel_id == Some(channel_id) && guild.members.get(&v.user_id).is_some_and(|m| !m.user.bot))
                .count();

            manages || member.roles.iter().any(|r| r.get() == dj_role) || listeners <= 1
        })
    }

    /// Kalıcı birleşik müzik ayarlarını yükler.
    pub async fn load_settings(&self, db: &PgPool) -> anyhow::Result<()> {
        let rows: Vec<(i64, bool, bool)> = sqlx::query_as(
            "SELECT guild_id, music_247, music_autoplay FROM guild_settings WHERE music_247 OR music_autoplay",
        )
        .fetch_all(db)
        .await?;
        for (guild, always_on, autoplay) in rows {
            if let Some(guild_id) = to_id(Some(guild)).map(GuildId::new) {
                self.settings.insert(
                    guild_id,
                    MusicSettings {
                        always_on,
                        autoplay,
                        ..Default::default()
                    },
                );
            }
        }

        let extras: Vec<(i64, String)> = sqlx::query_as(
            "SELECT guild_id, data FROM plugin_settings WHERE plugin = 'music'",
        )
        .fetch_all(db)
        .await
        .unwrap_or_default();

        for (guild, data) in extras {
            if let (Some(guild_id), Ok(cfg)) = (to_id(Some(guild)).map(GuildId::new), serde_json::from_str::<MusicSettings>(&data)) {
                self.settings
                    .entry(guild_id)
                    .and_modify(|s| {
                        s.dj_role = cfg.dj_role;
                        s.idle_disconnect_secs = cfg.idle_disconnect_secs;
                        s.max_session_mins = cfg.max_session_mins;
                        s.default_volume = cfg.default_volume;
                        s.max_length_secs = cfg.max_length_secs;
                        if cfg.always_on { s.always_on = true; }
                        if cfg.autoplay { s.autoplay = true; }
                    })
                    .or_insert(cfg);
            }
        }
        Ok(())
    }

    pub fn settings(&self, guild_id: GuildId) -> MusicSettings {
        self.settings.get(&guild_id).map(|s| *s).unwrap_or_default()
    }

    pub async fn save_settings(&self, db: &PgPool, guild_id: GuildId, settings: MusicSettings) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO guild_settings (guild_id, music_247, music_autoplay) VALUES ($1, $2, $3)
             ON CONFLICT (guild_id) DO UPDATE
                 SET music_247 = EXCLUDED.music_247, music_autoplay = EXCLUDED.music_autoplay, updated_at = now()",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(settings.always_on)
        .bind(settings.autoplay)
        .execute(db)
        .await?;

        if let Ok(json) = serde_json::to_string(&settings) {
            let _ = sqlx::query(
                "INSERT INTO plugin_settings (guild_id, plugin, data) VALUES ($1, 'music', $2)
                 ON CONFLICT (guild_id, plugin) DO UPDATE SET data = EXCLUDED.data",
            )
            .bind(snowflake(guild_id.get())?)
            .bind(json)
            .execute(db)
            .await;
        }

        self.settings.insert(guild_id, settings);
        Ok(())
    }

    /// Çalan şarkıyı belirli bir zamana sarar.
    pub async fn seek_to(&self, http: &Arc<Http>, guild_id: GuildId, target: Duration) -> Result<(), String> {
        let Some(call) = self.songbird.get(guild_id) else {
            return Err("Şu an çalan bir şarkı yok.".to_owned());
        };

        let (current, uuid, info) = {
            let handler = call.lock().await;
            let queue = handler.queue().clone();
            let Some(current) = queue.current() else {
                return Err("Şu an çalan bir şarkı yok.".to_owned());
            };
            let uuid = current.uuid();
            let Some(info) = self.tracks.get(&uuid).map(|t| t.value().clone()) else {
                return Err("Şarkı bilgisi bulunamadı.".to_owned());
            };
            (current, uuid, info)
        };

        if let Some(total) = info.duration {
            if target >= total {
                return Err(format!(
                    "Şarkı {} uzunluğunda; daha ileri sarılamaz.",
                    format_duration(total)
                ));
            }
        }

        let current_filter = self.filters.get(&guild_id).map(|f| *f).unwrap_or(filters::Filter::Off);

        // Yol A: Filtre kapalıysa Songbird'ün yerel/seekable TrackHandle::seek_async metodunu dene
        if current_filter == filters::Filter::Off && current.seek_async(target).await.is_ok() {
            self.position_offsets.insert(guild_id, Duration::ZERO);
            self.refresh_card_now(http, guild_id).await;
            return Ok(());
        }

        // Yol B: Hızlı FFmpeg input seeking (-ss <target> -i ...) ile taze akış başlatma
        let Some(url) = info.url.clone() else {
            return Err("Bu yayın/şarkı sarılamıyor.".to_owned());
        };

        let cached_path = cache::get_cached_track(&url);
        let input = if let Some(ref path) = cached_path {
            filters::open_file_seek(path, target, current_filter)
                .map_err(|e| format!("Önbellek dosyası açılamadı: {e}"))?
        } else {
            let direct_url = self.resolve_direct_stream_url(&url).await.unwrap_or_else(|| url.clone());
            filters::open_direct_url_seek(&direct_url, &std::collections::HashMap::new(), target, current_filter)
                .map_err(|e| format!("Doğrudan akış açılamadı: {e}"))?
        };

        let mut handler = call.lock().await;
        let queue = handler.queue().clone();
        let volume = self.volume_of(guild_id);

        // Eski parçanın TrackEvent::End olayında autoplay tetiklenmemesi ve kuyruğun bozulmaması için bastır
        self.suppressed_ends.insert(uuid);
        let _ = current.stop();
        self.tracks.remove(&uuid);

        let handle = queue.add_with_preload(Track::new(input).volume(volume), &mut handler, None);
        queue.modify_queue(|q| {
            if let Some(pos) = q.iter().position(|t| t.uuid() == handle.uuid()) {
                if let Some(track) = q.remove(pos) {
                    q.retain(|t| t.uuid() != uuid);
                    q.push_front(track);
                }
            }
        });
        self.tracks.insert(handle.uuid(), info);
        for (event, notice) in [
            (TrackEvent::Play, TrackNotice::Started),
            (TrackEvent::End, TrackNotice::Ended),
            (TrackEvent::Error, TrackNotice::Failed),
        ] {
            let notifier = TrackNotifier {
                http: Arc::clone(http),
                music: Arc::downgrade(&self.0),
                notice,
            };
            let _ = handle.add_event(Event::Track(event), notifier);
        }

        self.position_offsets.insert(guild_id, target);
        drop(handler);
        self.refresh_card_now(http, guild_id).await;
        Ok(())
    }

    /// Kuyruktan bir şarkıyı çıkarır (1 = sıradaki). Çıkarılanın bilgisini döner.
    pub async fn remove_at(&self, guild_id: GuildId, index: usize) -> Option<TrackInfo> {
        let call = self.songbird.get(guild_id)?;
        let removed = call.lock().await.queue().dequeue(index)?;
        let _ = removed.stop();
        self.tracks.remove(&removed.uuid()).map(|(_, info)| info)
    }

    /// Sıradaki bir şarkının yerini değiştirir (1 = sıradaki). Başarılıysa taşınanın adı.
    pub async fn move_track(&self, guild_id: GuildId, from: usize, to: usize) -> Option<String> {
        let call = self.songbird.get(guild_id)?;
        let handler = call.lock().await;
        let moved = handler.queue().modify_queue(|queue| reorder(queue, from, to).map(|track| track.uuid()))?;
        drop(handler);
        Some(self.track_info(moved).map_or_else(|| "Şarkı".to_owned(), |i| i.title))
    }

    /// Radyo yayınını çalar: kuyruk temizlenir ve sadece radyo kalır.
    pub async fn play_radio(
        &self,
        http: Arc<Http>,
        guild_id: GuildId,
        voice_channel: ChannelId,
        text_channel: ChannelId,
        station: &extra::Station,
        requester: UserId,
    ) -> anyhow::Result<()> {
        if let Some(call) = self.songbird.get(guild_id) {
            call.lock().await.queue().stop();
            self.tracks.retain(|_, t| t.guild_id != guild_id);
        }
        let input: Input = songbird::input::HttpRequest::new(self.http.clone(), station.url.to_owned()).into();
        let info = TrackInfo {
            guild_id,
            title: format!("📻 {}", station.name),
            url: None,
            duration: None,
            thumbnail: None,
            requester,
            announce: false,
            spotify_url: None,
        };
        self.enqueue_many(http, guild_id, voice_channel, text_channel, vec![(input, info)]).await?;
        Ok(())
    }

    /// Spotify şarkı/liste/albümünü YouTube araması olarak sıraya hazırlar (sırası gelince aranır).
    pub fn spotify_inputs(&self, tracks: Vec<extra::SpotifyTrack>, requester: UserId, guild_id: GuildId) -> Vec<(Input, TrackInfo)> {
        tracks
            .into_iter()
            .map(|t| {
                let query = format!("{} - {}", t.artists, t.title);
                let target = format!("ytsearch1:{query}");
                let source = YoutubeDl::new_search_ytdl_like(self.ytdlp, self.http.clone(), query.clone()).user_args(self.args());
                let input = prefetch::Prefetched::lazy(target.clone(), source, self.args(), Some(self.http.clone()));
                let info = TrackInfo {
                    guild_id,
                    title: query,
                    url: Some(target),
                    duration: t.duration,
                    thumbnail: t.cover,
                    requester,
                    announce: false,
                    spotify_url: t.url,
                };
                (input, info)
            })
            .collect()
    }

    /// Metni sese çeviren girdi (Türkçe). Ücretsiz, resmî olmayan bir uç nokta kullanılır: en fazla 200 karakter.
    pub fn tts_input(&self, text: &str) -> Option<Input> {
        let text: String = text.trim().chars().take(TTS_MAX_CHARS).collect();
        if text.is_empty() {
            return None;
        }
        let url = url::Url::parse_with_params("https://translate.google.com/translate_tts", &[("ie", "UTF-8"), ("tl", "tr"), ("client", "tw-ob"), ("q", text.as_str())]).ok()?;
        // Google'ın bu ucu HTTP/2 isteklerini otomatik trafik sayıp reddedebiliyor: tarayıcı gibi istemci.
        let client = crate::net::plain_client().map_or_else(|_| self.http.clone(), Clone::clone);
        Some(songbird::input::HttpRequest::new(client, url.to_string()).into())
    }

    /// Kayıtlı çalma listesinin parçaları: bağlantısı olan doğrudan, olmayan başlığıyla aranarak çalınır.
    pub fn saved_inputs(&self, tracks: Vec<playlists::SavedTrack>, requester: UserId, guild_id: GuildId) -> Vec<(Input, TrackInfo)> {
        tracks
            .into_iter()
            .map(|t| {
                // Yalnızca http(s) bağlantıları yt-dlp'ye bağlantı olarak verilir; gerisi arama metnidir.
                let url = t.url.filter(|u| u.starts_with("https://") || u.starts_with("http://"));
                let (target, source) = match &url {
                    Some(url) => (url.clone(), YoutubeDl::new_ytdl_like(self.ytdlp, self.http.clone(), url.clone())),
                    None => (format!("ytsearch1:{}", t.title), YoutubeDl::new_search_ytdl_like(self.ytdlp, self.http.clone(), t.title.clone())),
                };
                let input = prefetch::Prefetched::lazy(target, source.user_args(self.args()), self.args(), Some(self.http.clone()));
                let info = TrackInfo { guild_id, title: t.title, url, duration: None, thumbnail: None, requester, announce: false, spotify_url: None };
                (input, info)
            })
            .collect()
    }

    /// yt-dlp kurulu mu? Açılışta loglanır; `/çal` da kurulu değilse bunu söyler.
    pub async fn ytdlp_version(&self) -> Option<String> {
        let output = Command::new(self.ytdlp).arg("--version").output().await.ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    pub fn songbird(&self) -> &Arc<Songbird> {
        &self.songbird
    }

    pub fn track_info(&self, id: Uuid) -> Option<TrackInfo> {
        self.tracks.get(&id).map(|t| t.value().clone())
    }

    pub fn set_volume(&self, guild_id: GuildId, volume: f32) {
        self.volumes.insert(guild_id, volume);
    }

    pub fn volume_of(&self, guild_id: GuildId) -> f32 {
        self.0.volume_of(guild_id)
    }

    /// Ses seviyesini kaydeder ve çalan + sıradaki tüm şarkılara uygular.
    pub async fn apply_volume(&self, guild_id: GuildId, volume: f32) {
        self.set_volume(guild_id, volume);
        if let Some(call) = self.songbird.get(guild_id) {
            for handle in call.lock().await.queue().current_queue() {
                let _ = handle.set_volume(volume);
            }
        }
    }

    /// Çalan hariç sıradaki tüm şarkıları siler; silinen sayısı.
    pub async fn clear_upcoming(&self, guild_id: GuildId) -> usize {
        let Some(call) = self.songbird.get(guild_id) else {
            return 0;
        };
        let removed = call.lock().await.queue().modify_queue(|q| q.drain(1..).collect::<Vec<_>>());
        for track in &removed {
            let _ = track.stop();
            self.tracks.remove(&track.uuid());
        }
        removed.len()
    }

    pub async fn refresh_card_now(&self, http: &Arc<Http>, guild_id: GuildId) {
        self.0.refresh_card_now(http, guild_id).await;
    }

    /// Kartı müzik kanalına (yeni mesaj olarak) gönderir; önceki kart silinir.
    pub async fn post_card(&self, http: &Arc<Http>, guild_id: GuildId, channel: ChannelId) -> bool {
        self.text_channels.insert(guild_id, channel);
        Shared::post_card(Arc::clone(&self.0), Arc::clone(http), guild_id).await
    }

    /// Yeni kartı kaydeder, öncekinin butonlarını kaldırır ve canlı güncellemeyi başlatır.
    pub async fn set_card(&self, http: &Arc<Http>, guild_id: GuildId, channel: ChannelId, message: serenity::MessageId) {
        Shared::replace_card(&self.0, http, guild_id, channel, message).await;
    }

    /// Arama sorgusunu veya bağlantıyı çalınabilir bir kaynağa çevirir.
    /// Hata mesajı doğrudan kullanıcıya gösterilir.
    pub async fn resolve(&self, query: &str, requester: UserId, guild_id: GuildId) -> Result<(Input, TrackInfo), String> {
        let query = query.trim();
        let is_url = query.starts_with("http://") || query.starts_with("https://");
        if is_url && !(extra::is_media_site(query) && extra::is_public_url(query).await) {
            warn!(query, "Desteklenmeyen / yerel ağ bağlantısı reddedildi");
            return Err("❌ Bu bağlantı çalınamaz. YouTube, Spotify, SoundCloud, Bandcamp, Twitch gibi bilinen bir müzik/video sitesinin bağlantısını ver ya da şarkının adını yaz.".to_owned());
        }

        let result = self.fetch(query, !is_url).await;
        let (input, meta) = result.map_err(|text| {
            warn!(query, error = %text, "Şarkı çözülemedi");
            if text.contains("could not find executable") {
                "❌ Müzik için gereken **yt-dlp** programı bulunamadı. Bot yöneticisinin `tools` klasörüne kurması gerekiyor."
                    .to_owned()
            } else if is_bot_block(&text) {
                "❌ YouTube bu sunucunun IP adresini \"bot\" sanıp engelledi. Biraz sonra tekrar dene; sürerse bot yöneticisine \
                 haber ver (sunucuya YouTube çerezi eklenmesi gerekebilir)."
                    .to_owned()
            } else {
                format!("❌ **{query}** için çalınabilir bir sonuç bulunamadı.")
            }
        })?;

        let info = TrackInfo {
            guild_id,
            title: meta
                .title
                .or(meta.track)
                .unwrap_or_else(|| "Bilinmeyen parça".to_owned()),
            url: meta.source_url,
            duration: meta.duration,
            thumbnail: meta.thumbnail,
            requester,
            announce: false,
            spotify_url: None,
        };
        Ok((input, info))
    }

    fn check_cached_resolve(&self, target: &str, search: bool) -> Option<(Input, songbird::input::AuxMetadata)> {
        let key = format!("{search}:{target}");
        let entry = RESOLVE_CACHE.get(&key)?;
        let (ref meta, ref resolved_target, cached_at) = *entry;
        if cached_at.elapsed() < RESOLVE_CACHE_TTL {
            let source = if search {
                YoutubeDl::new_search_ytdl_like(self.ytdlp, self.http.clone(), resolved_target.clone())
            } else {
                YoutubeDl::new_ytdl_like(self.ytdlp, self.http.clone(), resolved_target.clone())
            }
            .user_args(self.args());
            return Some(prefetch::Prefetched::from_cached(
                resolved_target.clone(),
                source,
                self.args(),
                meta.clone(),
                Some(self.http.clone()),
            ));
        }
        None
    }

    fn cache_resolved(target: &str, search: bool, meta: &songbird::input::AuxMetadata) {
        let resolved_target = meta.source_url.clone().unwrap_or_else(|| target.to_owned());
        if RESOLVE_CACHE.len() >= RESOLVE_CACHE_MAX {
            let now = std::time::Instant::now();
            RESOLVE_CACHE.retain(|_, (_, _, at)| now.duration_since(*at) < RESOLVE_CACHE_TTL);
        }
        if RESOLVE_CACHE.len() >= RESOLVE_CACHE_MAX {
            RESOLVE_CACHE.clear();
        }
        RESOLVE_CACHE.insert(
            format!("{search}:{target}"),
            (meta.clone(), resolved_target, std::time::Instant::now()),
        );
    }

    /// yt-dlp'yi bir kez çalıştırır (sonuç çalarken de kullanılır, bkz. `prefetch`). Hata metni yt-dlp'ninkidir.
    /// YouTube "bot musun" engeli verirse diğer istemciler sırayla denenir; işe yarayan hatırlanır.
    async fn fetch(&self, target: &str, search: bool) -> Result<(Input, songbird::input::AuxMetadata), String> {
        if let Some(cached) = self.check_cached_resolve(target, search) {
            return Ok(cached);
        }
        let first = self.fetch_with(target, search, self.args()).await;
        let Err(first_error) = first else {
            if let Ok((_, ref meta)) = first {
                Self::cache_resolved(target, search, meta);
            }
            return first;
        };
        let blocked = is_bot_block(&first_error);
        let current = preferred_client();
        // Varsayılan istemcide engel dışı hata (bulunamadı vb.) gerçek sonuçtur. Tercih edilen istemci ise
        // bazı videoları açamayabilir (ör. `web_embedded` gömülemeyenleri): o zaman diğerleri de denenir.
        if !blocked && current.is_none() {
            return Err(first_error);
        }
        // Az önce hepsi engellendiyse kullanıcı her şarkıda beş denemeyi beklemesin.
        if blocked && chrono::Utc::now().timestamp() < ALL_BLOCKED_UNTIL.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(first_error);
        }
        let mut other_error = (!blocked).then(|| first_error.clone());
        for client in FALLBACK_CLIENTS.iter().filter(|c| current.as_deref() != Some(**c)) {
            let mut args = self.ytdlp_args.clone();
            args.extend(client_args(Some(client)));
            match self.fetch_with(target, search, args).await {
                Ok(found) => {
                    info!(client, "YouTube bu istemciyle açıldı; bundan sonra o kullanılacak");
                    set_preferred_client(client);
                    Self::cache_resolved(target, search, &found.1);
                    return Ok(found);
                }
                Err(e) if is_bot_block(&e) => warn!(client, "YouTube istemcisi de engellendi"),
                // Bu istemci bu videoyu açamadı; sıradaki denenir, hata sonda gösterilir.
                Err(e) => {
                    other_error.get_or_insert(e);
                }
            }
        }
        match other_error {
            Some(e) => Err(e),
            None => {
                ALL_BLOCKED_UNTIL.store(chrono::Utc::now().timestamp() + ALL_BLOCKED_PAUSE_SECS, std::sync::atomic::Ordering::Relaxed);
                Err(first_error)
            }
        }
    }

    async fn fetch_with(&self, target: &str, search: bool, mut args: Vec<String>) -> Result<(Input, songbird::input::AuxMetadata), String> {
        // Tekil şarkı ve aramalarda YouTube çalma listesi ayrıştırmasını atlayarak hızı artırır.
        if !args.iter().any(|a| a == "--no-playlist") {
            args.push("--no-playlist".to_owned());
        }
        let source = if search {
            YoutubeDl::new_search_ytdl_like(self.ytdlp, self.http.clone(), target.to_owned())
        } else {
            YoutubeDl::new_ytdl_like(self.ytdlp, self.http.clone(), target.to_owned())
        }
        .user_args(args.clone());
        prefetch::Prefetched::resolve(source, args, Some(self.http.clone())).await.map_err(|e| e.to_string())
    }

    /// Şarkı adıysa ve Spotify anahtarı varsa önce Spotify'da arar: kart, kapak ve bağlantı Spotify'ın
    /// (resmî ad ve sanatçı), ses aynı şarkının YouTube kaydından çalınır (Spotify sesi DRM'li; hiçbir bot
    /// doğrudan çalamaz). Spotify'da bulunamazsa ya da anahtar yoksa doğrudan YouTube'da aranır.
    pub async fn resolve_preferring_spotify(&self, query: &str, requester: UserId, guild_id: GuildId) -> Result<(Input, TrackInfo), String> {
        let trimmed = query.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            return self.resolve(trimmed, requester, guild_id).await;
        }
        match extra::spotify_search(&self.http, trimmed).await {
            Ok(Some(track)) => self.resolve_spotify_track(&track, requester, guild_id).await,
            Ok(None) => self.resolve(trimmed, requester, guild_id).await,
            Err(e) => {
                warn!(error = ?e, "Spotify araması başarısız, YouTube'da aranıyor");
                self.resolve(trimmed, requester, guild_id).await
            }
        }
    }

    /// Spotify'daki bir şarkıyı YouTube'da bulup çalınabilir hâle getirir; kartta Spotify bilgileri görünür.
    pub async fn resolve_spotify_track(&self, track: &extra::SpotifyTrack, requester: UserId, guild_id: GuildId) -> Result<(Input, TrackInfo), String> {
        let query = format!("{} - {}", track.artists, track.title);
        let (input, mut info) = self.resolve(&query, requester, guild_id).await?;
        info.title = if track.artists.is_empty() { track.title.clone() } else { query };
        info.spotify_url = track.url.clone();
        if track.cover.is_some() {
            info.thumbnail = track.cover.clone();
        }
        Ok((input, info))
    }

    /// Çalma listesindeki parçaları okur (en fazla `limit` tane). Hata mesajı kullanıcıya gösterilir.
    pub async fn playlist(&self, url: &str, requester: UserId, guild_id: GuildId, limit: usize) -> Result<Playlist, String> {
        if !(extra::is_media_site(url) && extra::is_public_url(url).await) {
            return Err("❌ Bu bağlantı okunamaz. Herkese açık bir YouTube / SoundCloud listesi ver.".to_owned());
        }
        let output = tokio::time::timeout(
            PLAYLIST_TIMEOUT,
            Command::new(self.ytdlp)
                .args(self.args())
                // Bir fazlası istenir: liste sınırdan uzun mu anlamak için.
                .args(["--flat-playlist", "-J", "--no-warnings", "--playlist-end", &(limit + 1).to_string()])
                .arg(url.trim())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| "❌ Çalma listesi çok uzun sürede okunamadı, tekrar dene.".to_owned())?
        .map_err(|e| {
            warn!(error = %e, "yt-dlp çalıştırılamadı");
            "❌ Müzik için gereken **yt-dlp** programı çalıştırılamadı.".to_owned()
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(url, error = %stderr.trim(), "Çalma listesi okunamadı");
            return Err("❌ Çalma listesi okunamadı. Bağlantının herkese açık bir liste olduğundan emin ol.".to_owned());
        }
        let json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|_| "❌ Çalma listesi bilgisi anlaşılamadı.".to_owned())?;
        let (title, mut entries, skipped) = parse_playlist(&json, limit + 1);
        let truncated = entries.len() > limit;
        entries.truncate(limit);

        let tracks = entries
            .into_iter()
            .map(|entry| {
                let input = prefetch::Prefetched::lazy(
                    entry.url.clone(),
                    YoutubeDl::new_ytdl_like(self.ytdlp, self.http.clone(), entry.url.clone()).user_args(self.args()),
                    self.args(),
                    Some(self.http.clone()),
                );
                let info = TrackInfo {
                    guild_id,
                    title: entry.title,
                    url: Some(entry.url),
                    duration: entry.duration,
                    thumbnail: None,
                    requester,
                    announce: false,
                    spotify_url: None,
                };
                (input, info)
            })
            .collect();
        Ok(Playlist { title, tracks, skipped, truncated })
    }

    /// YouTube'da arama yapar (`/ara youtube`). Hata mesajı kullanıcıya gösterilir.
    pub async fn search_youtube(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        // Sorgu yt-dlp'ye tek argüman olarak gider; baştaki "-" seçenek sanılmasın diye önek hep var.
        let target = format!("ytsearch{limit}:{}", query.trim());
        let output = tokio::time::timeout(
            SEARCH_TIMEOUT,
            Command::new(self.ytdlp)
                .args(self.args())
                .args(["--flat-playlist", "-J", "--no-warnings"])
                .arg(target)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| "❌ Arama çok uzun sürdü, tekrar dene.".to_owned())?
        .map_err(|e| {
            warn!(error = %e, "yt-dlp çalıştırılamadı");
            "❌ Arama için gereken **yt-dlp** programı çalıştırılamadı.".to_owned()
        })?;
        if !output.status.success() {
            warn!(error = %String::from_utf8_lossy(&output.stderr).trim(), "YouTube araması yapılamadı");
            return Err("❌ YouTube araması şu an yapılamıyor.".to_owned());
        }
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| "❌ Arama sonucu anlaşılamadı.".to_owned())?;
        Ok(parse_search(&json, limit))
    }

    /// Botun ses kanalına bağlı olduğundan emin olur; henüz bağlı değilse anında bağlanır ve sağırlaştırır.
    pub async fn ensure_connected(
        &self,
        http: &Arc<Http>,
        guild_id: GuildId,
        voice_channel: ChannelId,
    ) -> anyhow::Result<Arc<Mutex<Call>>> {
        let first_join = self.songbird.get(guild_id).is_none();
        let call = self.songbird.join(guild_id, voice_channel).await?;
        if first_join {
            self.session_started.insert(guild_id, std::time::Instant::now());
            let mut handler = call.lock().await;
            if let Err(e) = handler.deafen(true).await {
                warn!(guild = %guild_id, error = %e, "Bot kendini sağırlaştıramadı");
            }
            handler.add_global_event(
                Event::Periodic(Duration::from_secs(60), None),
                IdleLeaver::new(Arc::downgrade(&self.0), Arc::clone(http), guild_id),
            );
            info!(guild = %guild_id, channel = %voice_channel, "Ses kanalına katılındı");
        }
        Ok(call)
    }

    /// Gerekirse ses kanalına katılır ve parçayı kuyruğa ekler.
    pub async fn enqueue(
        &self,
        http: Arc<Http>,
        guild_id: GuildId,
        voice_channel: ChannelId,
        text_channel: ChannelId,
        input: Input,
        info: TrackInfo,
    ) -> anyhow::Result<Enqueued> {
        self.enqueue_many(http, guild_id, voice_channel, text_channel, vec![(input, info)])
            .await
    }

    /// Gerekirse ses kanalına (bir kez) katılır ve parçaları sırayla kuyruğa ekler.
    pub async fn enqueue_many(
        &self,
        http: Arc<Http>,
        guild_id: GuildId,
        voice_channel: ChannelId,
        text_channel: ChannelId,
        items: Vec<(Input, TrackInfo)>,
    ) -> anyhow::Result<Enqueued> {
        self.text_channels.insert(guild_id, text_channel);

        let call = self.ensure_connected(&http, guild_id, voice_channel).await?;
        let mut handler = call.lock().await;

        let position = handler.queue().len();
        Shared::push(&self.0, &mut handler, &http, items);
        drop(handler);

        Ok(Enqueued { position })
    }

    /// Müziği durdurur, kuyruğu temizler ve kanaldan ayrılır.
    pub async fn stop_and_leave(&self, guild_id: GuildId) {
        self.session_started.remove(&guild_id);
        self.autoplay_tasks.remove(&guild_id);
        self.empty_channels.remove(&guild_id);
        self.card_tasks.remove(&guild_id);
        self.position_offsets.remove(&guild_id);
        if let Some(call) = self.songbird.get(guild_id) {
            call.lock().await.queue().stop();
        }
        if let Err(e) = self.songbird.remove(guild_id).await {
            warn!(guild = %guild_id, error = %e, "Ses kanalından ayrılırken hata");
        }
        self.tracks.retain(|_, t| t.guild_id != guild_id);
        let _ = cache::prune_cache();
    }

    /// Ses kanalı değişikliklerinde: bot yalnız kaldıysa (7/24 kapalıysa) veya kanaldan atıldıysa ayrılır.
    pub async fn on_voice_state_update(&self, ctx: &serenity::Context, guild_id: GuildId) {
        let Some(call) = self.songbird.get(guild_id) else {
            return;
        };
        let bot_channel = call.lock().await.current_channel();
        let bot_id = ctx.cache.current_user().id;

        let humans_left = match bot_channel {
            Some(channel) => {
                let channel = ChannelId::new(channel.0.get());
                ctx.cache.guild(guild_id).map_or(1, |guild| {
                    guild
                        .voice_states
                        .values()
                        .filter(|vs| vs.channel_id == Some(channel) && vs.user_id != bot_id)
                        .filter(|vs| !vs.member.as_ref().is_some_and(|m| m.user.bot))
                        .count()
                })
            }
            // Bot kanaldan atıldı / bağlantı koptu.
            None => 0,
        };

        // 7/24 modunda kanal boşalsa da kalınır (bot kanaldan atıldıysa yine temizlenir).
        if humans_left == 0 && bot_channel.is_some() && self.settings(guild_id).always_on {
            return;
        }
        if humans_left == 0 {
            let settings = self.settings(guild_id);
            if settings.idle_disconnect_secs == 0 {
                info!(guild = %guild_id, "Ses kanalında kimse kalmadı, anında ayrılınıyor");
                self.stop_and_leave(guild_id).await;
                if let (Some(channel), true) = (self.text_channels.get(&guild_id).map(|c| *c), bot_channel.is_some()) {
                    let _ = channel
                        .say(&ctx.http, "👋 Ses kanalında kimse kalmadı, ayrıldım.")
                        .await;
                }
            } else {
                self.empty_channels.entry(guild_id).or_insert_with(std::time::Instant::now);
                info!(guild = %guild_id, wait = settings.idle_disconnect_secs, "Ses kanalında kimse kalmadı, boşta bekleme devrede");
            }
        } else {
            self.empty_channels.remove(&guild_id);
        }
    }
}

impl Shared {
    /// Çalan şarkı ve kartta gösterilecek anlık durum (kapak adresi yoksa burada tamamlanır).
    pub async fn card_state(&self, guild_id: GuildId) -> Option<(TrackInfo, card::CardState)> {
        let call = self.songbird.get(guild_id)?;
        let queue = call.lock().await.queue().clone();
        let current = queue.current()?;
        let mut info = self.tracks.get(&current.uuid()).map(|t| t.value().clone())?;
        self.ensure_cover(current.uuid(), &mut info).await;
        let state = current.get_info().await.ok()?;
        let next = queue
            .current_queue()
            .get(1)
            .and_then(|h| self.tracks.get(&h.uuid()).map(|t| t.title.clone()))
            .map(|title| card::split_title(&title))
            .map(|(artist, song)| artist.map_or_else(|| song.clone(), |a| format!("{a} - {song}")));
        let offset = self.position_offset_of(guild_id);
        let raw_pos = state.position + offset;
        let effective_position = match info.duration {
            Some(total) => raw_pos.min(total),
            None => raw_pos,
        };
        Some((
            info,
            card::CardState {
                volume: self.volumes.get(&guild_id).map_or(DEFAULT_VOLUME, |v| *v),
                paused: matches!(state.playing, songbird::tracks::PlayMode::Pause),
                looping: state.loops != songbird::tracks::LoopState::default(),
                upcoming: queue.len().saturating_sub(1),
                position: effective_position,
                next,
            },
        ))
    }

    /// Kapak adresi yoksa bulur: YouTube'da video kimliğinden, Spotify'da Spotify'ın oEmbed
    /// servisinden. Sonuç parça bilgisine yazılır (bir kez denenir; bulunamazsa boş kalır).
    pub(crate) async fn ensure_cover(&self, id: Uuid, info: &mut TrackInfo) {
        if info.thumbnail.is_some() {
            return;
        }
        let cover = match (&info.spotify_url, info.url.as_deref().and_then(extra::youtube_video_id)) {
            (_, Some(video)) => Some(extra::youtube_cover(&video)),
            (Some(spotify), None) => extra::spotify_cover(&self.http, spotify).await,
            (None, None) => None,
        };
        // Boş dizgi = denendi ama yok (her güncellemede tekrar sorulmasın).
        info.thumbnail = Some(cover.unwrap_or_default());
        if let Some(mut stored) = self.tracks.get_mut(&id) {
            stored.thumbnail = info.thumbnail.clone();
        }
    }

    /// Kapak görselini indirip çözümler (son kapaklar bellekte tutulur).
    async fn cover_image(&self, url: &str) -> Option<Arc<image::RgbaImage>> {
        if !url.starts_with("http") {
            return None;
        }
        if let Some(cached) = self.covers.get(url) {
            return cached.clone();
        }
        let bytes = async {
            self.http.get(url).timeout(Duration::from_secs(6)).send().await.ok()?.bytes().await.ok()
        }
        .await;
        let decoded = match bytes {
            Some(bytes) => tokio::task::spawn_blocking(move || render::decode_cover(&bytes)).await.ok().flatten().map(Arc::new),
            None => None,
        };
        if self.covers.len() > 40 {
            self.covers.clear();
        }
        self.covers.insert(url.to_owned(), decoded.clone());
        decoded
    }

    /// İsteyen kişinin görünen adı (önbellekli).
    async fn requester_name(&self, http: &Http, user: UserId) -> String {
        if let Some(name) = self.names.get(&user) {
            return name.clone();
        }
        let name = match user.to_user(http).await {
            Ok(u) => u.global_name.unwrap_or(u.name),
            Err(_) => "?".to_owned(),
        };
        self.names.insert(user, name.clone());
        name
    }

    async fn guild_name(&self, http: &Http, guild_id: GuildId) -> String {
        if let Some(name) = self.guild_names.get(&guild_id) {
            return name.clone();
        }
        let name = match guild_id.to_partial_guild(http).await {
            Ok(g) => g.name,
            Err(_) => "AxBot".to_owned(),
        };
        self.guild_names.insert(guild_id, name.clone());
        name
    }

    /// Kartı çizer: (JPEG, üst satır, butonlar).
    pub(crate) async fn render_card(&self, http: &Http, guild_id: GuildId) -> Option<(Vec<u8>, String, Vec<serenity::CreateActionRow>)> {
        let (info, state) = self.card_state(guild_id).await?;
        let cover = match info.thumbnail.as_deref() {
            Some(url) => self.cover_image(url).await,
            None => None,
        };
        let source = card::source_of(&info);
        let (artist, title) = card::split_title(&info.title);
        let data = render::CardData {
            title,
            artist,
            source: source.name.to_owned(),
            accent: source.accent,
            position: state.position,
            duration: info.duration,
            paused: state.paused,
            looping: state.looping,
            volume_percent: (state.volume * 100.0).round() as u32,
            upcoming: state.upcoming,
            requester: self.requester_name(http, info.requester).await,
            next: state.next.clone(),
            brand: self.guild_name(http, guild_id).await,
        };
        let _permit = crate::cards::RENDER_SEMAPHORE.acquire().await.ok();
        let jpeg = tokio::task::spawn_blocking(move || render::render(&data, cover.as_deref()))
            .await
            .ok()?
            .map_err(|e| warn!(error = ?e, "Şarkı kartı çizilemedi"))
            .ok()?;
        Some((jpeg, card::content(&info, &state), card::buttons(&info, &state)))
    }

    /// Yeni şarkı başladı: kartı metin kanalına gönderir (olay dinleyicisinden ayrı görevde çağrılır).
    /// Kartı metin kanalına normal mesaj olarak gönderir (önceki kart silinir). Gönderildiyse `true`.
    pub(crate) async fn post_card(this: Arc<Self>, http: Arc<Http>, guild_id: GuildId) -> bool {
        // Önceki şarkının kartı hemen kaldırılır: yoksa canlı güncelleme onu da yeni şarkıyla
        // yeniden çizer ve sohbette aynı şarkının iki kartı görünür.
        if let Some((_, (old_channel, old_message))) = this.cards.remove(&guild_id) {
            delete_card(&http, old_channel, old_message).await;
        }
        // Parça bilgisinin oturması için kısa bekleme (sıradaki şarkı yeni başladı).
        tokio::time::sleep(Duration::from_millis(300)).await;
        let Some(channel) = this.text_channels.get(&guild_id).map(|c| *c) else {
            return false;
        };
        let Some((jpeg, content, buttons)) = this.render_card(&http, guild_id).await else {
            return false;
        };
        let message = serenity::CreateMessage::new()
            .content(content)
            // Şarkı adları yükleyenin elinde: içindeki @everyone / rol etiketleri kimseyi çağırmasın.
            .allowed_mentions(serenity::CreateAllowedMentions::new())
            .add_file(serenity::CreateAttachment::bytes(jpeg, CARD_FILE))
            .components(buttons);
        match channel.send_message(&http, message).await {
            Ok(sent) => {
                Self::replace_card(&this, &http, guild_id, channel, sent.id).await;
                true
            }
            Err(e) => {
                warn!(channel = %channel, error = %e, "Şarkı kartı gönderilemedi");
                false
            }
        }
    }

    /// Sıradaki parçayı arka planda önceden çözümler ve yerel diske indirir (şarkı geçişini anında ve sıfır gecikmeli yapar).
    pub(crate) async fn prefetch_next(this: Arc<Self>, guild_id: GuildId) {
        let Some(call) = this.songbird.get(guild_id) else { return; };
        let next_track = {
            let handler = call.lock().await;
            let queue = handler.queue().current_queue();
            queue.get(1).and_then(|h| this.tracks.get(&h.uuid()).map(|t| (t.url.clone(), t.duration)))
        };
        let Some((Some(target), duration)) = next_track else { return; };
        if !target.is_empty() {
            if cache::get_cached_track(&target).is_some() {
                return;
            }
            let _ = Music(Arc::clone(&this)).fetch(&target, false).await;
            let _ = cache::download_to_cache(
                Some(&this.http),
                this.ytdlp,
                &this.args(),
                &target,
                None,
                None,
                duration,
            )
            .await;
        }
    }

    /// Kartı hemen yeniden çizer (buton/ses değişikliğinden sonra).
    pub(crate) async fn refresh_card_now(&self, http: &Http, guild_id: GuildId) {
        let Some((channel, message)) = self.cards.get(&guild_id).map(|c| *c) else {
            return;
        };
        if let Some((jpeg, content, buttons)) = self.render_card(http, guild_id).await {
            let edit = serenity::EditMessage::new()
                .content(content)
                .allowed_mentions(serenity::CreateAllowedMentions::new())
                .new_attachment(serenity::CreateAttachment::bytes(jpeg, CARD_FILE))
                .components(buttons);
            if let Err(e) = channel.edit_message(http, message, edit).await {
                tracing::debug!(error = %e, "Şarkı kartı güncellenemedi");
            }
        }
    }

    pub(crate) async fn replace_card(this: &Arc<Self>, http: &Arc<Http>, guild_id: GuildId, channel: ChannelId, message: serenity::MessageId) {
        if let Some((old_channel, old_message)) = this.cards.insert(guild_id, (channel, message)) {
            if old_message != message {
                delete_card(http, old_channel, old_message).await;
            }
        }
        // Sunucu başına tek güncelleme görevi.
        if this.card_tasks.insert(guild_id) {
            tokio::spawn(Self::live_card(Arc::clone(this), Arc::clone(http), guild_id));
        }
    }

    /// Kartı şarkıyla birlikte günceller (ilerleme çubuğu, duraklatma, sıra). Müzik bitince
    /// son kartın butonlarını kaldırıp kapanır.
    async fn live_card(this: Arc<Self>, http: Arc<Http>, guild_id: GuildId) {
        let mut last_key = None;
        loop {
            tokio::time::sleep(CARD_REFRESH).await;
            let Some((channel, message)) = this.cards.get(&guild_id).map(|c| *c) else {
                // Yeni şarkının kartı gönderiliyor olabilir; müzik tamamen bittiyse görev kapanır.
                if this.songbird.get(guild_id).is_none() {
                    break;
                }
                continue;
            };
            let Some(call) = this.songbird.get(guild_id) else {
                delete_card(&http, channel, message).await;
                this.cards.remove(&guild_id);
                break;
            };
            let in_vc = call.lock().await.current_channel().is_some();
            if !in_vc {
                delete_card(&http, channel, message).await;
                this.cards.remove(&guild_id);
                break;
            }
            let Some((info, state)) = this.card_state(guild_id).await else {
                // Müzik bitti: son kart da sohbette kalmasın.
                delete_card(&http, channel, message).await;
                this.cards.remove(&guild_id);
                break;
            };
            // Duraklatılmışsa / değişen bir şey yoksa Discord'a istek atılmaz.
            let key = (message, info.title.clone(), state.position.as_secs() / 12, state.paused, state.looping, state.upcoming);
            if last_key.as_ref() == Some(&key) {
                continue;
            }
            last_key = Some(key);
            let Some((jpeg, content, buttons)) = this.render_card(&http, guild_id).await else {
                continue;
            };
            let edit = serenity::EditMessage::new()
                .content(content)
                .allowed_mentions(serenity::CreateAllowedMentions::new())
                .new_attachment(serenity::CreateAttachment::bytes(jpeg, CARD_FILE))
                .components(buttons);
            if let Err(e) = channel.edit_message(&http, message, edit).await {
                tracing::debug!(error = %e, "Şarkı kartı güncellenemedi (silinmiş olabilir)");
                this.cards.remove(&guild_id);
                break;
            }
        }
        this.card_tasks.remove(&guild_id);
    }

    /// Parçaları kuyruğa ekler ve olay dinleyicilerini bağlar. `Call` kilidi çağıranda tutulur.
    fn push(this: &Arc<Self>, handler: &mut Call, http: &Arc<Http>, items: Vec<(Input, TrackInfo)>) {
        let Some(guild_id) = items.first().map(|(_, info)| info.guild_id) else {
            return;
        };
        let volume = this.volume_of(guild_id);
        for (input, mut info) in items {
            let input = filters::wrap(input, guild_id, &this.filters);
            // Sıraya eklenenler (veya otomatik çalınanlar) başlayınca duyurulur.
            info.announce = info.announce || !handler.queue().is_empty();
            // `Call::enqueue` her parça için süreyi öğrenmek üzere yt-dlp çalıştırır (parça başına
            // birkaç saniye; 100 şarkılık listede dakikalarca kilit). Süre zaten bilindiği için
            // doğrudan eklenir; sonraki parça bitişe 5 sn kala hazırlanır.
            let preload = info.duration.map(|d| d.saturating_sub(Duration::from_secs(5)));
            let queue = handler.queue().clone();
            let handle = queue.add_with_preload(Track::new(input).volume(volume), handler, preload);
            this.tracks.insert(handle.uuid(), info);
            for (event, notice) in [
                (TrackEvent::Play, TrackNotice::Started),
                (TrackEvent::End, TrackNotice::Ended),
                (TrackEvent::Error, TrackNotice::Failed),
            ] {
                let notifier = TrackNotifier {
                    http: Arc::clone(http),
                    music: Arc::downgrade(this),
                    notice,
                };
                if let Err(e) = handle.add_event(Event::Track(event), notifier) {
                    warn!(error = %e, "Parça olayı eklenemedi");
                }
            }
        }
    }

    /// Çalan YouTube şarkısını geçmişe ekler.
    fn remember(&self, guild_id: GuildId, video_id: String) {
        let mut history = self.history.entry(guild_id).or_default();
        history.retain(|id| *id != video_id);
        history.push_back(video_id);
        while history.len() > HISTORY_LEN {
            history.pop_front();
        }
    }

    /// Kuyruk bittiyse son şarkıya benzer bir şarkıyı (YouTube Mix) sıraya ekler.
    async fn autoplay_after(this: Arc<Self>, http: Arc<Http>, finished: TrackInfo) {
        let guild_id = finished.guild_id;
        if !this.autoplay_tasks.insert(guild_id) {
            return;
        }
        struct AutoplayGuard<'a>(&'a dashmap::DashSet<GuildId>, GuildId);
        impl Drop for AutoplayGuard<'_> {
            fn drop(&mut self) {
                self.0.remove(&self.1);
            }
        }
        let _guard = AutoplayGuard(&this.autoplay_tasks, guild_id);

        // Bitişten hemen sonra sıradaki şarkı başlamış olabilir; kısa bekleyip bakılır.
        tokio::time::sleep(Duration::from_millis(800)).await;
        let Some(call) = this.songbird.get(guild_id) else {
            return;
        };
        if !call.lock().await.queue().is_empty() {
            return;
        }
        let Some(seed) = finished.url.as_deref().and_then(extra::youtube_video_id) else {
            return;
        };
        let recent: Vec<String> = this.history.get(&guild_id).map(|h| h.iter().cloned().collect()).unwrap_or_default();
        let next = match extra::related_track(this.ytdlp, &this.args(), &seed, &recent).await {
            Ok(Some(next)) => next,
            Ok(None) => return,
            Err(e) => {
                warn!(guild = %guild_id, error = ?e, "Otomatik çalma için şarkı bulunamadı");
                return;
            }
        };
        let input = prefetch::Prefetched::lazy(
            next.url.clone(),
            YoutubeDl::new_ytdl_like(this.ytdlp, this.http.clone(), next.url.clone()).user_args(this.args()),
            this.args(),
            Some(this.http.clone()),
        );
        let info = TrackInfo {
            guild_id,
            title: next.title,
            url: Some(next.url),
            duration: next.duration,
            thumbnail: None,
            requester: finished.requester,
            announce: true,
            spotify_url: None,
        };
        let mut handler = call.lock().await;
        if !handler.queue().is_empty() || handler.current_channel().is_none() {
            return;
        }
        Shared::push(&this, &mut handler, &http, vec![(input, info)]);
        info!(guild = %guild_id, "Otomatik çalma: benzer şarkı eklendi");
    }
}

/// Şarkı kartını siler; silinemezse (izin yok vb.) loglanır.
async fn delete_card(http: &Http, channel: ChannelId, message: serenity::MessageId) {
    if let Err(e) = channel.delete_message(http, message).await {
        // Kart zaten silinmişse (Unknown Message) sorun değil.
        if !e.to_string().contains("Unknown Message") {
            warn!(channel = %channel, error = %e, "Eski şarkı kartı silinemedi");
        }
    }
}

/// `tools/<ad>.exe` (Windows) veya `tools/<ad>` varsa tam yolunu döner.
fn find_tool(dir: &Path, name: &str) -> Option<String> {
    let candidates = [dir.join(format!("{name}.exe")), dir.join(name)];
    candidates
        .iter()
        .find(|p| p.is_file())
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p: PathBuf| {
            // Windows'un `\\?\` önekini kaldır; bazı programlar bu biçimi anlamaz.
            let s = p.to_string_lossy().into_owned();
            s.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(s)
        })
}

/// `/ara youtube` sonucu.
pub struct SearchResult {
    pub url: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration: Option<Duration>,
}

/// yt-dlp `ytsearchN: --flat-playlist -J` çıktısı → sonuçlar.
fn parse_search(json: &serde_json::Value, limit: usize) -> Vec<SearchResult> {
    let (_, entries, _) = parse_playlist(json, limit);
    let channels: std::collections::HashMap<&str, &str> = json
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| {
                    let url = e.get("webpage_url").or_else(|| e.get("url")).and_then(serde_json::Value::as_str)?;
                    let channel = e.get("channel").or_else(|| e.get("uploader")).and_then(serde_json::Value::as_str)?;
                    Some((url, channel))
                })
                .collect()
        })
        .unwrap_or_default();
    entries
        .into_iter()
        .map(|entry| SearchResult {
            channel: channels.get(entry.url.as_str()).map(|c| (*c).to_owned()),
            url: entry.url,
            title: entry.title,
            duration: entry.duration,
        })
        .collect()
}

struct PlaylistEntry {
    url: String,
    title: String,
    duration: Option<Duration>,
}

/// yt-dlp `--flat-playlist -J` çıktısı → (liste adı, parçalar, atlanan sayısı).
fn parse_playlist(json: &serde_json::Value, limit: usize) -> (String, Vec<PlaylistEntry>, usize) {
    let title = json
        .get("title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Çalma listesi")
        .to_owned();
    let mut skipped = 0;
    let entries = json
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| {
                    let entry_title = e.get("title").and_then(serde_json::Value::as_str).unwrap_or_default();
                    let url = e
                        .get("webpage_url")
                        .or_else(|| e.get("url"))
                        .and_then(serde_json::Value::as_str)
                        .filter(|u| u.starts_with("http"));
                    // Özel/silinmiş videolar çalınamaz.
                    let unavailable = entry_title.starts_with("[Private") || entry_title.starts_with("[Deleted");
                    match url {
                        Some(url) if !unavailable => Some(PlaylistEntry {
                            url: url.to_owned(),
                            title: if entry_title.is_empty() { "Bilinmeyen parça".to_owned() } else { entry_title.to_owned() },
                            duration: e
                                .get("duration")
                                .and_then(serde_json::Value::as_f64)
                                .filter(|d| d.is_finite() && *d > 0.0)
                                .map(Duration::from_secs_f64),
                        }),
                        _ => {
                            skipped += 1;
                            None
                        }
                    }
                })
                .take(limit)
                .collect()
        })
        .unwrap_or_default();
    (title, entries, skipped)
}

/// "3:45" veya "1:02:03".
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Kuyrukta `from`'daki parçayı `to`'ya taşır (0 = çalan parça; ona dokunulmaz). Taşınan parçayı döner.
fn reorder<T>(queue: &mut VecDeque<T>, from: usize, to: usize) -> Option<&T> {
    if from == 0 || to == 0 || from >= queue.len() || to >= queue.len() {
        return None;
    }
    let item = queue.remove(from)?;
    queue.insert(to, item);
    queue.get(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reorders_queue_without_touching_current() {
        let mut q: VecDeque<char> = "çabcd".chars().collect();
        assert_eq!(reorder(&mut q, 4, 1), Some(&'d'));
        assert_eq!(q.iter().collect::<String>(), "çdabc");
        assert_eq!(reorder(&mut q, 1, 4), Some(&'d'));
        assert_eq!(q.iter().collect::<String>(), "çabcd");
        // Çalan parça (0) taşınamaz, yerine de bir şey konamaz; sınır dışı numaralar reddedilir.
        assert_eq!(reorder(&mut q, 0, 2), None);
        assert_eq!(reorder(&mut q, 2, 0), None);
        assert_eq!(reorder(&mut q, 5, 1), None);
        assert_eq!(reorder(&mut q, 1, 5), None);
        assert_eq!(q.iter().collect::<String>(), "çabcd");
        assert_eq!(reorder(&mut q, 2, 2), Some(&'b'));
    }

    #[test]
    fn detects_playlist_links() {
        assert!(is_playlist_url("https://www.youtube.com/playlist?list=PL123"));
        assert!(is_playlist_url("https://youtube.com/watch?v=abc&list=PL123"));
        assert!(is_playlist_url("https://soundcloud.com/artist/sets/album"));
        assert!(!is_playlist_url("https://www.youtube.com/watch?v=abc"));
        assert!(!is_playlist_url("tarkan list= şarkı"));
    }

    #[test]
    fn parses_flat_playlist() {
        let json = serde_json::json!({
            "title": "Favoriler",
            "entries": [
                { "url": "https://www.youtube.com/watch?v=a", "title": "Bir", "duration": 200.0 },
                { "url": "https://www.youtube.com/watch?v=b", "title": "[Private video]", "duration": null },
                { "url": "https://www.youtube.com/watch?v=c", "title": "Üç", "duration": null },
                { "url": "https://www.youtube.com/watch?v=d", "title": "Dört" }
            ]
        });
        let (title, entries, skipped) = parse_playlist(&json, 2);
        assert_eq!(title, "Favoriler");
        assert_eq!(skipped, 1);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Bir");
        assert_eq!(entries[0].duration, Some(Duration::from_secs(200)));
        assert_eq!(entries[1].title, "Üç");
        assert_eq!(entries[1].duration, None);
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(Duration::from_secs(5)), "0:05");
        assert_eq!(format_duration(Duration::from_secs(225)), "3:45");
        assert_eq!(format_duration(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn suppressed_ends_and_position_offset_logic() {
        let suppressed = dashmap::DashSet::new();
        let track_uuid = Uuid::new_v4();

        // Parça seek_to tarafından durdurulunca suppressed listesine eklenir
        suppressed.insert(track_uuid);
        assert!(suppressed.contains(&track_uuid));

        // TrackEvent::End geldiğinde tek sefer kontrol edilip tüketilir
        assert!(suppressed.remove(&track_uuid).is_some());
        assert!(!suppressed.contains(&track_uuid));
        assert!(suppressed.remove(&track_uuid).is_none());

        // Konum ofseti hesabı
        let offsets = DashMap::new();
        let guild_id = GuildId::new(42);
        let seek_target = Duration::from_secs(90);
        offsets.insert(guild_id, seek_target);

        let stream_position = Duration::from_secs(12);
        let offset = offsets.get(&guild_id).map(|o| *o).unwrap_or(Duration::ZERO);
        let total = Duration::from_secs(180);

        let effective = (stream_position + offset).min(total);
        assert_eq!(effective, Duration::from_secs(102));

        // Süre taşması durumunda clamp kontrolü
        let large_stream_pos = Duration::from_secs(100);
        let clamped = (large_stream_pos + offset).min(total);
        assert_eq!(clamped, Duration::from_secs(180));
    }
}

/// Gerçek YouTube araması + ses çözme testi (Discord'a bağlanmaz). Elle:
/// `cargo test live_resolves_and_decodes_youtube -- --ignored --nocapture`
#[cfg(test)]
mod live_tests {
    use super::*;
    use songbird::input::codecs::{get_codec_registry, get_probe};

    #[tokio::test]
    #[ignore = "gerçek yt-dlp ve YouTube'a istek atar"]
    async fn live_resolves_and_decodes_youtube() -> anyhow::Result<()> {
        let music = Music::new(Songbird::serenity(), reqwest::Client::new());
        let version = music.ytdlp_version().await;
        println!("yt-dlp: {version:?} · program: {} · ek argümanlar: {:?}", music.ytdlp, music.ytdlp_args);

        let started = std::time::Instant::now();
        let (input, info) = music
            .resolve("Tarkan Şımarık", UserId::new(1), GuildId::new(1))
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        println!("çözüldü ({:.1} sn): {} · {:?} · {:?}", started.elapsed().as_secs_f64(), info.title, info.duration, info.url);

        let playable = input
            .make_playable_async(get_codec_registry(), get_probe())
            .await?;
        assert!(playable.is_playable(), "ses çözülemedi");
        println!("çalınabilir ses akışı hazır ({:.1} sn)", started.elapsed().as_secs_f64());
        Ok(())
    }

    /// Çalma listesini kuyruğa eklemek beklemeden olmalı (eski `enqueue` parça başına yt-dlp çalıştırıyordu).
    /// `cargo test live_playlist_enqueue_is_instant -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "gerçek yt-dlp ve YouTube'a istek atar"]
    async fn live_playlist_enqueue_is_instant() -> anyhow::Result<()> {
        use songbird::tracks::TrackQueue;
        use songbird::{Config, Driver};

        let music = Music::new(Songbird::serenity(), reqwest::Client::new());
        let search = "https://www.youtube.com/results?search_query=t%C3%BCrk%C3%A7e+pop&sp=EgIQAw%253D%253D";
        let found = music.playlist(search, UserId::new(1), GuildId::new(1), 1).await.map_err(|e| anyhow::anyhow!(e))?;
        let url = found.tracks.first().and_then(|(_, i)| i.url.clone()).ok_or_else(|| anyhow::anyhow!("liste yok"))?;

        // Eski yöntem: 3 parça.
        let old = music.playlist(&url, UserId::new(1), GuildId::new(1), 3).await.map_err(|e| anyhow::anyhow!(e))?;
        let mut driver = Driver::new(Config::default());
        let queue = TrackQueue::new();
        let started = std::time::Instant::now();
        for (input, _) in old.tracks {
            queue.add(Track::new(input), &mut driver).await;
        }
        let old_secs = started.elapsed().as_secs_f64();
        queue.stop();

        // Yeni yöntem: 25 parça.
        let new = music.playlist(&url, UserId::new(1), GuildId::new(1), 25).await.map_err(|e| anyhow::anyhow!(e))?;
        let count = new.tracks.len();
        let queue = TrackQueue::new();
        let started = std::time::Instant::now();
        for (input, info) in new.tracks {
            let preload = info.duration.map(|d| d.saturating_sub(Duration::from_secs(5)));
            queue.add_with_preload(Track::new(input), &mut driver, preload);
        }
        let new_secs = started.elapsed().as_secs_f64();
        queue.stop();
        println!("eski yöntem: 3 parça {old_secs:.1} sn · yeni yöntem: {count} parça {new_secs:.3} sn");
        assert!(new_secs < 1.0, "liste ekleme yavaş: {new_secs} sn");
        Ok(())
    }

    /// `cargo test live_reads_youtube_playlist -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "gerçek yt-dlp ve YouTube'a istek atar"]
    async fn live_reads_youtube_playlist() -> anyhow::Result<()> {
        let music = Music::new(Songbird::serenity(), reqwest::Client::new());
        // Sabit bir liste silinebilir; bu yüzden aramayla herkese açık bir liste bulunur.
        let search = "https://www.youtube.com/results?search_query=t%C3%BCrk%C3%A7e+pop&sp=EgIQAw%253D%253D";
        let found = music.playlist(search, UserId::new(1), GuildId::new(1), 1).await.map_err(|e| anyhow::anyhow!(e))?;
        let url = found
            .tracks
            .first()
            .and_then(|(_, info)| info.url.clone())
            .ok_or_else(|| anyhow::anyhow!("çalma listesi bulunamadı"))?;
        assert!(is_playlist_url(&url), "arama liste döndürmedi: {url}");

        let started = std::time::Instant::now();
        let playlist = music.playlist(&url, UserId::new(1), GuildId::new(1), 5).await.map_err(|e| anyhow::anyhow!(e))?;
        println!(
            "{} → {} şarkı (atlanan {}, kesildi {}) {:.1} sn",
            playlist.title,
            playlist.tracks.len(),
            playlist.skipped,
            playlist.truncated,
            started.elapsed().as_secs_f64()
        );
        for (_, info) in &playlist.tracks {
            println!("  • {} {:?}", info.title, info.duration);
        }
        assert!(!playlist.tracks.is_empty() && playlist.tracks.len() <= 5);

        let (input, _) = playlist.tracks.into_iter().next().ok_or_else(|| anyhow::anyhow!("boş liste"))?;
        let playable = input.make_playable_async(get_codec_registry(), get_probe()).await?;
        assert!(playable.is_playable(), "listedeki ilk şarkı çözülemedi");
        println!("ilk şarkı çalınabilir ({:.1} sn)", started.elapsed().as_secs_f64());
        Ok(())
    }

    #[test]
    fn test_music_settings_default_values_and_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let defaults = MusicSettings::default();
        assert!(!defaults.always_on);
        assert!(!defaults.autoplay);
        assert_eq!(defaults.idle_disconnect_secs, 300);
        assert_eq!(defaults.max_session_mins, 0);
        assert_eq!(defaults.default_volume, 60);
        assert_eq!(defaults.max_length_secs, 0);
        assert_eq!(defaults.dj_role, None);

        let custom = MusicSettings {
            always_on: true,
            autoplay: true,
            idle_disconnect_secs: 120,
            max_session_mins: 60,
            default_volume: 80,
            max_length_secs: 600,
            dj_role: Some(999988887777),
        };
        let json = serde_json::to_string(&custom)?;
        let parsed: MusicSettings = serde_json::from_str(&json)?;
        assert_eq!(parsed, custom);
        Ok(())
    }

    #[test]
    fn test_music_settings_backward_compatibility_with_old_json() -> Result<(), Box<dyn std::error::Error>> {
        let old_extras_json = r#"{"dj_role": 123456789}"#;
        let parsed: MusicSettings = serde_json::from_str(old_extras_json)?;
        assert_eq!(parsed.dj_role, Some(123456789));
        assert_eq!(parsed.idle_disconnect_secs, 300);
        assert_eq!(parsed.default_volume, 60);
        assert!(!parsed.always_on);
        assert!(!parsed.autoplay);

        let old_bool_json = r#"{"always_on": true, "autoplay": false}"#;
        let parsed2: MusicSettings = serde_json::from_str(old_bool_json)?;
        assert!(parsed2.always_on);
        assert!(!parsed2.autoplay);
        assert_eq!(parsed2.idle_disconnect_secs, 300);
        assert_eq!(parsed2.dj_role, None);
        Ok(())
    }

    #[test]
    fn test_can_manage_dj_rules() {
        let music = Music::new(Songbird::serenity(), reqwest::Client::new());
        let cache = serenity::Cache::new();
        let guild_id = GuildId::new(100);
        let user1 = UserId::new(1);
        let user2 = UserId::new(2);
        let channel = ChannelId::new(50);

        // Default settings: dj_role is None -> everyone can manage
        assert!(music.can_manage(&cache, guild_id, user1, channel, None));
        assert!(music.can_manage(&cache, guild_id, user2, channel, Some(user1)));

        // Set DJ role
        let settings = MusicSettings {
            dj_role: Some(999),
            ..Default::default()
        };
        music.settings.insert(guild_id, settings);

        // Requester can always manage their own song even if DJ role is set
        assert!(music.can_manage(&cache, guild_id, user1, channel, Some(user1)));
    }

    #[test]
    fn test_volume_of_and_default_volume() {
        let music = Music::new(Songbird::serenity(), reqwest::Client::new());
        let guild_id = GuildId::new(42);

        // Varsayılan: 60 -> 0.60
        let v0 = music.volume_of(guild_id);
        assert!((v0 - 0.60).abs() < 0.001);

        // Sunucu ayarlarında default_volume = 80 -> 0.80
        music.settings.insert(guild_id, MusicSettings {
            default_volume: 80,
            ..Default::default()
        });
        let v1 = music.volume_of(guild_id);
        assert!((v1 - 0.80).abs() < 0.001);

        // Komutla özel ses seviyesi atanırsa (ör. %120 -> 1.20)
        music.set_volume(guild_id, 1.20);
        let v2 = music.volume_of(guild_id);
        assert!((v2 - 1.20).abs() < 0.001);
    }

    #[test]
    fn test_filter_toggling_in_music() {
        let music = Music::new(Songbird::serenity(), reqwest::Client::new());
        let guild_id = GuildId::new(77);

        assert!(music.filters.get(&guild_id).is_none());

        music.set_filter(guild_id, filters::Filter::Nightcore);
        assert_eq!(music.filters.get(&guild_id).map(|f| *f), Some(filters::Filter::Nightcore));

        // Kapalı yapılınca haritadan tamamen silinmeli
        music.set_filter(guild_id, filters::Filter::Off);
        assert!(music.filters.get(&guild_id).is_none());
    }

    #[test]
    fn test_autoplay_task_concurrency_lock() {
        let music = Music::new(Songbird::serenity(), reqwest::Client::new());
        let guild_id = GuildId::new(99);

        assert!(music.autoplay_tasks.insert(guild_id));
        // İkinci eşzamanlı ekleme başarısız olmalı (tekil çalışma garantisi)
        assert!(!music.autoplay_tasks.insert(guild_id));

        music.autoplay_tasks.remove(&guild_id);
        assert!(music.autoplay_tasks.insert(guild_id));
    }

    #[test]
    fn test_empty_channels_lifecycle() {
        let music = Music::new(Songbird::serenity(), reqwest::Client::new());
        let guild_id = GuildId::new(101);

        assert!(music.empty_channels.get(&guild_id).is_none());
        music.empty_channels.insert(guild_id, std::time::Instant::now());
        assert!(music.empty_channels.get(&guild_id).is_some());

        music.empty_channels.remove(&guild_id);
        assert!(music.empty_channels.get(&guild_id).is_none());
    }
}
