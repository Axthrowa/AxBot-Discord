//! Panel sayfaları: genel bakış, sunucu, loglar, sistem. Sunucu tarafında üretilen sade HTML;
//! tek JavaScript dosyası canlı loglar için (`/panel/panel.js`). Tüm dinamik metinler kaçışlanır.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::{Form, Json};
use poise::serenity_prelude::{GuildId, UserId};
use serde::Deserialize;
use tracing::{info, warn};

use super::auth::{self, CsrfForm, Session};
use super::can_manage;
use crate::logbuffer;
use crate::setup_report::{self, State as FeatureState, Target};
use crate::web::{escape, WebState};

pub struct Page<'a> {
    pub title: &'a str,
    pub body: String,
}

/// Sunucu sayfalarında yan menüde eklenti listesi de gösterilir.
pub struct GuildNav<'a> {
    pub id: GuildId,
    pub name: &'a str,
    /// Açık olan eklentinin adresi; sunucu ana sayfasında boş.
    pub active: &'a str,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Nav {
    Home,
    Logs,
    System,
    Look,
}

const CSS: &str = concat!(include_str!("tokens.css"), include_str!("panel.css"));

fn document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"tr\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"robots\" content=\"noindex\">\
         <title>{} · AxBot Panel</title><style>{CSS}</style><script src=\"/panel/theme.js\"></script></head><body>{body}</body></html>",
        escape(title)
    )
}

/// Giriş gerektirmeyen geniş sayfalar (herkese açık sıralama).
pub fn layout_public(title: &str, body: &str) -> String {
    document(
        title,
        &format!("<main style=\"margin:0 auto\"><div class=\"brand\" style=\"margin-left:0\">AX<span>BOT</span></div>{body}</main>"),
    )
}

/// Oturumsuz sayfalar (giriş, hata): ortada tek kart.
pub fn layout_bare(page: Page<'_>) -> String {
    document(
        page.title,
        &format!("<div class=\"center\"><div class=\"box\"><div class=\"brand\">AX<span>BOT</span> PANEL</div><h1>{}</h1>{}</div></div>", escape(page.title), page.body),
    )
}

