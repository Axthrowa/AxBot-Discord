//! Yıkım koruması (anti-nuke): bir yetkili kısa sürede çok sayıda kanal / rol siler ya da açar, üye
//! yasaklar / atarsa durdurulur. Ek kalkanlar (Wick benzeri, ayrı ayrı açılır): güvenilmeyen birinin açtığı
//! webhook silinir, eklediği bot atılır, bir role verdiği yönetici/moderasyon yetkisi ya da bir üyeye verdiği
//! yetkili rol geri alınır. Bu işlemler de sınıra sayılır.
//!
//! Kimin yaptığı denetim kaydından (audit log) okunur; bu yüzden botun 'Denetim Kaydını Görüntüle'
//! izni olmalı. Sayaçlar bellektedir. Sunucu sahibi, botun kendisi ve güvenilen roller sayılmaz.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use poise::serenity_prelude::audit_log::{Action, ChannelAction, MemberAction, RoleAction, WebhookAction};
use poise::serenity_prelude::{self as serenity, CreateAllowedMentions, CreateMessage, EditRole, GuildId, Permissions, Role, RoleId, UserId, WebhookId};
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
    /// Güvenilmeyen birinin açtığı webhook anında silinir.
    pub guard_webhooks: bool,
    /// Güvenilmeyen birinin eklediği bot anında atılır.
    pub guard_bots: bool,
    /// Güvenilmeyen birinin bir role verdiği yönetici/moderasyon yetkisi ya da üyeye verdiği yetkili rol geri alınır.
    pub guard_permissions: bool,
}

impl Default for AntiNukeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            limit: 3,
            window_secs: 30,
            punishment: Punishment::Strip,
            trusted_roles: Vec::new(),
            guard_webhooks: true,
            guard_bots: true,
            guard_permissions: true,
        }
    }
}

impl PluginConfig for AntiNukeSettings {
    const KEY: &'static str = "antinuke";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    ChannelDelete,
    ChannelCreate,
    RoleDelete,
    Ban,
    Kick,
    WebhookCreate,
    BotAdd,
    RoleCreate,
    RoleUpdate,
    MemberRoles,
}

impl Kind {
    fn action(self) -> Action {
        match self {
            Self::ChannelDelete => Action::Channel(ChannelAction::Delete),
            Self::ChannelCreate => Action::Channel(ChannelAction::Create),
            Self::RoleDelete => Action::Role(RoleAction::Delete),
            Self::Ban => Action::Member(MemberAction::BanAdd),
            Self::Kick => Action::Member(MemberAction::Kick),
            Self::WebhookCreate => Action::Webhook(WebhookAction::Create),
            Self::BotAdd => Action::Member(MemberAction::BotAdd),
            Self::RoleCreate => Action::Role(RoleAction::Create),
            Self::RoleUpdate => Action::Role(RoleAction::Update),
            Self::MemberRoles => Action::Member(MemberAction::RoleUpdate),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ChannelDelete => "kanal silme",
            Self::ChannelCreate => "kanal açma",
            Self::RoleDelete => "rol silme",
            Self::Ban => "yasaklama",
            Self::Kick => "atma",
            Self::WebhookCreate => "webhook açma",
            Self::BotAdd => "bot ekleme",
            Self::RoleCreate | Self::RoleUpdate => "role yönetici yetkisi verme",
            Self::MemberRoles => "üyeye yetkili rol verme",
        }
    }
}

/// Sayaçlar: (sunucu, yapan, kova). Kanal açma ayrı kovada ve iki kat sınırla sayılır (yöneticiler meşru olarak
/// art arda birkaç kanal açabilir).
static RECENT: LazyLock<DashMap<(GuildId, UserId, u8), Vec<Instant>>> = LazyLock::new(DashMap::new);

/// Yeni işlemi ekler ve pencere içindeki işlem sayısını döner (eskiler atılır).
fn push_and_count(times: &mut Vec<Instant>, now: Instant, window: Duration) -> usize {
    times.retain(|at| now.duration_since(*at) <= window);
    times.push(now);
    times.len()
}

