//! Songbird olay dinleyicileri: "şimdi çalıyor" duyurusu, hata bildirimi, otomatik çalma,
//! boşta kalınca ayrılma.
//!
//! Bu dinleyiciler ses sürücüsünün olay görevinde çalışır; burada `Call` kilidi alınmaz
//! (komutlar kilidi tutarken kilitlenme olmasın). Gereken her şey kendi tablolarımızdan okunur.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use poise::serenity_prelude::{ChannelId, CreateMessage, GuildId, Http};
use songbird::{Event, EventContext, EventHandler, Songbird};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{extra, format_duration, MusicSettings, Shared, TrackInfo};

#[derive(Debug, Clone, Copy)]
pub enum TrackNotice {
    Started,
    Ended,
    Failed,
}

pub struct TrackNotifier {
    pub http: Arc<Http>,
    /// Zayıf referans: parçalar `Shared`'ın içindeki songbird'de yaşar (döngüsel referans olmasın).
    pub music: Weak<Shared>,
    pub notice: TrackNotice,
}

#[async_trait]
impl EventHandler for TrackNotifier {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        let EventContext::Track(tracks) = ctx else {
            return None;
        };
        let music = self.music.upgrade()?;

        for (_, handle) in *tracks {
            let id = handle.uuid();
            match self.notice {
                TrackNotice::Started => {
                    // Duraklatıp devam edince de Play gelir; sadece ilk başlangıçta duyur.
                    if let Some((guild, video)) = music
                        .tracks
                        .get(&id)
                        .and_then(|t| Some((t.guild_id, t.url.as_deref().and_then(extra::youtube_video_id)?)))
                    {
                        music.remember(guild, video);
                    }
                    let info = music.tracks.get_mut(&id).and_then(|mut t| {
                        let announce = t.announce;
                        t.announce = false;
                        announce.then(|| t.clone())
                    });
                    let guild_id = music.tracks.get(&id).map(|t| t.guild_id);
                    if let Some(info) = info {
                        // Kart çizimi kuyruk kilidi ister; olay görevinde değil, ayrı görevde yapılır.
                        tokio::spawn(Shared::post_card(Arc::clone(&music), Arc::clone(&self.http), info.guild_id));
                    }
                    if let Some(guild_id) = guild_id {
                        tokio::spawn(Shared::prefetch_next(Arc::clone(&music), guild_id));
                    }
                }
                TrackNotice::Ended => {
                    if let Some((_, info)) = music.tracks.remove(&id) {
                        if music.settings.get(&info.guild_id).is_some_and(|s| s.autoplay) {
                            // Kuyruk kilidi burada alınmaz; ayrı görevde bakılır.
                            tokio::spawn(Shared::autoplay_after(Arc::clone(&music), Arc::clone(&self.http), info));
                        }
                    }
                }
                TrackNotice::Failed => {
                    if let Some((_, info)) = music.tracks.remove(&id) {
                        let err_detail = handle.get_info().await.map(|st| format!("{:?}", st.playing)).unwrap_or_default();
                        warn!(title = %info.title, error = %err_detail, "Parça çalınamadı");
                        let text = format!("⚠️ **{}** çalınamadı, sıradakine geçiliyor.", info.title);
                        self.send(&music, info.guild_id, CreateMessage::new().content(text)).await;
                    }
                }
            }
        }
        None
    }
}

impl TrackNotifier {
    async fn send(&self, music: &Shared, guild_id: GuildId, message: CreateMessage) -> Option<(ChannelId, poise::serenity_prelude::MessageId)> {
        let channel = music.text_channels.get(&guild_id).map(|c| *c)?;
        // Şarkı adlarındaki @everyone / rol etiketleri kimseyi çağırmasın.
        let message = message.allowed_mentions(poise::serenity_prelude::CreateAllowedMentions::new());
        match channel.send_message(&self.http, message).await {
            Ok(sent) => Some((channel, sent.id)),
            Err(e) => {
                debug!(channel = %channel, error = %e, "Müzik bildirimi gönderilemedi");
                None
            }
        }
    }
}

/// Kuyruk belirli bir süre boş kalırsa kanaldan ayrılır.
pub struct IdleLeaver {
    songbird: Arc<Songbird>,
    tracks: Arc<DashMap<Uuid, TrackInfo>>,
    text_channels: Arc<DashMap<GuildId, ChannelId>>,
    settings: Arc<DashMap<GuildId, MusicSettings>>,
    http: Arc<Http>,
    guild_id: GuildId,
    leave_after: Duration,
    idle_secs: AtomicU64,
}

impl IdleLeaver {
    pub fn new(
        songbird: Arc<Songbird>,
        tracks: Arc<DashMap<Uuid, TrackInfo>>,
        text_channels: Arc<DashMap<GuildId, ChannelId>>,
        settings: Arc<DashMap<GuildId, MusicSettings>>,
        http: Arc<Http>,
        guild_id: GuildId,
        leave_after: Duration,
    ) -> Self {
        Self {
            songbird,
            tracks,
            text_channels,
            settings,
            http,
            guild_id,
            leave_after,
            idle_secs: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl EventHandler for IdleLeaver {
    async fn act(&self, _ctx: &EventContext<'_>) -> Option<Event> {
        let busy = self.tracks.iter().any(|t| t.guild_id == self.guild_id);
        let always_on = self.settings.get(&self.guild_id).is_some_and(|s| s.always_on);
        if busy || always_on {
            self.idle_secs.store(0, Ordering::Relaxed);
            return None;
        }

        let idle = self.idle_secs.fetch_add(60, Ordering::Relaxed) + 60;
        if idle < self.leave_after.as_secs() {
            return None;
        }

        info!(guild = %self.guild_id, "Kuyruk boş kaldı, ses kanalından ayrılınıyor");
        // Ayrılma sürücünün kendisini kapatır; olay görevinin içinden değil, ayrı görevde yapılır.
        let songbird = Arc::clone(&self.songbird);
        let http = Arc::clone(&self.http);
        let guild_id = self.guild_id;
        let channel = self.text_channels.get(&guild_id).map(|c| *c);
        tokio::spawn(async move {
            if let Err(e) = songbird.remove(guild_id).await {
                warn!(guild = %guild_id, error = %e, "Ses kanalından ayrılırken hata");
            }
            if let Some(channel) = channel {
                let _ = channel
                    .say(&http, "💤 Bir süredir müzik çalmadığı için ses kanalından ayrıldım.")
                    .await;
            }
        });
        Some(Event::Cancel)
    }
}

/// "[Başlık](bağlantı) `3:45`"
pub fn track_line(info: &TrackInfo) -> String {
    let title = info.title.replace(['[', ']'], "");
    let mut line = match &info.url {
        Some(url) => format!("[{title}]({url})"),
        None => format!("**{title}**"),
    };
    if let Some(d) = info.duration {
        line.push_str(&format!(" `{}`", format_duration(d)));
    }
    line
}
