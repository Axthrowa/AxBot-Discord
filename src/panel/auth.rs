//! Panel girişi: Discord'daki `/panel` komutunun verdiği tek kullanımlık bağlantı → oturum çerezi.
//!
//! Redis anahtarları:
//! - `panel:giris:<anahtar>` → kullanıcı ID'si (10 dk)
//! - `panel:oturum:<oturum>` → `kullanıcı:csrf` (7 gün)
//!
//! Formlar (çıkış, yeniden başlatma, ayarlar) oturuma bağlı bir CSRF değeri taşır; başka bir site
//! kullanıcının tarayıcısından panele istek yaptıramaz. Çerez HttpOnly + SameSite=Lax (+ https'te Secure).

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use poise::serenity_prelude::UserId;
use redis::aio::ConnectionManager;
use serde::Deserialize;
use tracing::{info, warn};

use super::pages::{layout_bare, Page};
use crate::web::{escape, WebState};

const COOKIE: &str = "axbot_oturum";
const LOGIN_SECS: u64 = 10 * 60;
const SESSION_SECS: u64 = 7 * 24 * 60 * 60;

pub struct Session {
    pub id: String,
    pub user: UserId,
    pub csrf: String,
}

fn random_hex() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

fn valid_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// `/panel` komutu için tek kullanımlık giriş anahtarı oluşturur.
pub async fn create_login(redis: &ConnectionManager, user: UserId) -> anyhow::Result<String> {
    let token = random_hex();
    let mut conn = redis.clone();
    redis::cmd("SET")
        .arg(format!("panel:giris:{token}"))
        .arg(user.get())
        .arg("EX")
        .arg(LOGIN_SECS)
        .query_async::<()>(&mut conn)
        .await?;
    Ok(token)
}

/// İstekteki çerezden oturumu bulur.
pub async fn session(state: &WebState, headers: &HeaderMap) -> Option<Session> {
    let id = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_owned())
        .filter(|v| valid_hex(v))?;
    let mut conn = state.redis.clone();
    let value: Option<String> = redis::cmd("GET").arg(format!("panel:oturum:{id}")).query_async(&mut conn).await.ok()?;
    let (user, csrf) = value?.split_once(':').map(|(u, c)| (u.to_owned(), c.to_owned()))?;
    let user = user.parse::<u64>().ok().filter(|u| *u != 0).map(UserId::new)?;
    Some(Session { id, user, csrf })
}