/// Denetim kaydından işlemi yapanı bulur. Yapan güvenilirse (sunucu sahibi, bot, güvenilen rol) ya da kayıt
/// taze değilse `None`. `target`: kaydın ait olması gereken hedef (üye, rol ya da bot kimliği).
async fn untrusted_executor(
    ctx: &serenity::Context,
    guild_id: GuildId,
    settings: &AntiNukeSettings,
    kind: Kind,
    target: Option<u64>,
) -> Option<(UserId, Vec<RoleId>, Option<u64>)> {
    let logs = match guild_id.audit_logs(&ctx.http, Some(kind.action()), None, None, Some(5)).await {
        Ok(logs) => logs,
        Err(e) => {
            warn!(guild = %guild_id, error = %e, "Yıkım koruması: denetim kaydı okunamadı ('Denetim Kaydını Görüntüle' izni gerekli)");
            return None;
        }
    };
    let now = serenity::Timestamp::now().unix_timestamp();
    let entry = logs.entries.iter().find(|entry| {
        let age = now - entry.id.created_at().unix_timestamp();
        (0..=AUDIT_FRESH_SECS).contains(&age) && target.is_none_or(|t| entry.target_id.map(|id| id.get()) == Some(t))
    })?;
    let executor = entry.user_id;
    let bot = ctx.cache.current_user().id;
    // Önbellek kilidi hemen bırakılır.
    let (is_owner, roles) = {
        let guild = ctx.cache.guild(guild_id)?;
        (guild.owner_id == executor, guild.members.get(&executor).map(|m| m.roles.clone()).unwrap_or_default())
    };
    if executor == bot || is_owner || roles.iter().any(|r| settings.trusted_roles.contains(&r.get())) {
        return None;
    }
    Some((executor, roles, entry.target_id.map(|id| id.get())))
}

async fn enabled_settings(data: &Data, guild_id: GuildId) -> Option<AntiNukeSettings> {
    let settings = data.settings.get::<AntiNukeSettings>(&data.db, guild_id).await.ok()?;
    settings.enabled.then(|| (*settings).clone())
}

/// Bir yıkıcı olaydan sonra çağrılır. `target`: atma olayında ayrılan üye (denetim kaydıyla eşleşmeli;
/// kendi ayrılanlar denetim kaydı bırakmaz).
pub async fn record(ctx: &serenity::Context, data: &Data, guild_id: GuildId, kind: Kind, target: Option<UserId>) {
    let Some(settings) = enabled_settings(data, guild_id).await else { return };
    let Some((executor, roles, _)) = untrusted_executor(ctx, guild_id, &settings, kind, target.map(UserId::get)).await else {
        return;
    };
    count_and_punish(ctx, data, guild_id, &settings, kind, (executor, &roles), None).await;
}

