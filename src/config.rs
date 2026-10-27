//! Ortam değişkenlerinden okunan, tip güvenli yapılandırma.

use std::fmt;

use poise::serenity_prelude::GuildId;

use crate::error::ConfigError;

const DEFAULT_DB_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_STREAM_POLL_SECS: u32 = 60;
/// 2026-10 karşılaştırmasında Türkçesi en doğal ve hızlı (~6-17 sn) model; araç çağırmayı destekler.
/// Önceki nemotron-3-super yabancı kelime karıştırıp hatalı cümle kuruyordu.
const DEFAULT_NIM_MODEL: &str = "nvidia/nemotron-3-ultra-550b-a55b";
/// Her zaman en yeni Flash modelini gösteren takma ad; model emekliye ayrılınca kırılmaz.
const DEFAULT_GEMINI_MODEL: &str = "gemini-flash-lite-latest";

pub struct Config {
    pub discord_token: String,
    /// Doluysa slash komutlar sadece bu sunucuya (anında) kaydedilir.
    pub dev_guild_id: Option<GuildId>,
    pub database: DatabaseConfig,
    pub redis_url: String,
    /// Anahtar verilmediyse yapay zekâ kapalıdır.
    pub ai: Option<AiConfig>,
    pub twitch: Option<OAuthApp>,
    pub kick: Option<OAuthApp>,
    pub stream_poll_secs: u32,
    /// Web sunucusu (panel + ban itirazı formu); her zaman açık.
    pub web: WebConfig,
}

/// Web sunucusu: bot bu adreste dinler; dışarıdan `public_url` ya da Cloudflare tüneliyle erişilir.
pub struct WebConfig {
    /// Varsayılan 127.0.0.1:8080 (sadece bu bilgisayar); Docker'da 0.0.0.0:8080.
    pub bind: std::net::SocketAddr,
    /// Sabit dış adres (örn. https://bot.ornek.com, sonunda / yok). Boşsa tünel / yerel adres.
    pub public_url: Option<String>,
    /// cloudflared'in `--metrics` adresi (örn. http://tunnel:2000); hızlı tünel adresini buradan öğrenir.
    pub tunnel_metrics: Option<String>,
    /// Docker'da mı çalışıyor (`AXBOT_DOCKER=1`)? Panelden yeniden başlatma sadece orada.
    pub in_docker: bool,
}

fn web_from_env() -> Result<WebConfig, ConfigError> {
    let public_url = optional("WEB_PUBLIC_URL");
    if let Some(url) = &public_url {
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return Err(ConfigError::Invalid { key: "WEB_PUBLIC_URL", reason: "http:// veya https:// ile başlamalı".to_owned() });
        }
    }
    let bind = optional("WEB_BIND")
        .unwrap_or_else(|| "127.0.0.1:8080".to_owned())
        .parse()
        .map_err(|_| ConfigError::Invalid { key: "WEB_BIND", reason: "adres:port biçiminde olmalı (örn. 127.0.0.1:8080)".to_owned() })?;
    Ok(WebConfig {
        bind,
        public_url: public_url.map(|u| u.trim_end_matches('/').to_owned()),
        tunnel_metrics: optional("TUNNEL_METRICS_URL").map(|u| u.trim_end_matches('/').to_owned()),
        in_docker: optional("AXBOT_DOCKER").is_some_and(|v| v == "1"),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProvider {
    /// NVIDIA NIM (build.nvidia.com), OpenAI uyumlu API.
    Nim,
    /// Google Gemini API.
    Gemini,
}

impl AiProvider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Nim => "NVIDIA NIM",
            Self::Gemini => "Google Gemini",
        }
    }
}

pub struct AiConfig {
    pub provider: AiProvider,
    pub api_key: String,
    pub model: String,
    pub system_prompt: Option<String>,
    /// NVIDIA modelinin web araması için kullandığı servis.
    /// Gemini kendi Google araması özelliğini kullanır, buna ihtiyaç duymaz.
    pub search: Option<SearchBackend>,
    /// Asıl sağlayıcıya ulaşılamazsa denenecek ikinci sağlayıcı (iki anahtar da tanımlıysa).
    pub fallback: Option<Box<AiConfig>>,
}

pub enum SearchBackend {
    /// Yerel SearXNG (hesap/anahtar gerekmez).
    Searxng { base_url: String },
    /// Tavily API (hesap gerektirir).
    Tavily { api_key: String },
}

impl SearchBackend {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Searxng { .. } => "SearXNG",
            Self::Tavily { .. } => "Tavily",
        }
    }
}

/// OAuth client credentials (Twitch / Kick uygulama bilgileri).
pub struct OAuthApp {
    pub client_id: String,
    pub client_secret: String,
}

pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let discord_token = required("DISCORD_TOKEN")?;

        let dev_guild_id = match optional("DEV_GUILD_ID") {
            Some(raw) => Some(parse_guild_id("DEV_GUILD_ID", &raw)?),
            None => None,
        };

        let max_connections = match optional("DB_MAX_CONNECTIONS") {
            Some(raw) => parse_positive_u32("DB_MAX_CONNECTIONS", &raw)?,
            None => DEFAULT_DB_MAX_CONNECTIONS,
        };

        Ok(Self {
            discord_token,
            dev_guild_id,
            database: DatabaseConfig {
                url: required("DATABASE_URL")?,
                max_connections,
            },
            redis_url: required("REDIS_URL")?,
            ai: ai_from_env()?,
            twitch: oauth_app("TWITCH_CLIENT_ID", "TWITCH_CLIENT_SECRET")?,
            kick: oauth_app("KICK_CLIENT_ID", "KICK_CLIENT_SECRET")?,
            stream_poll_secs: match optional("STREAM_POLL_SECS") {
                Some(raw) => parse_positive_u32("STREAM_POLL_SECS", &raw)?.max(30),
                None => DEFAULT_STREAM_POLL_SECS,
            },
            web: web_from_env()?,
        })
    }
}

