//! Botun web sunucusu: yönetim paneli (`/panel`, bkz. `panel` modülü) ve ban itirazı formu.
//! Dışarıdan erişim `WEB_PUBLIC_URL` ya da Cloudflare tüneliyle (https) olur.
//!
//! Yasaklıyken botla ortak sunucusu kalmayan kişi Discord butonlarını kullanamaz; bu yüzden yasak
//! DM'inde kişiye özel, tek kullanımlık bir bağlantı (`/itiraz/<anahtar>`) verilir. Anahtar 244 bit
//! rastgeledir, 30 gün geçerlidir ve itiraz gönderilince kullanılmış sayılır.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Form, Router};
use poise::serenity_prelude::{Cache, Http};
use serde::Deserialize;
use sqlx::PgPool;
use tracing::{info, warn};

use crate::appeals::{self, Filed, TokenState};

const MAX_ANSWER: usize = 1000;

#[derive(Clone)]
pub struct WebState {
    pub http: Arc<Http>,
    pub cache: Arc<Cache>,
    pub db: PgPool,
    pub redis: redis::aio::ConnectionManager,
    pub configs: crate::guild_config::GuildConfigs,
    pub panel: Arc<crate::panel::PanelInfo>,
    pub leveling: Arc<crate::leveling::Leveling>,
    pub settings: crate::settings::PluginSettings,
    pub automod: Arc<crate::automod::AutoMod>,
    pub modlog: Arc<crate::modlog::ModLog>,
    pub ai: Arc<crate::ai::Ai>,
    pub streams: Arc<crate::streams::Streams>,
    pub autoresponder: Arc<crate::autoresponder::AutoResponder>,
    /// Dış istekler (YouTube kanalı doğrulama) için ortak istemci.
    pub http_client: reqwest::Client,
    pub tags: Arc<crate::tags::Tags>,
    pub scheduler: Arc<crate::scheduler::Scheduler>,
    pub music: crate::music::Music,
    pub event_bus: tokio::sync::broadcast::Sender<crate::panel::RealtimeEvent>,
}

pub fn router(state: WebState) -> Router {
    Router::new()
        .route("/", get(|| async { "AxBot çalışıyor ✅" }))
        .route("/itiraz/{token}", get(show_form).post(submit_form))
        .merge(crate::panel::router())
        // En büyük form (moderasyon sayfasındaki 500 kelimelik listeler; Türkçe harfler adres kodlamasında
        // 6 bayta çıkar) için yeterli; daha büyük istekler reddedilir.
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(axum::middleware::map_response(security_headers))
        .with_state(state)
}

/// Tarayıcı güvenlik başlıkları: sayfa başka sitede çerçevelenemez, dış kaynak yüklenmez,
/// bağlantıdaki gizli anahtar Referer ile sızmaz ve sayfa önbelleğe alınmaz.
async fn security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        // Panel: kendi betiği, Discord avatarları ve kendi adresine istekler dışında hiçbir şey yüklenmez.
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; img-src 'self' https://cdn.discordapp.com;              connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    response
}

/// Web sunucusunu arka planda başlatır.
pub fn spawn(bind: SocketAddr, state: WebState) {
    tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(bind).await {
            Ok(l) => l,
            Err(e) => {
                warn!(%bind, error = %e, "Web sunucusu başlatılamadı (port kullanımda olabilir; WEB_BIND ile değiştir)");
                return;
            }
        };
        info!(%bind, "Web sunucusu (panel + itiraz formu) dinliyor");
        if let Err(e) = axum::serve(listener, router(state)).await {
            warn!(error = %e, "Web formu sunucusu durdu");
        }
    });
}

#[derive(Deserialize)]
struct AppealForm {
    why: String,
    plea: String,
}

