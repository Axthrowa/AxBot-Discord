//! WebSocket canlı panel akışı (`/panel/ws`).

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use poise::serenity_prelude::{GuildId, UserId};
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::auth;
use super::can_manage;
use crate::web::WebState;

/// Panel WebSocket üzerinden yayınlanan hafif olaylar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum RealtimeEvent {
    TrackChange {
        guild_id: u64,
        title: String,
        artist: Option<String>,
    },
    Moderation {
        guild_id: u64,
        action: String,
        target: String,
    },
    Ticket {
        guild_id: u64,
        ticket_id: u64,
        status: String,
    },
    MemberFlow {
        guild_id: u64,
        joined: bool,
        tag: String,
    },
    SystemHealth {
        memory_mb: u64,
        ping_ms: u64,
    },
    StreamStatus {
        guild_id: u64,
        platform: String,
        streamer: String,
        live: bool,
        title: Option<String>,
        game: Option<String>,
        viewers: Option<u64>,
    },
}

pub async fn ws_handler(
    State(state): State<WebState>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    let Some(session) = auth::session(&state, &headers).await else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    Ok(ws.on_upgrade(move |socket| handle_socket(state, socket, session.user)))
}

async fn handle_socket(state: WebState, mut socket: WebSocket, user_id: UserId) {
    let mut rx = state.event_bus.subscribe();
    let mut ping_ticker = tokio::time::interval(Duration::from_secs(25));
    ping_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ping_ticker.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(event) => {
                        if is_authorized_event(&state, user_id, &event).await {
                            if let Ok(json) = serde_json::to_string(&event) {
                                if socket.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        debug!(skipped, "Panel WebSocket alıcısı gecikti");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

async fn is_authorized_event(state: &WebState, user_id: UserId, event: &RealtimeEvent) -> bool {
    let is_owner = state.panel.is_owner(user_id);
    match event {
        RealtimeEvent::SystemHealth { .. } => is_owner,
        RealtimeEvent::TrackChange { guild_id, .. }
        | RealtimeEvent::Moderation { guild_id, .. }
        | RealtimeEvent::Ticket { guild_id, .. }
        | RealtimeEvent::MemberFlow { guild_id, .. }
        | RealtimeEvent::StreamStatus { guild_id, .. } => {
            if is_owner {
                true
            } else {
                can_manage(state, user_id, GuildId::new(*guild_id)).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realtime_event_serialization_roundtrip() -> anyhow::Result<()> {
        let event = RealtimeEvent::TrackChange {
            guild_id: 12345,
            title: "Dönence".to_owned(),
            artist: Some("Barış Manço".to_owned()),
        };
        let json = serde_json::to_string(&event)?;
        assert!(json.contains("\"type\":\"track_change\""));
        assert!(json.contains("\"title\":\"Dönence\""));

        let mod_event = RealtimeEvent::Moderation {
            guild_id: 98765,
            action: "ban".to_owned(),
            target: "Troll#1234".to_owned(),
        };
        let json2 = serde_json::to_string(&mod_event)?;
        assert!(json2.contains("\"type\":\"moderation\""));

        let stream_event = RealtimeEvent::StreamStatus {
            guild_id: 55555,
            platform: "twitch".to_owned(),
            streamer: "elraenn".to_owned(),
            live: true,
            title: Some("Canlı Yayın Başladı".to_owned()),
            game: Some("Just Chatting".to_owned()),
            viewers: Some(15000),
        };
        let json3 = serde_json::to_string(&stream_event)?;
        assert!(json3.contains("\"type\":\"stream_status\""));
        assert!(json3.contains("\"streamer\":\"elraenn\""));
        assert!(json3.contains("\"live\":true"));
        assert!(json3.contains("\"viewers\":15000"));
        Ok(())
    }
}
