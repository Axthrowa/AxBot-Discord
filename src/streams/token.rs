//! OAuth "client credentials" uygulama token'ının önbelleklenmesi (Twitch ve Kick ortak).

use std::time::{Duration, Instant};

use anyhow::{bail, Context as _};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::config::OAuthApp;

/// Süresi dolmadan bu kadar önce yenilenir.
const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

pub struct AppToken {
    token_url: &'static str,
    app: OAuthApp,
    cached: Mutex<Option<(String, Instant)>>,
}

impl AppToken {
    pub fn new(token_url: &'static str, app: OAuthApp) -> Self {
        Self {
            token_url,
            app,
            cached: Mutex::new(None),
        }
    }

    pub fn client_id(&self) -> &str {
        &self.app.client_id
    }

    /// Geçerli token'ı döner; gerekirse yenisini alır.
    pub async fn get(&self, client: &reqwest::Client) -> anyhow::Result<String> {
        let mut cached = self.cached.lock().await;
        if let Some((token, expires_at)) = cached.as_ref() {
            if Instant::now() + REFRESH_MARGIN < *expires_at {
                return Ok(token.clone());
            }
        }

        let response = client
            .post(self.token_url)
            .timeout(REQUEST_TIMEOUT)
            .form(&[
                ("client_id", self.app.client_id.as_str()),
                ("client_secret", self.app.client_secret.as_str()),
                ("grant_type", "client_credentials"),
            ])
            .send()
            .await
            .context("token sunucusuna ulaşılamadı")?;

        let status = response.status();
        if !status.is_success() {
            let body: String = response.text().await.unwrap_or_default().chars().take(300).collect();
            bail!("token alınamadı (HTTP {status}): {body} — client id/secret doğru mu?");
        }
        let token: TokenResponse = response.json().await.context("token yanıtı okunamadı")?;

        let expires_at = Instant::now() + Duration::from_secs(token.expires_in);
        *cached = Some((token.access_token.clone(), expires_at));
        Ok(token.access_token)
    }

    /// 401 alındığında çağrılır; bir sonraki `get` yeni token alır.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }
}