/// Oturum yoksa gösterilecek sayfa.
pub fn login_required() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Html(layout_bare(Page {
            title: "Giriş gerekli",
            body: format!(
                "<p class=\"lead\">Panele girmek için Discord'da <b>/panel</b> yaz; bot sana sadece senin görebileceğin \
                 bir giriş bağlantısı verir.</p>{}",
                if oauth_enabled() { "<p><a class=\"btn\" href=\"/panel/giris\">Discord ile giriş yap</a></p>" } else { "" }
            ),
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------- Discord ile giriş (OAuth2)

const OAUTH_COOKIE: &str = "axbot_oauth";
const OAUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// `.env`'de `DISCORD_CLIENT_SECRET` varsa "Discord ile giriş" açıktır.
pub fn oauth_enabled() -> bool {
    crate::integrations::get().discord_client_secret.is_some()
}

/// Developer Portal → OAuth2 → Redirects'e eklenmesi gereken adres.
pub fn oauth_redirect_uri(state: &WebState) -> String {
    let base = state.panel.public_url.get().unwrap_or_else(|| "http://127.0.0.1:8080".to_owned());
    format!("{}/panel/oauth", base.trim_end_matches('/'))
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// Discord'un izin sayfasına yönlendirir. `state` değeri hem Redis'te hem tarayıcı çerezinde tutulur;
/// böylece başkasının başlattığı bir giriş bu tarayıcıda tamamlanamaz.
pub async fn oauth_start(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if !oauth_enabled() {
        return login_required();
    }
    let token = random_hex();
    let mut conn = state.redis.clone();
    let saved = redis::cmd("SET").arg(format!("panel:oauth:{token}")).arg(1).arg("EX").arg(LOGIN_SECS).query_async::<()>(&mut conn).await;
    if let Err(e) = saved {
        warn!(error = %e, "Panel: OAuth durumu kaydedilemedi");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Giriş başlatılamadı, tekrar dene.").into_response();
    }
    let mut target = match url::Url::parse("https://discord.com/oauth2/authorize") {
        Ok(target) => target,
        Err(_) => return login_required(),
    };
    target
        .query_pairs_mut()
        .append_pair("client_id", &state.cache.current_user().id.to_string())
        .append_pair("response_type", "code")
        .append_pair("scope", "identify")
        .append_pair("redirect_uri", &oauth_redirect_uri(&state))
        .append_pair("state", &token)
        .append_pair("prompt", "none");
    let secure = if is_https(&state, &headers) { "; Secure" } else { "" };
    let mut response = Redirect::to(target.as_str()).into_response();
    if let Ok(value) = HeaderValue::from_str(&format!("{OAUTH_COOKIE}={token}; Path=/panel; HttpOnly; SameSite=Lax; Max-Age={LOGIN_SECS}{secure}")) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

#[derive(Deserialize)]
pub struct OauthCallback {
    code: Option<String>,
    state: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct Identity {
    id: String,
}

/// Koddan kullanıcı kimliğini öğrenir. Erişim anahtarı saklanmaz; yalnızca kimlik okunur.
async fn oauth_identity(state: &WebState, secret: &str, code: &str) -> anyhow::Result<u64> {
    let client = crate::net::fetch_client()?;
    // Önbellek kilidi `await`'ten önce bırakılır.
    let client_id = state.cache.current_user().id.to_string();
    let token: TokenResponse = client
        .post("https://discord.com/api/oauth2/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", secret.to_owned()),
            ("grant_type", "authorization_code".to_owned()),
            ("code", code.to_owned()),
            ("redirect_uri", oauth_redirect_uri(state)),
        ])
        .timeout(OAUTH_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let identity: Identity = client
        .get("https://discord.com/api/users/@me")
        .bearer_auth(&token.access_token)
        .timeout(OAUTH_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    identity.id.parse::<u64>().ok().filter(|id| *id != 0).ok_or_else(|| anyhow::anyhow!("kimlik okunamadı"))
}

pub async fn oauth_callback(State(state): State<WebState>, headers: HeaderMap, axum::extract::Query(query): axum::extract::Query<OauthCallback>) -> Response {
    let Some(secret) = crate::integrations::get().discord_client_secret.as_deref() else {
        return login_required();
    };
    let (Some(code), Some(token)) = (query.code.as_deref(), query.state.as_deref()) else {
        return expired();
    };
    // Kod en fazla birkaç yüz karakterdir; anlamsız büyük girdiler Discord'a gönderilmez.
    if !valid_hex(token) || cookie_value(&headers, OAUTH_COOKIE) != Some(token) || code.is_empty() || code.len() > 512 {
        return expired();
    }
    let mut conn = state.redis.clone();
    let known: Option<u8> = redis::cmd("GETDEL").arg(format!("panel:oauth:{token}")).query_async(&mut conn).await.ok().flatten();
    if known.is_none() {
        return expired();
    }
    match oauth_identity(&state, secret, code).await {
        Ok(user) => start_session(&state, &headers, user).await,
        Err(e) => {
            // Hata metninde gizli değer yoktur (reqwest adres ve durum kodu yazar).
            warn!(error = %e, "Panel: Discord ile giriş tamamlanamadı");
            expired()
        }
    }
}

/// https üzerinden mi gelindi? (tünel bunu başlıkta bildirir; ya da istek dış https adresine gelmiştir)
fn is_https(state: &WebState, headers: &HeaderMap) -> bool {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let via_public = match (header("host"), state.panel.public_url.public()) {
        (Some(host), Some(url)) => url.trim_start_matches("https://") == host,
        _ => false,
    };
    via_public || header("x-forwarded-proto") == Some("https") || header("cf-visitor").is_some_and(|v| v.contains("https"))
}

pub async fn login_page(State(state): State<WebState>, Path(token): Path<String>) -> Response {
    let mut conn = state.redis.clone();
    let exists: bool = valid_hex(&token)
        && redis::cmd("EXISTS").arg(format!("panel:giris:{token}")).query_async(&mut conn).await.unwrap_or(false);
    if !exists {
        return expired();
    }
    Html(layout_bare(Page {
        title: "AxBot paneline giriş",
        body: "<p class=\"lead\">Discord'daki <b>/panel</b> komutundan geldin. Devam etmek için giriş yap.</p>\
               <form method=\"post\"><button type=\"submit\">Giriş yap</button></form>\
               <p class=\"note\">Bu bağlantı tek kullanımlıktır ve 10 dakika geçerlidir; kimseyle paylaşma.</p>"
            .to_owned(),
    }))
    .into_response()
}

pub async fn login(State(state): State<WebState>, Path(token): Path<String>, headers: HeaderMap) -> Response {
    if !valid_hex(&token) {
        return expired();
    }
    let mut conn = state.redis.clone();
    // GETDEL: anahtar sadece bir kez kullanılabilir.
    let user: Option<u64> = redis::cmd("GETDEL").arg(format!("panel:giris:{token}")).query_async(&mut conn).await.ok().flatten();
    let Some(user) = user.filter(|u| *u != 0) else {
        return expired();
    };
    start_session(&state, &headers, user).await
}

/// Oturumu açar, çerezi yazar ve panele yönlendirir.
async fn start_session(state: &WebState, headers: &HeaderMap, user: u64) -> Response {
    let mut conn = state.redis.clone();
    let sid = random_hex();
    let csrf = uuid::Uuid::new_v4().simple().to_string();
    let saved = redis::cmd("SET")
        .arg(format!("panel:oturum:{sid}"))
        .arg(format!("{user}:{csrf}"))
        .arg("EX")
        .arg(SESSION_SECS)
        .query_async::<()>(&mut conn)
        .await;
    if let Err(e) = saved {
        warn!(error = %e, "Panel oturumu kaydedilemedi");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Oturum açılamadı, tekrar dene.").into_response();
    }
    info!(user, "Panele giriş yapıldı");
    let secure = if is_https(state, headers) { "; Secure" } else { "" };
    let cookie = format!("{COOKIE}={sid}; Path=/panel; HttpOnly; SameSite=Lax; Max-Age={SESSION_SECS}{secure}");
    let mut response = Redirect::to("/panel").into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

#[derive(Deserialize)]
pub struct CsrfForm {
    pub csrf: String,
}

pub async fn logout(State(state): State<WebState>, headers: HeaderMap, Form(form): Form<CsrfForm>) -> Response {
    if let Some(session) = session(&state, &headers).await {
        if session.csrf == form.csrf {
            let mut conn = state.redis.clone();
            let _: Result<(), _> = redis::cmd("DEL").arg(format!("panel:oturum:{}", session.id)).query_async(&mut conn).await;
        }
    }
    let mut response = Redirect::to("/panel").into_response();
    if let Ok(value) = HeaderValue::from_str(&format!("{COOKIE}=; Path=/panel; HttpOnly; SameSite=Lax; Max-Age=0")) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

fn expired() -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(layout_bare(Page {
            title: "Bağlantı geçersiz",
            body: format!(
                "<p class=\"lead\">{}</p>",
                escape("Bu giriş bağlantısı kullanılmış ya da süresi dolmuş. Discord'da /panel yazıp yenisini al.")
            ),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    #[test]
    fn tokens_are_long_hex() {
        let t = super::random_hex();
        assert!(super::valid_hex(&t));
        assert!(!super::valid_hex("abc"));
        assert!(!super::valid_hex(&"g".repeat(64)));
    }
}