/// İşlemi sayar; sınır aşıldıysa cezalandırır. `undone`: anında geri alınan bir işlem varsa kayda yazılacak not.
async fn count_and_punish(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    settings: &AntiNukeSettings,
    kind: Kind,
    (executor, roles): (UserId, &[RoleId]),
    undone: Option<String>,
) {
    let window = Duration::from_secs(u64::from(settings.window_secs.clamp(MIN_WINDOW, MAX_WINDOW)));
    let base = settings.limit.clamp(MIN_LIMIT, MAX_LIMIT) as usize;
    let (bucket, limit) = if kind == Kind::ChannelCreate { (1, base * 2) } else { (0, base) };
    let count = {
        let mut times = RECENT.entry((guild_id, executor, bucket)).or_default();
        push_and_count(&mut times, Instant::now(), window)
    };
    let tripped = count >= limit;
    if !tripped && undone.is_none() {
        return;
    }
    let mut lines = Vec::new();
    if let Some(note) = undone {
        lines.push(format!("🛡️ **Yıkım koruması** — <@{executor}> izinsiz {}: {note}", kind.label()));
    }
    if tripped {
        RECENT.remove(&(guild_id, executor, bucket));
        let reason = format!("Yıkım koruması: {} sn içinde {count} yıkıcı işlem (son: {})", settings.window_secs, kind.label());
        let outcome = punish(ctx, guild_id, executor, roles, settings.punishment, &reason).await;
        info!(guild = %guild_id, user = %executor, count, outcome, "Yıkım koruması tetiklendi");
        lines.push(format!(
            "🚨 **Yıkım koruması** — <@{executor}> {} saniye içinde **{count}** yıkıcı işlem yaptı (son: {}).\n{outcome}",
            settings.window_secs,
            kind.label()
        ));
    }
    if let Ok(cfg) = data.guild_configs.get(&data.db, guild_id).await {
        if let Some(channel) = cfg.event_log_channel {
            let message = CreateMessage::new().content(lines.join("\n")).allowed_mentions(CreateAllowedMentions::new());
            let _ = channel.send_message(&ctx.http, message).await;
        }
    }
}

/// Bir kanalın webhook'ları değişti (açıldı / düzenlendi / silindi; hangisi olduğu denetim kaydından anlaşılır).
pub async fn on_webhook(ctx: &serenity::Context, data: &Data, guild_id: GuildId) {
    let Some(settings) = enabled_settings(data, guild_id).await else { return };
    let Some((executor, roles, target)) = untrusted_executor(ctx, guild_id, &settings, Kind::WebhookCreate, None).await else {
        return;
    };
    let mut undone = None;
    if settings.guard_webhooks {
        if let Some(id) = target.filter(|id| *id != 0) {
            undone = Some(match ctx.http.delete_webhook(WebhookId::new(id), Some("Yıkım koruması: izinsiz webhook")).await {
                Ok(()) => "webhook silindi.".to_owned(),
                Err(e) => format!("webhook SİLİNEMEDİ ({e}); botun 'Webhook'ları Yönet' izni olmalı."),
            });
        }
    }
    count_and_punish(ctx, data, guild_id, &settings, Kind::WebhookCreate, (executor, &roles), undone).await;
}

/// Sunucuya bir bot eklendi.
pub async fn on_bot_join(ctx: &serenity::Context, data: &Data, guild_id: GuildId, bot: UserId) {
    let Some(settings) = enabled_settings(data, guild_id).await else { return };
    let Some((executor, roles, _)) = untrusted_executor(ctx, guild_id, &settings, Kind::BotAdd, Some(bot.get())).await else {
        return;
    };
    let mut undone = None;
    if settings.guard_bots {
        undone = Some(match guild_id.kick_with_reason(&ctx.http, bot, "Yıkım koruması: güvenilmeyen bir üyenin eklediği bot").await {
            Ok(()) => format!("eklenen bot (<@{bot}>) atıldı."),
            Err(e) => format!("eklenen bot (<@{bot}>) ATILAMADI ({e})."),
        });
    }
    count_and_punish(ctx, data, guild_id, &settings, Kind::BotAdd, (executor, &roles), undone).await;
}

/// Bir role yeni yönetici/moderasyon yetkisi verildiyse kazanılan yetkiler (yoksa boş).
pub fn gained_dangerous(old: Permissions, new: Permissions) -> Permissions {
    new & crate::reaction_roles::dangerous() & !old
}