pub async fn layout(state: &WebState, session: &Session, nav: Nav, guild: Option<GuildNav<'_>>, page: Page<'_>) -> String {
    let (name, avatar) = match state.http.get_user(session.user).await {
        Ok(u) => (u.global_name.clone().unwrap_or_else(|| u.name.clone()), u.face()),
        Err(_) => ("Kullanıcı".to_owned(), String::new()),
    };
    let owner = state.panel.is_owner(session.user);
    let link = |target: Nav, href: &str, label: &str| {
        format!("<a class=\"nav{}\" href=\"{href}\">{label}</a>", if nav == target { " on" } else { "" })
    };
    let l = session.lang;
    let t_overview = format!("🏠 {}", crate::i18n::t(l, "panel.overview", &[]));
    let t_logs = format!("📜 {}", crate::i18n::t(l, "panel.logs", &[]));
    let t_system = format!("⚙️ {}", crate::i18n::t(l, "panel.system", &[]));
    let t_plugins = format!("🧩 {}", crate::i18n::t(l, "panel.plugins", &[]));
    let t_wizard = format!("🧭 {}", crate::i18n::t(l, "panel.wizard", &[]));
    let t_stats = format!("📈 {}", crate::i18n::t(l, "panel.stats", &[]));
    let t_transcripts = format!("🎫 {}", crate::i18n::t(l, "panel.transcripts", &[]));
    let t_templates = format!("🧩 {}", crate::i18n::t(l, "panel.templates", &[]));
    let t_health = format!("🩺 {}", crate::i18n::t(l, "panel.health", &[]));
    let t_logout = crate::i18n::t(l, "panel.logout", &[]);
    let t_connecting = crate::i18n::t(l, "panel.connecting", &[]);

    let mut aside = String::from("<div class=\"brand\">AX<span>BOT</span> PANEL</div>");
    aside.push_str(&link(Nav::Home, "/panel", &t_overview));
    aside.push_str(&link(Nav::Look, "/panel/gorunum", "🎨 Görünüm"));
    if owner {
        aside.push_str(&link(Nav::Logs, "/panel/loglar", &t_logs));
        aside.push_str(&link(Nav::System, "/panel/sistem", &t_system));
    }
    if let Some(guild) = &guild {
        let _ = write!(
            aside,
            "<div class=\"navh\">{}</div><a class=\"nav sm{}\" href=\"/panel/sunucu/{}\">{}</a>",
            escape(guild.name),
            if guild.active.is_empty() { " on" } else { "" },
            guild.id,
            escape(&t_plugins)
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/kurulum\">{}</a>",
            if guild.active == "kurulum" { " on" } else { "" },
            guild.id,
            escape(&t_wizard)
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/siralama\">📊 Sıralama</a>",
            if guild.active == "siralama" { " on" } else { "" },
            guild.id
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/istatistik\">{}</a>",
            if guild.active == "istatistik" { " on" } else { "" },
            guild.id,
            escape(&t_stats)
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/transkriptler\">{}</a>",
            if guild.active == "transkriptler" { " on" } else { "" },
            guild.id,
            escape(&t_transcripts)
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/sablonlar\">{}</a>",
            if guild.active == "sablonlar" { " on" } else { "" },
            guild.id,
            escape(&t_templates)
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/saglik\">{}</a>",
            if guild.active == "saglik" { " on" } else { "" },
            guild.id,
            escape(&t_health)
        );
        let mut group = "";
        for plugin in super::plugins::PLUGINS {
            if plugin.group != group {
                group = plugin.group;
                let _ = write!(aside, "<div class=\"navg\">{}</div>", escape(group));
            }
            let _ = write!(
                aside,
                "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/eklenti/{}\">{} {}</a>",
                if guild.active == plugin.slug { " on" } else { "" },
                guild.id,
                plugin.slug,
                plugin.icon,
                escape(plugin.name)
            );
        }
    }
    let lang_switch = format!(
        "<div style=\"display:flex;gap:6px;justify-content:center;margin:6px 0 2px;font-size:0.75rem;\">\
         <a href=\"?lang=tr\" onclick=\"document.cookie='lang=tr;path=/;max-age=31536000';location.reload();return false;\" style=\"color:{};font-weight:{};\">TR</a>\
         <span style=\"color:var(--muted)\">|</span>\
         <a href=\"?lang=en\" onclick=\"document.cookie='lang=en;path=/;max-age=31536000';location.reload();return false;\" style=\"color:{};font-weight:{};\">EN</a>\
         </div>",
        if l == crate::i18n::Lang::Tr { "var(--accent)" } else { "var(--muted)" },
        if l == crate::i18n::Lang::Tr { "bold" } else { "normal" },
        if l == crate::i18n::Lang::En { "var(--accent)" } else { "var(--muted)" },
        if l == crate::i18n::Lang::En { "bold" } else { "normal" },
    );
    let _ = write!(
        aside,
        "<div class=\"me\">{}<div>{}<small>{} · <span id=\"ws-live-badge\" style=\"color:var(--warn);font-weight:bold;\">○ {}</span></small></div></div>\
         <form method=\"post\" action=\"/panel/cikis\"><input type=\"hidden\" name=\"csrf\" value=\"{}\">\
         <button class=\"ghost\" style=\"width:100%\">{}</button></form>{lang_switch}",
        if avatar.is_empty() { String::new() } else { format!("<img src=\"{}\" alt=\"\">", escape(&avatar)) },
        escape(&name),
        if owner { "Bot sahibi" } else { "Sunucu yöneticisi" },
        escape(&t_connecting),
        escape(&session.csrf),
        escape(&t_logout),
    );
    document(page.title, &format!("<div class=\"shell\"><aside>{aside}</aside><main>{}</main></div>", page.body))
}

