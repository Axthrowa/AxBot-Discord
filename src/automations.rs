//! Otomasyonlar: "şu olunca şunu yap" kuralları. Tetikleyiciler: mesaj, üye girişi/çıkışı, rol
//! verilmesi/alınması, tepki, alt başlık açılması. Eylemler: mesaj gönder, özelden yaz, rol ver/al,
//! tepki ekle, mesajı sil, alt başlık aç.
//!
//! Kurallar bellekte durur (`PluginSettings`); olay başına veritabanına gidilmez.
//! Güvenlik: roller her seferinde `rolepanel::check_role` ile denetlenir; bir üye için kısa sürede
//! çalışabilecek otomasyon sayısı sınırlıdır (birbirini tetikleyen kurallar sonsuz döngüye giremez).

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateAllowedMentions, CreateMessage, CreateThread, GuildId, Mentionable, Message,
    ReactionType, RoleId, User, UserId,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::data::Data;
use crate::rolepanel;
use crate::settings::PluginConfig;

pub const MAX_AUTOMATIONS: usize = 20;
/// Bir üye için bu sürede en fazla bu kadar otomasyon çalışır.
const BURST_WINDOW: Duration = Duration::from_secs(10);
const BURST_LIMIT: u8 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    #[default]
    Message,
    MemberJoin,
    MemberLeave,
    RoleAdded,
    RoleRemoved,
    Reaction,
    ThreadCreated,
}

impl Trigger {
    pub const ALL: [(Self, &'static str, &'static str); 7] = [
        (Self::Message, "message", "Mesaj gönderildiğinde"),
        (Self::MemberJoin, "member_join", "Üye sunucuya katıldığında"),
        (Self::MemberLeave, "member_leave", "Üye sunucudan ayrıldığında"),
        (Self::RoleAdded, "role_added", "Üyeye rol verildiğinde"),
        (Self::RoleRemoved, "role_removed", "Üyeden rol alındığında"),
        (Self::Reaction, "reaction", "Mesaja tepki eklendiğinde"),
        (Self::ThreadCreated, "thread_created", "Alt başlık açıldığında"),
    ];

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, c, _)| *c == code).map(|(t, _, _)| *t)
    }

    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|(t, _, _)| *t == self).map_or("", |(_, _, label)| label)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Automation {
    pub id: u32,
    pub name: String,
    pub enabled: bool,
    pub trigger: Trigger,
    // --- Koşullar (boş olan koşul aranmaz)
    /// Mesaj tetikleyicisi: mesaj bu metni içermeli (büyük/küçük harf fark etmez).
    pub keyword: Option<String>,
    /// Olay bu kanalda olmalı.
    pub channel: Option<u64>,
    /// Mesaj/tepki: üyede bu rol olmalı. Rol tetikleyicileri: verilen/alınan rol bu olmalı.
    pub role: Option<u64>,
    /// Tepki tetikleyicisi: eklenen emoji bu olmalı.
    pub emoji: Option<String>,
    // --- Eylemler
    pub message: Option<String>,
    /// Mesajın gideceği kanal; boşsa olayın olduğu kanal.
    pub message_channel: Option<u64>,
    pub dm: Option<String>,
    pub add_roles: Vec<u64>,
    pub remove_roles: Vec<u64>,
    /// Tetikleyen mesaja eklenecek tepki.
    pub react: Option<String>,
    pub delete_message: bool,
    /// Tetikleyen mesajdan bu adla alt başlık açılır.
    pub thread_name: Option<String>,
    /// Gönderilen mesajdaki rol etiketleri bildirim göndersin.
    pub ping_roles: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationSettings {
    pub automations: Vec<Automation>,
}

impl PluginConfig for AutomationSettings {
    const KEY: &'static str = "automations";
}

impl AutomationSettings {
    pub fn next_id(&self) -> u32 {
        self.automations.iter().map(|a| a.id).max().unwrap_or(0) + 1
    }
}

/// Bir olayın koşullarla karşılaştırılan özeti.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts<'a> {
    pub trigger: Trigger,
    pub channel: Option<u64>,
    pub content: &'a str,
    /// Rol tetikleyicilerinde verilen/alınan rol.
    pub role: Option<u64>,
    /// `reactionroles::emoji_key` biçiminde.
    pub emoji: Option<&'a str>,
    pub member_roles: &'a [RoleId],
}

