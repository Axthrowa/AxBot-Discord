//! Sunucu yedeği: rollerin ve kanalların yapısı (adlar, renkler, izinler, kategoriler, konu, yavaş mod…).
//!
//! Geri yükleme YIKICI DEĞİLDİR: hiçbir şeyi silmez ya da değiştirmez; yalnızca yedekte olup sunucuda
//! (aynı adla) bulunmayan rolleri ve kanalları yeniden oluşturur. Mesajlar, üyeler ve kanala özel
//! izinler yedeklenmez.

use std::collections::HashMap;

use poise::serenity_prelude::{self as serenity, ChannelId, ChannelType, CreateChannel, EditRole, GuildId, Permissions};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::warn;

use crate::db::snowflake;

pub const MAX_BACKUPS: i64 = 5;
pub const MAX_NAME: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSnap {
    pub name: String,
    pub colour: u32,
    pub hoist: bool,
    pub mentionable: bool,
    pub permissions: u64,
    pub position: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSnap {
    pub name: String,
    /// `text`, `news`, `voice`, `stage`, `forum`, `category`
    pub kind: String,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub nsfw: bool,
    #[serde(default)]
    pub slowmode: u16,
    /// Bağlı olduğu kategorinin adı.
    #[serde(default)]
    pub parent: Option<String>,
    pub position: u16,
    #[serde(default)]
    pub user_limit: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub guild_name: String,
    pub roles: Vec<RoleSnap>,
    pub channels: Vec<ChannelSnap>,
}

fn kind_code(kind: ChannelType) -> Option<&'static str> {
    Some(match kind {
        ChannelType::Text => "text",
        ChannelType::News => "news",
        ChannelType::Voice => "voice",
        ChannelType::Stage => "stage",
        ChannelType::Forum => "forum",
        ChannelType::Category => "category",
        _ => return None,
    })
}

fn kind_from(code: &str) -> Option<ChannelType> {
    Some(match code {
        "text" => ChannelType::Text,
        // Duyuru ve sahne kanalları her sunucuda açılamaz; düz metin / ses olarak geri yüklenir.
        "news" => ChannelType::Text,
        "voice" | "stage" => ChannelType::Voice,
        "forum" => ChannelType::Forum,
        "category" => ChannelType::Category,
        _ => return None,
    })
}

/// Botun göremediği kanal: Discord 16 Kasım 2026'dan beri böyle kanalları adı gizlenmiş olarak gönderir.
pub fn is_hidden(channel: &serenity::GuildChannel) -> bool {
    channel.name == "___hidden___"
}

/// Sunucunun şu anki yapısı (önbellekten; ağ isteği yok). Botun göremediği kanallar alınmaz.
pub fn take(cache: &serenity::Cache, guild_id: GuildId) -> Option<Snapshot> {
    let guild = cache.guild(guild_id)?;
    let mut roles: Vec<RoleSnap> = guild
        .roles
        .values()
        // @everyone ve botlara / entegrasyonlara ait roller elle oluşturulamaz.
        .filter(|r| r.id != guild_id.everyone_role() && !r.managed)
        .map(|r| RoleSnap { name: r.name.clone(), colour: r.colour.0, hoist: r.hoist, mentionable: r.mentionable, permissions: r.permissions.bits(), position: r.position })
        .collect();
    roles.sort_by(|a, b| b.position.cmp(&a.position).then(a.name.cmp(&b.name)));
    let mut channels: Vec<ChannelSnap> = guild
        .channels
        .values()
        .filter(|c| !is_hidden(c))
        .filter_map(|c| {
            Some(ChannelSnap {
                name: c.name.clone(),
                kind: kind_code(c.kind)?.to_owned(),
                topic: c.topic.clone().filter(|t| !t.is_empty()),
                nsfw: c.nsfw,
                slowmode: c.rate_limit_per_user.unwrap_or(0),
                parent: c.parent_id.and_then(|p| guild.channels.get(&p)).map(|p| p.name.clone()),
                position: c.position,
                user_limit: c.user_limit,
            })
        })
        .collect();
    channels.sort_by(|a, b| a.position.cmp(&b.position).then(a.name.cmp(&b.name)));
    Some(Snapshot { guild_name: guild.name.clone(), roles, channels })
}

