//! Kick resmi public API: canlı yayın sorgusu ve kanal doğrulama.
//! https://docs.kick.com/apis/channels

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Context as _};
use serde::Deserialize;

use super::token::AppToken;
use super::LiveStream;
use crate::config::OAuthApp;

const TOKEN_URL: &str = "https://id.kick.com/oauth/token";
const CHANNELS_URL: &str = "https://api.kick.com/public/v1/channels";
/// Kick tek istekte en fazla 50 slug kabul eder.
const BATCH: usize = 50;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    data: Vec<Channel>,
}

#[derive(Deserialize)]
struct Channel {
    slug: String,
    #[serde(default)]
    stream_title: Option<String>,
    #[serde(default)]
    category: Option<Category>,
    #[serde(default)]
    stream: Option<Stream>,
}

#[derive(Deserialize)]
struct Category {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct Stream {
    #[serde(default)]
    is_live: bool,
    #[serde(default)]
    viewer_count: Option<u64>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
}

pub struct Kick {
    token: AppToken,
}

impl Kick {
    pub fn new(app: OAuthApp) -> Self {
        Self {
            token: AppToken::new(TOKEN_URL, app),
        }
    }

    pub async fn live(
        &self,
        client: &reqwest::Client,
        slugs: &[String],
    ) -> anyhow::Result<HashMap<String, LiveStream>> {
        let mut live = HashMap::new();
        for batch in slugs.chunks(BATCH) {
            for channel in self.channels(client, batch).await? {
                let Some(stream) = channel.stream.filter(|s| s.is_live) else {
                    continue;
                };
                let slug = channel.slug.to_lowercase();
                // Kick yayın kimliği vermiyor; başlangıç zamanı her yayın için benzersizdir.
                let stream_id = stream.start_time.unwrap_or_else(|| "live".to_owned());
                live.insert(
                    slug.clone(),
                    LiveStream {
                        url: format!("https://kick.com/{slug}"),
                        display_name: channel.slug,
                        stream_id,
                        title: channel.stream_title.unwrap_or_default(),
                        category: channel.category.and_then(|c| c.name).filter(|n| !n.is_empty()),
                        viewers: stream.viewer_count,
                        thumbnail: stream.thumbnail.filter(|t| !t.is_empty()),
                    },
                );
            }
        }
        Ok(live)
    }

    pub async fn exists(&self, client: &reqwest::Client, slug: &str) -> anyhow::Result<bool> {
        Ok(!self.channels(client, &[slug.to_owned()]).await?.is_empty())
    }

    async fn channels(&self, client: &reqwest::Client, slugs: &[String]) -> anyhow::Result<Vec<Channel>> {
        let token = self.token.get(client).await.context("Kick token alınamadı")?;
        let query: Vec<(&str, &str)> = slugs.iter().map(|s| ("slug", s.as_str())).collect();

        let response = client
            .get(CHANNELS_URL)
            .query(&query)
            .bearer_auth(token)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .context("Kick API'ye ulaşılamadı")?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.token.invalidate().await;
        }
        if !status.is_success() {
            let body: String = response.text().await.unwrap_or_default().chars().take(300).collect();
            bail!("Kick API HTTP {status}: {body}");
        }
        let page: Page = response.json().await.context("Kick yanıtı okunamadı")?;
        Ok(page.data)
    }
}
