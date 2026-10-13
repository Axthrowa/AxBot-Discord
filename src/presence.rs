//! Bot kişiselleştirme.
//!
//! - **Durum ve etkinlik** bütün sunucularda aynıdır (Discord böyle çalışır); bot sahibi panelin
//!   Sistem sayfasından ayarlar, Redis'te saklanır ve açılışta yeniden uygulanır.
//! - **Sunucuya özel profil** (takma ad, avatar, afiş, hakkında) sunucu yöneticisince paneldeki
//!   "Bot profili" sayfasından ayarlanır.

use poise::serenity_prelude::{self as serenity, ActivityData, GuildId, OnlineStatus, ShardManager};
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};

use crate::settings::PluginConfig;

const REDIS_KEY: &str = "bot:presence";
pub const MAX_ACTIVITY_LEN: usize = 100;
pub const MAX_NICK_LEN: usize = 32;
pub const MAX_BIO_LEN: usize = 190;
/// Discord'un avatar/afiş dosyası için kabul ettiği boyutun altında güvenli bir sınır.
pub const MAX_PROFILE_IMAGE_BYTES: usize = 6 * 1024 * 1024;

pub const STATUSES: &[(&str, &str)] = &[("online", "Çevrimiçi"), ("idle", "Boşta"), ("dnd", "Rahatsız etmeyin"), ("invisible", "Görünmez")];
pub const ACTIVITIES: &[(&str, &str)] = &[
    ("none", "Yok"),
    ("custom", "Özel durum"),
    ("playing", "Oynuyor"),
    ("listening", "Dinliyor"),
    ("watching", "İzliyor"),
    ("competing", "Yarışıyor"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Presence {
    pub status: String,
    pub activity: String,
    pub text: String,
}

impl Default for Presence {
    fn default() -> Self {
        Self { status: "online".to_owned(), activity: "none".to_owned(), text: String::new() }
    }
}

impl Presence {
    /// Formdan gelen değerleri doğrular.
    pub fn parse(status: &str, activity: &str, text: &str) -> Option<Self> {
        let text = text.trim();
        let known = STATUSES.iter().any(|(k, _)| *k == status) && ACTIVITIES.iter().any(|(k, _)| *k == activity);
        let valid = known && text.chars().count() <= MAX_ACTIVITY_LEN && (activity == "none" || !text.is_empty());
        valid.then(|| Self {
            status: status.to_owned(),
            activity: activity.to_owned(),
            text: if activity == "none" { String::new() } else { text.to_owned() },
        })
    }

    pub fn online_status(&self) -> OnlineStatus {
        match self.status.as_str() {
            "idle" => OnlineStatus::Idle,
            "dnd" => OnlineStatus::DoNotDisturb,
            "invisible" => OnlineStatus::Invisible,
            _ => OnlineStatus::Online,
        }
    }

    pub fn activity_data(&self) -> Option<ActivityData> {
        if self.text.trim().is_empty() {
            return None;
        }
        Some(match self.activity.as_str() {
            "custom" => ActivityData::custom(&self.text),
            "playing" => ActivityData::playing(&self.text),
            "listening" => ActivityData::listening(&self.text),
            "watching" => ActivityData::watching(&self.text),
            "competing" => ActivityData::competing(&self.text),
            _ => return None,
        })
    }
}

/// Kayıtlı durum; hiç ayarlanmamışsa `None` (Discord'un varsayılanı kalır).
pub async fn load(redis: &ConnectionManager) -> Option<Presence> {
    let mut conn = redis.clone();
    let raw: Option<String> = redis::cmd("GET").arg(REDIS_KEY).query_async(&mut conn).await.ok()?;
    serde_json::from_str(&raw?).ok()
}

pub async fn save(redis: &ConnectionManager, presence: &Presence) -> anyhow::Result<()> {
    let mut conn = redis.clone();
    let _: () = redis::cmd("SET").arg(REDIS_KEY).arg(serde_json::to_string(presence)?).query_async(&mut conn).await?;
    Ok(())
}

/// Durumu çalışan bütün bağlantılara (shard) uygular.
pub async fn apply(shards: &ShardManager, presence: &Presence) {
    for runner in shards.runners.lock().await.values() {
        runner.runner_tx.set_presence(presence.activity_data(), presence.online_status());
    }
}

/// Botun bu sunucudaki profili (panelde gösterilmek için saklanır; asıl değer Discord'dadır).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BotProfile {
    pub nick: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub banner_url: Option<String>,
}

impl PluginConfig for BotProfile {
    const KEY: &'static str = "bot_profile";
}

impl BotProfile {
    pub fn is_custom(&self) -> bool {
        self.nick.is_some() || self.bio.is_some() || self.avatar_url.is_some() || self.banner_url.is_some()
    }
}

/// Dosyanın türünü ilk baytlarından tanır (Discord `data:` adresinde tür ister).
fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

/// Resmi indirip Discord'un istediği `data:` biçimine çevirir. İndirilemezse / resim değilse `None`.
async fn image_data(url: &str) -> Option<String> {
    let bytes = crate::cards::fetch_bytes(url, MAX_PROFILE_IMAGE_BYTES).await?;
    let mime = image_mime(&bytes)?;
    // serenity türü hep PNG yazar; baştaki tür bilgisi gerçek türle değiştirilir.
    let encoded = serenity::CreateAttachment::bytes(bytes, "resim").to_base64();
    let payload = encoded.split_once(";base64,").map(|(_, data)| data.to_owned())?;
    Some(format!("data:{mime};base64,{payload}"))
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// Resim indirilemedi ya da desteklenmeyen türde.
    Image,
    /// Discord değişikliği reddetti.
    Discord,
}

/// Yalnızca DEĞİŞEN alanları Discord'a gönderir (aynı değerlerle kaydetmek istek atmaz).
pub async fn apply_profile(http: &serenity::Http, guild_id: GuildId, old: &BotProfile, new: &BotProfile) -> Result<(), ProfileError> {
    let mut map = serde_json::Map::new();
    if old.nick != new.nick {
        map.insert("nick".to_owned(), new.nick.clone().map_or(serde_json::Value::Null, Into::into));
    }
    if old.bio != new.bio {
        map.insert("bio".to_owned(), new.bio.clone().unwrap_or_default().into());
    }
    for (key, before, after) in [("avatar", &old.avatar_url, &new.avatar_url), ("banner", &old.banner_url, &new.banner_url)] {
        if before == after {
            continue;
        }
        let value = match after {
            Some(url) => image_data(url).await.ok_or(ProfileError::Image)?.into(),
            None => serde_json::Value::Null,
        };
        map.insert(key.to_owned(), value);
    }
    if map.is_empty() {
        return Ok(());
    }
    http.edit_member_me(guild_id, &map, Some("Panelden bot profili değiştirildi")).await.map(|_| ()).map_err(|e| {
        tracing::warn!(guild = %guild_id, error = %e, "Bot profili değiştirilemedi");
        ProfileError::Discord
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_presence() {
        assert_eq!(Presence::parse("online", "none", "yok sayılır").map(|p| p.text), Some(String::new()));
        assert!(Presence::parse("dnd", "watching", "sunucuyu").is_some_and(|p| p.online_status() == OnlineStatus::DoNotDisturb && p.activity_data().is_some()));
        // Etkinlik seçildiyse metin zorunlu.
        assert_eq!(Presence::parse("online", "playing", "  "), None);
        assert_eq!(Presence::parse("uyuyor", "none", ""), None);
        assert_eq!(Presence::parse("online", "dans", "x"), None);
        assert_eq!(Presence::parse("online", "custom", &"a".repeat(MAX_ACTIVITY_LEN + 1)), None);
        assert!(Presence::default().activity_data().is_none());
    }

    #[test]
    fn recognises_image_types() {
        assert_eq!(image_mime(&[0x89, b'P', b'N', b'G', 0x0D]), Some("image/png"));
        assert_eq!(image_mime(&[0xFF, 0xD8, 0xFF]), Some("image/jpeg"));
        assert_eq!(image_mime(b"GIF89a"), Some("image/gif"));
        assert_eq!(image_mime(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_mime(b"<html>"), None);
        assert_eq!(image_mime(&[]), None);
    }

    #[test]
    fn profile_custom_flag() {
        assert!(!BotProfile::default().is_custom());
        assert!(BotProfile { nick: Some("Ax".to_owned()), ..BotProfile::default() }.is_custom());
    }
}