pub struct Stored {
    pub id: i64,
    pub name: String,
    pub created_unix: i64,
    pub roles: usize,
    pub channels: usize,
}

/// Elle alınan yedekler (otomatik yedekler kendi sınırıyla ayrı sayılır).
pub async fn count(db: &PgPool, guild_id: GuildId) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM guild_backups WHERE guild_id = $1 AND name NOT LIKE $2")
        .bind(snowflake(guild_id.get())?)
        .bind(format!("{AUTO_PREFIX}%"))
        .fetch_one(db)
        .await?)
}

// ---------------------------------------------------------------- Zamanlanmış yedek

/// Otomatik yedeklerin ad öneki; en fazla `MAX_AUTO` tanesi tutulur, eskisi silinir.
pub const AUTO_PREFIX: &str = "Otomatik yedek";
const MAX_AUTO: i64 = 3;
/// Panelde seçilebilen aralıklar (gün; 0 = kapalı).
pub const AUTO_DAYS: [u32; 5] = [0, 1, 3, 7, 30];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupSettings {
    pub every_days: u32,
    /// Son otomatik yedeğin zamanı (Unix saniyesi).
    pub last_unix: i64,
}

impl crate::settings::PluginConfig for BackupSettings {
    const KEY: &'static str = "backup";
}

/// Yedek zamanı geldi mi?
pub fn auto_due(settings: &BackupSettings, now: i64) -> bool {
    settings.every_days > 0 && now - settings.last_unix >= i64::from(settings.every_days) * 86_400
}

/// Saatte bir, zamanı gelen sunucuların yapısını (roller, kanallar, izinler) yedekler.
pub fn spawn_auto(ctx: serenity::Context, db: PgPool, settings: crate::settings::PluginSettings) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            crate::supervise::round("otomatik sunucu yedeği", std::time::Duration::from_secs(600), async {
                let now = serenity::Timestamp::now().unix_timestamp();
                for guild_id in ctx.cache.guilds() {
                    let Ok(current) = settings.get::<BackupSettings>(&db, guild_id).await else { continue };
                    if !auto_due(&current, now) {
                        continue;
                    }
                    let Some(snapshot) = take(&ctx.cache, guild_id) else { continue };
                    // Önce zaman yazılır: hata olsa bile her saat yeniden denenip yedek yığılmaz.
                    if let Err(e) = settings.set(&db, guild_id, BackupSettings { last_unix: now, ..(*current).clone() }).await {
                        warn!(guild = %guild_id, error = ?e, "Otomatik yedek zamanı kaydedilemedi");
                        continue;
                    }
                    let name = format!("{AUTO_PREFIX} {}", chrono::Utc::now().format("%Y-%m-%d"));
                    let saved = async {
                        save(&db, guild_id, &name, &snapshot).await?;
                        sqlx::query(
                            "DELETE FROM guild_backups WHERE guild_id = $1 AND name LIKE $2 AND id NOT IN \
                             (SELECT id FROM guild_backups WHERE guild_id = $1 AND name LIKE $2 ORDER BY id DESC LIMIT $3)",
                        )
                        .bind(snowflake(guild_id.get())?)
                        .bind(format!("{AUTO_PREFIX}%"))
                        .bind(MAX_AUTO)
                        .execute(&db)
                        .await?;
                        anyhow::Ok(())
                    };
                    match saved.await {
                        Ok(()) => tracing::info!(guild = %guild_id, "Otomatik sunucu yedeği alındı"),
                        Err(e) => warn!(guild = %guild_id, error = ?e, "Otomatik sunucu yedeği alınamadı"),
                    }
                }
            })
            .await;
        }
    });
}

pub async fn save(db: &PgPool, guild_id: GuildId, name: &str, snapshot: &Snapshot) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("INSERT INTO guild_backups (guild_id, name, data) VALUES ($1, $2, $3) RETURNING id")
        .bind(snowflake(guild_id.get())?)
        .bind(name)
        .bind(serde_json::to_string(snapshot)?)
        .fetch_one(db)
        .await?)
}

