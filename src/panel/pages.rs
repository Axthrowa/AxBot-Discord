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
}

const CSS: &str = r#"
:root { color-scheme: dark; --bg:#111216; --panel:#1b1d24; --line:#2e313c; --text:#e8e9ee; --muted:#8f93a3;
        --accent:#5865f2; --ok:#3ba55d; --warn:#faa61a; --err:#ed4245; }
* { box-sizing: border-box; }
body { margin:0; background:var(--bg); color:var(--text); font-family:"Segoe UI",system-ui,-apple-system,sans-serif; }
a { color:#9aa4ff; text-decoration:none; } a:hover { text-decoration:underline; }
.shell { display:grid; grid-template-columns:230px 1fr; min-height:100vh; }
aside { background:#16171c; border-right:1px solid var(--line); padding:22px 16px; display:flex; flex-direction:column; gap:6px; }
.brand { font-weight:800; letter-spacing:.06em; font-size:1.05rem; margin:0 8px 18px; }
.brand span { color:var(--accent); }
.nav { display:block; padding:10px 12px; border-radius:10px; color:var(--text); }
.nav:hover { background:#22242d; text-decoration:none; }
.nav.on { background:var(--accent); color:#fff; }
.me { margin-top:auto; display:flex; align-items:center; gap:10px; padding:10px 8px; border-top:1px solid var(--line); }
.me img { width:34px; height:34px; border-radius:50%; }
.me small { color:var(--muted); display:block; }
main { padding:28px clamp(16px,4vw,44px); max-width:1200px; width:100%; }
h1 { margin:0 0 6px; font-size:1.6rem; } h2 { margin:30px 0 12px; font-size:1.15rem; }
.sub { color:var(--muted); margin:0 0 22px; }
.cards { display:grid; grid-template-columns:repeat(auto-fill,minmax(190px,1fr)); gap:14px; }
.card { background:var(--panel); border:1px solid var(--line); border-radius:14px; padding:16px; }
.card .k { color:var(--muted); font-size:.82rem; } .card .v { font-size:1.35rem; font-weight:700; margin-top:6px; }
.guilds { display:grid; grid-template-columns:repeat(auto-fill,minmax(260px,1fr)); gap:14px; }
.guild { display:flex; gap:14px; align-items:center; background:var(--panel); border:1px solid var(--line); border-radius:14px; padding:14px; color:var(--text); }
.guild:hover { border-color:var(--accent); text-decoration:none; }
.ghead { display:flex; gap:16px; align-items:center; margin:8px 0 22px; }
.guild img, .ghead img, .gicon { width:52px; height:52px; border-radius:14px; background:#2b2d38; display:grid; place-items:center; font-weight:700; flex:none; }
.badge { display:inline-block; padding:2px 9px; border-radius:99px; font-size:.75rem; font-weight:700; }
.b-ok { background:#1f3d2a; color:#7ee2a0; } .b-warn { background:#4a3a14; color:#ffd27a; } .b-err { background:#4a1d1f; color:#ff9a9c; } .b-off { background:#2a2c35; color:var(--muted); }
table { width:100%; border-collapse:collapse; background:var(--panel); border:1px solid var(--line); border-radius:14px; overflow:hidden; }
td, th { text-align:left; padding:11px 14px; border-bottom:1px solid var(--line); vertical-align:top; }
th { color:var(--muted); font-weight:600; font-size:.85rem; } tr:last-child td { border-bottom:0; }
code { background:#262833; padding:2px 6px; border-radius:6px; font-size:.88em; }
.box { background:var(--panel); border:1px solid var(--line); border-radius:14px; padding:16px 18px; line-height:1.6; }
.box.warn { border-color:#6b5420; } .box.err { border-color:#6b2a2c; }
button, .btn { background:var(--accent); color:#fff; border:0; border-radius:10px; padding:11px 18px; font:inherit; font-weight:700; cursor:pointer; }
button.danger { background:var(--err); } button.ghost { background:#2a2c35; }
#log { background:#0c0d10; border:1px solid var(--line); border-radius:12px; padding:12px; height:65vh; overflow:auto;
       font:12.5px/1.5 ui-monospace,Consolas,monospace; white-space:pre-wrap; word-break:break-word; margin:0; }
.l-WARN { color:#ffd27a; } .l-ERROR { color:#ff9a9c; } .l-DEBUG, .l-TRACE { color:#7d8190; } .t { color:#7d8190; }
.navh { margin:14px 8px 4px; font-weight:700; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
.navg { margin:12px 12px 2px; color:var(--muted); font-size:.72rem; font-weight:700; letter-spacing:.08em; text-transform:uppercase; }
.nav.sm { padding:7px 12px; font-size:.93rem; }
.pcards { display:grid; grid-template-columns:repeat(auto-fill,minmax(250px,1fr)); gap:14px; }
.pcard { display:flex; flex-direction:column; gap:8px; background:var(--panel); border:1px solid var(--line); border-radius:14px; padding:18px; color:var(--text); }
.pcard:hover { border-color:var(--accent); text-decoration:none; }
.pico { width:44px; height:44px; border-radius:12px; background:#262a3d; display:grid; place-items:center; font-size:1.4rem; }
.ptitle { font-weight:700; display:flex; gap:8px; align-items:center; flex-wrap:wrap; }
.plink { margin-top:auto; color:#9aa4ff; font-weight:600; font-size:.9rem; }
.phead { display:flex; justify-content:space-between; align-items:flex-start; gap:16px; }
.sect { margin-bottom:16px; } .sect h3 { margin:0 0 12px; font-size:1.02rem; }
.sect h4 { margin:14px 0 6px; font-size:.8rem; color:var(--muted); text-transform:uppercase; letter-spacing:.06em; } .sect h4:first-child { margin-top:0; }
input[type=color] { width:64px; height:40px; padding:2px; background:#12131a; border:1px solid var(--line); border-radius:10px; cursor:pointer; }
input[type=text].wide { max-width:560px; }
.preview { display:block; width:100%; max-width:560px; border-radius:12px; border:1px solid var(--line); margin-top:6px; min-height:60px; background:#12131a; }
input[type=text] { width:100%; max-width:260px; background:#12131a; color:var(--text); border:1px solid var(--line); border-radius:10px; padding:10px 12px; font:inherit; }
.sect td .note { margin:2px 0 0; } .sect td select { max-width:240px; }
.field { display:block; margin-bottom:14px; } .field:last-child { margin-bottom:0; }
.field > span { display:block; font-weight:600; font-size:.9rem; margin-bottom:6px; }
.field small { display:block; color:var(--muted); margin-top:6px; line-height:1.5; }
select, textarea, input[type=number] { width:100%; max-width:560px; background:#12131a; color:var(--text); border:1px solid var(--line);
  border-radius:10px; padding:10px 12px; font:inherit; }
textarea { max-width:100%; resize:vertical; } input[type=number] { max-width:160px; }
select:focus, textarea:focus, input:focus { outline:2px solid var(--accent); outline-offset:1px; }
.savebar { display:flex; gap:10px; align-items:center; margin-top:6px; } .btn.ghost { background:#2a2c35; }
.btn:hover { text-decoration:none; }
.check { display:flex; align-items:center; gap:8px; padding:5px 0; cursor:pointer; }
.check input { width:17px; height:17px; accent-color:var(--accent); flex:none; }
.checks { display:grid; grid-template-columns:repeat(auto-fill,minmax(210px,1fr)); gap:2px 16px; max-height:260px; overflow:auto;
  background:#12131a; border:1px solid var(--line); border-radius:10px; padding:8px 12px; }
.checks.col { grid-template-columns:1fr; max-height:none; background:none; border:0; padding:0; margin-bottom:10px; }
.sect table { margin-bottom:4px; } .sect .note { margin:8px 0 0; }
.board { display:flex; flex-direction:column; gap:8px; }
.brow { display:flex; align-items:center; gap:14px; background:var(--panel); border:1px solid var(--line); border-radius:14px; padding:12px 16px; }
.brank { width:44px; text-align:center; font-weight:800; font-size:1.05rem; color:var(--muted); flex:none; }
.bava { width:44px; height:44px; border-radius:50%; background:#2b2d38; flex:none; }
.bmain { flex:1; min-width:0; } .bmain b { display:block; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
.bmain small, .blevel small, .bxp small { color:var(--muted); font-size:.72rem; }
.bbar { height:7px; border-radius:99px; background:#2a2c35; margin:6px 0 4px; overflow:hidden; }
.bbar span { display:block; height:100%; background:var(--accent); border-radius:99px; }
.blevel, .bxp { text-align:center; min-width:74px; } .blevel b, .bxp b { display:block; font-size:1.1rem; }
@media (max-width:560px) { .bxp { display:none; } }
.heat { display:grid; grid-template-columns:38px repeat(24, 1fr); gap:3px; background:var(--panel); border:1px solid var(--line); border-radius:14px; padding:14px; align-items:center; }
.heat small { color:var(--muted); font-size:.62rem; text-align:center; } .heat i { display:block; aspect-ratio:1; border-radius:3px; background:var(--accent); min-height:8px; }
.tpls { display:flex; flex-wrap:wrap; gap:8px; } .tpls .btn { padding:9px 14px; font-weight:600; }
.ptop { display:flex; justify-content:space-between; align-items:center; }
.sw { width:42px; height:24px; border-radius:99px; background:#3a3d4a; position:relative; flex:none; transition:background .15s; }
.sw::after { content:""; position:absolute; top:3px; left:3px; width:18px; height:18px; border-radius:50%; background:#c9cbd5; transition:left .15s; }
.sw.on { background:var(--ok); } .sw.on::after { left:21px; background:#fff; }
.pcard.on { border-color:#2f5d41; } .pcard.on .pico { background:#1f3d2a; }
.psearch { width:100%; max-width:420px !important; margin:0 0 4px; background:#12131a; color:var(--text); border:1px solid var(--line); border-radius:12px; padding:11px 14px; font:inherit; }
.check input[type=checkbox] { appearance:none; -webkit-appearance:none; width:38px !important; height:22px !important; border-radius:99px; background:#3a3d4a; position:relative; cursor:pointer; transition:background .15s; }
.check input[type=checkbox]::after { content:""; position:absolute; top:3px; left:3px; width:16px; height:16px; border-radius:50%; background:#c9cbd5; transition:left .15s; }
.check input[type=checkbox]:checked { background:var(--accent); } .check input[type=checkbox]:checked::after { left:19px; background:#fff; }
.checks .check input[type=checkbox] { width:17px !important; height:17px !important; border-radius:5px; }
.checks .check input[type=checkbox]::after { display:none; }
.checks .check input[type=checkbox]:checked { box-shadow:inset 0 0 0 3px #12131a; }
.chart { display:flex; align-items:flex-end; gap:6px; height:170px; background:var(--panel); border:1px solid var(--line); border-radius:14px; padding:14px 14px 8px; }
.chart .col { flex:1; display:flex; flex-direction:column; justify-content:flex-end; align-items:center; gap:4px; height:100%; min-width:0; }
.chart .bar { width:100%; max-width:38px; background:var(--accent); border-radius:6px 6px 2px 2px; min-height:2px; }
.chart small { color:var(--muted); font-size:.68rem; white-space:nowrap; } .chart b { font-size:.72rem; font-weight:600; }
.flash { border-radius:12px; padding:12px 16px; margin-bottom:16px; font-weight:600; }
.flash.ok { background:#1f3d2a; color:#9df0b8; } .flash.err { background:#4a1d1f; color:#ffb3b5; }
.toolbar { display:flex; gap:14px; align-items:center; margin-bottom:10px; flex-wrap:wrap; }
.center { min-height:100vh; display:grid; place-items:center; padding:16px; }
.center .box { max-width:520px; width:100%; padding:28px; }
.lead { line-height:1.6; color:#c9cbd5; } .note { color:var(--muted); font-size:.85rem; }
@media (max-width:760px) { .shell { grid-template-columns:1fr; } aside { flex-direction:row; flex-wrap:wrap; border-right:0; border-bottom:1px solid var(--line); }
  .brand { width:100%; margin-bottom:6px; } .me { margin-top:0; border-top:0; } }
"#;

fn document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"tr\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"robots\" content=\"noindex\">\
         <title>{} · AxBot Panel</title><style>{CSS}</style></head><body>{body}</body></html>",
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
    let mut aside = String::from("<div class=\"brand\">AX<span>BOT</span> PANEL</div>");
    aside.push_str(&link(Nav::Home, "/panel", "🏠 Genel bakış"));
    if owner {
        aside.push_str(&link(Nav::Logs, "/panel/loglar", "📜 Loglar"));
        aside.push_str(&link(Nav::System, "/panel/sistem", "⚙️ Sistem"));
    }
    if let Some(guild) = &guild {
        let _ = write!(
            aside,
            "<div class=\"navh\">{}</div><a class=\"nav sm{}\" href=\"/panel/sunucu/{}\">🧩 Eklentiler</a>",
            escape(guild.name),
            if guild.active.is_empty() { " on" } else { "" },
            guild.id
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/kurulum\">🧭 Kurulum sihirbazı</a>",
            if guild.active == "kurulum" { " on" } else { "" },
            guild.id
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/siralama\">📊 Sıralama</a>",
            if guild.active == "siralama" { " on" } else { "" },
            guild.id
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/istatistik\">📈 İstatistikler</a>",
            if guild.active == "istatistik" { " on" } else { "" },
            guild.id
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/saglik\">🩺 Sağlık kontrolü</a>",
            if guild.active == "saglik" { " on" } else { "" },
            guild.id
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
    let _ = write!(
        aside,
        "<div class=\"me\">{}<div>{}<small>{}</small></div></div>\
         <form method=\"post\" action=\"/panel/cikis\"><input type=\"hidden\" name=\"csrf\" value=\"{}\">\
         <button class=\"ghost\" style=\"width:100%\">Çıkış</button></form>",
        if avatar.is_empty() { String::new() } else { format!("<img src=\"{}\" alt=\"\">", escape(&avatar)) },
        escape(&name),
        if owner { "Bot sahibi" } else { "Sunucu yöneticisi" },
        escape(&session.csrf),
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
    );
    Html(layout(&state, &session, Nav::System, None, Page { title: "Sistem", body }).await).into_response()
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
})();"#;

pub async fn script() -> Response {
    ([(header::CONTENT_TYPE, "application/javascript; charset=utf-8")], SCRIPT).into_response()
}
