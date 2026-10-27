//! İsteğe bağlı dış servis ayarları (`.env`). Hepsi boş olabilir; boş olan özellik kapalı kalır ve
//! ilgili komut kullanıcıya neyin eksik olduğunu söyler.
//!
//! - `IMGUR_CLIENT_ID`       → `/ara imgur`
//! - `RSS_BRIDGE_URL`        → `/akış ekle` X / Instagram / TikTok (RSSHub uyumlu köprü adresi)
//! - `DISCORD_CLIENT_SECRET` → panele "Discord ile giriş"
//! - `ALERT_WEBHOOK_URL`     → hata bildirimleri (Discord webhook adresi; boşsa bot sahibine özel mesaj)
//! - `USER_INSTALL=1`        → `/çevir` ve `/hayal-et` kullanıcı hesabına kurulabilir (Developer Portal'da "User Install" açık olmalı)
//! - `AI_IMAGE_MODEL`        → `/hayal-et` modeli (varsayılan FLUX.1-dev; `NVIDIA_API_KEY` gerekir)
//! - `SPOTIFY_CLIENT_ID` + `SPOTIFY_CLIENT_SECRET` → `/çal` şarkı adını önce Spotify'da arar
//!   (developer.spotify.com → Dashboard → Create app; ses yine YouTube'dan çalınır)
//! - `TMDB_API_KEY`          → `/tavsiye film · dizi` (themoviedb.org → Ayarlar → API)
//! - `RAWG_API_KEY`          → `/tavsiye oyun` (rawg.io/apidocs)

use std::sync::OnceLock;

/// 2026-10-11 ölçümü: flux.1-dev ~5 sn'de cevap veriyor; flux.1-schnell ücretsiz katmanda yanıt vermiyor.
const DEFAULT_IMAGE_MODEL: &str = "black-forest-labs/flux.1-dev";

pub struct Integrations {
    pub imgur_client_id: Option<String>,
    /// Sonunda `/` yok.
    pub rss_bridge_url: Option<String>,
    pub discord_client_secret: Option<String>,
    pub nvidia_api_key: Option<String>,
    pub image_model: String,
    /// Yalnızca geçerli bir Discord webhook adresiyse dolu.
    pub alert_webhook_url: Option<String>,
    /// `/çevir` ve `/hayal-et` genel (hesaba kurulabilir) komut olarak da kaydedilsin mi?
    pub user_install: bool,
    /// Spotify Web API (client id, secret): şarkı adıyla arama.
    pub spotify: Option<(String, String)>,
    pub tmdb_api_key: Option<String>,
    pub rawg_api_key: Option<String>,
}

static GLOBAL: OnceLock<Integrations> = OnceLock::new();

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// Ortam değişkenlerini okur (bir kez, açılışta).
pub fn init() {
    let bridge = env("RSS_BRIDGE_URL")
        .map(|u| u.trim_end_matches('/').to_owned())
        .filter(|u| u.starts_with("http://") || u.starts_with("https://"));
    let _ = GLOBAL.set(Integrations {
        imgur_client_id: env("IMGUR_CLIENT_ID"),
        rss_bridge_url: bridge,
        discord_client_secret: env("DISCORD_CLIENT_SECRET"),
        nvidia_api_key: env("NVIDIA_API_KEY"),
        image_model: env("AI_IMAGE_MODEL").unwrap_or_else(|| DEFAULT_IMAGE_MODEL.to_owned()),
        alert_webhook_url: env("ALERT_WEBHOOK_URL").filter(|url| crate::alerts::valid_webhook(url)),
        user_install: env("USER_INSTALL").is_some_and(|v| v == "1"),
        spotify: env("SPOTIFY_CLIENT_ID").zip(env("SPOTIFY_CLIENT_SECRET")),
        tmdb_api_key: env("TMDB_API_KEY"),
        rawg_api_key: env("RAWG_API_KEY"),
    });
}

/// `init` çağrılmadıysa (testler) her şey kapalıdır.
pub fn get() -> &'static Integrations {
    static EMPTY: Integrations = Integrations {
        imgur_client_id: None,
        rss_bridge_url: None,
        discord_client_secret: None,
        nvidia_api_key: None,
        image_model: String::new(),
        alert_webhook_url: None,
        user_install: false,
        spotify: None,
        tmdb_api_key: None,
        rawg_api_key: None,
    };
    GLOBAL.get().unwrap_or(&EMPTY)
}

// Anahtarlar loglara düşmesin diye Debug elle yazıldı.
impl std::fmt::Debug for Integrations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Integrations")
            .field("imgur", &self.imgur_client_id.is_some())
            .field("rss_bridge", &self.rss_bridge_url.is_some())
            .field("discord_oauth", &self.discord_client_secret.is_some())
            .field("image_model", &self.image_model)
            .field("alert_webhook", &self.alert_webhook_url.is_some())
            .field("user_install", &self.user_install)
            .field("tmdb", &self.tmdb_api_key.is_some())
            .field("rawg", &self.rawg_api_key.is_some())
            .finish()
    }
}
