//! Sunucu bazlı topluluk ayarları (karşılama, otomatik rol, olay logu, koruma, starboard, öneriler).
//!
//! Her üye girişinde / silinen mesajda okunur; bu yüzden bellekte önbelleklenir ve sadece
//! ilk okumada ve ayar değişince PostgreSQL'e gidilir.

use std::sync::Arc;

use dashmap::DashMap;
use poise::serenity_prelude::{ChannelId, GuildId, RoleId};
use sqlx::PgPool;

use crate::db::snowflake;

#[derive(Debug, Clone, Default)]
pub struct GuildConfig {
    pub event_log_channel: Option<ChannelId>,
    pub welcome_channel: Option<ChannelId>,
    pub welcome_message: Option<String>,
    pub goodbye_channel: Option<ChannelId>,
    pub goodbye_message: Option<String>,
    pub autorole: Option<RoleId>,
    /// 0 = kapalı.
    pub min_account_age_days: u32,
    /// 10 saniyede bu kadar giriş olursa baskın modu; 0 = kapalı.
    pub raid_join_threshold: u32,
    pub starboard_channel: Option<ChannelId>,
    /// Starboard'a düşmek için gereken ⭐ sayısı.
    pub starboard_threshold: u32,
    pub suggestion_channel: Option<ChannelId>,
    pub birthday_channel: Option<ChannelId>,
    pub birthday_role: Option<RoleId>,
    /// Girilince geçici oda açılan ses kanalı.
    pub tempvoice_hub: Option<ChannelId>,
    /// Doğrulama butonuna basana verilen rol.
    pub verify_role: Option<RoleId>,
    /// Doğrulamada işlem sorusu da sorulsun mu?
    pub verify_math: bool,
    /// Ban itirazlarının düştüğü kanal (boşsa itiraz kapalı).
    pub appeal_channel: Option<ChannelId>,
    /// İtiraz sunucusunun davet linki (yasaklı kişi butonu kullanabilsin diye).
    pub appeal_invite: Option<String>,
    /// İtiraz sunucusu (itirazı kabul edilip geri dönenler oradan atılır).
    pub appeal_guild: Option<GuildId>,
    /// Hayvan yakalama oyununun kanalı.
    pub animal_channel: Option<ChannelId>,
}

#[derive(sqlx::FromRow)]
struct Row {
    event_log_channel_id: Option<i64>,
    welcome_channel_id: Option<i64>,
    welcome_message: Option<String>,
    goodbye_channel_id: Option<i64>,
    goodbye_message: Option<String>,
    autorole_id: Option<i64>,
    min_account_age_days: i32,
    raid_join_threshold: i32,
    starboard_channel_id: Option<i64>,
    starboard_threshold: i32,
    suggestion_channel_id: Option<i64>,
    birthday_channel_id: Option<i64>,
    birthday_role_id: Option<i64>,
    tempvoice_hub_id: Option<i64>,
    verify_role_id: Option<i64>,
    verify_math: bool,
    appeal_channel_id: Option<i64>,
    animal_channel_id: Option<i64>,
    appeal_invite: Option<String>,
    appeal_guild_id: Option<i64>,
}

/// Ayarlanabilir alanlar. Her biri kendi sabit SQL'ine sahiptir (dinamik SQL yok).
pub enum Setting {
    EventLog(Option<ChannelId>),
    Welcome(Option<ChannelId>, Option<String>),
    Goodbye(Option<ChannelId>, Option<String>),
    Autorole(Option<RoleId>),
    MinAccountAge(u32),
    RaidThreshold(u32),
    /// Kanal (boşsa kapalı) ve eşik.
    Starboard(Option<ChannelId>, u32),
    SuggestionChannel(Option<ChannelId>),
    Birthday(Option<ChannelId>, Option<RoleId>),
    TempVoiceHub(Option<ChannelId>),
    Verify(Option<RoleId>, bool),
    /// Kanal (boşsa kapalı), itiraz sunucusu daveti ve o sunucunun kimliği.
    AppealChannel(Option<ChannelId>, Option<String>, Option<GuildId>),
    AnimalChannel(Option<ChannelId>),
}

/// Klonlanabilir: bot ve web paneli aynı önbelleği paylaşır (panelden yapılan ayar bota anında yansır).
#[derive(Default, Clone)]
pub struct GuildConfigs {
    cache: Arc<DashMap<GuildId, Arc<GuildConfig>>>,
}