async fn show_form(State(state): State<WebState>, Path(token): Path<String>) -> Response {
    match appeals::token_state(&state.db, &token).await {
        Ok(TokenState::Valid { guild_id, user }) => {
            let guild = guild_name(&state, guild_id);
            let user_name = state
                .http
                .get_user(user)
                .await
                .map(|u| u.global_name.unwrap_or(u.name))
                .unwrap_or_else(|_| "kullanıcı".to_owned());
            Html(page(
                "Yasak itirazı",
                &format!(
                    r#"<p class="lead">Merhaba <b>{user}</b>, <b>{guild}</b> sunucusundaki yasağına itiraz ediyorsun.
                    İtirazın sunucu yetkililerine iletilecek; sonuç sana <b>Discord DM'i</b> ile bildirilecek.</p>
                    <form method="post">
                      <label for="why">Neden yasaklandığını düşünüyorsun?</label>
                      <textarea id="why" name="why" maxlength="{max}" required rows="4"></textarea>
                      <label for="plea">Yasağın neden kaldırılmalı?</label>
                      <textarea id="plea" name="plea" maxlength="{max}" required rows="5"></textarea>
                      <button type="submit">İtirazı gönder</button>
                    </form>
                    <p class="note">Bu bağlantı sadece sana özel ve tek kullanımlık; kimseyle paylaşma.</p>"#,
                    user = escape(&user_name),
                    guild = escape(&guild),
                    max = MAX_ANSWER,
                ),
            ))
            .into_response()
        }
        Ok(other) => token_problem(&other).into_response(),
        Err(e) => server_error(&e),
    }
}

async fn submit_form(State(state): State<WebState>, Path(token): Path<String>, Form(form): Form<AppealForm>) -> Response {
    let (guild_id, user_id) = match appeals::token_state(&state.db, &token).await {
        Ok(TokenState::Valid { guild_id, user }) => (guild_id, user),
        Ok(other) => return token_problem(&other).into_response(),
        Err(e) => return server_error(&e),
    };
    let (why, plea) = (form.why.trim(), form.plea.trim());
    if why.is_empty() || plea.is_empty() {
        return (StatusCode::BAD_REQUEST, Html(page("Eksik bilgi", "<p>İki soruyu da cevaplamalısın. Geri dönüp formu tamamla.</p>"))).into_response();
    }
    if why.chars().count() > MAX_ANSWER || plea.chars().count() > MAX_ANSWER {
        return (StatusCode::BAD_REQUEST, Html(page("Çok uzun", &format!("<p>Cevaplar en fazla {MAX_ANSWER} karakter olabilir.</p>")))).into_response();
    }
    let user = match state.http.get_user(user_id).await {
        Ok(u) => u,
        Err(e) => return server_error(&anyhow::Error::from(e)),
    };
    let channel = match appeals::appeal_channel(&state.db, guild_id).await {
        Ok(c) => c,
        Err(e) => return server_error(&e),
    };
    let filed = match appeals::file_appeal(&state.http, &state.db, guild_id, channel, &user, why, plea).await {
        Ok(f) => f,
        Err(e) => return server_error(&e),
    };
    // Gönderilen ya da artık gerekmeyen (yasak kalkmış) itirazlarda bağlantı bir daha kullanılamaz.
    if matches!(filed, Filed::Sent | Filed::NotBanned) {
        if let Err(e) = appeals::mark_token_used(&state.db, &token).await {
            warn!(error = ?e, "İtiraz anahtarı kullanıldı olarak işaretlenemedi");
        }
    }
    let (title, status) = match &filed {
        Filed::Sent => ("İtirazın gönderildi ✅", StatusCode::OK),
        Filed::Failed => ("Bir sorun oluştu", StatusCode::BAD_GATEWAY),
        _ => ("İtiraz gönderilemedi", StatusCode::OK),
    };
    (status, Html(page(title, &format!("<p>{}</p>", escape(&strip_markdown(&filed.message())))))).into_response()
}

fn guild_name(state: &WebState, guild_id: poise::serenity_prelude::GuildId) -> String {
    state.cache.guild(guild_id).map_or_else(|| "sunucu".to_owned(), |g| g.name.clone())
}

fn token_problem(state: &TokenState) -> (StatusCode, Html<String>) {
    let (title, text) = match state {
        TokenState::Used => ("Bu bağlantı kullanıldı", "Bu bağlantıyla zaten bir itiraz gönderildi. Sonuç sana Discord DM'i ile bildirilecek."),
        TokenState::Expired => ("Bağlantının süresi doldu", "İtiraz bağlantıları 30 gün geçerlidir."),
        _ => ("Geçersiz bağlantı", "Bu itiraz bağlantısı geçersiz. Yasak DM'indeki bağlantıyı eksiksiz açtığından emin ol."),
    };
    (StatusCode::NOT_FOUND, Html(page(title, &format!("<p>{text}</p>"))))
}

fn server_error(e: &anyhow::Error) -> Response {
    warn!(error = ?e, "Web formu hatası");
    (StatusCode::INTERNAL_SERVER_ERROR, Html(page("Bir sorun oluştu", "<p>Lütfen biraz sonra tekrar dene.</p>"))).into_response()
}

