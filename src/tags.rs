//! Etiketler (özel komutlar): `/etiket ekle kurallar …` ile kaydedilen metin, sohbette
//! `!kurallar` yazılınca gönderilir.
//!
//! Her mesajda kontrol edildiği için etiketler sunucu başına bellekte tutulur; veritabanına
//! sadece sunucunun ilk kullanımında ve etiket değişince gidilir.

use std::collections::BTreeMap;
use std::sync::Arc;

use dashmap::DashMap;
use poise::serenity_prelude::{GuildId, UserId};
use sqlx::PgPool;

use crate::db::snowflake;

pub const PREFIX: char = '!';
pub const MAX_NAME: usize = 32;
pub const MAX_TAGS: usize = 200;

#[derive(Default)]
pub struct Tags {
    /// ad → içerik (alfabetik).
    cache: DashMap<GuildId, Arc<BTreeMap<String, String>>>,
}

impl Tags {
    pub async fn all(&self, db: &PgPool, guild_id: GuildId) -> anyhow::Result<Arc<BTreeMap<String, String>>> {
        if let Some(tags) = self.cache.get(&guild_id) {
            return Ok(Arc::clone(tags.value()));
        }
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT name, content FROM tags WHERE guild_id = $1")
            .bind(snowflake(guild_id.get())?)
            .fetch_all(db)
            .await?;
        let tags = Arc::new(rows.into_iter().collect::<BTreeMap<_, _>>());
        self.cache.insert(guild_id, Arc::clone(&tags));
        Ok(tags)
    }

    /// Ekler veya günceller; yeni eklendiyse `true`.
    pub async fn set(&self, db: &PgPool, guild_id: GuildId, name: &str, content: &str, author: UserId) -> anyhow::Result<bool> {
        let inserted: bool = sqlx::query_scalar(
            "INSERT INTO tags (guild_id, name, content, created_by) VALUES ($1, $2, $3, $4)
             ON CONFLICT (guild_id, name) DO UPDATE SET content = EXCLUDED.content
             RETURNING (xmax = 0)",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(name)
        .bind(content)
        .bind(snowflake(author.get())?)
        .fetch_one(db)
        .await?;
        self.cache.remove(&guild_id);
        Ok(inserted)
    }

    pub async fn remove(&self, db: &PgPool, guild_id: GuildId, name: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM tags WHERE guild_id = $1 AND name = $2")
            .bind(snowflake(guild_id.get())?)
            .bind(name)
            .execute(db)
            .await?;
        self.cache.remove(&guild_id);
        Ok(result.rows_affected() > 0)
    }

    /// Mesaj `!ad` ile başlıyorsa etiketin içeriğini döner.
    pub async fn for_message(&self, db: &PgPool, guild_id: GuildId, content: &str) -> anyhow::Result<Option<String>> {
        let Some(name) = trigger(content) else {
            return Ok(None);
        };
        let tags = self.all(db, guild_id).await?;
        Ok(tags.get(&name).cloned())
    }
}

/// `!kurallar bla bla` → `kurallar`. Etiket adı değilse `None` (ağ/veritabanı işi yapılmadan elenir).
pub fn trigger(content: &str) -> Option<String> {
    let rest = content.strip_prefix(PREFIX)?;
    if rest.starts_with(char::is_whitespace) {
        return None;
    }
    let word = rest.split_whitespace().next()?;
    normalize_name(word).ok()
}

/// Etiket adını küçük harfe çevirir ve doğrular (Türkçe İ/I doğru küçültülür).
pub fn normalize_name(raw: &str) -> Result<String, String> {
    let name: String = raw
        .trim()
        .chars()
        .map(|c| match c {
            'İ' => 'i',
            'I' => 'ı',
            c => c.to_lowercase().next().unwrap_or(c),
        })
        .collect();
    if name.is_empty() {
        return Err("Etiket adı boş olamaz.".to_owned());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("Etiket adı en fazla {MAX_NAME} karakter olabilir."));
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err("Etiket adında sadece harf, rakam, `-` ve `_` kullanılabilir (boşluk olmaz).".to_owned());
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_turkish_names() -> Result<(), String> {
        assert_eq!(normalize_name("KURALLAR")?, "kurallar");
        assert_eq!(normalize_name("İLETİŞİM")?, "iletişim");
        assert_eq!(normalize_name("SIRA")?, "sıra");
        assert!(normalize_name("iki kelime").is_err());
        assert!(normalize_name("").is_err());
        Ok(())
    }

    #[test]
    fn detects_triggers() {
        assert_eq!(trigger("!kurallar"), Some("kurallar".to_owned()));
        assert_eq!(trigger("!Kurallar lütfen oku"), Some("kurallar".to_owned()));
        assert_eq!(trigger("kurallar"), None);
        assert_eq!(trigger("! boşluk"), None);
        assert_eq!(trigger("!"), None);
        assert_eq!(trigger("!?!"), None);
    }
}
