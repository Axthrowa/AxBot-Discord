//! Eklenti ayarları deposu: sunucu + eklenti başına bir JSON kaydı (`plugin_settings` tablosu).
//!
//! Ayarlar mesaj başına okunabildiği için bellekte tutulur; PostgreSQL'e yalnızca ilk okumada ve
//! ayar değişince gidilir. Depo klonlanabilir: bot ve web paneli aynı önbelleği paylaşır, panelden
//! kaydedilen ayar bota anında yansır.

use std::any::{Any, TypeId};
use std::sync::Arc;

use dashmap::DashMap;
use poise::serenity_prelude::GuildId;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::PgPool;
use tracing::warn;

use crate::db::snowflake;

/// Bir eklentinin ayar türü. Alanlar `#[serde(default)]` olmalı ki sonradan eklenen ayarlar
/// eski kayıtları bozmasın.
pub trait PluginConfig: Serialize + DeserializeOwned + Default + Send + Sync + 'static {
    /// Tablodaki `plugin` değeri; değiştirilirse eski kayıtlar okunmaz.
    const KEY: &'static str;
}

type Cached = Arc<dyn Any + Send + Sync>;

#[derive(Clone, Default)]
pub struct PluginSettings {
    cache: Arc<DashMap<(GuildId, TypeId), Cached>>,
}

impl PluginSettings {
    pub async fn get<T: PluginConfig>(&self, db: &PgPool, guild_id: GuildId) -> anyhow::Result<Arc<T>> {
        let key = (guild_id, TypeId::of::<T>());
        // Kilit `await`'ten önce bırakılır.
        let cached = self.cache.get(&key).map(|entry| Arc::clone(entry.value()));
        if let Some(Ok(found)) = cached.map(Arc::downcast::<T>) {
            return Ok(found);
        }

        let raw: Option<String> = sqlx::query_scalar("SELECT data FROM plugin_settings WHERE guild_id = $1 AND plugin = $2")
            .bind(snowflake(guild_id.get())?)
            .bind(T::KEY)
            .fetch_optional(db)
            .await?;
        let value = raw.map_or_else(T::default, |raw| {
            serde_json::from_str(&raw).unwrap_or_else(|e| {
                warn!(guild = %guild_id, plugin = T::KEY, error = %e, "Eklenti ayarı okunamadı, varsayılan kullanılıyor");
                T::default()
            })
        });
        let value = Arc::new(value);
        self.cache.insert(key, Arc::clone(&value) as Cached);
        Ok(value)
    }

    pub async fn set<T: PluginConfig>(&self, db: &PgPool, guild_id: GuildId, value: T) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO plugin_settings (guild_id, plugin, data) VALUES ($1, $2, $3)
             ON CONFLICT (guild_id, plugin) DO UPDATE SET data = EXCLUDED.data, updated_at = now()",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(T::KEY)
        .bind(serde_json::to_string(&value)?)
        .execute(db)
        .await?;
        self.cache.insert((guild_id, TypeId::of::<T>()), Arc::new(value) as Cached);
        Ok(())
    }

    /// Sunucu verisi silindiğinde bellekteki ayarları da unutur.
    pub fn forget_guild(&self, guild_id: GuildId) {
        self.cache.retain(|(guild, _), _| *guild != guild_id);
    }
}
