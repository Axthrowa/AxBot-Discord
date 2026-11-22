//! Başarımlar: üyeler mesaj yazdıkça, tepki verdikçe, seste zaman geçirdikçe ve alt başlık açtıkça
//! rozet kazanır.
//!
//! Sayaçlar Redis'te bir hash'te tutulur (`ach:{sunucu}:{üye}`); mesaj başına tek `HINCRBY`.
//! PostgreSQL'e yalnızca bir başarım kazanıldığında yazılır. Eşik "tam geçildiği anda" yakalandığı
//! için kazanılmış başarımları mesaj başına okumak gerekmez.

use std::collections::HashMap;
use std::sync::Arc;

use poise::serenity_prelude::{self as serenity, ChannelId, CreateAllowedMentions, CreateMessage, GuildId, Mentionable, RoleId, User, UserId, VoiceState};
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{debug, warn};

use crate::data::Data;
use crate::db::snowflake;
use crate::reaction_roles;
use crate::settings::PluginConfig;

pub const MAX_CUSTOM: usize = 30;
pub const MAX_REWARD_XP: u32 = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Messages,
    Reactions,
    Voice,
    Threads,
}

impl Kind {
    pub const ALL: [(Self, &'static str, &'static str); 4] = [
        (Self::Messages, "messages", "gönderilen mesaj"),
        (Self::Reactions, "reactions", "eklenen tepki"),
        (Self::Voice, "voice", "seste geçirilen dakika"),
        (Self::Threads, "threads", "açılan alt başlık"),
    ];

    pub fn code(self) -> &'static str {
        Self::ALL.iter().find(|(k, _, _)| *k == self).map_or("messages", |(_, code, _)| code)
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, c, _)| *c == code).map(|(k, _, _)| *k)
    }

    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|(k, _, _)| *k == self).map_or("", |(_, _, label)| label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    /// Kalıcı kimlik (kazanılan başarımlar bununla saklanır).
    pub key: String,
    pub name: String,
    pub kind: Kind,
    pub threshold: u64,
    #[serde(default)]
    pub reward_role: Option<u64>,
    #[serde(default)]
    pub reward_xp: u32,
}

/// Hazır başarımlar: `(kimlik, ad, tür, eşik)`.
pub const DEFAULTS: &[(&str, &str, Kind, u64)] = &[
    ("msg_100", "Sohbete Isınan", Kind::Messages, 100),
    ("msg_1000", "Sohbet Kuşu", Kind::Messages, 1_000),
    ("msg_10000", "Klavye Delen", Kind::Messages, 10_000),
    ("msg_100000", "Efsane Geveze", Kind::Messages, 100_000),
    ("react_10", "Tepki Çırağı", Kind::Reactions, 10),
    ("react_100", "Gerçek Yıldız", Kind::Reactions, 100),
    ("react_500", "Emoji Ustası", Kind::Reactions, 500),
    ("react_1000", "Ateşli Tepkici", Kind::Reactions, 1_000),
    ("voice_20", "Dinleyici", Kind::Voice, 20),
    ("voice_60", "Muhabbet Ehli", Kind::Voice, 60),
    ("voice_180", "Otur Biraz Dinle", Kind::Voice, 180),
    ("voice_720", "Sabahçı", Kind::Voice, 720),
    ("voice_1440", "Uykuya Kim İhtiyaç Duyar", Kind::Voice, 1_440),
    ("thread_5", "Kıvılcım", Kind::Threads, 5),
    ("thread_10", "Konu Açan", Kind::Threads, 10),
    ("thread_25", "Alt Başlık Kralı", Kind::Threads, 25),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AchievementSettings {
    pub enabled: bool,
    /// Kazanılan başarım kanalda duyurulur.
    pub announce: bool,
    /// Duyuru kanalı; boşsa başarımın kazanıldığı kanal (seste kazanılanlar duyurulmaz).
    pub channel: Option<u64>,
    /// Kapatılmış hazır başarımların kimlikleri.
    pub disabled_defaults: Vec<String>,
    pub custom: Vec<Definition>,
}

impl Default for AchievementSettings {
    fn default() -> Self {
        Self { enabled: false, announce: true, channel: None, disabled_defaults: Vec::new(), custom: Vec::new() }
    }
}

impl PluginConfig for AchievementSettings {
    const KEY: &'static str = "achievements";
}

impl AchievementSettings {
    /// Açık hazır başarımlar + sunucunun kendi başarımları.
    pub fn definitions(&self) -> Vec<Definition> {
        DEFAULTS
            .iter()
            .filter(|(key, ..)| !self.disabled_defaults.iter().any(|d| d == key))
            .map(|(key, name, kind, threshold)| Definition {
                key: (*key).to_owned(),
                name: (*name).to_owned(),
                kind: *kind,
                threshold: *threshold,
                reward_role: None,
                reward_xp: 0,
            })
            .chain(self.custom.iter().cloned())
            .collect()
    }
}

/// Sayaç `old` → `new` olunca eşiği geçilen başarımlar.
pub fn crossed(definitions: &[Definition], kind: Kind, old: u64, new: u64) -> Vec<&Definition> {
    definitions.iter().filter(|d| d.kind == kind && d.threshold > old && d.threshold <= new).collect()
}

fn counter_key(guild_id: GuildId, user: UserId) -> String {
    format!("ach:{guild_id}:{user}")
}

/// Başarım kontrolleri için gereken bağımlılıklar.
pub struct AchievementContext<'a> {
    pub ctx: &'a serenity::Context,
    pub db: &'a PgPool,
    pub redis: &'a ConnectionManager,
    pub settings: &'a crate::settings::PluginSettings,
    pub leveling: &'a crate::leveling::Leveling,
}