/// Büyük/küçük harf ve Türkçe noktalı-noktasız i farkını yok sayan karşılaştırma biçimi:
/// "NASIL", "nasıl" ve "nasil" aynı sayılır (İngilizce klavyeyle yazanlar da eşleşsin).
pub(crate) fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| *c != '\u{0307}')
        .map(|c| if c == 'ı' { 'i' } else { c })
        .collect()
}

/// Otomasyon bu olayda çalışmalı mı?
pub fn matches(automation: &Automation, facts: &Facts<'_>) -> bool {
    if !automation.enabled || automation.trigger != facts.trigger {
        return false;
    }
    if automation.channel.is_some() && automation.channel != facts.channel {
        return false;
    }
    let role_ok = match (automation.role, facts.trigger) {
        (None, _) => true,
        (Some(role), Trigger::RoleAdded | Trigger::RoleRemoved) => facts.role == Some(role),
        (Some(role), _) => facts.member_roles.iter().any(|r| r.get() == role),
    };
    if !role_ok {
        return false;
    }
    match facts.trigger {
        Trigger::Message => automation
            .keyword
            .as_deref()
            .is_none_or(|keyword| fold(facts.content).contains(&fold(keyword))),
        Trigger::Reaction => {
            let wanted = automation.emoji.as_deref().and_then(crate::reactionroles::emoji_key);
            automation.emoji.is_none() || (wanted.is_some() && wanted.as_deref() == facts.emoji)
        }
        _ => true,
    }
}

/// Şablon: `{kullanıcı}` (etiket), `{isim}`, `{sunucu}`, `{kanal}`, `{rol}`, `{mesaj}`.
pub fn render(template: &str, user: &User, guild: &str, channel: Option<u64>, role: Option<u64>, content: &str) -> String {
    let name = user.global_name.clone().unwrap_or_else(|| user.name.clone());
    let quoted: String = content.chars().take(300).collect();
    template
        .replace("{kullanıcı}", &user.id.mention().to_string())
        .replace("{kullanici}", &user.id.mention().to_string())
        .replace("{isim}", &name)
        .replace("{sunucu}", guild)
        .replace("{kanal}", &channel.map(|c| format!("<#{c}>")).unwrap_or_default())
        .replace("{rol}", &role.map(|r| format!("<@&{r}>")).unwrap_or_default())
        .replace("{mesaj}", &quoted)
}

static RECENT: LazyLock<DashMap<(GuildId, UserId), (Instant, u8)>> = LazyLock::new(DashMap::new);

/// Üyenin bu pencerede çalıştırma hakkı varsa bir hak düşer ve `true` döner.
fn claim_run(guild_id: GuildId, user: UserId) -> bool {
    if RECENT.len() > 5_000 {
        RECENT.retain(|_, (at, _)| at.elapsed() < BURST_WINDOW);
    }
    let mut entry = RECENT.entry((guild_id, user)).or_insert((Instant::now(), 0));
    if entry.0.elapsed() >= BURST_WINDOW {
        *entry = (Instant::now(), 0);
    }
    if entry.1 >= BURST_LIMIT {
        return false;
    }
    entry.1 += 1;
    true
}

/// Bir olay: koşullar için `facts`, eylemler için ilgili nesneler.
pub struct Event<'a> {
    pub guild_id: GuildId,
    pub user: &'a User,
    pub facts: Facts<'a>,
    /// Tetikleyen mesaj (mesaj tetikleyicisinde); tepki ekleme, silme ve alt başlık bunu gerektirir.
    pub message: Option<&'a Message>,
}