pub async fn list(db: &PgPool, guild_id: GuildId) -> anyhow::Result<Vec<Stored>> {
    let rows: Vec<(i64, String, i64, String)> =
        sqlx::query_as("SELECT id, name, EXTRACT(EPOCH FROM created_at)::BIGINT, data FROM guild_backups WHERE guild_id = $1 ORDER BY id DESC")
            .bind(snowflake(guild_id.get())?)
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, created_unix, data)| {
            let snapshot: Snapshot = serde_json::from_str(&data).unwrap_or_default();
            Stored { id, name, created_unix, roles: snapshot.roles.len(), channels: snapshot.channels.len() }
        })
        .collect())
}

/// Yalnızca bu sunucunun yedeği okunabilir.
pub async fn load(db: &PgPool, guild_id: GuildId, id: i64) -> anyhow::Result<Option<Snapshot>> {
    let data: Option<String> = sqlx::query_scalar("SELECT data FROM guild_backups WHERE id = $1 AND guild_id = $2")
        .bind(id)
        .bind(snowflake(guild_id.get())?)
        .fetch_optional(db)
        .await?;
    Ok(data.and_then(|d| serde_json::from_str(&d).ok()))
}

pub async fn delete(db: &PgPool, guild_id: GuildId, id: i64) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM guild_backups WHERE id = $1 AND guild_id = $2").bind(id).bind(snowflake(guild_id.get())?).execute(db).await?;
    Ok(result.rows_affected() > 0)
}

