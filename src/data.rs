//! Tüm komut ve event'lerin paylaştığı global durum.
//!
//! poise bu struct'ı kendi içinde `Arc` ile sarar; her komut `ctx.data()` ile
//! kopyalama yapmadan erişir. `PgPool` ve `ConnectionManager` da içeride `Arc`
//! tutar, yani klonlamaları ucuzdur ve arka plan görevlerine güvenle verilebilir.
//! Arka plan görevleriyle paylaşılan parçalar ayrıca `Arc` içindedir.

use std::sync::Arc;
use std::time::Instant;

use redis::aio::ConnectionManager;
use sqlx::PgPool;

use crate::ai::Ai;
use crate::automod::AutoMod;
use crate::error::Error;
use crate::guild_config::GuildConfigs;
use crate::leveling::Leveling;
use crate::modlog::ModLog;
use crate::music::Music;
use crate::scheduler::Scheduler;
use crate::streams::Streams;
use crate::invites::Invites;
use crate::tags::Tags;
use crate::tempvoice::TempVoice;

pub type Context<'a> = poise::Context<'a, Data, Error>;

pub struct Data {
    pub db: PgPool,
    pub redis: ConnectionManager,
    /// Web paneliyle paylaşılır (ayar değişince filtre yeniden derlenir).
    pub automod: Arc<AutoMod>,
    /// Zamanlayıcı (süreli ban bitişi) ile paylaşılır.
    pub modlog: Arc<ModLog>,
    pub guild_configs: GuildConfigs,
    /// Web paneliyle paylaşılır (ödüller, XP listeleri, sıralama).
    pub leveling: Arc<Leveling>,
    /// Eklenti ayarları (panelle ortak önbellek).
    pub settings: crate::settings::PluginSettings,
    /// Web paneliyle paylaşılır (sohbet kanalları).
    pub ai: Arc<Ai>,
    /// Arka plandaki yayın kontrol görevi ile paylaşılır.
    pub streams: Arc<Streams>,
    pub music: Music,
    pub scheduler: Arc<Scheduler>,
    /// Etiketler (`!ad`); mesaj başına okunduğu için bellekte.
    pub tags: Arc<Tags>,
    /// Davet kullanım sayıları (girişte hangi davetin kullanıldığını bulmak için).
    pub invites: Invites,
    pub tempvoice: TempVoice,
    /// Hayvan yakalama: kanal sayaçları ve aktif hayvanlar (bellekte).
    pub animals: crate::games::animals::Animals,
    /// Sohbet oyunu (adam asmaca / bilgi yarışması) süren kanallar.
    pub game_channels: dashmap::DashSet<poise::serenity_prelude::ChannelId>,
    /// Otomatik cevap kuralları (mesaj başına okunduğu için bellekte).
    pub autoresponder: Arc<crate::autoresponder::AutoResponder>,
    /// Web panelinin / itiraz formunun dış adresi (tünel adresi değişebilir).
    pub public_url: crate::panel::PublicUrl,
    /// Doluysa komutlar sadece bu sunucuya kaydedilir; boşsa bot eklendiği her sunucuya.
    pub dev_guild_id: Option<poise::serenity_prelude::GuildId>,
    /// Ortak HTTP istemcisi (YouTube vb. dış istekler).
    pub http_client: reqwest::Client,
    /// Developer Portal'da "Server Members Intent" açık mı? Kapalıysa giriş/çıkış özellikleri çalışmaz.
    pub members_intent: bool,
    pub started_at: Instant,
    #[allow(dead_code)]
    pub event_bus: tokio::sync::broadcast::Sender<crate::panel::RealtimeEvent>,
}