/// `AI_PROVIDER` verilmediyse hangi anahtar varsa o sağlayıcı seçilir (önce NVIDIA).
fn ai_from_env() -> Result<Option<AiConfig>, ConfigError> {
    let provider = match optional("AI_PROVIDER").map(|p| p.to_lowercase()).as_deref() {
        Some("nim" | "nvidia") => AiProvider::Nim,
        Some("gemini" | "google") => AiProvider::Gemini,
        Some(other) => {
            return Err(ConfigError::Invalid {
                key: "AI_PROVIDER",
                reason: format!("'{other}' bilinmiyor (nim veya gemini olmalı)"),
            })
        }
        None if optional("NVIDIA_API_KEY").is_some() => AiProvider::Nim,
        None if optional("GEMINI_API_KEY").is_some() => AiProvider::Gemini,
        None => return Ok(None),
    };

    let (key_var, default_model) = match provider {
        AiProvider::Nim => ("NVIDIA_API_KEY", DEFAULT_NIM_MODEL),
        AiProvider::Gemini => ("GEMINI_API_KEY", DEFAULT_GEMINI_MODEL),
    };

    // Diğer sağlayıcının anahtarı da varsa o yedek olur (kendi varsayılan modeliyle).
    let fallback = match provider {
        AiProvider::Nim => optional("GEMINI_API_KEY").map(|api_key| (AiProvider::Gemini, api_key, DEFAULT_GEMINI_MODEL)),
        AiProvider::Gemini => optional("NVIDIA_API_KEY").map(|api_key| (AiProvider::Nim, api_key, DEFAULT_NIM_MODEL)),
    }
    .map(|(provider, api_key, model)| {
        Box::new(AiConfig { provider, api_key, model: model.to_owned(), system_prompt: optional("AI_SYSTEM_PROMPT"), search: search_from_env(), fallback: None })
    });

    Ok(Some(AiConfig {
        provider,
        api_key: required(key_var)?,
        model: optional("AI_MODEL").unwrap_or_else(|| default_model.to_owned()),
        system_prompt: optional("AI_SYSTEM_PROMPT"),
        search: search_from_env(),
        fallback,
    }))
}

/// Tavily anahtarı verildiyse o, yoksa SearXNG adresi varsa o kullanılır.
fn search_from_env() -> Option<SearchBackend> {
    if let Some(api_key) = optional("TAVILY_API_KEY") {
        return Some(SearchBackend::Tavily { api_key });
    }
    optional("SEARXNG_URL").map(|url| SearchBackend::Searxng {
        base_url: url.trim_end_matches('/').to_owned(),
    })
}

/// İkisi de boşsa `None`; sadece biri doluysa yapılandırma hatası.
fn oauth_app(id_key: &'static str, secret_key: &'static str) -> Result<Option<OAuthApp>, ConfigError> {
    match (optional(id_key), optional(secret_key)) {
        (Some(client_id), Some(client_secret)) => Ok(Some(OAuthApp {
            client_id,
            client_secret,
        })),
        (None, None) => Ok(None),
        (Some(_), None) => Err(ConfigError::Missing(secret_key)),
        (None, Some(_)) => Err(ConfigError::Missing(id_key)),
    }
}

// Token ve bağlantı adresleri (şifre içerir) loglara düşmesin diye Debug elle yazıldı.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("discord_token", &"<gizli>")
            .field("dev_guild_id", &self.dev_guild_id)
            .field("database", &self.database)
            .field("redis_url", &"<gizli>")
            .field("ai", &self.ai.as_ref().map(|a| (a.provider, &a.model)))
            .field("twitch", &self.twitch.is_some())
            .field("kick", &self.kick.is_some())
            .field("stream_poll_secs", &self.stream_poll_secs)
            .finish()
    }
}

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("url", &"<gizli>")
            .field("max_connections", &self.max_connections)
            .finish()
    }
}

fn required(key: &'static str) -> Result<String, ConfigError> {
    optional(key).ok_or(ConfigError::Missing(key))
}

/// Tanımsız veya boş değişkenler `None` döner.
fn optional(key: &'static str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn parse_positive_u32(key: &'static str, raw: &str) -> Result<u32, ConfigError> {
    match raw.parse::<u32>() {
        Ok(0) => Err(ConfigError::Invalid {
            key,
            reason: "sıfır olamaz".to_owned(),
        }),
        Ok(v) => Ok(v),
        Err(e) => Err(ConfigError::Invalid {
            key,
            reason: e.to_string(),
        }),
    }
}

fn parse_guild_id(key: &'static str, raw: &str) -> Result<GuildId, ConfigError> {
    let id = raw.parse::<u64>().map_err(|e| ConfigError::Invalid {
        key,
        reason: e.to_string(),
    })?;

    // GuildId::new(0) panikler, bu yüzden sıfırı önceden eliyoruz.
    if id == 0 {
        return Err(ConfigError::Invalid {
            key,
            reason: "sıfır olamaz".to_owned(),
        });
    }

    Ok(GuildId::new(id))
}