impl GuildConfigs {
    pub async fn get(&self, db: &PgPool, guild_id: GuildId) -> anyhow::Result<Arc<GuildConfig>> {
        if let Some(cfg) = self.cache.get(&guild_id) {
            return Ok(Arc::clone(cfg.value()));
        }

        let row: Option<Row> = sqlx::query_as(
            "SELECT event_log_channel_id, welcome_channel_id, welcome_message, goodbye_channel_id,
                    goodbye_message, autorole_id, min_account_age_days, raid_join_threshold,
                    starboard_channel_id, starboard_threshold, suggestion_channel_id,
                    birthday_channel_id, birthday_role_id, tempvoice_hub_id,
                    verify_role_id, verify_math, appeal_channel_id, animal_channel_id, appeal_invite,
                    appeal_guild_id
             FROM guild_settings WHERE guild_id = $1",
        )
        .bind(snowflake(guild_id.get())?)
        .fetch_optional(db)
        .await?;

        let cfg = Arc::new(row.map(GuildConfig::from).unwrap_or_default());
        self.cache.insert(guild_id, Arc::clone(&cfg));
        Ok(cfg)
    }

    pub async fn set(&self, db: &PgPool, guild_id: GuildId, setting: Setting) -> anyhow::Result<()> {
        let gid = snowflake(guild_id.get())?;
        let channel = |c: Option<ChannelId>| c.map(|c| snowflake(c.get())).transpose();

        match setting {
            Setting::EventLog(ch) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, event_log_channel_id) VALUES ($1, $2)
                     ON CONFLICT (guild_id) DO UPDATE
                         SET event_log_channel_id = EXCLUDED.event_log_channel_id, updated_at = now()",
                )
                .bind(gid)
                .bind(channel(ch)?)
                .execute(db)
                .await?;
            }
            Setting::Welcome(ch, message) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, welcome_channel_id, welcome_message) VALUES ($1, $2, $3)
                     ON CONFLICT (guild_id) DO UPDATE
                         SET welcome_channel_id = EXCLUDED.welcome_channel_id,
                             welcome_message = EXCLUDED.welcome_message, updated_at = now()",
                )
                .bind(gid)
                .bind(channel(ch)?)
                .bind(message)
                .execute(db)
                .await?;
            }
            Setting::Goodbye(ch, message) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, goodbye_channel_id, goodbye_message) VALUES ($1, $2, $3)
                     ON CONFLICT (guild_id) DO UPDATE
                         SET goodbye_channel_id = EXCLUDED.goodbye_channel_id,
                             goodbye_message = EXCLUDED.goodbye_message, updated_at = now()",
                )
                .bind(gid)
                .bind(channel(ch)?)
                .bind(message)
                .execute(db)
                .await?;
            }
            Setting::Autorole(role) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, autorole_id) VALUES ($1, $2)
                     ON CONFLICT (guild_id) DO UPDATE SET autorole_id = EXCLUDED.autorole_id, updated_at = now()",
                )
                .bind(gid)
                .bind(role.map(|r| snowflake(r.get())).transpose()?)
                .execute(db)
                .await?;
            }
            Setting::MinAccountAge(days) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, min_account_age_days) VALUES ($1, $2)
                     ON CONFLICT (guild_id) DO UPDATE
                         SET min_account_age_days = EXCLUDED.min_account_age_days, updated_at = now()",
                )
                .bind(gid)
                .bind(i32::try_from(days)?)
                .execute(db)
                .await?;
            }
            Setting::RaidThreshold(n) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, raid_join_threshold) VALUES ($1, $2)
                     ON CONFLICT (guild_id) DO UPDATE
                         SET raid_join_threshold = EXCLUDED.raid_join_threshold, updated_at = now()",
                )
                .bind(gid)
                .bind(i32::try_from(n)?)
                .execute(db)
                .await?;
            }
            Setting::Starboard(ch, threshold) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, starboard_channel_id, starboard_threshold) VALUES ($1, $2, $3)
                     ON CONFLICT (guild_id) DO UPDATE
                         SET starboard_channel_id = EXCLUDED.starboard_channel_id,
                             starboard_threshold = EXCLUDED.starboard_threshold, updated_at = now()",
                )
                .bind(gid)
                .bind(channel(ch)?)
                .bind(i32::try_from(threshold)?)
                .execute(db)
                .await?;
            }
            Setting::Birthday(ch, role) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, birthday_channel_id, birthday_role_id) VALUES ($1, $2, $3)
                     ON CONFLICT (guild_id) DO UPDATE
                         SET birthday_channel_id = EXCLUDED.birthday_channel_id,
                             birthday_role_id = EXCLUDED.birthday_role_id, updated_at = now()",
                )
                .bind(gid)
                .bind(channel(ch)?)
                .bind(role.map(|r| snowflake(r.get())).transpose()?)
                .execute(db)
                .await?;
            }
            Setting::Verify(role, math) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, verify_role_id, verify_math) VALUES ($1, $2, $3)
                     ON CONFLICT (guild_id) DO UPDATE
                         SET verify_role_id = EXCLUDED.verify_role_id, verify_math = EXCLUDED.verify_math, updated_at = now()",
                )
                .bind(gid)
                .bind(role.map(|r| snowflake(r.get())).transpose()?)
                .bind(math)
                .execute(db)
                .await?;
            }
            Setting::AnimalChannel(ch) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, animal_channel_id) VALUES ($1, $2)
                     ON CONFLICT (guild_id) DO UPDATE SET animal_channel_id = EXCLUDED.animal_channel_id, updated_at = now()",
                )
                .bind(gid)
                .bind(channel(ch)?)
                .execute(db)
                .await?;
            }
            Setting::AppealChannel(ch, invite, appeal_guild) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, appeal_channel_id, appeal_invite, appeal_guild_id)
                     VALUES ($1, $2, $3, $4)
                     ON CONFLICT (guild_id) DO UPDATE
                         SET appeal_channel_id = EXCLUDED.appeal_channel_id,
                             appeal_invite = EXCLUDED.appeal_invite,
                             appeal_guild_id = EXCLUDED.appeal_guild_id, updated_at = now()",
                )
                .bind(gid)
                .bind(channel(ch)?)
                .bind(invite)
                .bind(appeal_guild.map(|g| snowflake(g.get())).transpose()?)
                .execute(db)
                .await?;
            }
            Setting::TempVoiceHub(ch) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, tempvoice_hub_id) VALUES ($1, $2)
                     ON CONFLICT (guild_id) DO UPDATE SET tempvoice_hub_id = EXCLUDED.tempvoice_hub_id, updated_at = now()",
                )
                .bind(gid)
                .bind(channel(ch)?)
                .execute(db)
                .await?;
            }
            Setting::SuggestionChannel(ch) => {
                sqlx::query(
                    "INSERT INTO guild_settings (guild_id, suggestion_channel_id) VALUES ($1, $2)
                     ON CONFLICT (guild_id) DO UPDATE
                         SET suggestion_channel_id = EXCLUDED.suggestion_channel_id, updated_at = now()",
                )
                .bind(gid)
                .bind(channel(ch)?)
                .execute(db)
                .await?;
            }
        }

        self.cache.remove(&guild_id);
        Ok(())
    }
}