/// Olaya uyan otomasyonları çalıştırır.
pub async fn run(ctx: &serenity::Context, data: &Data, event: Event<'_>) {
    run_with_target(ctx, data, event, None).await;
}

/// `target`: kuralda kanal seçilmemişse mesajın gideceği kanal (alt başlıkta: alt başlığın kendisi).
pub async fn run_with_target(ctx: &serenity::Context, data: &Data, event: Event<'_>, target: Option<ChannelId>) {
    if event.user.bot {
        return;
    }
    let settings: Arc<AutomationSettings> = match data.settings.get(&data.db, event.guild_id).await {
        Ok(settings) => settings,
        Err(e) => {
            warn!(guild = %event.guild_id, error = ?e, "Otomasyonlar okunamadı");
            return;
        }
    };
    for automation in settings.automations.iter().filter(|a| matches(a, &event.facts)) {
        if !claim_run(event.guild_id, event.user.id) {
            debug!(guild = %event.guild_id, user = %event.user.id, "Otomasyon sınırı aşıldı, atlandı");
            return;
        }
        execute(ctx, &event, automation, target).await;
    }
}

async fn execute(ctx: &serenity::Context, event: &Event<'_>, automation: &Automation, target: Option<ChannelId>) {
    let guild = ctx.cache.guild(event.guild_id).map(|g| g.name.clone()).unwrap_or_default();
    let fill = |template: &str| render(template, event.user, &guild, event.facts.channel, event.facts.role, event.facts.content);
    let failed = |what: &str, e: serenity::Error| {
        debug!(guild = %event.guild_id, automation = automation.id, error = %e, "Otomasyon eylemi başarısız: {what}");
    };

    if let Some(template) = automation.message.as_deref().filter(|m| !m.trim().is_empty()) {
        if let Some(channel) = automation.message_channel.or(target.map(ChannelId::get)).or(event.facts.channel).filter(|c| *c != 0) {
            // Üye her zaman etiketlenebilir; rol etiketleri yalnızca kural izin veriyorsa bildirim gönderir.
            let mentions = CreateAllowedMentions::new().users(vec![event.user.id]).all_roles(automation.ping_roles);
            let message = CreateMessage::new().content(fill(template)).allowed_mentions(mentions);
            if let Err(e) = ChannelId::new(channel).send_message(&ctx.http, message).await {
                failed("mesaj", e);
            }
        }
    }
    if let Some(template) = automation.dm.as_deref().filter(|m| !m.trim().is_empty()) {
        if let Err(e) = event.user.direct_message(ctx, CreateMessage::new().content(fill(template))).await {
            failed("özel mesaj", e);
        }
    }
    for (list, add) in [(&automation.add_roles, true), (&automation.remove_roles, false)] {
        for role_id in list.iter().filter(|id| **id != 0).map(|id| RoleId::new(*id)) {
            let role = ctx.cache.guild(event.guild_id).and_then(|g| g.roles.get(&role_id).cloned());
            let Some(role) = role else { continue };
            if rolepanel::check_role(ctx, event.guild_id, &role).is_err() {
                continue;
            }
            let result = if add {
                ctx.http.add_member_role(event.guild_id, event.user.id, role_id, Some("Otomasyon")).await
            } else {
                ctx.http.remove_member_role(event.guild_id, event.user.id, role_id, Some("Otomasyon")).await
            };
            if let Err(e) = result {
                failed("rol", e);
            }
        }
    }
    let Some(message) = event.message else { return };
    if let Some(emoji) = automation.react.as_deref().and_then(|raw| ReactionType::try_from(raw.trim()).ok()) {
        if let Err(e) = message.react(&ctx.http, emoji).await {
            failed("tepki", e);
        }
    }
    if let Some(name) = automation.thread_name.as_deref().filter(|n| !n.trim().is_empty()) {
        let name: String = fill(name).chars().take(100).collect();
        if let Err(e) = message.channel_id.create_thread_from_message(&ctx.http, message.id, CreateThread::new(name)).await {
            failed("alt başlık", e);
        }
    }
    if automation.delete_message {
        if let Err(e) = message.delete(&ctx.http).await {
            failed("mesaj silme", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(trigger: Trigger) -> Automation {
        Automation { id: 1, name: "t".to_owned(), enabled: true, trigger, ..Automation::default() }
    }

    #[test]
    fn message_conditions() {
        let roles = [RoleId::new(5)];
        let facts = Facts { trigger: Trigger::Message, channel: Some(10), content: "Sunucuya NASIL katılırım?", member_roles: &roles, ..Facts::default() };
        assert!(matches(&rule(Trigger::Message), &facts));
        assert!(matches(&Automation { keyword: Some("nasıl".to_owned()), ..rule(Trigger::Message) }, &facts));
        assert!(!matches(&Automation { keyword: Some("fiyat".to_owned()), ..rule(Trigger::Message) }, &facts));
        // Türkçe i/ı ve İ/I: hangi klavyeyle yazılırsa yazılsın eşleşir.
        for keyword in ["NASIL", "nasil", "Nasıl", "KATILIRIM", "katilirim"] {
            assert!(matches(&Automation { keyword: Some(keyword.to_owned()), ..rule(Trigger::Message) }, &facts), "{keyword}");
        }
        assert_eq!(fold("İSTANBUL ISPARTA"), "istanbul isparta");
        assert!(!matches(&Automation { channel: Some(11), ..rule(Trigger::Message) }, &facts));
        assert!(matches(&Automation { role: Some(5), ..rule(Trigger::Message) }, &facts));
        assert!(!matches(&Automation { role: Some(6), ..rule(Trigger::Message) }, &facts));
        assert!(!matches(&Automation { enabled: false, ..rule(Trigger::Message) }, &facts));
        assert!(!matches(&rule(Trigger::MemberJoin), &facts));
    }

    #[test]
    fn role_and_reaction_conditions() {
        let added = Facts { trigger: Trigger::RoleAdded, role: Some(7), ..Facts::default() };
        assert!(matches(&rule(Trigger::RoleAdded), &added));
        assert!(matches(&Automation { role: Some(7), ..rule(Trigger::RoleAdded) }, &added));
        assert!(!matches(&Automation { role: Some(8), ..rule(Trigger::RoleAdded) }, &added));

        let key = crate::reactionroles::emoji_key("⭐");
        let reaction = Facts { trigger: Trigger::Reaction, emoji: key.as_deref(), ..Facts::default() };
        assert!(matches(&rule(Trigger::Reaction), &reaction));
        assert!(matches(&Automation { emoji: Some("⭐\u{FE0F}".to_owned()), ..rule(Trigger::Reaction) }, &reaction));
        assert!(!matches(&Automation { emoji: Some("🎮".to_owned()), ..rule(Trigger::Reaction) }, &reaction));
    }

    #[test]
    fn burst_limit_stops_loops() {
        let (guild, user) = (GuildId::new(424_242), UserId::new(1));
        let allowed = (0..20).filter(|_| claim_run(guild, user)).count();
        assert_eq!(allowed, usize::from(BURST_LIMIT));
        // Başka bir üye etkilenmez.
        assert!(claim_run(guild, UserId::new(2)));
    }

    #[test]
    fn renders_and_loads_old_records() -> anyhow::Result<()> {
        let mut user = User::default();
        user.name = "ali".to_owned();
        let text = render("{isim} {kanal} {rol} {mesaj}", &user, "S", Some(3), Some(4), "merhaba");
        assert_eq!(text, "ali <#3> <@&4> merhaba");
        let settings: AutomationSettings = serde_json::from_str("{\"automations\":[{\"id\":2,\"trigger\":\"member_join\"}]}")?;
        assert_eq!(settings.automations[0].trigger, Trigger::MemberJoin);
        assert_eq!(settings.next_id(), 3);
        for (trigger, code, _) in Trigger::ALL {
            assert_eq!(Trigger::from_code(code), Some(trigger));
        }
        Ok(())
    }
}
