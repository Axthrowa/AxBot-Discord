//! Tekrarlı mesajlar (zamanlayıcılar; isteğe bağlı otomatik kanal temizleme) ve panelden gönderilen gömülü mesajların kayıtları.
//!
//! Zamanlayıcılar `PluginSettings` deposunda durur; arka plan görevi yarım dakikada bir bellekteki
//! ayarlara bakar (veritabanına yalnızca bir mesaj gönderildiğinde, yeni zamanı yazmak için gider).

use std::sync::Arc;
use std::time::Duration;

use poise::serenity_prelude::{self as serenity, ChannelId, CreateAllowedMentions, CreateEmbed, CreateEmbedFooter, CreateMessage, GetMessages, MessageId};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{info, warn};

use crate::settings::{PluginConfig, PluginSettings};

pub const MAX_TIMERS: usize = 10;
pub const MIN_INTERVAL_MINS: u32 = 5;
pub const MAX_INTERVAL_MINS: u32 = 7 * 24 * 60;
const TICK: Duration = Duration::from_secs(30);

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

/// Uygulama kapanana kadar zamanlayıcıları çalıştırır.
pub fn spawn(ctx: serenity::Context, db: PgPool, settings: PluginSettings) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            crate::supervise::round("tekrarlı mesajlar", std::time::Duration::from_secs(300), async {
                let now = serenity::Timestamp::now().unix_timestamp();
                for guild_id in ctx.cache.guilds() {
                    let current: Arc<TimerSettings> = match settings.get(&db, guild_id).await {
                        Ok(current) => current,
                        Err(e) => {
                            warn!(guild = %guild_id, error = ?e, "Zamanlayıcılar okunamadı");
                            continue;
                        }
                    };
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
    // 14 günün sınırına bir dakika pay bırakılır.
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
        // Son sayfaya ya da 14 günden eski mesajlara gelindiyse devam etmenin anlamı yok.
        if batch.len() < 100 || batch.last().is_some_and(|m| m.timestamp.unix_timestamp() <= cutoff) {
            break;
        }
    }
    deleted
}

// ---------------------------------------------------------------- Gömülü mesajlar

pub const MAX_EMBEDS: usize = 50;

/// Panelden gönderilmiş bir gömülü mesaj (düzenlemek ve silmek için saklanır).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedEmbed {
    pub message_id: u64,
    pub channel_id: u64,
    pub title: String,
    pub description: String,
    /// `#RRGGBB`
    pub colour: String,
    pub image: Option<String>,
    pub thumbnail: Option<String>,
    pub footer: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedSettings {
    pub embeds: Vec<SavedEmbed>,
}

impl PluginConfig for EmbedSettings {
    const KEY: &'static str = "embeds";
}

impl SavedEmbed {
    pub fn build(&self) -> CreateEmbed {
        let [r, g, b] = crate::cards::parse_hex(&self.colour).unwrap_or([88, 101, 242]);
        let mut embed = CreateEmbed::new().colour(serenity::Colour::from_rgb(r, g, b));
        if !self.title.is_empty() {
            embed = embed.title(&self.title);
        }
        if !self.description.is_empty() {
            embed = embed.description(&self.description);
        }
        if let Some(image) = &self.image {
            embed = embed.image(image);
        }
        if let Some(thumbnail) = &self.thumbnail {
            embed = embed.thumbnail(thumbnail);
        }
        if let Some(footer) = &self.footer {
            embed = embed.footer(CreateEmbedFooter::new(footer));
        }
        embed
    }
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
        // 1 numaralı zamanlayıcı 3 tur kaçırmış (bot kapalıydı): tek mesaj gider, sıradaki zaman ileride.
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
        // Kayıt elle bozulsa bile en az 5 dakikada bir gönderilir.
        let mut settings = TimerSettings { timers: vec![timer(1, 0, 100)] };
        take_due(&mut settings, 100);
        assert_eq!(settings.timers[0].next_unix, 100 + 300);
    }
}
