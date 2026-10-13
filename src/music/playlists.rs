//! Kişiye özel kayıtlı çalma listeleri ve DJ rolü.
//!
//! Listeler üyeye aittir (sunucuya değil): bir sunucuda kaydedilen liste, botun olduğu başka sunucuda da çalınabilir.

use poise::serenity_prelude::UserId;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::db::snowflake;
use crate::settings::PluginConfig;

pub const MAX_LISTS: i64 = 10;
pub const MAX_TRACKS: usize = 50;
pub const MAX_NAME: usize = 40;

/// Müzik eklentisinin ek ayarları (7/24 ve otomatik çalma `guild_settings` tablosundadır).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicExtras {
    /// Doluysa geç / durdur / ses gibi kontrol komutlarını yalnızca bu roldekiler (ve yöneticiler) kullanır.
    pub dj_role: Option<u64>,
}

impl PluginConfig for MusicExtras {
    const KEY: &'static str = "music";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedTrack {
    pub title: String,
    /// Şarkının bağlantısı; yoksa başlıkla aranır.
    #[serde(default)]
    pub url: Option<String>,
}

/// Liste adını temizler: baştaki / sondaki boşluklar atılır, küçük harfe çevrilir; boş ya da uzunsa `None`.
pub fn normalize_name(raw: &str) -> Option<String> {
    let name = raw.trim().to_lowercase();
    let valid = (1..=MAX_NAME).contains(&name.chars().count()) && !name.contains('\n');
    valid.then_some(name)
}

pub async fn count(db: &PgPool, user: UserId) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM user_playlists WHERE user_id = $1").bind(snowflake(user.get())?).fetch_one(db).await?)
}

/// Listeyi kaydeder (aynı ad varsa üzerine yazar). Yeni eklendiyse `true`.
pub async fn save(db: &PgPool, user: UserId, name: &str, tracks: &[SavedTrack]) -> anyhow::Result<bool> {
    let inserted: bool = sqlx::query_scalar(
        "INSERT INTO user_playlists (user_id, name, tracks) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, name) DO UPDATE SET tracks = EXCLUDED.tracks, updated_at = now() RETURNING (xmax = 0)",
    )
    .bind(snowflake(user.get())?)
    .bind(name)
    .bind(serde_json::to_string(tracks)?)
    .fetch_one(db)
    .await?;
    Ok(inserted)
}

pub async fn load(db: &PgPool, user: UserId, name: &str) -> anyhow::Result<Option<Vec<SavedTrack>>> {
    let raw: Option<String> = sqlx::query_scalar("SELECT tracks FROM user_playlists WHERE user_id = $1 AND name = $2")
        .bind(snowflake(user.get())?)
        .bind(name)
        .fetch_optional(db)
        .await?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
}

/// (ad, şarkı sayısı) listesi.
pub async fn list(db: &PgPool, user: UserId) -> anyhow::Result<Vec<(String, usize)>> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT name, tracks FROM user_playlists WHERE user_id = $1 ORDER BY name").bind(snowflake(user.get())?).fetch_all(db).await?;
    Ok(rows.into_iter().map(|(name, raw)| (name, serde_json::from_str::<Vec<SavedTrack>>(&raw).map_or(0, |t| t.len()))).collect())
}

pub async fn delete(db: &PgPool, user: UserId, name: &str) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM user_playlists WHERE user_id = $1 AND name = $2").bind(snowflake(user.get())?).bind(name).execute(db).await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_normalised() {
        assert_eq!(normalize_name("  Sabah Listem "), Some("sabah listem".to_owned()));
        assert_eq!(normalize_name(""), None);
        assert_eq!(normalize_name("   "), None);
        assert_eq!(normalize_name(&"a".repeat(MAX_NAME + 1)), None);
        assert_eq!(normalize_name("iki\nsatır"), None);
    }

    #[test]
    fn tracks_round_trip_and_old_rows_load() -> anyhow::Result<()> {
        let tracks = vec![SavedTrack { title: "Şarkı".to_owned(), url: Some("https://youtu.be/x".to_owned()) }, SavedTrack { title: "Aranacak".to_owned(), url: None }];
        assert_eq!(serde_json::from_str::<Vec<SavedTrack>>(&serde_json::to_string(&tracks)?)?, tracks);
        let old: Vec<SavedTrack> = serde_json::from_str(r#"[{"title":"Eski"}]"#)?;
        assert_eq!(old.first().and_then(|t| t.url.clone()), None);
        Ok(())
    }
}