pub fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, Html(layout_bare(Page { title: "Yetkin yok", body: "<p class=\"lead\">Bu sayfayı görme yetkin yok.</p><p><a href=\"/panel\">← Panele dön</a></p>".to_owned() })))
        .into_response()
}

fn human_duration(d: Duration) -> String {
    let s = d.as_secs();
    let (days, hours, mins) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    match (days, hours) {
        (0, 0) => format!("{mins} dk"),
        (0, h) => format!("{h} sa {mins} dk"),
        (d, h) => format!("{d} gün {h} sa"),
    }
}

/// Türkiye saatiyle (UTC+3) tarih.
fn turkish_time(unix: i64) -> String {
    let offset = chrono::FixedOffset::east_opt(3 * 3600);
    match (chrono::DateTime::from_timestamp(unix, 0), offset) {
        (Some(t), Some(o)) => t.with_timezone(&o).format("%d.%m.%Y %H:%M").to_string(),
        _ => "?".to_owned(),
    }
}

pub fn build_time() -> String {
    turkish_time(env!("AXBOT_BUILD_UNIX").parse().unwrap_or(0))
}

fn card(key: &str, value: &str) -> String {
    format!("<div class=\"card\"><div class=\"k\">{}</div><div class=\"v\">{value}</div></div>", escape(key))
}

async fn status_cards(state: &WebState) -> String {
    let (online, latency) = match state.panel.shards.get() {
        Some(manager) => {
            let runners = manager.runners.lock().await;
            let online = !runners.is_empty() && runners.values().all(|r| r.stage == poise::serenity_prelude::ConnectionStage::Connected);
            let latency = runners.values().filter_map(|r| r.latency).max();
            (online, latency)
        }
        None => (false, None),
    };
    let guilds = state.cache.guilds();
    let members: u64 = guilds.iter().filter_map(|g| state.cache.guild(*g).map(|g| g.member_count)).sum();

    let started = Instant::now();
    let db_ok = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();
    let db_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let mut conn = state.redis.clone();
    let redis_ok = redis::cmd("PING").query_async::<String>(&mut conn).await.is_ok();
    let redis_ms = started.elapsed().as_millis();

    let memory = memory_stats::memory_stats().map_or_else(|| "?".to_owned(), |m| format!("{} MB", m.physical_mem / 1_048_576));
    let (warns, errors) = logbuffer::counts();
    let health = |ok: bool, ms: u128| if ok { format!("{ms} ms") } else { "<span class=\"badge b-err\">KAPALI</span>".to_owned() };

    let mut out = String::from("<div class=\"cards\">");
    out.push_str(&card(
        "Durum",
        if online { "<span class=\"badge b-ok\">ÇEVRİMİÇİ</span>" } else { "<span class=\"badge b-warn\">BAĞLANIYOR</span>" },
    ));
    out.push_str(&card("Sunucular", &guilds.len().to_string()));
    out.push_str(&card("Toplam üye", &members.to_string()));
    out.push_str(&card("Çalışma süresi", &human_duration(state.panel.started_at.elapsed())));
    out.push_str(&card("Discord gecikmesi", &latency.map_or_else(|| "?".to_owned(), |l| format!("{} ms", l.as_millis()))));
    out.push_str(&card("Bellek", &memory));
    out.push_str(&card("PostgreSQL", &health(db_ok, db_ms)));
    out.push_str(&card("Redis", &health(redis_ok, redis_ms)));
    out.push_str(&card("Ses bağlantıları", &state.panel.songbird.iter().count().to_string()));
    out.push_str(&card(
        "Uyarı / hata",
        &format!(
            "<a href=\"/panel/loglar\"><span class=\"badge {}\">{warns}</span> <span class=\"badge {}\">{errors}</span></a>",
            if warns > 0 { "b-warn" } else { "b-off" },
            if errors > 0 { "b-err" } else { "b-off" }
        ),
    ));
    out.push_str("</div>");
    out
}

