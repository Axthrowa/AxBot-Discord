//! Web yönetim paneli (`/panel`).
//!
//! - Giriş Discord üzerinden: `/panel` komutu tek kullanımlık, 10 dakika geçerli bir bağlantı verir.
//!   Bağlantı açılınca "Giriş yap"a basılır (bağlantı önizleyicileri anahtarı harcamasın diye POST),
//!   tarayıcıya 7 günlük bir oturum çerezi verilir. Oturumlar Redis'te tutulur (bot yeniden başlasa da
//!   düşmez).
//! - Bot sahibi her şeyi görür (durum, loglar, yeniden başlatma); diğer kullanıcılar sadece
//!   'Sunucuyu Yönet' izni olduğu sunucuları görür.
//! - Panel adresi: `WEB_PUBLIC_URL` verildiyse o; yoksa Cloudflare hızlı tüneli (sunucuda, hesapsız,
//!   port açmadan `https://….trycloudflare.com`); o da yoksa sadece bu bilgisayar (`http://127.0.0.1`).

mod analytics;
mod auth;
mod automation;
mod botpage;
mod community;
mod extras;
mod health;
mod levels;
mod manage;
mod media;
mod moderation;
mod pages;
mod plugins;

/// Panelde bu adla bir eklenti sayfası var mı?
#[cfg(test)]
pub fn plugin_exists(slug: &str) -> bool {
    plugins::find(slug).is_some()
}
mod welcome;
mod templates;
mod transcripts;
pub mod transfer;
mod wizard;
pub mod ws;

use std::collections::HashSet;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use axum::routing::{get, post};
use axum::Router;
use dashmap::DashMap;
use poise::serenity_prelude::{GuildId, ShardManager, UserId};
use serde::Deserialize;
use tracing::{debug, info};

pub use auth::create_login;
pub use ws::RealtimeEvent;

use crate::web::WebState;

/// Panelin dışarıdan erişilen adresi (tünel adresi çalışırken değişebilir).
#[derive(Clone, Default)]
pub struct PublicUrl(Arc<RwLock<Option<String>>>);

impl PublicUrl {
    pub fn new(initial: Option<String>) -> Self {
        Self(Arc::new(RwLock::new(initial)))
    }

    /// Bilinen adres (yerel olabilir).
    pub fn get(&self) -> Option<String> {
        self.0.read().ok().and_then(|u| u.clone())
    }

    /// Sadece internetten erişilebilir (https) adres; ban itirazı bağlantıları bunu kullanır.
    pub fn public(&self) -> Option<String> {
        self.get().filter(|u| u.starts_with("https://"))
    }

    fn set(&self, url: String) {
        if let Ok(mut guard) = self.0.write() {
            *guard = Some(url);
        }
    }
}

/// Panelin bot hakkında bildiği, komutlardan bağımsız bilgiler.
/// Panelden yeniden başlatma istendi mi? `main` buna bakıp 3 koduyla çıkar (bekçi hemen yeniden açar).
pub static RESTART_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub struct PanelInfo {
    pub owners: HashSet<UserId>,
    pub public_url: PublicUrl,
    /// Discord istemcisi kurulduktan sonra doldurulur (gecikme + yeniden başlatma için).
    pub shards: OnceLock<Arc<ShardManager>>,
    pub started_at: Instant,
    /// Docker'da mı çalışıyor?
    pub in_docker: bool,
    /// Kapanınca biri botu yeniden açar mı? (Docker ya da Windows bekçisi `scripts/run-bot.cmd`)
    pub restartable: bool,
    pub songbird: Arc<songbird::Songbird>,
    pub ai: Option<String>,
    pub members_intent: bool,
    /// (kullanıcı, sunucu) → (yönetebilir mi, ne zaman bakıldı); her sayfada Discord'a gitmemek için.
    perm_cache: DashMap<(UserId, GuildId), (bool, Instant)>,
}

impl PanelInfo {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owners: HashSet<UserId>,
        public_url: PublicUrl,
        in_docker: bool,
        songbird: Arc<songbird::Songbird>,
        ai: Option<String>,
        members_intent: bool,
    ) -> Self {
        Self {
            owners,
            public_url,
            shards: OnceLock::new(),
            started_at: Instant::now(),
            in_docker,
            restartable: in_docker || std::env::var("AXBOT_WATCHDOG").is_ok_and(|v| v == "1"),
            songbird,
            ai,
            members_intent,
            perm_cache: DashMap::new(),
        }
    }

    pub fn is_owner(&self, user: UserId) -> bool {
        self.owners.contains(&user)
    }
}

/// Kullanıcı bu sunucuyu panelden yönetebilir mi? (bot sahibi, sunucu sahibi, Yönetici veya Sunucuyu Yönet)
pub async fn can_manage(state: &WebState, user: UserId, guild_id: GuildId) -> bool {
    const TTL: Duration = Duration::from_secs(60);
    if state.panel.is_owner(user) {
        return state.cache.guild(guild_id).is_some();
    }
    if let Some(entry) = state.panel.perm_cache.get(&(user, guild_id)) {
        if entry.1.elapsed() < TTL {
            return entry.0;
        }
    }
    let member = fetch_member(state, user, guild_id).await;
    let mut allowed = member.as_ref().is_some_and(|m| is_admin_member(state, user, guild_id, m));
    // Panelden tanımlanan "bot yöneticisi" rolleri.
    if !allowed {
        if let (Some(member), Ok(settings)) = (&member, state.settings.get::<crate::community::CommunitySettings>(&state.db, guild_id).await) {
            allowed = member.roles.iter().any(|role| settings.manager_roles.contains(&role.get()));
        }
    }
    state.panel.perm_cache.insert((user, guild_id), (allowed, Instant::now()));
    allowed
}