/// Geri yüklemede oluşturulacaklar: yedekte olup sunucuda aynı adla (ve kanal için aynı türde) bulunmayanlar.
pub fn missing(snapshot: &Snapshot, current: &Snapshot) -> (Vec<RoleSnap>, Vec<ChannelSnap>) {
    let roles = snapshot.roles.iter().filter(|r| !current.roles.iter().any(|c| c.name == r.name)).cloned().collect();
    let channels = snapshot.channels.iter().filter(|ch| !current.channels.iter().any(|c| c.name == ch.name && c.kind == ch.kind)).cloned().collect();
    (roles, channels)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Restored {
    pub roles: usize,
    pub channels: usize,
    pub failed: usize,
}

/// Eksik rolleri ve kanalları oluşturur. `bot_permissions`: bot kendinde olmayan bir izni role veremez,
/// bu yüzden rol izinleri botun izinleriyle kesiştirilir.
pub async fn restore(http: &serenity::Http, cache: &serenity::Cache, guild_id: GuildId, snapshot: &Snapshot, bot_permissions: Permissions) -> Option<Restored> {
    let current = take(cache, guild_id)?;
    let (roles, channels) = missing(snapshot, &current);
    let mut done = Restored::default();
    let reason = "Yedekten geri yükleme";

    // Roller alttan üste oluşturulur: Discord yeni rolü en alta koyar, böylece sıra yaklaşık korunur.
    for role in roles.iter().rev() {
        let permissions = Permissions::from_bits_truncate(role.permissions) & bot_permissions;
        let builder = EditRole::new().name(&role.name).colour(role.colour).hoist(role.hoist).mentionable(role.mentionable).permissions(permissions).audit_log_reason(reason);
        match guild_id.create_role(http, builder).await {
            Ok(_) => done.roles += 1,
            Err(e) => {
                warn!(guild = %guild_id, role = %role.name, error = %e, "Yedekten rol oluşturulamadı");
                done.failed += 1;
            }
        }
    }

    // Önce kategoriler; kanallar kategorinin adına göre bağlanır.
    let mut categories: HashMap<String, ChannelId> = cache
        .guild(guild_id)
        .map(|g| g.channels.values().filter(|c| c.kind == ChannelType::Category).map(|c| (c.name.clone(), c.id)).collect())
        .unwrap_or_default();
    let (cats, others): (Vec<&ChannelSnap>, Vec<&ChannelSnap>) = channels.iter().partition(|c| c.kind == "category");
    for channel in cats.into_iter().chain(others) {
        let Some(kind) = kind_from(&channel.kind) else {
            done.failed += 1;
            continue;
        };
        let mut builder = CreateChannel::new(&channel.name).kind(kind).audit_log_reason(reason);
        if let Some(parent) = channel.parent.as_ref().and_then(|name| categories.get(name)) {
            builder = builder.category(*parent);
        }
        if matches!(kind, ChannelType::Text | ChannelType::Forum) {
            if let Some(topic) = &channel.topic {
                builder = builder.topic(topic);
            }
            builder = builder.nsfw(channel.nsfw).rate_limit_per_user(channel.slowmode.min(21_600));
        }
        if let (ChannelType::Voice, Some(limit)) = (kind, channel.user_limit) {
            builder = builder.user_limit(limit.min(99));
        }
        match guild_id.create_channel(http, builder).await {
            Ok(created) => {
                done.channels += 1;
                if kind == ChannelType::Category {
                    categories.insert(channel.name.clone(), created.id);
                }
            }
            Err(e) => {
                warn!(guild = %guild_id, channel = %channel.name, error = %e, "Yedekten kanal oluşturulamadı");
                done.failed += 1;
            }
        }
    }
    Some(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_backup_runs_on_schedule() {
        let off = BackupSettings::default();
        assert!(!auto_due(&off, 1_000_000));
        let weekly = BackupSettings { every_days: 7, last_unix: 1_000_000 };
        assert!(!auto_due(&weekly, 1_000_000 + 6 * 86_400));
        assert!(auto_due(&weekly, 1_000_000 + 7 * 86_400));
        // Hiç yedek alınmamışsa ilk turda alınır.
        assert!(auto_due(&BackupSettings { every_days: 1, last_unix: 0 }, 1_000_000));
    }

    fn role(name: &str) -> RoleSnap {
        RoleSnap { name: name.to_owned(), colour: 0, hoist: false, mentionable: false, permissions: 0, position: 1 }
    }

    fn channel(name: &str, kind: &str) -> ChannelSnap {
        ChannelSnap { name: name.to_owned(), kind: kind.to_owned(), topic: None, nsfw: false, slowmode: 0, parent: None, position: 0, user_limit: None }
    }

    #[test]
    fn only_missing_items_are_restored() {
        let backup = Snapshot { guild_name: "S".to_owned(), roles: vec![role("Üye"), role("VIP")], channels: vec![channel("genel", "text"), channel("genel", "voice"), channel("Sohbet", "category")] };
        let now = Snapshot { guild_name: "S".to_owned(), roles: vec![role("Üye")], channels: vec![channel("genel", "text")] };
        let (roles, channels) = missing(&backup, &now);
        assert_eq!(roles.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["VIP"]);
        // Aynı adlı ama farklı türdeki kanal eksik sayılır.
        assert_eq!(channels.iter().map(|c| (c.name.as_str(), c.kind.as_str())).collect::<Vec<_>>(), [("genel", "voice"), ("Sohbet", "category")]);
        assert_eq!(missing(&backup, &backup), (Vec::new(), Vec::new()));
    }

    #[test]
    fn snapshots_survive_json_and_old_formats() -> anyhow::Result<()> {
        let backup = Snapshot { guild_name: "S".to_owned(), roles: vec![role("VIP")], channels: vec![channel("genel", "text")] };
        assert_eq!(serde_json::from_str::<Snapshot>(&serde_json::to_string(&backup)?)?, backup);
        // Eksik alanlı eski kayıt varsayılanlarla okunur.
        let old: Snapshot = serde_json::from_str(r#"{"channels":[{"name":"a","kind":"text","position":1}]}"#)?;
        assert_eq!(old.channels.first().map(|c| c.slowmode), Some(0));
        Ok(())
    }

    #[test]
    fn channel_kinds_map_back() {
        for kind in [ChannelType::Text, ChannelType::Voice, ChannelType::Forum, ChannelType::Category] {
            assert_eq!(kind_code(kind).and_then(kind_from), Some(kind));
        }
        assert_eq!(kind_from("news"), Some(ChannelType::Text));
        assert_eq!(kind_code(ChannelType::PublicThread), None);
        assert_eq!(kind_from("x"), None);
    }
}
