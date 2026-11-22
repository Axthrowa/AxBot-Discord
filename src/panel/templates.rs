//! Web Panel: Xenon tarzı sunucu şablonları yönetimi ve dışa aktarma.

use std::fmt::Write as _;

use axum::extract::{Form, Path, Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{Html, IntoResponse, Redirect, Response};
use poise::serenity_prelude::GuildId;
use serde::Deserialize;

use super::auth;
use super::is_guild_admin;
use super::pages::{forbidden, layout, GuildNav, Nav, Page};
use crate::backup;
use crate::panel::transfer;
use crate::web::{escape, WebState};

#[derive(Deserialize, Default)]
pub struct TemplateFilter {
    pub cat: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateTemplateForm {
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub include_settings: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
pub struct ApplyTemplateForm {
    pub code: String,
}

/// `GET /panel/sunucu/{guild}/sablonlar`
pub async fn list_page(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(guild): Path<u64>,
    Query(query): Query<TemplateFilter>,
) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !is_guild_admin(&state, session.user, guild_id).await {
        return forbidden();
    }
    let Some(guild_name) = state.cache.guild(guild_id).map(|g| g.name.clone()) else {
        return forbidden();
    };

    let selected_cat = query.cat.as_deref().unwrap_or("").trim();
    let templates = match backup::list_templates(&state.db, (!selected_cat.is_empty()).then_some(selected_cat), 30).await {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(guild = %guild_id, error = ?e, "Sunucu şablonları listelenemedi");
            Vec::new()
        }
    };

    let mut body = String::with_capacity(8192);

    let _ = write!(
        body,
        "<div class=\"card\"><h2>🧩 Xenon Tarzı Sunucu Şablonları</h2>\
         <p>Sunucunuzun kanal, kategori, rol ve izin yapısını şablon olarak dışa aktarabilir, paylaşabilir veya hazır şablonları uygulayabilirsiniz.</p>\
         <div style=\"margin-top:16px;display:flex;gap:10px;flex-wrap:wrap;\">\
         <a href=\"/panel/sunucu/{}/sablon-disa-aktar\" class=\"btn on\">💾 Şablonu JSON Olarak İndir</a>\
         </div></div>",
        guild
    );

    // Yeni Şablon Oluşturma Kartı
    let _ = write!(
        body,
        "<div class=\"card\" style=\"margin-top:16px;\">\
         <h3>✨ Bu Sunucudan Yeni Şablon Oluştur</h3>\
         <form method=\"POST\" action=\"/panel/sunucu/{}/sablon-olustur\" style=\"margin-top:12px;display:grid;gap:12px;max-width:540px;\">\
         <div><label style=\"display:block;margin-bottom:4px;font-weight:600;\">Şablon Adı:</label>\
         <input type=\"text\" name=\"name\" value=\"{} Şablonu\" required style=\"width:100%;\">\
         </div>\
         <div><label style=\"display:block;margin-bottom:4px;font-weight:600;\">Kategori:</label>\
         <select name=\"category\" style=\"width:100%;\">\
         <option value=\"genel\">Genel</option>\
         <option value=\"oyun\">Oyun Topluluğu</option>\
         <option value=\"topluluk\">Topluluk / Sohbet</option>\
         <option value=\"yazilim\">Yazılım & Teknoloji</option>\
         <option value=\"egitim\">Eğitim & Akademi</option>\
         </select></div>\
         <div><label style=\"display:block;margin-bottom:4px;font-weight:600;\">Açıklama (opsiyonel):</label>\
         <textarea name=\"description\" placeholder=\"Şablon hakkında kısa bilgi…\" style=\"width:100%;height:60px;\"></textarea></div>\
         <div><label style=\"display:flex;align-items:center;gap:8px;cursor:pointer;\">\
         <input type=\"checkbox\" name=\"include_settings\" value=\"1\" checked> Güvenli bot ayarlarını da dahil et (seviye oranları, automod)\
         </label></div>\
         <div><button type=\"submit\" class=\"btn on\">Şablon Kodu Oluştur</button></div>\
         </form></div>",
        guild,
        escape(&guild_name)
    );

    // Kayıtlı Şablonlar Listesi
    let _ = write!(
        body,
        "<div class=\"card\" style=\"margin-top:16px;\">\
         <div style=\"display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;flex-wrap:wrap;gap:8px;\">\
         <h3>🌐 Paylaşılan Şablonlar</h3>\
         <div style=\"display:flex;gap:6px;\">\
         <a href=\"/panel/sunucu/{guild}/sablonlar\" class=\"btn sm{}\">Tümü</a>\
         <a href=\"/panel/sunucu/{guild}/sablonlar?cat=oyun\" class=\"btn sm{}\">Oyun</a>\
         <a href=\"/panel/sunucu/{guild}/sablonlar?cat=topluluk\" class=\"btn sm{}\">Topluluk</a>\
         <a href=\"/panel/sunucu/{guild}/sablonlar?cat=yazilim\" class=\"btn sm{}\">Yazılım</a>\
         </div></div>",
        if selected_cat.is_empty() { " on" } else { "" },
        if selected_cat == "oyun" { " on" } else { "" },
        if selected_cat == "topluluk" { " on" } else { "" },
        if selected_cat == "yazilim" { " on" } else { "" }
    );

    if templates.is_empty() {
        body.push_str("<p class=\"note\">Henüz kayıtlı şablon bulunmuyor. İlk şablonu yukarıdaki formdan oluşturabilirsiniz!</p>");
    } else {
        body.push_str("<div style=\"display:grid;grid-template-columns:repeat(auto-fit, minmax(280px, 1fr));gap:12px;\">");
        for tmpl in &templates {
            let desc = tmpl.description.as_deref().unwrap_or("Açıklama yok");
            let _ = write!(
                body,
                "<div style=\"background:var(--bg-tertiary, #1e1f22);border:1px solid var(--border, #3f4147);border-radius:6px;padding:14px;\">\
                 <div style=\"display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;\">\
                 <h4 style=\"margin:0;\">{}</h4>\
                 <span class=\"badge\" style=\"background:rgba(88,101,242,0.2);color:#5865f2;padding:2px 6px;border-radius:4px;font-size:11px;\">{}</span>\
                 </div>\
                 <p style=\"font-size:13px;color:var(--text-muted, #949ba4);margin-bottom:10px;\">{}</p>\
                 <div style=\"font-size:12px;margin-bottom:10px;\">\
                 • <b>{}</b> Rol · <b>{}</b> Kanal (İzinler dahil)<br>\
                 • Kullanım: <b>{}</b> kez\
                 </div>\
                 <div style=\"display:flex;align-items:center;justify-content:space-between;gap:8px;\">\
                 <code style=\"background:#000;padding:3px 8px;border-radius:4px;font-size:13px;color:#57f287;\">{}</code>\
                 <span class=\"note\" style=\"font-size:11px;\">Discord: /yedek şablon-yükle kod:{}</span>\
                 </div>\
                 </div>",
                escape(&tmpl.name),
                escape(&tmpl.category),
                escape(desc),
                tmpl.roles_count,
                tmpl.channels_count,
                tmpl.uses_count,
                escape(&tmpl.id),
                escape(&tmpl.id)
            );
        }
        body.push_str("</div>");
    }
    body.push_str("</div>");

    let title = format!("Sunucu Şablonları — {guild_name}");
    let nav = GuildNav {
        id: guild_id,
        name: &guild_name,
        active: "sablonlar",
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

/// `POST /panel/sunucu/{guild}/sablon-olustur`
pub async fn create_template(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(guild): Path<u64>,
    Form(form): Form<CreateTemplateForm>,
) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !is_guild_admin(&state, session.user, guild_id).await {
        return forbidden();
    }

    let Some(snapshot) = backup::take(&state.cache, guild_id) else {
        return forbidden();
    };

    let safe_settings = if form.include_settings.as_deref() == Some("1") {
        transfer::export_safe_settings(&state.db, guild_id).await.ok()
    } else {
        None
    };

    let tmpl = backup::ServerTemplate {
        id: backup::generate_template_id(),
        name: form.name.trim().chars().take(50).collect(),
        description: form.description.map(|d| d.trim().chars().take(150).collect()).filter(|d: &String| !d.is_empty()),
        category: form.category.map(|c| c.trim().to_lowercase()).unwrap_or_else(|| "genel".to_owned()),
        snapshot,
        settings: safe_settings,
    };

    if let Err(e) = backup::save_template(&state.db, Some(guild_id), Some(session.user), &tmpl).await {
        tracing::warn!(guild = %guild_id, error = ?e, "Web panelden şablon kaydedilemedi");
    }

    Redirect::to(&format!("/panel/sunucu/{guild}/sablonlar")).into_response()
}

/// `GET /panel/sunucu/{guild}/sablon-disa-aktar`
pub async fn export_json(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(guild): Path<u64>,
) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !is_guild_admin(&state, session.user, guild_id).await {
        return forbidden();
    }

    let Some(snapshot) = backup::take(&state.cache, guild_id) else {
        return forbidden();
    };

    let safe_settings = transfer::export_safe_settings(&state.db, guild_id).await.ok();
    let tmpl = backup::ServerTemplate {
        id: backup::generate_template_id(),
        name: snapshot.guild_name.clone(),
        description: Some(format!("{} sunucusundan dışa aktarılan Xenon şablonu", snapshot.guild_name)),
        category: "genel".to_owned(),
        snapshot,
        settings: safe_settings,
    };

    let text = serde_json::to_string_pretty(&tmpl).unwrap_or_default();
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8".to_owned()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"server-template-{guild_id}.json\"")),
        ],
        text,
    )
        .into_response()
}
