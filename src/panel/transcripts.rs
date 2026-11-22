//! Web Panel: Bilet konuşma kayıtları (transkriptler) ve HTML görüntüleyici.

use std::fmt::Write as _;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use poise::serenity_prelude::{GuildId, UserId};
use serde::Deserialize;

use super::auth;
use super::can_manage;
use super::pages::{forbidden, layout, GuildNav, Nav, Page};
use crate::tickets;
use crate::web::{escape, WebState};

#[derive(Deserialize, Default)]
pub struct TranscriptQuery {
    pub q: Option<String>,
}

/// `GET /panel/sunucu/{guild}/transkriptler`
pub async fn list_page(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(guild): Path<u64>,
    Query(query): Query<TranscriptQuery>,
) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await {
        return forbidden();
    }
    let Some(guild_name) = state.cache.guild(guild_id).map(|g| g.name.clone()) else {
        return forbidden();
    };

    let search_q = query.q.as_deref().unwrap_or("").trim();
    let entries = match tickets::list_transcripts(&state.db, guild_id, (!search_q.is_empty()).then_some(search_q), 50).await {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(guild = %guild_id, error = ?e, "Transkriptler listelenemedi");
            Vec::new()
        }
    };

    let mut body = String::with_capacity(8192);

    let _ = write!(
        body,
        "<div class=\"card\"><h2>🎫 Destek Biletleri & Konuşma Kayıtları</h2>\
         <p>Kapatılmış destek biletlerinin konuşma kayıtlarını tarayıcıda görüntüleyebilir veya indirebilirsin.</p>\
         <form method=\"GET\" action=\"/panel/sunucu/{}/transkriptler\" style=\"margin-top:16px;display:flex;gap:8px;max-width:480px;\">\
         <input type=\"text\" name=\"q\" value=\"{}\" placeholder=\"Bilet no, konu veya gerekçe ara…\" style=\"flex:1;\">\
         <button type=\"submit\" class=\"btn sm on\">Ara</button>\
         {}</form></div>",
        guild,
        escape(search_q),
        if !search_q.is_empty() {
            format!("<a href=\"/panel/sunucu/{guild}/transkriptler\" class=\"btn sm\">Temizle</a>")
        } else {
            String::new()
        }
    );

    let _ = write!(body, "<div class=\"card\" style=\"margin-top:16px;\">");
    if entries.is_empty() {
        body.push_str("<p class=\"note\">Kayıtlı kapatılmış bilet bulunamadı.</p>");
    } else {
        body.push_str("<div style=\"overflow-x:auto;\"><table style=\"width:100%;border-collapse:collapse;\">\
                       <thead><tr>\
                       <th style=\"text-align:left;padding:8px;\">No</th>\
                       <th style=\"text-align:left;padding:8px;\">Konu</th>\
                       <th style=\"text-align:left;padding:8px;\">Açan</th>\
                       <th style=\"text-align:left;padding:8px;\">Kapatan</th>\
                       <th style=\"text-align:left;padding:8px;\">Kapanış Tarihi</th>\
                       <th style=\"text-align:left;padding:8px;\">Gerekçe</th>\
                       <th style=\"text-align:right;padding:8px;\">İşlem</th>\
                       </tr></thead><tbody>");

        for t in &entries {
            let topic_display = t.topic.as_deref().unwrap_or("Genel");
            let opener_name = state
                .cache
                .guild(guild_id)
                .and_then(|g| g.members.get(&UserId::new(t.opener_id as u64)).map(|m| m.display_name().to_owned()))
                .unwrap_or_else(|| format!("<@{}>", t.opener_id));
            let closer_name = t.closed_by.map(|c| {
                state
                    .cache
                    .guild(guild_id)
                    .and_then(|g| g.members.get(&UserId::new(c as u64)).map(|m| m.display_name().to_owned()))
                    .unwrap_or_else(|| format!("<@{c}>"))
            }).unwrap_or_else(|| "—".to_owned());

            let closed_date = t.closed_at_unix.and_then(|u| {
                chrono::DateTime::from_timestamp(u, 0)
            }).map(|dt| dt.format("%d.%m.%Y %H:%M").to_string()).unwrap_or_else(|| "—".to_owned());

            let reason_display = t.close_reason.as_deref().unwrap_or("—");

            let action_btn = if t.has_html {
                format!(
                    "<a href=\"/panel/sunucu/{}/transkript/{}\" target=\"_blank\" class=\"btn sm on\" style=\"padding:4px 8px;font-size:12px;\">Görüntüle</a>",
                    guild, t.number
                )
            } else {
                "<span class=\"note\" style=\"font-size:12px;\">HTML Yok</span>".to_owned()
            };

            let _ = write!(
                body,
                "<tr style=\"border-bottom:1px solid var(--border, #3f4147);\">\
                 <td style=\"padding:8px;\"><b>#{:04}</b></td>\
                 <td style=\"padding:8px;\"><span class=\"badge\" style=\"background:rgba(88,101,242,0.2);color:#5865f2;padding:2px 6px;border-radius:4px;\">{}</span></td>\
                 <td style=\"padding:8px;\">{}</td>\
                 <td style=\"padding:8px;\">{}</td>\
                 <td style=\"padding:8px;\">{}</td>\
                 <td style=\"padding:8px;max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;\">{}</td>\
                 <td style=\"padding:8px;text-align:right;\">{}</td>\
                 </tr>",
                t.number,
                escape(topic_display),
                escape(&opener_name),
                escape(&closer_name),
                closed_date,
                escape(reason_display),
                action_btn
            );
        }
        body.push_str("</tbody></table></div>");
    }
    body.push_str("</div>");

    let title = format!("Bilet Kayıtları — {guild_name}");
    let nav = GuildNav {
        id: guild_id,
        name: &guild_name,
        active: "transkriptler",
    };

    Html(
        layout(
            &state,
            &session,
            Nav::Home,
            Some(nav),
            Page {
                title: &title,
                body,
            },
        )
        .await,
    )
    .into_response()
}

/// `GET /panel/sunucu/{guild}/transkript/{number}`
pub async fn view_html(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((guild, number)): Path<(u64, i32)>,
) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await {
        return forbidden();
    }

    match tickets::get_html_transcript(&state.db, guild_id, number).await {
        Ok(Some(html)) => Html(html).into_response(),
        Ok(None) => Html(format!(
            "<!DOCTYPE html><html><body style=\"background:#313338;color:#fff;font-family:sans-serif;padding:40px;text-align:center;\">\
             <h2>Transkript Bulunamadı</h2>\
             <p>#{:04} numaralı biletin HTML konuşma kaydı henüz veritabanında saklanmamış veya bulunamadı.</p>\
             </body></html>",
            number
        ))
        .into_response(),
        Err(e) => {
            tracing::warn!(guild = %guild_id, ticket = number, error = ?e, "Transkript okunamadı");
            forbidden()
        }
    }
}