struct GuildCard {
    id: GuildId,
    name: String,
    icon: Option<String>,
    members: u64,
}

async fn manageable_guilds(state: &WebState, user: UserId) -> Vec<GuildCard> {
    let mut list = Vec::new();
    for gid in state.cache.guilds() {
        if !can_manage(state, user, gid).await {
            continue;
        }
        if let Some(g) = state.cache.guild(gid) {
            list.push(GuildCard { id: gid, name: g.name.clone(), icon: g.icon_url(), members: g.member_count });
        }
    }
    list.sort_by_key(|g| g.name.to_lowercase());
    list
}

fn guild_icon(name: &str, icon: Option<&str>) -> String {
    match icon {
        Some(url) => format!("<img src=\"{}\" alt=\"\">", escape(url)),
        None => format!("<div class=\"gicon\">{}</div>", escape(&name.chars().take(2).collect::<String>())),
    }
}

pub async fn home(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let owner = state.panel.is_owner(session.user);
    let mut body = String::from("<h1>Genel bakış</h1>");
    if owner {
        let _ = write!(body, "<p class=\"sub\">Sürüm {} · derleme {}</p>", env!("CARGO_PKG_VERSION"), build_time());
        body.push_str(&status_cards(&state).await);
        body.push_str("<h2>Sunucular</h2>");
    } else {
        body.push_str("<p class=\"sub\">Yönetebildiğin sunucular</p>");
    }
    let guilds = manageable_guilds(&state, session.user).await;
    if guilds.is_empty() {
        body.push_str("<div class=\"box\">Botun bulunduğu ve 'Sunucuyu Yönet' iznin olan bir sunucu yok.</div>");
    } else {
        body.push_str("<div class=\"guilds\">");
        for g in &guilds {
            let _ = write!(
                body,
                "<a class=\"guild\" href=\"/panel/sunucu/{}\">{}<div><b>{}</b><div class=\"note\">{} üye</div></div></a>",
                g.id,
                guild_icon(&g.name, g.icon.as_deref()),
                escape(&g.name),
                g.members
            );
        }
        body.push_str("</div>");
    }
    Html(layout(&state, &session, Nav::Home, None, Page { title: "Genel bakış", body }).await).into_response()
}