/// Sayacı artırır; bir eşik geçildiyse başarımı kazandırır. `channel`: olayın olduğu kanal (duyuru için).
pub async fn bump(ctx: &serenity::Context, data: &Data, guild_id: GuildId, user: &User, kind: Kind, amount: u64, channel: Option<ChannelId>) {
    let actx = AchievementContext {
        ctx,
        db: &data.db,
        redis: &data.redis,
        settings: &data.settings,
        leveling: &data.leveling,
    };
    bump_with(actx, guild_id, user, kind, amount, channel).await;
}

/// Arka plan görevleri veya kısmi durumlar için `Data` referansı olmadan başarım sayacını artıran yardımcı.
pub async fn bump_with(
    actx: AchievementContext<'_>,
    guild_id: GuildId,
    user: &User,
    kind: Kind,
    amount: u64,
    channel: Option<ChannelId>,
) {
    if user.bot || amount == 0 {
        return;
    }
    let settings: Arc<AchievementSettings> = match actx.settings.get(actx.db, guild_id).await {
        Ok(settings) => settings,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Başarım ayarları okunamadı");
            return;
        }
    };
    if !settings.enabled {
        return;
    }
    let mut conn = actx.redis.clone();
    let new: redis::RedisResult<i64> = redis::cmd("HINCRBY")
        .arg(counter_key(guild_id, user.id))
        .arg(kind.code())
        .arg(amount)
        .query_async(&mut conn)
        .await;
    let new = match new {
        Ok(new) => u64::try_from(new).unwrap_or(0),
        Err(e) => {
            debug!(error = %e, "Başarım sayacı güncellenemedi");
            return;
        }
    };
    let definitions = settings.definitions();
    for definition in crossed(&definitions, kind, new.saturating_sub(amount), new) {
        if let Err(e) = unlock_with(&actx, guild_id, user, definition, &settings, channel).await {
            warn!(guild = %guild_id, user = %user.id, key = definition.key, error = ?e, "Başarım kaydedilemedi");
        }
    }
}

async fn unlock_with(
    actx: &AchievementContext<'_>,
    guild_id: GuildId,
    user: &User,
    definition: &Definition,
    settings: &AchievementSettings,
    channel: Option<ChannelId>,
) -> anyhow::Result<()> {
    let inserted = sqlx::query("INSERT INTO member_achievements (guild_id, user_id, key) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.id.get())?)
        .bind(&definition.key)
        .execute(actx.db)
        .await?
        .rows_affected();
    // Sayaç sıfırlanıp yeniden geçildiyse başarım ikinci kez verilmez.
    if inserted == 0 {
        return Ok(());
    }

    let mut rewards = Vec::new();
    if definition.reward_xp > 0 {
        actx.leveling.add_xp(actx.db, actx.redis, guild_id, user.id, i64::from(definition.reward_xp)).await?;
        rewards.push(format!("+{} XP", definition.reward_xp));
    }
    if let Some(role_id) = definition.reward_role.filter(|id| *id != 0).map(RoleId::new) {
        let role = actx.ctx.cache.guild(guild_id).and_then(|g| g.roles.get(&role_id).cloned());
        // Yetkili roller ödül olarak verilmez.
        if role.is_some_and(|role| reaction_roles::check_role(actx.ctx, guild_id, &role).is_ok()) {
            match actx.ctx.http.add_member_role(guild_id, user.id, role_id, Some("Başarım ödülü")).await {
                Ok(()) => rewards.push(role_id.mention().to_string()),
                Err(e) => warn!(guild = %guild_id, role = %role_id, error = %e, "Başarım ödül rolü verilemedi"),
            }
        }
    }

    let target = settings.channel.filter(|id| *id != 0).map(ChannelId::new).or(channel);
    if let (true, Some(target)) = (settings.announce, target) {
        let mut text = format!(
            "🏅 {} **{}** başarımını kazandı! ({} {})",
            user.id.mention(),
            definition.name,
            definition.threshold,
            definition.kind.label()
        );
        if !rewards.is_empty() {
            text.push_str(&format!(" Ödül: {}", rewards.join(", ")));
        }
        let message = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new().users(vec![user.id]));
        if let Err(e) = target.send_message(&actx.ctx.http, message).await {
            debug!(channel = %target, error = %e, "Başarım duyurusu gönderilemedi");
        }
    }
    Ok(())
}

