//! Twitch Helix API: canlı yayın sorgusu ve kanal doğrulama.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Context as _};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::token::AppToken;
use super::LiveStream;
use crate::config::OAuthApp;

const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const API: &str = "https://api.twitch.tv/helix";
/// Helix tek istekte en fazla 100 kullanıcı kabul eder.
const BATCH: usize = 100;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
struct Page<T> {
    data: Vec<T>,
}

#[derive(Deserialize)]
struct Stream {
    id: String,
    user_login: String,
    user_name: String,
    #[serde(default)]
    game_name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    viewer_count: u64,
    #[serde(default)]
    thumbnail_url: String,
}

#[derive(Deserialize)]
pub struct User {
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub profile_image_url: String,
}

pub struct Twitch {
    token: AppToken,
}

impl Twitch {
    pub fn new(app: OAuthApp) -> Self {
        Self {
            token: AppToken::new(TOKEN_URL, app),
        }
    }

    /// Verilen kanallardan şu an canlı olanlar (anahtar: küçük harf kullanıcı adı).
    pub async fn live(
        &self,
        client: &reqwest::Client,
        logins: &[String],
    ) -> anyhow::Result<HashMap<String, LiveStream>> {
        let mut live = HashMap::new();
        for batch in logins.chunks(BATCH) {
            let query: Vec<(&str, &str)> = batch.iter().map(|l| ("user_login", l.as_str())).collect();
            let page: Page<Stream> = self.get(client, "/streams", &query).await?;

            for s in page.data {
                let thumbnail = (!s.thumbnail_url.is_empty()).then(|| {
                    // Discord resmi önbelleklemesin diye her yayına özel parametre eklenir.
                    format!(
                        "{}?s={}",
                        s.thumbnail_url.replace("{width}x{height}", "1280x720"),
                        s.id
                    )
                });
                let login = s.user_login.to_lowercase();
                live.insert(
                    login.clone(),
                    LiveStream {
                        url: format!("https://twitch.tv/{login}"),
                        display_name: s.user_name,
                        stream_id: s.id,
                        title: s.title,
                        category: (!s.game_name.is_empty()).then_some(s.game_name),
                        viewers: Some(s.viewer_count),
                        thumbnail,
                    },
                );
            }
        }
        Ok(live)
    }

    /// Kanal varsa profil bilgisini döner (`/ara twitch`).
    pub async fn profile(&self, client: &reqwest::Client, login: &str) -> anyhow::Result<Option<User>> {
        let page: Page<User> = self.get(client, "/users", &[("login", login)]).await?;
        Ok(page.data.into_iter().next())
    }

    /// Kanal varsa görünen adını döner.
    pub async fn display_name(&self, client: &reqwest::Client, login: &str) -> anyhow::Result<Option<String>> {
        let page: Page<User> = self.get(client, "/users", &[("login", login)]).await?;
        Ok(page.data.into_iter().next().map(|u| u.display_name))
    }

    async fn get<T: DeserializeOwned>(
        &self,
        client: &reqwest::Client,
        path: &str,
        query: &[(&str, &str)],
    ) -> anyhow::Result<T> {
        let token = self.token.get(client).await.context("Twitch token alınamadı")?;
        let response = client
            .get(format!("{API}{path}"))
            .query(query)
            .header("Client-Id", self.token.client_id())
            .bearer_auth(token)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("Twitch API'ye ulaşılamadı")?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.token.invalidate().await;
        }
        if !status.is_success() {
            let body: String = response.text().await.unwrap_or_default().chars().take(300).collect();
            bail!("Twitch API HTTP {status}: {body}");
        }
        response.json().await.context("Twitch yanıtı okunamadı")
    }
}