pub async fn guild(State(state): State<WebState>, headers: HeaderMap, Path(guild): Path<u64>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await {
        return forbidden();
    }
    let report = match setup_report::build(&state.http, &state.cache, &state.db, &state.configs, guild_id).await {
        Ok(r) => r,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Panel: sunucu raporu alınamadı");
            return (StatusCode::INTERNAL_SERVER_ERROR, Html(layout_bare(Page { title: "Bir sorun oluştu", body: "<p>Sunucu bilgisi alınamadı.</p>".to_owned() })))
                .into_response();
        }
    };

    // Kanal / rol adları önbellekten (await yok; önbellek kilidi hemen bırakılır).
    let rows: String = {
        let g = state.cache.guild(guild_id);
        report
            .features
            .iter()
            .map(|f| {
                let state_html = match &f.state {
                    FeatureState::On(Target::Channel(c)) => {
                        let name = g.as_ref().and_then(|g| g.channels.get(c).map(|c| c.name.clone())).unwrap_or_default();
                        format!("<span class=\"badge b-ok\">AÇIK</span> #{}", escape(&name))
                    }
                    FeatureState::On(Target::Role(r)) => {
                        let name = g.as_ref().and_then(|g| g.roles.get(r).map(|r| r.name.clone())).unwrap_or_default();
                        format!("<span class=\"badge b-ok\">AÇIK</span> @{}", escape(&name))
                    }
                    FeatureState::On(Target::Text(t)) => format!("<span class=\"badge b-ok\">AÇIK</span> {}", escape(t)),
                    FeatureState::Broken => "<span class=\"badge b-warn\">SİLİNMİŞ</span>".to_owned(),
                    FeatureState::Off => "<span class=\"badge b-off\">KAPALI</span>".to_owned(),
                };
                format!("<tr><td>{}</td><td>{state_html}</td><td><code>{}</code></td></tr>", escape(f.name), escape(f.command))
            })
            .collect()
    };

    let plugins = match state.configs.get(&state.db, guild_id).await {
        Ok(cfg) => {
            super::plugins::cards(guild_id, &cfg, super::plugins::Extra::load(&state, guild_id).await)
        }
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Panel: sunucu ayarları okunamadı");
            String::new()
        }
    };
    let perms = if report.missing.is_empty() {
        "<div class=\"box\">✅ Botun tüm gerekli izinleri var.</div>".to_owned()
    } else {
        format!(
            "<div class=\"box err\">❌ Eksik izinler: <b>{}</b><br><span class=\"note\">Sunucu Ayarları → Roller → botun rolü → izinleri aç.</span></div>",
            escape(&crate::error::permission_names(report.missing))
        )
    };
    let role = if report.role_on_top {
        "<div class=\"box\">✅ Botun rolü en üstte.</div>"
    } else {
        "<div class=\"box warn\">⚠️ Botun rolü en üstte değil: altındaki rolleri veremez, o rollerdeki üyeleri susturamaz/atamaz. \
         Sunucu Ayarları → Roller → botun rolünü en üste sürükle.</div>"
    };
    let body = format!(
        "<p><a href=\"/panel\">← Sunucular</a></p>\
         <div class=\"ghead\">{icon}<div><h1>{name}</h1>\
         <div class=\"sub\" style=\"margin:0\">{members} üye · <a href=\"https://discord.com/channels/{id}\" target=\"_blank\" rel=\"noopener\">Discord'da aç</a></div></div></div>\
         <input type=\"search\" id=\"pfilter\" class=\"psearch\" placeholder=\"🔎 Eklenti ara…\" autocomplete=\"off\">\
         {plugins}<script src=\"/panel/panel.js\"></script><h2>Bot izinleri</h2>{perms}<div style=\"height:10px\"></div>{role}\
         <h2>Tüm özelliklerin durumu</h2><table><tr><th>Özellik</th><th>Durum</th><th>Discord'dan açmak / değiştirmek için</th></tr>{rows}</table>",
        icon = guild_icon(&report.guild_name, report.icon_url.as_deref()),
        name = escape(&report.guild_name),
        members = report.member_count,
        id = guild_id,
    );
    let nav = GuildNav { id: guild_id, name: &report.guild_name, active: "" };
    Html(layout(&state, &session, Nav::Home, Some(nav), Page { title: &report.guild_name, body }).await).into_response()
}

pub async fn logs(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    if !state.panel.is_owner(session.user) {
        return forbidden();
    }
    let body = "<h1>Loglar</h1><p class=\"sub\">Canlı: yeni satırlar 2 saniyede bir eklenir (bot açıldığından beri son 1000 satır).</p>\
                <div class=\"toolbar\"><label><input type=\"checkbox\" id=\"onlywarn\"> Sadece uyarı ve hatalar</label>\
                <label><input type=\"checkbox\" id=\"follow\" checked> Otomatik kaydır</label></div>\
                <pre id=\"log\"></pre><script src=\"/panel/panel.js\"></script>"
        .to_owned();
    Html(layout(&state, &session, Nav::Logs, None, Page { title: "Loglar", body }).await).into_response()
}

#[derive(Deserialize)]
pub struct LogQuery {
    after: Option<u64>,
    uyari: Option<u8>,
}

pub async fn logs_api(State(state): State<WebState>, headers: HeaderMap, Query(q): Query<LogQuery>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !state.panel.is_owner(session.user) {
        return StatusCode::FORBIDDEN.into_response();
    }
    Json(logbuffer::recent(q.after.unwrap_or(0), 400, q.uyari == Some(1))).into_response()
}