/// HTML'e güvenle yazmak için özel karakterleri kaçışlar.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Discord biçimlendirmesini (`**`, `_`) düz metne çevirir.
fn strip_markdown(text: &str) -> String {
    text.replace("**", "").replace('_', "")
}

fn page(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="tr">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>{title} · AxBot</title>
<style>
  :root {{ color-scheme: dark; }}
  * {{ box-sizing: border-box; }}
  body {{ margin: 0; min-height: 100vh; display: grid; place-items: center; padding: 16px;
         background: radial-gradient(circle at top, #2b2d42, #111216 70%); color: #e8e9ee;
         font-family: "Segoe UI", system-ui, -apple-system, sans-serif; }}
  main {{ width: 100%; max-width: 560px; background: #1d1f26; border: 1px solid #343744;
         border-radius: 18px; padding: 28px; box-shadow: 0 20px 60px rgba(0,0,0,.45); }}
  h1 {{ margin: 0 0 12px; font-size: 1.5rem; }}
  .brand {{ color: #8f93a3; font-size: .85rem; letter-spacing: .08em; margin-bottom: 18px; }}
  .lead {{ line-height: 1.55; color: #c9cbd5; }}
  label {{ display: block; margin: 18px 0 6px; font-weight: 600; }}
  textarea {{ width: 100%; resize: vertical; border-radius: 10px; border: 1px solid #3c4050;
             background: #14151b; color: #e8e9ee; padding: 12px; font: inherit; }}
  textarea:focus {{ outline: 2px solid #5865f2; border-color: transparent; }}
  button {{ margin-top: 22px; width: 100%; padding: 14px; border: 0; border-radius: 10px;
           background: #5865f2; color: white; font-size: 1rem; font-weight: 700; cursor: pointer; }}
  button:hover {{ background: #4752c4; }}
  .note {{ color: #8f93a3; font-size: .85rem; margin-top: 16px; }}
</style>
</head>
<body><main><div class="brand">AXBOT · BAN İTİRAZI</div><h1>{title}</h1>{body}</main></body>
</html>"#,
        title = escape(title),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html() {
        assert_eq!(escape(r#"<script>alert("x")</script> & 'a'"#), "&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;a&#39;");
    }

    #[test]
    fn page_escapes_title() {
        let html = page("<b>", "<p>gövde</p>");
        assert!(html.contains("<title>&lt;b&gt; · AxBot</title>"));
        assert!(html.contains("<p>gövde</p>"));
    }
}

/// Gerçek PostgreSQL ve yerel web sunucusuyla uçtan uca deneme (Discord'a mesaj gönderilmez:
/// sahte sunucu ID'si 1'de itiraz kanalı yok). Elle: `cargo test live_web_form -- --ignored --nocapture`
#[cfg(test)]
mod live_tests {
    use super::*;
    use poise::serenity_prelude::{GuildId, UserId};

    #[tokio::test]
    #[ignore = "gerçek PostgreSQL gerektirir"]
    async fn live_web_form() -> anyhow::Result<()> {
        let _ = dotenvy::dotenv();
        let config = crate::config::DatabaseConfig { url: std::env::var("DATABASE_URL")?, max_connections: 2 };
        let db = crate::db::connect(&config).await?;
        let token_env = std::env::var("DISCORD_TOKEN")?;
        let http = Arc::new(crate::net::discord_http(&token_env, crate::net::http_client()?));
        let redis = crate::cache::connect(&std::env::var("REDIS_URL")?).await?;
        // Sahte bot sahibi: kullanıcı 1 (panel girişi bu kullanıcıyla denenir).
        let panel = Arc::new(crate::panel::PanelInfo::new(
            std::collections::HashSet::from([UserId::new(1)]),
            crate::panel::PublicUrl::new(None),
            false,
            songbird::Songbird::serenity(),
            None,
            false,
        ));
        let (event_bus, _) = tokio::sync::broadcast::channel(16);
        let state = WebState {
            http,
            cache: Arc::new(Cache::new()),
            db: db.clone(),
            redis: redis.clone(),
            configs: crate::guild_config::GuildConfigs::default(),
            panel,
            leveling: Arc::new(crate::leveling::Leveling::new()),
            settings: crate::settings::PluginSettings::default(),
            automod: Arc::new(crate::automod::AutoMod::new()?),
            modlog: Arc::new(crate::modlog::ModLog::new(crate::settings::PluginSettings::default())),
            ai: Arc::new(crate::ai::Ai::new(None, reqwest::Client::new())),
            streams: Arc::new(crate::streams::Streams::new(reqwest::Client::new(), None, None)),
            autoresponder: Arc::new(crate::autoresponder::AutoResponder::default()),
            http_client: reqwest::Client::new(),
            tags: Arc::new(crate::tags::Tags::default()),
            scheduler: crate::scheduler::Scheduler::new(db.clone()),
            music: crate::music::Music::new(songbird::Songbird::serenity(), reqwest::Client::new()),
            event_bus,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move { axum::serve(listener, router(state)).await });
        let client = reqwest::Client::new();
        let base = format!("http://{addr}");

        // Botun kendi kullanıcı kimliği (gerçek bir kullanıcı; sahte sunucuda yasaklı değil).
        let token = appeals::create_token(&db, GuildId::new(1), UserId::new(1557460476525420605)).await?;
        let form_page = client.get(format!("{base}/itiraz/{token}")).send().await?;
        let status = form_page.status();
        let html = form_page.text().await?;
        println!("form: {status} · form var: {}", html.contains("<form"));
        assert!(status.is_success() && html.contains("İtirazı gönder"));

        let bad = client.get(format!("{base}/itiraz/yanlis")).send().await?;
        assert_eq!(bad.status(), 404);

        let empty = client.post(format!("{base}/itiraz/{token}")).form(&[("why", " "), ("plea", "x")]).send().await?;
        assert_eq!(empty.status(), 400);

        let xss = client
            .post(format!("{base}/itiraz/{token}"))
            .form(&[("why", "<script>alert(1)</script>"), ("plea", "lütfen")])
            .send()
            .await?;
        let body = xss.text().await?;
        println!("gönderim sonucu: {}", body.lines().find(|l| l.contains("<p>")).unwrap_or_default().trim());
        // Sahte sunucuda itiraz kanalı olmadığı için "itiraz kabul etmiyor" denmeli; betik çalışmamalı.
        assert!(body.contains("itiraz kabul etmiyor") && !body.contains("<script>alert"));

        let huge = "a".repeat(20 * 1024);
        let too_big = client.post(format!("{base}/itiraz/{token}")).form(&[("why", huge.as_str()), ("plea", "x")]).send().await?;
        assert_eq!(too_big.status(), 413);

        sqlx::query("DELETE FROM appeal_tokens WHERE guild_id = 1").execute(&db).await?;

        // ---- Panel ----
        // Oturumsuz: giriş gerekli.
        assert_eq!(client.get(format!("{base}/panel")).send().await?.status(), 401);
        let login = crate::panel::create_login(&redis, UserId::new(1)).await?;
        // GET anahtarı harcamaz (bağlantı önizleyicileri için), POST harcar.
        assert!(client.get(format!("{base}/panel/giris/{login}")).send().await?.status().is_success());
        let no_redirect = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build()?;
        let posted = no_redirect.post(format!("{base}/panel/giris/{login}")).send().await?;
        let cookie = posted
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("çerez yok"))?;
        println!("panel çerezi alındı: {}…", &cookie[..20.min(cookie.len())]);
        assert_eq!(posted.status(), 303);
        // Aynı anahtar ikinci kez kullanılamaz.
        assert_eq!(no_redirect.post(format!("{base}/panel/giris/{login}")).send().await?.status(), 404);

        let home = client.get(format!("{base}/panel")).header("cookie", &cookie).send().await?;
        let csp = home.headers().get("content-security-policy").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
        let html = home.text().await?;
        assert!(html.contains("Genel bakış") && html.contains("PostgreSQL") && csp.contains("script-src 'self'"));
        let logs = client.get(format!("{base}/panel/api/loglar?after=0")).header("cookie", &cookie).send().await?;
        assert!(logs.status().is_success());
        let sys = client.get(format!("{base}/panel/sistem")).header("cookie", &cookie).send().await?.text().await?;
        // Windows'ta (Docker değil) yeniden başlatma butonu gösterilmez.
        assert!(sys.contains("Sürüm") && !sys.contains("yeniden-baslat\""));
        // CSRF değeri olmadan yeniden başlatma reddedilir.
        let restart = client
            .post(format!("{base}/panel/sistem/yeniden-baslat"))
            .header("cookie", &cookie)
            .form(&[("csrf", "yanlis")])
            .send()
            .await?;
        assert_eq!(restart.status(), 403);
        println!("panel: giriş, ana sayfa, loglar, sistem ve CSRF kontrolü tamam");
        Ok(())
    }
}