impl From<Row> for GuildConfig {
    fn from(r: Row) -> Self {
        Self {
            event_log_channel: to_id(r.event_log_channel_id).map(ChannelId::new),
            welcome_channel: to_id(r.welcome_channel_id).map(ChannelId::new),
            welcome_message: r.welcome_message,
            goodbye_channel: to_id(r.goodbye_channel_id).map(ChannelId::new),
            goodbye_message: r.goodbye_message,
            autorole: to_id(r.autorole_id).map(RoleId::new),
            min_account_age_days: u32::try_from(r.min_account_age_days).unwrap_or(0),
            raid_join_threshold: u32::try_from(r.raid_join_threshold).unwrap_or(0),
            starboard_channel: to_id(r.starboard_channel_id).map(ChannelId::new),
            starboard_threshold: u32::try_from(r.starboard_threshold).unwrap_or(3).max(1),
            suggestion_channel: to_id(r.suggestion_channel_id).map(ChannelId::new),
            birthday_channel: to_id(r.birthday_channel_id).map(ChannelId::new),
            birthday_role: to_id(r.birthday_role_id).map(RoleId::new),
            tempvoice_hub: to_id(r.tempvoice_hub_id).map(ChannelId::new),
            verify_role: to_id(r.verify_role_id).map(RoleId::new),
            verify_math: r.verify_math,
            appeal_channel: to_id(r.appeal_channel_id).map(ChannelId::new),
            appeal_invite: r.appeal_invite,
            appeal_guild: to_id(r.appeal_guild_id).map(GuildId::new),
            animal_channel: to_id(r.animal_channel_id).map(ChannelId::new),
        }
    }
}

pub fn to_id(raw: Option<i64>) -> Option<u64> {
    raw.and_then(|id| u64::try_from(id).ok()).filter(|id| *id != 0)
}