#[derive(Deserialize)]
pub struct SystemQuery {
    durum: Option<String>,
}

pub async fn system(State(state): State<WebState>, headers: HeaderMap, Query(query): Query<SystemQuery>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    if !state.panel.is_owner(session.user) {
        return forbidden();
    }
    let url = state.panel.public_url.get().unwrap_or_else(|| "—".to_owned());
    let invite = crate::commands::invite_url(state.cache.current_user().id);
    let restart = if state.panel.restartable {
        format!(
            "<form method=\"post\" action=\"/panel/sistem/yeniden-baslat\"><input type=\"hidden\" name=\"csrf\" value=\"{}\">\
             <button class=\"danger\">Botu yeniden başlat</button></form>\
             <p class=\"note\">Bot ~20 saniye çevrimdışı olur, sonra kendiliğinden açılır. Çalan müzik durur.</p>",
            escape(&session.csrf)
        )
    } else {
        "<p class=\"note\">Bot bekçisiz çalışıyor; panelden yeniden başlatılamaz.\
         <code>Botu Durdur.cmd</code> → <code>Botu Başlat.cmd</code> ile aç (bekçiyle açılır).</p>"
            .to_owned()
    };
    let presence = super::botpage::presence_form(&state, &session.csrf, query.durum.as_deref()).await;
    let body = format!(
        "<h1>Sistem</h1><p class=\"sub\">Sürüm, yeniden başlatma ve güncelleme</p>\
         <table><tr><th style=\"width:220px\">Sürüm</th><td>{version}</td></tr>\
         <tr><th>Derleme</th><td>{build}</td></tr>\
         <tr><th>Çalıştığı yer</th><td>{place}</td></tr>\
         <tr><th>Panel adresi</th><td><code>{url}</code></td></tr>\
         <tr><th>Yapay zekâ</th><td>{ai}</td></tr>\
         <tr><th>Üye olayları (intent)</th><td>{intent}</td></tr>\
         <tr><th>Discord ile giriş</th><td>{oauth}</td></tr>\
         <tr><th>Botu sunucuya ekle</th><td><a href=\"{invite}\" target=\"_blank\" rel=\"noopener\">Davet bağlantısı</a></td></tr></table>\
         <h2>Komut süreleri</h2><div class=\"box\">{timings}</div>\
         <h2>Bot durumu</h2><div class=\"box\">{presence}</div>\
         <h2>Yeniden başlat</h2><div class=\"box\">{restart}</div>\
         <h2>Güncelleme</h2><div class=\"box\">1. Bilgisayarında <code>Sunucu Güncellemesi Hazırla.cmd</code>'ye çift tıkla → \
         <code>dist\\AxBot-guncelleme.tar.gz</code> oluşur.<br>2. Dosyayı arkadaşına gönder.<br>\
         3. Arkadaşın sunucuda: <code>tar -xzf AxBot-guncelleme.tar.gz &amp;&amp; cd AxBot-sunucu &amp;&amp; sudo bash kur.sh</code><br>\
         Veriler korunur; güncelleme sonrası bu sayfadaki <b>Derleme</b> tarihi değişir.</div>",
        version = env!("CARGO_PKG_VERSION"),
        build = build_time(),
        place = if state.panel.in_docker { "Sunucu (Docker)" } else { "Bu bilgisayar (Windows)" },
        url = escape(&url),
        ai = escape(&state.panel.ai.as_deref().unwrap_or("kapalı").replace('`', "")),
        intent = if state.panel.members_intent { "açık" } else { "kapalı" },
        oauth = format_args!(
            "{} · Developer Portal → OAuth2 → Redirects'e eklenecek adres: <code>{}</code>",
            if auth::oauth_enabled() { "açık" } else { "kapalı (<code>.env</code>: <code>DISCORD_CLIENT_SECRET</code>)" },
            escape(&auth::oauth_redirect_uri(&state))
        ),
        invite = escape(&invite),
        timings = timings_table(),
    );
    Html(layout(&state, &session, Nav::System, None, Page { title: "Sistem", body }).await).into_response()
}

