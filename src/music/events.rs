//! Songbird olay dinleyicileri: "şimdi çalıyor" duyurusu, hata bildirimi, otomatik çalma,
//! boşta kalınca ayrılma.
//!
//! Bu dinleyiciler ses sürücüsünün olay görevinde çalışır; burada `Call` kilidi alınmaz
//! (komutlar kilidi tutarken kilitlenme olmasın). Gereken her şey kendi tablolarımızdan okunur.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use poise::serenity_prelude::{ChannelId, CreateMessage, GuildId, Http};
use songbird::{Event, EventContext, EventHandler};
use tracing::{debug, info, warn};

use super::{extra, format_duration, Shared, TrackInfo};

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
                        // Otomatik çalmada sıradaki benzer şarkı şimdiden seçilir (bitişte beklenmez).
                        if music.settings.get(&guild_id).is_some_and(|s| s.autoplay) {
                            if let Some(current) = music.tracks.get(&id).map(|t| t.clone()) {
                                tokio::spawn(Shared::autoplay_after(Arc::clone(&music), Arc::clone(&self.http), current, true));
                            }
                        }
                    }
                }
                TrackNotice::Ended => {
                    // Eğer parça seek_to tarafından durdurulduysa olay bastırılır (kuyruk ve autoplay bozulmaz)
                    if music.suppressed_ends.remove(&id).is_some() {
                        return None;
                    }
                    if let Some((_, info)) = music.tracks.remove(&id) {
                        music.position_offsets.remove(&info.guild_id);
                        if music.settings.get(&info.guild_id).is_some_and(|s| s.autoplay) {
                            // Kuyruk kilidi burada alınmaz; ayrı görevde bakılır.
                            tokio::spawn(Shared::autoplay_after(Arc::clone(&music), Arc::clone(&self.http), info, false));
                        }
                    }
                }
                TrackNotice::Failed => {
                    if music.suppressed_ends.remove(&id).is_some() {
                        return None;
                    }
                    if let Some((_, info)) = music.tracks.remove(&id) {
                        music.position_offsets.remove(&info.guild_id);
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
    pub music: Weak<Shared>,
    pub http: Arc<Http>,
    pub guild_id: GuildId,
    pub idle_secs: AtomicU64,
}

impl IdleLeaver {
    pub fn new(music: Weak<Shared>, http: Arc<Http>, guild_id: GuildId) -> Self {
        Self {
            music,
            http,
            guild_id,
            idle_secs: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl EventHandler for IdleLeaver {
    async fn act(&self, _ctx: &EventContext<'_>) -> Option<Event> {
        let Some(music) = self.music.upgrade() else {
            return Some(Event::Cancel);
        };
        let settings = music.settings.get(&self.guild_id).map(|s| *s).unwrap_or_default();

        // 1. Maksimum oturum süresi kontrolü (max_session_mins > 0)
        if settings.max_session_mins > 0 {
            if let Some(started) = music.session_started.get(&self.guild_id) {
                if started.elapsed() >= Duration::from_secs(u64::from(settings.max_session_mins) * 60) {
                    info!(guild = %self.guild_id, mins = settings.max_session_mins, "Maksimum oturum süresi doldu, ses kanalından ayrılınıyor");
                    music.session_started.remove(&self.guild_id);
                    music.empty_channels.remove(&self.guild_id);
                    let songbird = Arc::clone(&music.songbird);
                    let http = Arc::clone(&self.http);
                    let guild_id = self.guild_id;
                    let channel = music.text_channels.get(&guild_id).map(|c| *c);
                    tokio::spawn(async move {
                        if let Err(e) = songbird.remove(guild_id).await {
                            warn!(guild = %guild_id, error = %e, "Ses kanalından ayrılırken hata");
                        }
                        if let Some(channel) = channel {
                            let _ = channel
                                .say(&http, "⏳ Maksimum oturum süresi dolduğu için ses kanalından ayrıldım.")
                                .await;
                        }
                    });
                    return Some(Event::Cancel);
                }
            }
        }

        let always_on = settings.always_on;
        let idle_target = u64::from(settings.idle_disconnect_secs);

        // Kanal boşalınca ayrılma `Music::on_voice_state_update` içindeki zamanlayıcıda yapılır.

        // 3. Boşta kalma (kuyruk boş) kontrolü
        let is_autoplay_busy = music.autoplay_tasks.contains(&self.guild_id);
        let busy = music.tracks.iter().any(|t| t.guild_id == self.guild_id) || is_autoplay_busy;
        if busy || always_on {
            self.idle_secs.store(0, Ordering::Relaxed);
            return None;
        }

        let idle = self.idle_secs.fetch_add(60, Ordering::Relaxed) + 60;
        if idle_target > 0 && idle < idle_target {
            return None;
        }

        info!(guild = %self.guild_id, "Kuyruk boş kaldı, ses kanalından ayrılınıyor");
        music.session_started.remove(&self.guild_id);
        music.empty_channels.remove(&self.guild_id);
        // Ayrılma sürücünün kendisini kapatır; olay görevinin içinden değil, ayrı görevde yapılır.
        let songbird = Arc::clone(&music.songbird);
        let http = Arc::clone(&self.http);
        let guild_id = self.guild_id;
        let channel = music.text_channels.get(&guild_id).map(|c| *c);
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

#[cfg(test)]
mod tests {
    use songbird::Songbird;

    use super::*;

    #[test]
    fn test_track_line_formatting() {
        let info = TrackInfo {
            guild_id: GuildId::new(1),
            title: "Test Track".to_owned(),
            url: Some("https://example.com/audio".to_owned()),
            duration: Some(Duration::from_secs(125)),
            thumbnail: None,
            requester: poise::serenity_prelude::UserId::new(1),
            announce: false,
            spotify_url: None,
        };
        let line = track_line(&info);
        assert!(line.contains("[Test Track](https://example.com/audio)"));
        assert!(line.contains("2:05"));
    }

    #[test]
    fn test_idle_leaver_initialization() {
        let music = crate::music::Music::new(Songbird::serenity(), reqwest::Client::new());
        let http = Arc::new(Http::new(""));

        let leaver = IdleLeaver::new(
            Arc::downgrade(&music.0),
            http,
            GuildId::new(42),
        );
        assert_eq!(leaver.guild_id, GuildId::new(42));
        assert_eq!(leaver.idle_secs.load(Ordering::Relaxed), 0);
    }
}