/// Bir rolün yetkileri değişti ya da rol oluşturuldu. `old`: önceki yetkiler (`None` = yeni rol).
pub async fn on_role_permissions(ctx: &serenity::Context, data: &Data, guild_id: GuildId, role: &Role, old: Option<Permissions>) {
    let gained = gained_dangerous(old.unwrap_or_else(Permissions::empty), role.permissions);
    if gained.is_empty() {
        return;
    }
    let Some(settings) = enabled_settings(data, guild_id).await else { return };
    let kind = if old.is_none() { Kind::RoleCreate } else { Kind::RoleUpdate };
    let Some((executor, roles, _)) = untrusted_executor(ctx, guild_id, &settings, kind, Some(role.id.get())).await else {
        return;
    };
    let mut undone = None;
    if settings.guard_permissions {
        let restored = role.permissions & !gained;
        let edit = EditRole::new().permissions(restored).audit_log_reason("Yıkım koruması: izinsiz yetki");
        undone = Some(match guild_id.edit_role(&ctx.http, role.id, edit).await {
            Ok(_) => format!("“{}” rolüne verilen yetki geri alındı.", role.name),
            Err(e) => format!("“{}” rolüne verilen yetki GERİ ALINAMADI ({e}); botun rolü bu rolün üstünde olmalı.", role.name),
        });
    }
    count_and_punish(ctx, data, guild_id, &settings, kind, (executor, &roles), undone).await;
}

/// Bir üyeye rol verildi: yetkili bir rolse ve veren güvenilmiyorsa geri alınır.
pub async fn on_member_roles_added(ctx: &serenity::Context, data: &Data, guild_id: GuildId, member: UserId, added: &[RoleId]) {
    let dangerous: Vec<RoleId> = ctx
        .cache
        .guild(guild_id)
        .map(|g| {
            added
                .iter()
                .copied()
                .filter(|id| g.roles.get(id).is_some_and(|r| r.permissions.intersects(crate::reaction_roles::dangerous())))
                .collect()
        })
        .unwrap_or_default();
    if dangerous.is_empty() {
        return;
    }
    let Some(settings) = enabled_settings(data, guild_id).await else { return };
    let Some((executor, roles, _)) = untrusted_executor(ctx, guild_id, &settings, Kind::MemberRoles, Some(member.get())).await else {
        return;
    };
    let mut undone = None;
    if settings.guard_permissions {
        let mut failed = 0;
        for role in &dangerous {
            if ctx.http.remove_member_role(guild_id, member, *role, Some("Yıkım koruması: izinsiz yetkili rol")).await.is_err() {
                failed += 1;
            }
        }
        undone = Some(if failed == 0 {
            format!("<@{member}> üyesine verilen yetkili rol geri alındı.")
        } else {
            format!("<@{member}> üyesine verilen {failed} yetkili rol GERİ ALINAMADI; botun rolü üstte olmalı.")
        });
    }
    count_and_punish(ctx, data, guild_id, &settings, Kind::MemberRoles, (executor, &roles), undone).await;
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
                .map(|g| roles.iter().copied().filter(|id| g.roles.get(id).is_some_and(|r| r.permissions.intersects(crate::reaction_roles::dangerous()))).collect())
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
    fn detects_only_newly_gained_dangerous_permissions() {
        let plain = Permissions::SEND_MESSAGES | Permissions::VIEW_CHANNEL;
        assert!(gained_dangerous(plain, plain | Permissions::ATTACH_FILES).is_empty());
        assert_eq!(gained_dangerous(plain, plain | Permissions::ADMINISTRATOR), Permissions::ADMINISTRATOR);
        // Zaten olan yetki "kazanılmış" sayılmaz.
        let admin = plain | Permissions::ADMINISTRATOR;
        assert!(gained_dangerous(admin, admin | Permissions::ATTACH_FILES).is_empty());
        assert!(gained_dangerous(Permissions::empty(), Permissions::BAN_MEMBERS).contains(Permissions::BAN_MEMBERS));
    }

    #[test]
    fn punishments_round_trip_and_defaults_are_safe() {
        // Panel şeması anahtarı doğrudan JSON'a yazar; serde adıyla aynı olmalı.
        for (punishment, key, _) in Punishment::ALL {
            assert_eq!(serde_json::to_value(punishment).unwrap(), serde_json::json!(key));
        }
        let defaults = AntiNukeSettings::default();
        assert!(!defaults.enabled && defaults.punishment == Punishment::Strip && defaults.limit >= MIN_LIMIT);
    }
}