/// Bot açıldığından beri komut başına süreler (kullanıcının gördüğü süreye Discord gidiş-dönüşü eklenir).
fn timings_table() -> String {
    let rows = crate::timings::summaries();
    if rows.is_empty() {
        return "<p class=\"note\">Bot açıldığından beri komut kullanılmadı.</p>".to_owned();
    }
    let mut out = String::from("<table><tr><th>Komut</th><th>Kullanım</th><th>Tipik</th><th>En yavaş %5</th><th>En uzun</th></tr>");
    for row in rows {
        let _ = write!(
            out,
            "<tr><td><code>/{}</code></td><td>{}</td><td>{} ms</td><td>{} ms</td><td>{} ms</td></tr>",
            escape(&row.command),
            row.count,
            row.p50,
            row.p95,
            row.max
        );
    }
    out.push_str(
        "</table><p class=\"note\">Bot açıldığından beri, komut başına son 500 kullanım. Hedef: tipik süre 300 ms altı. \
         Oyunlar ve anketler oyuncu beklediği için sayılmaz.</p>",
    );
    out
}

pub async fn restart(State(state): State<WebState>, headers: HeaderMap, Form(form): Form<CsrfForm>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    if !state.panel.is_owner(session.user) || form.csrf != session.csrf || !state.panel.restartable {
        return forbidden();
    }
    let Some(manager) = state.panel.shards.get().cloned() else {
        return forbidden();
    };
    info!(user = %session.user, "Panelden yeniden başlatma istendi");
    // Cevap tarayıcıya ulaşsın diye kısa bir bekleme; kapanınca Docker botu yeniden açar.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        super::RESTART_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
        manager.shutdown_all().await;
    });
    Html(layout_bare(Page {
        title: "Yeniden başlatılıyor",
        body: "<p class=\"lead\">Bot kapanıp yeniden açılıyor. Bu sayfa 30 saniye sonra panele döner.</p>\
               <meta http-equiv=\"refresh\" content=\"30;url=/panel\">"
            .to_owned(),
    }))
    .into_response()
}

