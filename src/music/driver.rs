//! Müzik ses sürücüleri soyutlama katmanı: Native Songbird (yt-dlp + symphonia) ve Lavalink v4.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackState {
    #[default]
    Idle,
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackTransitionEvent {
    Start,
    End,
    Error,
    Pause,
    Resume,
    Seek,
    Skip,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum DriverKind {
    NativeSongbird,
    Lavalink,
}

/// Müzik ses sağlayıcısı arayüzü.
#[async_trait::async_trait]
#[allow(dead_code)]
pub trait AudioDriver: Send + Sync {
    /// Sürücü türü.
    fn kind(&self) -> DriverKind;

    /// İnsan tarafından okunabilir sürücü adı.
    fn name(&self) -> &'static str;

    /// Sürücünün anlık olarak kullanılabilir olup olmadığı.
    fn is_available(&self) -> bool;

    /// Sürücünün sağlık kontrolünü yapar.
    async fn check_health(&self) -> bool;

    /// Gerçekte kullanılacak aktif sürücü modu (harici sürücü devre dışıysa Native fallback'e düşer).
    fn active_kind(&self) -> DriverKind {
        if self.is_available() {
            self.kind()
        } else {
            DriverKind::NativeSongbird
        }
    }

    /// Oynatma yapılabilir mi?
    fn can_playback(&self) -> bool {
        self.is_available() || self.kind() != DriverKind::NativeSongbird
    }

    /// Olaylara göre tutarlı oynatma durumu geçişi sağlar.
    fn next_state(&self, current: PlaybackState, event: TrackTransitionEvent) -> PlaybackState {
        match event {
            TrackTransitionEvent::Start | TrackTransitionEvent::Seek => PlaybackState::Playing,
            TrackTransitionEvent::Pause => {
                if current == PlaybackState::Playing {
                    PlaybackState::Paused
                } else {
                    current
                }
            }
            TrackTransitionEvent::Resume => {
                if current == PlaybackState::Paused {
                    PlaybackState::Playing
                } else {
                    current
                }
            }
            TrackTransitionEvent::Skip | TrackTransitionEvent::End | TrackTransitionEvent::Error => PlaybackState::Idle,
            TrackTransitionEvent::Stop => PlaybackState::Stopped,
        }
    }

    /// Belirtilen durumda olayın uygulanıp uygulanamayacağını doğrular.
    fn can_transition(&self, current: PlaybackState, event: TrackTransitionEvent) -> bool {
        match event {
            TrackTransitionEvent::Start => true,
            TrackTransitionEvent::Pause => current == PlaybackState::Playing,
            TrackTransitionEvent::Resume => current == PlaybackState::Paused,
            TrackTransitionEvent::Seek => current == PlaybackState::Playing || current == PlaybackState::Paused,
            TrackTransitionEvent::Skip => current == PlaybackState::Playing || current == PlaybackState::Paused,
            TrackTransitionEvent::End | TrackTransitionEvent::Error => current == PlaybackState::Playing || current == PlaybackState::Paused,
            TrackTransitionEvent::Stop => current != PlaybackState::Stopped,
        }
    }
}

/// Varsayılan yerel sürücü: Songbird + Symphonia + yt-dlp.
#[derive(Default)]
pub struct NativeSongbirdDriver;

#[async_trait::async_trait]
impl AudioDriver for NativeSongbirdDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::NativeSongbird
    }

    fn name(&self) -> &'static str {
        "NativeSongbird (Symphonia + yt-dlp)"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn check_health(&self) -> bool {
        true
    }
}

/// Harici Lavalink v4 ses düğümü sürücüsü.
#[allow(dead_code)]
pub struct LavalinkDriver {
    pub endpoint: String,
    pub password: String,
    pub http: reqwest::Client,
    pub available: AtomicBool,
}

#[allow(dead_code)]
impl LavalinkDriver {
    pub fn new(endpoint: String, password: String, http: reqwest::Client) -> Self {
        Self {
            endpoint,
            password,
            http,
            available: AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl AudioDriver for LavalinkDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::Lavalink
    }

    fn name(&self) -> &'static str {
        "Lavalink v4 (Harici Ses Düğümü)"
    }

