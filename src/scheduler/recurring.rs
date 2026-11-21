//! Tekrarlı mesajlar (zamanlayıcılar) ve otomatik kanal temizliği (purge).
//!
//! Zamanlayıcılar `PluginSettings` deposunda `"timers"` anahtarında tutulur.
//! Boş sunucu yükünü önlemek için yalnızca aktif zamanlayıcısı olan sunucular (`ACTIVE_GUILDS`)
//! döngü tarafından taranır.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use dashmap::DashSet;
use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateAllowedMentions, CreateMessage, GetMessages, GuildId, MessageId,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{debug, info, warn};

use crate::settings::{PluginConfig, PluginSettings};

pub const MAX_TIMERS: usize = 10;
pub const MIN_INTERVAL_MINS: u32 = 5;
pub const MAX_INTERVAL_MINS: u32 = 7 * 24 * 60;
const TICK: Duration = Duration::from_secs(30);

/// Aktif zamanlayıcısı bulunan sunucuların hızlı bellek içi kümesi.
static ACTIVE_GUILDS: LazyLock<DashSet<GuildId>> = LazyLock::new(DashSet::new);
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Bir sunucuyu aktif tekrarlı mesaj listesine ekler.
pub fn mark_active(guild_id: GuildId) {
    ACTIVE_GUILDS.insert(guild_id);
}

/// Bir sunucuyu aktif tekrarlı mesaj listesinden çıkarır.
pub fn mark_inactive(guild_id: GuildId) {
    ACTIVE_GUILDS.remove(&guild_id);
}

/// Sunucunun aktif zamanlayıcı kümesinde olup olmadığını döner (testler için).
#[cfg(test)]
pub fn is_active(guild_id: GuildId) -> bool {
    ACTIVE_GUILDS.contains(&guild_id)
}

/// Aktif zamanlayıcısı bulunan sunucu sayısını döner.
pub fn active_guild_count() -> usize {
    ACTIVE_GUILDS.len()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timer {
    /// Sunucu içinde benzersiz numara.
    pub id: u32,
    pub channel: u64,
    pub message: String,
    pub interval_mins: u32,
    /// Bir sonraki gönderim zamanı (Unix saniyesi).
    pub next_unix: i64,
    /// Otomatik temizleme: zamanı gelince önce kanaldaki mesajlar silinir (sabitlenenler kalır), mesaj boş değilse ardından gönderilir.
    #[serde(default)]
    pub purge: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimerSettings {
    pub timers: Vec<Timer>,
}

impl PluginConfig for TimerSettings {
    const KEY: &'static str = "timers";
}

impl TimerSettings {
    pub fn next_id(&self) -> u32 {
        self.timers.iter().map(|t| t.id).max().unwrap_or(0) + 1
    }
}

/// Zamanı gelen zamanlayıcılar için sıradaki zamanı ilerletir ve gönderilecekleri döner.
/// Bot kapalıyken kaçan turlar tek tek gönderilmez; sıradaki zaman şimdiden sonraya atlar.
pub fn take_due(settings: &mut TimerSettings, now: i64) -> Vec<Timer> {
    let mut due = Vec::new();
    for timer in &mut settings.timers {
        if timer.next_unix <= now {
            due.push(timer.clone());
            let step = i64::from(timer.interval_mins.max(MIN_INTERVAL_MINS)) * 60;
            let missed = (now - timer.next_unix) / step;
            timer.next_unix += (missed + 1) * step;
        }
    }
    due
}

/// Uygulama kapanana kadar periyodik zamanlayıcıları çalıştırır.
pub fn spawn(ctx: serenity::Context, db: PgPool, settings: PluginSettings) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            crate::supervise::round("tekrarlı mesajlar", Duration::from_secs(300), async {
                let now = serenity::Timestamp::now().unix_timestamp();

                // İlk turda mevcut tüm sunucular taranarak aktif zamanlayıcısı olanlar indekse alınır.
                if !INITIALIZED.load(Ordering::Relaxed) {
                    for guild_id in ctx.cache.guilds() {
                        if let Ok(current) = settings.get::<TimerSettings>(&db, guild_id).await {
                            if !current.timers.is_empty() {
                                mark_active(guild_id);
                            }
                        }
                    }
                    INITIALIZED.store(true, Ordering::SeqCst);
                    debug!(active_guilds = active_guild_count(), "Tekrarlı mesaj sunucu indeksi hazırlandı");
                }

                // Sadece aktif zamanlayıcısı bulunan sunucular işlenir.
                let guilds_to_process: Vec<GuildId> = ACTIVE_GUILDS.iter().map(|g| *g).collect();

                for guild_id in guilds_to_process {
                    let current: Arc<TimerSettings> = match settings.get(&db, guild_id).await {
                        Ok(current) => current,
                        Err(e) => {
                            warn!(guild = %guild_id, error = ?e, "Zamanlayıcılar okunamadı");
                            continue;
                        }
                    };

                    if current.timers.is_empty() {
                        mark_inactive(guild_id);
                        continue;
                    }

                    if !current.timers.iter().any(|t| t.next_unix <= now) {
                        continue;
                    }

                    let mut updated = (*current).clone();
                    let due = take_due(&mut updated, now);

                    // Önce yeni zaman yazılır: gönderim uzun sürse bile aynı mesaj iki kez gitmez.
                    if let Err(e) = settings.set(&db, guild_id, updated).await {
                        warn!(guild = %guild_id, error = ?e, "Zamanlayıcı zamanı kaydedilemedi");
                        continue;
                    }

                    for timer in due {
                        let (channel, text) = (timer.channel, timer.message);
                        if timer.purge {
                            let count = purge(&ctx.http, ChannelId::new(channel.max(1))).await;
                            info!(guild = %guild_id, channel, count, "Kanal otomatik temizlendi");
                            if text.trim().is_empty() {
                                continue;
                            }
                        }
                        // Tekrarlı mesajlar kimseyi etiketlemez (@everyone dahil).
                        let message = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
                        match ChannelId::new(channel.max(1)).send_message(&ctx.http, message).await {
                            Ok(_) => info!(guild = %guild_id, channel, "Tekrarlı mesaj gönderildi"),
                            Err(e) => warn!(guild = %guild_id, channel, error = %e, "Tekrarlı mesaj gönderilemedi"),
                        }
                    }
                }
            })
            .await;
        }
    });
}

