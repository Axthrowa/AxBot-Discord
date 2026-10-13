//! Yıkım koruması (anti-nuke): bir yetkili kısa sürede çok sayıda kanal / rol siler ya da üye
//! yasaklar / atarsa durdurulur.
//!
//! Kimin yaptığı denetim kaydından (audit log) okunur; bu yüzden botun 'Denetim Kaydını Görüntüle'
//! izni olmalı. Sayaçlar bellektedir. Sunucu sahibi, botun kendisi ve güvenilen roller sayılmaz.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use poise::serenity_prelude::audit_log::{Action, ChannelAction, MemberAction, RoleAction};
use poise::serenity_prelude::{self as serenity, CreateAllowedMentions, CreateMessage, GuildId, RoleId, UserId};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::data::Data;
use crate::settings::PluginConfig;

pub const MIN_LIMIT: u32 = 2;
pub const MAX_LIMIT: u32 = 20;
pub const MIN_WINDOW: u32 = 10;
pub const MAX_WINDOW: u32 = 300;
/// Denetim kaydı bu kadar saniyeden eskiyse olayla ilgisiz sayılır.
const AUDIT_FRESH_SECS: i64 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Punishment {
    /// Yetki içeren rolleri alınır (varsayılan; geri alması kolay).
    Strip,
    Kick,
    Ban,
}

impl Punishment {
    pub const ALL: [(Self, &'static str, &'static str); 3] = [
        (Self::Strip, "strip", "Yetkili rollerini al (önerilen)"),
        (Self::Kick, "kick", "Sunucudan at"),
        (Self::Ban, "ban", "Yasakla"),
    ];

    pub fn key(self) -> &'static str {
        Self::ALL.iter().find(|(p, ..)| *p == self).map_or("strip", |(_, key, _)| key)
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, key, _)| *key == raw).map(|(p, ..)| *p)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AntiNukeSettings {
    pub enabled: bool,
    /// `window_secs` içinde bu kadar yıkıcı işlem yapan durdurulur.
    pub limit: u32,
    pub window_secs: u32,
    pub punishment: Punishment,
    /// Bu rollerdeki üyeler sayılmaz.
    pub trusted_roles: Vec<u64>,
}

impl Default for AntiNukeSettings {
    fn default() -> Self {
        Self { enabled: false, limit: 3, window_secs: 30, punishment: Punishment::Strip, trusted_roles: Vec::new() }
    }
}

impl PluginConfig for AntiNukeSettings {
    const KEY: &'static str = "antinuke";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    ChannelDelete,
    RoleDelete,
    Ban,
    Kick,
}

impl Kind {
    fn action(self) -> Action {
        match self {
            Self::ChannelDelete => Action::Channel(ChannelAction::Delete),
            Self::RoleDelete => Action::Role(RoleAction::Delete),
            Self::Ban => Action::Member(MemberAction::BanAdd),
            Self::Kick => Action::Member(MemberAction::Kick),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ChannelDelete => "kanal silme",
            Self::RoleDelete => "rol silme",
            Self::Ban => "yasaklama",
            Self::Kick => "atma",
        }
    }
}

static RECENT: LazyLock<DashMap<(GuildId, UserId), Vec<Instant>>> = LazyLock::new(DashMap::new);

/// Yeni işlemi ekler ve pencere içindeki işlem sayısını döner (eskiler atılır).
fn push_and_count(times: &mut Vec<Instant>, now: Instant, window: Duration) -> usize {
    times.retain(|at| now.duration_since(*at) <= window);
    times.push(now);
    times.len()
}