async fn fetch_member(state: &WebState, user: UserId, guild_id: GuildId) -> Option<poise::serenity_prelude::Member> {
    match state.cache.guild(guild_id).and_then(|g| g.members.get(&user).cloned()) {
        Some(m) => Some(m),
        None => guild_id.member(&state.http, user).await.ok(),
    }
}

fn is_admin_member(state: &WebState, user: UserId, guild_id: GuildId, member: &poise::serenity_prelude::Member) -> bool {
    use poise::serenity_prelude::Permissions;
    state.cache.guild(guild_id).is_some_and(|g| {
        g.owner_id == user || g.member_permissions(member).intersects(Permissions::ADMINISTRATOR | Permissions::MANAGE_GUILD)
    })
}

/// Yalnızca gerçek yöneticiler (bot sahibi, sunucu sahibi, Yönetici / Sunucuyu Yönet). "Bot yöneticisi" rolleri
/// burada sayılmaz: kendi yetkilerini genişletebilecekleri ayarlar (yönetici rolleri, yıkım koruması) bunu kullanır.
/// Önbelleğe alınmaz; bu sayfalar seyrek açılır ve yetki alındığında hemen etkili olmalı.
pub async fn is_guild_admin(state: &WebState, user: UserId, guild_id: GuildId) -> bool {
    if state.panel.is_owner(user) {
        return state.cache.guild(guild_id).is_some();
    }
    fetch_member(state, user, guild_id).await.is_some_and(|m| is_admin_member(state, user, guild_id, &m))
}

/// Yalnızca gerçek yöneticilerin açabileceği eklenti sayfaları.
pub const ADMIN_ONLY_PLUGINS: &[&str] = &["ayarlar", "yikim-korumasi"];

pub fn router() -> Router<WebState> {
    Router::new()
        .route("/saglik", get(health::probe))
        .route("/gizlilik", get(crate::privacy::page_tr))
        .route("/privacy", get(crate::privacy::page_en))
        .route("/panel", get(pages::home))
        .route("/panel/giris/{token}", get(auth::login_page).post(auth::login))
        .route("/panel/cikis", post(auth::logout))
        .route("/panel/giris", get(auth::oauth_start))
        .route("/panel/oauth", get(auth::oauth_callback))
        .route("/panel/sunucu/{guild}", get(pages::guild))
        .route("/panel/sunucu/{guild}/eklenti/{slug}", get(plugins::page).post(plugins::save))
        .route("/panel/sunucu/{guild}/modul/{slug}", post(plugins::toggle_module))
        .route("/panel/sunucu/{guild}/siralama", get(levels::leaderboard))
        .route("/panel/sunucu/{guild}/kart-onizleme", get(welcome::card_preview))
        .route("/siralama/{guild}", get(levels::public_leaderboard))
        .route("/panel/loglar", get(pages::logs))
        .route("/panel/api/loglar", get(pages::logs_api))
        .route("/panel/sistem", get(pages::system))
        .route("/panel/sistem/yeniden-baslat", post(pages::restart))
        .route("/panel/sistem/durum", post(botpage::presence_save))
        .route("/panel/sunucu/{guild}/istatistik", get(analytics::page))
        .route("/panel/sunucu/{guild}/transkriptler", get(transcripts::list_page))
        .route("/panel/sunucu/{guild}/transkript/{number}", get(transcripts::view_html))
        .route("/panel/sunucu/{guild}/sablonlar", get(templates::list_page))
        .route("/panel/sunucu/{guild}/sablon-olustur", post(templates::create_template))
        .route("/panel/sunucu/{guild}/sablon-disa-aktar", get(templates::export_json))
        .route("/panel/sunucu/{guild}/saglik", get(health::page))
        .route("/panel/sunucu/{guild}/kurulum", get(wizard::page))
        .route("/panel/sunucu/{guild}/ayarlar-disa-aktar", get(transfer::export))
        .route("/panel/sunucu/{guild}/otomasyon/test", post(automation::dry_run_handler))
        .route("/panel/panel.js", get(pages::script))
        .route("/panel/ws", get(ws::ws_handler))
}

#[derive(Deserialize)]
struct QuickTunnel {
    hostname: String,
}

/// Cloudflare hızlı tünelinin (cloudflared `--metrics`) verdiği adresi izler; değişince günceller.
pub fn spawn_tunnel_watcher(client: reqwest::Client, metrics_url: String, public_url: PublicUrl) {
    tokio::spawn(async move {
        let mut last: Option<String> = None;
        loop {
            let found = client
                .get(format!("{metrics_url}/quicktunnel"))
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .and_then(reqwest::Response::error_for_status);
            match found {
                Ok(response) => match response.json::<QuickTunnel>().await {
                    Ok(t) if !t.hostname.is_empty() => {
                        let url = format!("https://{}", t.hostname);
                        if last.as_deref() != Some(url.as_str()) {
                            info!(url = %url, "Panel adresi (Cloudflare tüneli) hazır — giriş için Discord'da /panel");
                            public_url.set(url.clone());
                            last = Some(url);
                        }
                    }
                    Ok(_) => debug!("Tünel henüz adres almadı"),
                    Err(e) => debug!(error = %e, "Tünel cevabı okunamadı"),
                },
                Err(e) => debug!(error = %e, "Tünel servisine ulaşılamadı"),
            }
            // Adres bulunana kadar sık, sonra seyrek bakılır.
            let wait = if last.is_some() { 30 } else { 3 };
            tokio::time::sleep(Duration::from_secs(wait)).await;
        }
    });
}