/// Üyenin sayaçları ve kazandığı başarımların kimlikleri.
pub async fn progress(data: &Data, guild_id: GuildId, user: UserId) -> anyhow::Result<(HashMap<Kind, u64>, Vec<String>)> {
    let mut conn = data.redis.clone();
    let raw: HashMap<String, i64> = redis::cmd("HGETALL").arg(counter_key(guild_id, user)).query_async(&mut conn).await?;
    let counters = raw
        .into_iter()
        .filter_map(|(code, value)| Some((Kind::from_code(&code)?, u64::try_from(value).unwrap_or(0))))
        .collect();
    let unlocked: Vec<String> = sqlx::query_scalar("SELECT key FROM member_achievements WHERE guild_id = $1 AND user_id = $2")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .fetch_all(&data.db)
        .await?;
    Ok((counters, unlocked))
}

/// Ses kanalına giriş/çıkışı izler.
/// Ses dakikaları ve ses XP'si `analytics::spawn_voice_tracker` tarafından periyodik sayılır.
pub async fn on_voice(_ctx: &serenity::Context, _data: &Data, _new: &VoiceState) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_are_crossed_once() {
        let definitions = AchievementSettings::default().definitions();
        let names = |list: Vec<&Definition>| list.into_iter().map(|d| d.key.clone()).collect::<Vec<_>>();
        assert_eq!(names(crossed(&definitions, Kind::Messages, 99, 100)), vec!["msg_100"]);
        assert!(crossed(&definitions, Kind::Messages, 100, 101).is_empty());
        assert!(crossed(&definitions, Kind::Reactions, 99, 100).iter().all(|d| d.kind == Kind::Reactions));
        // Seste tek seferde birden fazla eşik geçilebilir.
        assert_eq!(names(crossed(&definitions, Kind::Voice, 10, 200)), vec!["voice_20", "voice_60", "voice_180"]);
    }

    #[test]
    fn defaults_have_unique_keys_and_can_be_disabled() {
        let mut keys: Vec<&str> = DEFAULTS.iter().map(|(key, ..)| *key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), DEFAULTS.len());

        let custom = Definition { key: "c1".to_owned(), name: "Özel".to_owned(), kind: Kind::Threads, threshold: 2, reward_role: None, reward_xp: 50 };
        let settings = AchievementSettings { disabled_defaults: vec!["msg_100".to_owned()], custom: vec![custom], ..AchievementSettings::default() };
        let definitions = settings.definitions();
        assert_eq!(definitions.len(), DEFAULTS.len());
        assert!(!definitions.iter().any(|d| d.key == "msg_100"));
        assert!(definitions.iter().any(|d| d.key == "c1" && d.reward_xp == 50));
    }

    #[test]
    fn kinds_round_trip_and_old_records_load() -> anyhow::Result<()> {
        for (kind, code, _) in Kind::ALL {
            assert_eq!(Kind::from_code(code), Some(kind));
            assert_eq!(kind.code(), code);
        }
        let settings: AchievementSettings = serde_json::from_str("{\"enabled\":true,\"custom\":[{\"key\":\"a\",\"name\":\"A\",\"kind\":\"voice\",\"threshold\":5}]}")?;
        assert!(settings.enabled && settings.announce);
        assert_eq!(settings.custom[0].reward_xp, 0);
        Ok(())
    }
}