/// Bir yıkıcı olaydan sonra çağrılır. `target`: atma olayında ayrılan üye (denetim kaydıyla eşleşmeli).
pub async fn record(ctx: &serenity::Context, data: &Data, guild_id: GuildId, kind: Kind, target: Option<UserId>) {
    let Ok(settings) = data.settings.get::<AntiNukeSettings>(&data.db, guild_id).await else { return };
    if !settings.enabled {
        return;
    }
    let logs = match guild_id.audit_logs(&ctx.http, Some(kind.action()), None, None, Some(1)).await {
        Ok(logs) => logs,
        Err(e) => {
            warn!(guild = %guild_id, error = %e, "Yıkım koruması: denetim kaydı okunamadı ('Denetim Kaydını Görüntüle' izni gerekli)");
            return;
        }
    };
    let Some(entry) = logs.entries.first() else { return };
    let age = serenity::Timestamp::now().unix_timestamp() - entry.id.created_at().unix_timestamp();
    if !(0..=AUDIT_FRESH_SECS).contains(&age) {
        return;
    }
    // Atma: kayıt gerçekten bu üyeye aitse sayılır (kendi ayrılanlar denetim kaydı bırakmaz).
    if let Some(target) = target {
        if entry.target_id.map(|id| id.get()) != Some(target.get()) {
            return;
        }
    }
    let executor = entry.user_id;
    let bot = ctx.cache.current_user().id;
    // Önbellek kilidi hemen bırakılır.
    let (is_owner, roles) = match ctx.cache.guild(guild_id) {
        Some(guild) => (guild.owner_id == executor, guild.members.get(&executor).map(|m| m.roles.clone()).unwrap_or_default()),
        None => return,
    };
    if executor == bot || is_owner || roles.iter().any(|r| settings.trusted_roles.contains(&r.get())) {
        return;
    }

    let window = Duration::from_secs(u64::from(settings.window_secs.clamp(MIN_WINDOW, MAX_WINDOW)));
    let count = {
        let mut times = RECENT.entry((guild_id, executor)).or_default();
        push_and_count(&mut times, Instant::now(), window)
    };
    if count < settings.limit.clamp(MIN_LIMIT, MAX_LIMIT) as usize {
        return;
    }
    RECENT.remove(&(guild_id, executor));

    let reason = format!("Yıkım koruması: {} sn içinde {count} yıkıcı işlem (son: {})", settings.window_secs, kind.label());
    let outcome = punish(ctx, guild_id, executor, &roles, settings.punishment, &reason).await;
    info!(guild = %guild_id, user = %executor, count, outcome, "Yıkım koruması tetiklendi");
    if let Ok(cfg) = data.guild_configs.get(&data.db, guild_id).await {
        if let Some(channel) = cfg.event_log_channel {
            let text = format!("🚨 **Yıkım koruması** — <@{executor}> {} saniye içinde **{count}** yıkıcı işlem yaptı (son: {}).\n{outcome}", settings.window_secs, kind.label());
            let _ = channel.send_message(&ctx.http, CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await;
        }
    }
}

async fn punish(ctx: &serenity::Context, guild_id: GuildId, user: UserId, roles: &[RoleId], punishment: Punishment, reason: &str) -> &'static str {
    let result = match punishment {
        Punishment::Ban => guild_id.ban_with_reason(&ctx.http, user, 0, reason).await.map(|()| "Yasaklandı."),
        Punishment::Kick => guild_id.kick_with_reason(&ctx.http, user, reason).await.map(|()| "Sunucudan atıldı."),
        Punishment::Strip => {
            // Yetki içeren roller (önbellekten); kilit `await`'ten önce bırakılır.
            let dangerous: Vec<RoleId> = ctx
                .cache
                .guild(guild_id)
                .map(|g| roles.iter().copied().filter(|id| g.roles.get(id).is_some_and(|r| r.permissions.intersects(crate::rolepanel::dangerous()))).collect())
                .unwrap_or_default();
            match guild_id.member(&ctx.http, user).await {
                Ok(member) => member.remove_roles(&ctx.http, &dangerous).await.map(|_| "Yetki içeren rolleri alındı."),
                Err(e) => Err(e),
            }
        }
    };
    match result {
        Ok(text) => text,
        Err(e) => {
            warn!(guild = %guild_id, user = %user, error = %e, "Yıkım koruması cezayı uygulayamadı");
            "⚠️ Ceza UYGULANAMADI: botun rolü bu üyenin rollerinin üstünde olmalı. Hemen elle müdahale edin."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_recent_actions() {
        let start = Instant::now();
        let window = Duration::from_secs(30);
        let mut times = Vec::new();
        assert_eq!(push_and_count(&mut times, start, window), 1);
        assert_eq!(push_and_count(&mut times, start + Duration::from_secs(10), window), 2);
        assert_eq!(push_and_count(&mut times, start + Duration::from_secs(29), window), 3);
        // İlk iki işlem pencerenin dışında kaldı.
        assert_eq!(push_and_count(&mut times, start + Duration::from_secs(45), window), 2);
    }

    #[test]
    fn punishments_round_trip_and_defaults_are_safe() {
        for (punishment, key, _) in Punishment::ALL {
            assert_eq!(Punishment::parse(key), Some(punishment));
            assert_eq!(punishment.key(), key);
        }
        assert_eq!(Punishment::parse("sil"), None);
        let defaults = AntiNukeSettings::default();
        assert!(!defaults.enabled && defaults.punishment == Punishment::Strip && defaults.limit >= MIN_LIMIT);
    }
}