const SCRIPT: &str = r#"(() => {
  const filter = document.getElementById('pfilter');
  if (!filter) return;
  window.addEventListener('keydown', (e) => {
    if ((e.key === '/' || (e.ctrlKey && e.key === 'k')) && document.activeElement.tagName !== 'INPUT' && document.activeElement.tagName !== 'TEXTAREA') {
      e.preventDefault(); filter.focus();
    }
  });
  filter.addEventListener('input', () => {
    const q = filter.value.trim().toLocaleLowerCase('tr');
    document.querySelectorAll('.pcards').forEach(grid => {
      let any = false;
      grid.querySelectorAll('.pcard').forEach(card => {
        const show = !q || (card.dataset.ara || '').includes(q);
        card.style.display = show ? '' : 'none';
        any = any || show;
      });
      grid.style.display = any ? '' : 'none';
      const title = grid.previousElementSibling;
      if (title && title.classList.contains('pgroup')) title.style.display = any ? '' : 'none';
    });
  });
})();
(() => {
  document.querySelectorAll('.tpl').forEach(button => button.addEventListener('click', () => {
    let fields;
    try { fields = JSON.parse(button.dataset.doldur); } catch (e) { return; }
    let first = null;
    for (const [name, value] of Object.entries(fields)) {
      document.querySelectorAll('[name="' + name + '"]').forEach(el => {
        if (el.type === 'checkbox') el.checked = value === '1'; else el.value = value;
        first = first || el;
      });
    }
    if (first) first.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }));
})();
(() => {
  const box = document.getElementById('log');
  if (!box) return;
  const onlyWarn = document.getElementById('onlywarn');
  const follow = document.getElementById('follow');
  let after = 0, busy = false;
  async function tick() {
    if (busy) return; busy = true;
    try {
      const r = await fetch('/panel/api/loglar?after=' + after + '&uyari=' + (onlyWarn.checked ? 1 : 0), { credentials: 'same-origin' });
      if (r.ok) {
        const lines = await r.json();
        for (const l of lines) {
          after = Math.max(after, l.id);
          const row = document.createElement('div');
          row.className = 'l-' + l.level;
          const t = document.createElement('span'); t.className = 't'; t.textContent = l.time + ' ';
          row.appendChild(t);
          row.appendChild(document.createTextNode(l.level.padEnd(5) + ' ' + l.target + ': ' + l.text));
          box.appendChild(row);
        }
        while (box.childNodes.length > 1500) box.removeChild(box.firstChild);
        if (lines.length && follow.checked) box.scrollTop = box.scrollHeight;
      } else if (r.status === 401) { location.href = '/panel'; }
    } catch (e) {}
    busy = false;
  }
  onlyWarn.addEventListener('change', () => { box.textContent = ''; after = 0; tick(); });
  tick(); setInterval(tick, 2000);
})();
(() => {
  if (!location.pathname.startsWith('/panel')) return;
  let ws, timer;
  function connect() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(proto + '//' + location.host + '/panel/ws');
    ws.onopen = () => {
      const badge = document.getElementById('ws-live-badge');
      if (badge) { badge.textContent = '● Canlı'; badge.style.color = '#57f287'; }
    };
    ws.onmessage = (e) => {
      try {
        const ev = JSON.parse(e.data);
        if (window.onRealtimeEvent) window.onRealtimeEvent(ev);
        if (ev.type === 'track_change' && ev.data) {
          const trackEl = document.getElementById('now-track-title');
          if (trackEl) trackEl.textContent = ev.data.title;
        }
      } catch (err) {}
    };
    ws.onclose = () => {
      const badge = document.getElementById('ws-live-badge');
      if (badge) { badge.textContent = '○ Bağlanıyor'; badge.style.color = '#faa81a'; }
      clearTimeout(timer);
      timer = setTimeout(connect, 3000);
    };
  }
  connect();
})();
(() => {
  const preview = document.getElementById('embed-preview');
  if (!preview) return;
  const inTitle = document.querySelector('input[name="embed_title"]');
  const inDesc = document.querySelector('textarea[name="embed_description"]');
  const inColour = document.querySelector('input[name="embed_colour"]');
  const inAuthor = document.querySelector('input[name="embed_author"]');
  const inFooter = document.querySelector('input[name="embed_footer"]');
  const pTitle = document.getElementById('prev-title');
  const pDesc = document.getElementById('prev-desc');
  const pAuthor = document.getElementById('prev-author');
  const pFooter = document.getElementById('prev-footer');

  function update() {
    if (inColour && preview) preview.style.borderLeftColor = inColour.value;
    if (inTitle && pTitle) pTitle.textContent = inTitle.value || 'Gömülü Mesaj Başlığı';
    if (inDesc && pDesc) pDesc.textContent = inDesc.value || 'Mesaj açıklaması burada görünecektir.';
    if (inAuthor && pAuthor) {
      pAuthor.textContent = inAuthor.value;
      pAuthor.style.display = inAuthor.value ? 'block' : 'none';
    }
    if (inFooter && pFooter) {
      pFooter.textContent = inFooter.value;
      pFooter.style.display = inFooter.value ? 'block' : 'none';
    }
  }
  ['input', 'change'].forEach(ev => {
    document.addEventListener(ev, (e) => {
      if (e.target && e.target.name && e.target.name.startsWith('embed_')) update();
    });
  });
  update();
})();"#;

pub async fn theme_script() -> Response {
    ([(header::CONTENT_TYPE, "application/javascript; charset=utf-8")], include_str!("theme.js")).into_response()
}

pub async fn script() -> Response {
    ([(header::CONTENT_TYPE, "application/javascript; charset=utf-8")], SCRIPT).into_response()
}