/// Kanaldaki son 14 gün içindeki mesajları siler (Discord daha eskileri toplu silmez); sabitlenenler kalır.
/// Tur başına en fazla 500 mesaj. Silinen mesaj sayısını döner.
async fn purge(http: &serenity::Http, channel: ChannelId) -> usize {
    let cutoff = serenity::Timestamp::now().unix_timestamp() - 14 * 86_400 + 60;
    let mut deleted = 0;
    let mut before: Option<MessageId> = None;
    for _ in 0..5 {
        let request = before.map_or_else(|| GetMessages::new().limit(100), |b| GetMessages::new().limit(100).before(b));
        let batch = match channel.messages(http, request).await {
            Ok(batch) => batch,
            Err(e) => {
                warn!(channel = %channel, error = %e, "Otomatik temizleme mesajları okuyamadı");
                break;
            }
        };
        before = batch.last().map(|m| m.id);
        let ids: Vec<MessageId> = batch.iter().filter(|m| !m.pinned && m.timestamp.unix_timestamp() > cutoff).map(|m| m.id).collect();
        let result = match ids.as_slice() {
            [] => Ok(()),
            [id] => channel.delete_message(http, *id).await,
            _ => channel.delete_messages(http, &ids).await,
        };
        if let Err(e) = result {
            warn!(channel = %channel, error = %e, "Otomatik temizleme mesajları silemedi");
            break;
        }
        deleted += ids.len();
        if batch.len() < 100 || batch.last().is_some_and(|m| m.timestamp.unix_timestamp() <= cutoff) {
            break;
        }
    }
    deleted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timer(id: u32, interval_mins: u32, next_unix: i64) -> Timer {
        Timer { id, channel: 1, message: format!("m{id}"), interval_mins, next_unix, purge: false }
    }

    #[test]
    fn due_timers_advance_past_now_without_backlog() {
        let mut settings = TimerSettings { timers: vec![timer(1, 10, 1_000), timer(2, 60, 5_000)] };
        let due = take_due(&mut settings, 2_900);
        assert_eq!(due.iter().map(|t| (t.id, t.message.as_str())).collect::<Vec<_>>(), vec![(1, "m1")]);
        assert_eq!(settings.timers[0].next_unix, 3_400);
        assert_eq!(settings.timers[1].next_unix, 5_000);
        assert!(take_due(&mut settings, 2_900).is_empty());
        assert_eq!(settings.next_id(), 3);
        assert_eq!(TimerSettings::default().next_id(), 1);
    }

    #[test]
    fn too_small_intervals_cannot_spam() {
        let mut settings = TimerSettings { timers: vec![timer(1, 0, 100)] };
        take_due(&mut settings, 100);
        assert_eq!(settings.timers[0].next_unix, 100 + 300);
    }

    #[test]
    fn active_guilds_index_tracking() {
        let g1 = GuildId::new(1001);
        let g2 = GuildId::new(1002);

        mark_active(g1);
        mark_active(g2);
        assert!(is_active(g1));
        assert!(is_active(g2));

        mark_inactive(g1);
        assert!(!is_active(g1));
        assert!(is_active(g2));

        mark_inactive(g2);
        assert!(!is_active(g2));
    }
}