    fn is_available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }

    async fn check_health(&self) -> bool {
        let url = format!("{}/version", self.endpoint.trim_end_matches('/'));
        let res = self
            .http
            .get(&url)
            .header("Authorization", &self.password)
            .timeout(Duration::from_secs(3))
            .send()
            .await;

        let ok = res.is_ok_and(|r| r.status().is_success());
        self.available.store(ok, Ordering::Relaxed);
        ok
    }
}

/// Ortam değişkenlerine göre aktif ses sürücüsünü çözer.
/// `LAVALINK_URL` tanımlıysa `LavalinkDriver` aktifleşir; aksi takdirde yerel `NativeSongbirdDriver` devrede kalır.
pub fn resolve_driver(client: &reqwest::Client) -> Arc<dyn AudioDriver> {
    if let Ok(url) = std::env::var("LAVALINK_URL") {
        let trimmed = url.trim().to_owned();
        if !trimmed.is_empty() {
            let password = std::env::var("LAVALINK_PASSWORD").unwrap_or_else(|_| "youshallnotpass".to_owned());
            let lavalink = LavalinkDriver::new(trimmed, password, client.clone());
            info!(endpoint = %lavalink.endpoint, "Müzik Sürücüsü: Lavalink v4 ses düğümü yapılandırıldı");
            return Arc::new(lavalink);
        }
    }
    Arc::new(NativeSongbirdDriver)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_driver_is_always_available() {
        let driver = NativeSongbirdDriver;
        assert_eq!(driver.kind(), DriverKind::NativeSongbird);
        assert!(driver.is_available());
        assert!(driver.check_health().await);
        assert!(driver.name().contains("NativeSongbird"));
    }

    #[tokio::test]
    async fn lavalink_driver_health_check_handles_unreachable_endpoint() {
        let client = reqwest::Client::new();
        let lavalink = LavalinkDriver::new("http://127.0.0.1:1".to_owned(), "dummy".to_owned(), client);
        assert_eq!(lavalink.kind(), DriverKind::Lavalink);
        assert!(!lavalink.check_health().await);
        assert!(!lavalink.is_available());
        // Lavalink kapalıyken fallback olarak NativeSongbird'e düşmeli
        assert_eq!(lavalink.active_kind(), DriverKind::NativeSongbird);
    }

    #[test]
    fn test_driver_playback_state_transitions() {
        let driver = NativeSongbirdDriver;
        let s0 = PlaybackState::Idle;
        let s1 = driver.next_state(s0, TrackTransitionEvent::Start);
        assert_eq!(s1, PlaybackState::Playing);

        let s2 = driver.next_state(s1, TrackTransitionEvent::Pause);
        assert_eq!(s2, PlaybackState::Paused);

        let s3 = driver.next_state(s2, TrackTransitionEvent::Resume);
        assert_eq!(s3, PlaybackState::Playing);

        let s4 = driver.next_state(s3, TrackTransitionEvent::End);
        assert_eq!(s4, PlaybackState::Idle);

        let s5 = driver.next_state(s3, TrackTransitionEvent::Stop);
        assert_eq!(s5, PlaybackState::Stopped);

        let s6 = driver.next_state(s0, TrackTransitionEvent::Seek);
        assert_eq!(s6, PlaybackState::Playing);

        let s7 = driver.next_state(s6, TrackTransitionEvent::Skip);
        assert_eq!(s7, PlaybackState::Idle);

        assert!(driver.can_transition(PlaybackState::Playing, TrackTransitionEvent::Pause));
        assert!(!driver.can_transition(PlaybackState::Idle, TrackTransitionEvent::Pause));
        assert!(driver.can_transition(PlaybackState::Paused, TrackTransitionEvent::Resume));
        assert!(!driver.can_transition(PlaybackState::Stopped, TrackTransitionEvent::Resume));
    }
}
