//! Panel destekli durumlu (stateful) rol menüleri:
//! Üyeler bir mesaja emoji tepkisi vererek, butona basarak ya da açılır menüden
//! seçerek rol alır. Menüler panelden oluşturulur; ayarlar `PluginSettings` deposunda durur
//! (tepkiler sık geldiği için bellekten okunur, veritabanına gidilmez).
//!
//! Güvenlik: rol her verilişte yeniden denetlenir (`super::security::check_role`).

use std::sync::Arc;

use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow,
    CreateButton, CreateEmbed, CreateMessage, EditInteractionResponse,
    CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, GuildId, Http, Mentionable, MessageId, Reaction,
    ReactionType, RoleId, UserId,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::security::check_role;
use crate::data::Data;
use crate::settings::PluginConfig;

/// Buton ve açılır menü kimliklerinin öneki.
pub const PREFIX: &str = "rrm:";
pub const SELECT_ID: &str = "rrm:select";
pub const MAX_MENUS: usize = 20;
pub const MAX_PAIRS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Reactions,
    Buttons,
    Dropdown,
}

impl Mode {
    pub const ALL: [(Self, &'static str, &'static str); 3] = [
        (Self::Reactions, "reactions", "Emoji tepkileri"),
        (Self::Buttons, "buttons", "Butonlar"),
        (Self::Dropdown, "dropdown", "Açılır menü"),
    ];

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, c, _)| *c == code).map(|(m, _, _)| *m)
    }

    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|(m, _, _)| *m == self).map_or("", |(_, _, label)| label)
    }
}

/// Menünün rolü nasıl verip aldığı (tepki rolü modları).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Behavior {
    /// Seçince verilir, bırakınca alınır.
    #[default]
    Normal,
    /// Yalnızca verilir; geri bırakılamaz (kural onayı, doğrulama).
    Verify,
    /// Tersi: tepki verince rol alınır, tepki geri alınınca verilir (yalnızca emoji tepkileri).
    Reverse,
    /// Bağlayıcı: menüden yalnızca bir kez, tek rol seçilir; sonra değiştirilemez ve bırakılamaz.
    Binding,
    /// Geçici: verilen rol `temp_minutes` dakika sonra kendiliğinden alınır.
    Temporary,
}

impl Behavior {
    pub const ALL: [(Self, &'static str, &'static str); 5] = [
        (Self::Normal, "normal", "Normal (seç / bırak)"),
        (Self::Verify, "verify", "Doğrulama (yalnızca verilir)"),
        (Self::Reverse, "reverse", "Ters (tepki rolü alır; yalnızca emoji tepkileri)"),
        (Self::Binding, "binding", "Bağlayıcı (bir kez seçilir, değişmez)"),
        (Self::Temporary, "temporary", "Geçici (süre sonunda alınır)"),
    ];

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, c, _)| *c == code).map(|(b, _, _)| *b)
    }

    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|(b, _, _)| *b == self).map_or("", |(_, _, label)| label)
    }
}

/// Bir rol için yapılacak iş.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Give,
    Take,
    /// Yapılmaz; üyeye gösterilecek sebep.
    Refuse(&'static str),
    Nothing,
}

/// `wants`: üye rolü istiyor mu (tepki ekledi / seçti / sahip değilken butona bastı). `has`: rol üyede mi.
/// `has_other`: üye bu menünün başka bir rolünü taşıyor mu.
pub fn decide(behavior: Behavior, wants: bool, has: bool, has_other: bool) -> Act {
    match (behavior, wants, has) {
        (Behavior::Reverse, true, true) => Act::Take,
        (Behavior::Reverse, false, false) => Act::Give,
        (Behavior::Reverse, ..) => Act::Nothing,
        (Behavior::Verify, false, true) => Act::Refuse("Bu rol geri bırakılamaz."),
        (Behavior::Binding, false, true) => Act::Refuse("Bu menüdeki seçim kalıcıdır; rol bırakılamaz."),
        (Behavior::Binding, true, false) if has_other => Act::Refuse("Bu menüden zaten bir rol seçtin; seçim değiştirilemez."),
        (_, true, false) => Act::Give,
        (_, false, true) => Act::Take,
        _ => Act::Nothing,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pair {
    /// Unicode emoji ya da `<:ad:kimlik>`; tepki modunda zorunlu.
    pub emoji: Option<String>,
    pub role: u64,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Menu {
    pub message_id: u64,
    pub channel_id: u64,
    pub mode: Mode,
    pub title: String,
    pub description: Option<String>,
    /// Üye bu menüden aynı anda yalnızca bir rol taşıyabilir (renk rolleri gibi).
    pub unique: bool,
    pub pairs: Vec<Pair>,
    #[serde(default)]
    pub behavior: Behavior,
    /// Geçici modda rolün kalacağı süre (dakika).
    #[serde(default)]
    pub temp_minutes: u32,
}

impl Menu {
    fn holds_other(&self, roles: &[RoleId], role: RoleId) -> bool {
        self.pairs.iter().map(|p| RoleId::new(p.role.max(1))).any(|r| r != role && roles.contains(&r))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReactionRoleSettings {
    pub menus: Vec<Menu>,
}

impl PluginConfig for ReactionRoleSettings {
    const KEY: &'static str = "reaction_roles";
}

/// Emojiyi karşılaştırılabilir anahtara çevirir: sunucu emojisinde kimlik, Unicode'da görünüm
/// seçicisi (U+FE0F) atılmış metin. Geçersizse `None`.
pub fn emoji_key(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('<').and_then(|r| r.strip_suffix('>')) {
        let id = inner.rsplit(':').next()?;
        let named = inner.trim_start_matches('a').starts_with(':') && inner.matches(':').count() == 2;
        return (named && id.len() >= 15 && id.chars().all(|c| c.is_ascii_digit())).then(|| id.to_owned());
    }
    let cleaned: String = raw.chars().filter(|c| *c != '\u{FE0F}').collect();
    let looks_like_emoji = !cleaned.is_empty()
        && cleaned.chars().count() <= 8
        && cleaned.chars().all(|c| !c.is_alphanumeric() || c.is_ascii_digit())
        && !cleaned.is_ascii();
    looks_like_emoji.then_some(cleaned)
}

fn reaction_key(emoji: &ReactionType) -> Option<String> {
    match emoji {
        ReactionType::Unicode(text) => Some(text.chars().filter(|c| *c != '\u{FE0F}').collect()),
        ReactionType::Custom { id, .. } => Some(id.to_string()),
        _ => None,
    }
}

fn reaction_type(raw: &str) -> Option<ReactionType> {
    ReactionType::try_from(raw.trim()).ok()
}

/// Menü mesajını kurar (tepkiler mesaj gönderildikten sonra eklenir).
pub fn build_message(menu: &Menu) -> CreateMessage {
    let list = menu
        .pairs
        .iter()
        .map(|p| format!("{} {}", p.emoji.as_deref().unwrap_or("•"), RoleId::new(p.role.max(1)).mention()))
        .collect::<Vec<_>>()
        .join("\n");
    let how = match menu.mode {
        Mode::Reactions => "_Rolü almak için emojisine tıkla; bırakmak için tepkini geri al._",
        Mode::Buttons => "_Butona basarak rolü al; tekrar basarak bırak._",
        Mode::Dropdown => "_Aşağıdaki menüden istediğin rolleri seç._",
    };
    let intro = menu.description.as_deref().map(|d| format!("{d}\n\n")).unwrap_or_default();
    let embed = CreateEmbed::new()
        .title(&menu.title)
        .description(format!("{intro}{list}\n\n{how}"))
        .colour(serenity::Colour::BLURPLE);
    let message = CreateMessage::new().embed(embed);

    match menu.mode {
        Mode::Reactions => message,
        Mode::Buttons => {
            let buttons: Vec<CreateButton> = menu
                .pairs
                .iter()
                .map(|p| {
                    let mut button = CreateButton::new(format!("{PREFIX}{}", p.role))
                        .label(p.label.chars().take(80).collect::<String>())
                        .style(ButtonStyle::Secondary);
                    if let Some(emoji) = p.emoji.as_deref().and_then(reaction_type) {
                        button = button.emoji(emoji);
                    }
                    button
                })
                .collect();
            message.components(buttons.chunks(5).map(|row| CreateActionRow::Buttons(row.to_vec())).collect())
        }
        Mode::Dropdown => {
            let options: Vec<CreateSelectMenuOption> = menu
                .pairs
                .iter()
                .map(|p| {
                    let mut option = CreateSelectMenuOption::new(p.label.chars().take(100).collect::<String>(), p.role.to_string());
                    if let Some(emoji) = p.emoji.as_deref().and_then(reaction_type) {
                        option = option.emoji(emoji);
                    }
                    option
                })
                .collect();
            let max = if menu.unique { 1 } else { u8::try_from(menu.pairs.len()).unwrap_or(25).clamp(1, 25) };
            let select = CreateSelectMenu::new(SELECT_ID, CreateSelectMenuKind::String { options })
                .placeholder("Rol seç")
                .min_values(0)
                .max_values(max);
            message.components(vec![CreateActionRow::SelectMenu(select)])
        }
    }
}

/// Menüyü kanala gönderir ve mesaj kimliğini menüye yazar. Tepki modunda emojiler arka planda eklenir.
pub async fn publish(http: &Arc<Http>, menu: &mut Menu) -> anyhow::Result<()> {
    let channel = ChannelId::new(menu.channel_id.max(1));
    let sent = channel.send_message(http, build_message(menu)).await?;
    menu.message_id = sent.id.get();
    if menu.mode == Mode::Reactions {
        let emojis: Vec<ReactionType> = menu.pairs.iter().filter_map(|p| p.emoji.as_deref().and_then(reaction_type)).collect();
        let http = Arc::clone(http);
        tokio::spawn(async move {
            for emoji in emojis {
                if let Err(e) = http.create_reaction(channel, sent.id, &emoji).await {
                    warn!(channel = %channel, error = %e, "Rol menüsüne tepki eklenemedi");
                }
            }
        });
    }
    Ok(())
}

async fn settings(data: &Data, guild_id: GuildId) -> Option<Arc<ReactionRoleSettings>> {
    match data.settings.get::<ReactionRoleSettings>(&data.db, guild_id).await {
        Ok(settings) => Some(settings),
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Rol menüleri okunamadı");
            None
        }
    }
}

/// Rol hâlâ var ve hâlâ güvenle dağıtılabilir mi? (Önbellekten; ağ isteği yok.)
fn safe_role(ctx: &serenity::Context, guild_id: GuildId, role_id: RoleId) -> Result<(), String> {
    let role = ctx.cache.guild(guild_id).and_then(|g| g.roles.get(&role_id).cloned());
    match role {
        Some(role) => check_role(ctx, guild_id, &role),
        None => Err("Bu rol artık yok.".to_owned()),
    }
}

/// Rolü verir; menü geçiciyse süresi dolunca alınması için görev kurar, tek seçimliyse diğer rolleri alır.
async fn grant(ctx: &serenity::Context, data: &Data, guild_id: GuildId, user: UserId, menu: &Menu, role: RoleId, current: &[RoleId]) -> bool {
    if !give(ctx, guild_id, user, role).await {
        return false;
    }
    schedule_expiry(data, guild_id, user, menu, role).await;
    if menu.unique {
        clear_others(ctx, guild_id, user, menu, role, current).await;
    }
    true
}

/// Geçici menüde rolün alınacağı zamanı zamanlayıcıya yazar.
async fn schedule_expiry(data: &Data, guild_id: GuildId, user: UserId, menu: &Menu, role: RoleId) {
    if menu.behavior != Behavior::Temporary || menu.temp_minutes == 0 {
        return;
    }
    let at = serenity::Timestamp::now().unix_timestamp() + i64::from(menu.temp_minutes) * 60;
    let task = crate::scheduler::Task::TempRoleEnd { guild_id: guild_id.get(), user_id: user.get(), role_id: role.get() };
    if let Err(e) = data.scheduler.schedule(at, &task).await {
        warn!(guild = %guild_id, error = ?e, "Geçici rolün bitişi kaydedilemedi");
    }
}

async fn give(ctx: &serenity::Context, guild_id: GuildId, user: UserId, role: RoleId) -> bool {
    if let Err(reason) = safe_role(ctx, guild_id, role) {
        debug!(guild = %guild_id, role = %role, reason, "Rol menüsü: rol verilmedi");
        return false;
    }
    match ctx.http.add_member_role(guild_id, user, role, Some("Rol menüsü")).await {
        Ok(()) => true,
        Err(e) => {
            warn!(guild = %guild_id, role = %role, error = %e, "Rol menüsü rol veremedi");
            false
        }
    }
}

async fn take(ctx: &serenity::Context, guild_id: GuildId, user: UserId, role: RoleId) -> bool {
    if safe_role(ctx, guild_id, role).is_err() {
        return false;
    }
    match ctx.http.remove_member_role(guild_id, user, role, Some("Rol menüsü")).await {
        Ok(()) => true,
        Err(e) => {
            warn!(guild = %guild_id, role = %role, error = %e, "Rol menüsü rol alamadı");
            false
        }
    }
}

/// Tek seçimli menüde, üyenin taşıdığı diğer menü rollerini alır.
async fn clear_others(ctx: &serenity::Context, guild_id: GuildId, user: UserId, menu: &Menu, keep: RoleId, current: &[RoleId]) {
    for pair in &menu.pairs {
        let role = RoleId::new(pair.role.max(1));
        if role != keep && current.contains(&role) {
            take(ctx, guild_id, user, role).await;
        }
    }
}

/// Tepki eklendi (`added`) ya da geri alındı.
pub async fn on_reaction(ctx: &serenity::Context, data: &Data, reaction: &Reaction, added: bool) {
    let (Some(guild_id), Some(user)) = (reaction.guild_id, reaction.user_id) else { return };
    if user == ctx.cache.current_user().id {
        return;
    }
    let Some(settings) = settings(data, guild_id).await else { return };
    let Some(menu) = settings
        .menus
        .iter()
        .find(|m| m.message_id == reaction.message_id.get() && m.mode == Mode::Reactions)
    else {
        return;
    };
    let Some(key) = reaction_key(&reaction.emoji) else { return };
    let Some(pair) = menu.pairs.iter().find(|p| p.emoji.as_deref().and_then(emoji_key).as_deref() == Some(key.as_str())) else {
        return;
    };
    let role = RoleId::new(pair.role.max(1));
    if reaction.member.as_ref().is_some_and(|m| m.user.bot) {
        return;
    }
    let current: Vec<RoleId> = match &reaction.member {
        Some(member) => member.roles.clone(),
        None => ctx.cache.guild(guild_id).and_then(|g| g.members.get(&user).map(|m| m.roles.clone())).unwrap_or_default(),
    };
    let has = current.contains(&role);
    match decide(menu.behavior, added, has, menu.holds_other(&current, role)) {
        Act::Give => {
            grant(ctx, data, guild_id, user, menu, role, &current).await;
        }
        Act::Take => {
            take(ctx, guild_id, user, role).await;
        }
        Act::Refuse(_) if added => {
            let _ = ctx.http.delete_reaction(reaction.channel_id, reaction.message_id, user, &reaction.emoji).await;
        }
        Act::Refuse(_) | Act::Nothing => {}
    }
}

fn menu_for(settings: &ReactionRoleSettings, message: MessageId) -> Option<&Menu> {
    settings.menus.iter().find(|m| m.message_id == message.get())
}

/// Buton ve açılır menü etkileşimleri.
pub async fn handle_component(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    interaction.defer_ephemeral(ctx).await?;
    let text = respond(ctx, data, interaction).await;
    interaction.edit_response(ctx, EditInteractionResponse::new().content(text)).await?;
    Ok(())
}

async fn respond(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> String {
    let (Some(guild_id), Some(member)) = (interaction.guild_id, interaction.member.as_ref()) else {
        return "Bu menü sadece sunucuda çalışır.".to_owned();
    };
    let Some(settings) = settings(data, guild_id).await else {
        return "❌ Menü şu an okunamıyor; biraz sonra tekrar dene.".to_owned();
    };
    let Some(menu) = menu_for(&settings, interaction.message.id) else {
        return "Bu menü artık kullanılmıyor.".to_owned();
    };
    let user = member.user.id;
    let menu_role = |raw: &str| {
        raw.parse::<u64>()
            .ok()
            .filter(|id| menu.pairs.iter().any(|p| p.role == *id) && *id != 0)
            .map(RoleId::new)
    };

    match &interaction.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => {
            let wanted: Vec<RoleId> = values.iter().filter_map(|v| menu_role(v)).collect();
            let (mut given, mut taken, mut refused) = (Vec::new(), Vec::new(), None);
            for pair in &menu.pairs {
                let role = RoleId::new(pair.role.max(1));
                let has = member.roles.contains(&role);
                match decide(menu.behavior, wanted.contains(&role), has, menu.holds_other(&member.roles, role)) {
                    Act::Give => {
                        if give(ctx, guild_id, user, role).await {
                            given.push(role);
                        }
                    }
                    Act::Take => {
                        if take(ctx, guild_id, user, role).await {
                            taken.push(role.mention().to_string());
                        }
                    }
                    Act::Refuse(reason) => refused = Some(reason),
                    Act::Nothing => {}
                }
            }
            for role in &given {
                schedule_expiry(data, guild_id, user, menu, *role).await;
            }
            let given: Vec<String> = given.iter().map(|r| r.mention().to_string()).collect();
            let mut text = match (given.is_empty(), taken.is_empty()) {
                (true, true) => "Değişiklik yok.".to_owned(),
                (false, true) => format!("✅ Verildi: {}", given.join(", ")),
                (true, false) => format!("➖ Alındı: {}", taken.join(", ")),
                (false, false) => format!("✅ Verildi: {}\n➖ Alındı: {}", given.join(", "), taken.join(", ")),
            };
            if let Some(reason) = refused {
                text.push_str(&format!("\n⚠️ {reason}"));
            }
            text
        }
        _ => {
            let Some(role) = interaction.data.custom_id.strip_prefix(PREFIX).and_then(menu_role) else {
                return "Geçersiz buton.".to_owned();
            };
            if let Err(reason) = safe_role(ctx, guild_id, role) {
                return format!("❌ {reason}");
            }
            let has = member.roles.contains(&role);
            match decide(menu.behavior, !has, has, menu.holds_other(&member.roles, role)) {
                Act::Take => {
                    if take(ctx, guild_id, user, role).await {
                        format!("➖ {} rolü alındı.", role.mention())
                    } else {
                        "❌ Rol alınamadı; botun 'Rolleri Yönet' izni ve rol sıralaması kontrol edilmeli.".to_owned()
                    }
                }
                Act::Give => {
                    if !grant(ctx, data, guild_id, user, menu, role, &member.roles).await {
                        return "❌ Rol verilemedi; botun 'Rolleri Yönet' izni ve rol sıralaması kontrol edilmeli.".to_owned();
                    }
                    match (menu.behavior, menu.temp_minutes) {
                        (Behavior::Temporary, mins) if mins > 0 => format!("✅ {} rolü verildi; {mins} dakika sonra alınacak.", role.mention()),
                        _ => format!("✅ {} rolü verildi.", role.mention()),
                    }
                }
                Act::Refuse(reason) => format!("⚠️ {reason}"),
                Act::Nothing => "Değişiklik yok.".to_owned(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_keys() {
        assert_eq!(emoji_key("<:pepe:123456789012345678>"), Some("123456789012345678".to_owned()));
        assert_eq!(emoji_key("<a:dans:123456789012345678>"), Some("123456789012345678".to_owned()));
        assert_eq!(emoji_key("❤\u{FE0F}"), emoji_key("❤"));
    }

    #[test]
    fn behaviors_decide_what_happens() {
        assert_eq!(decide(Behavior::Normal, true, false, false), Act::Give);
        assert_eq!(decide(Behavior::Normal, false, true, false), Act::Take);
        assert_eq!(decide(Behavior::Verify, false, true, false), Act::Refuse("Bu rol geri bırakılamaz."));
        assert_eq!(decide(Behavior::Reverse, true, true, false), Act::Take);
        assert_eq!(decide(Behavior::Reverse, false, false, false), Act::Give);
        assert_eq!(decide(Behavior::Binding, true, false, true), Act::Refuse("Bu menüden zaten bir rol seçtin; seçim değiştirilemez."));
    }

    #[test]
    fn modes_round_trip_and_old_records_load() -> anyhow::Result<()> {
        let menu = Menu {
            message_id: 1,
            channel_id: 2,
            mode: Mode::Buttons,
            title: "Başlık".to_owned(),
            description: None,
            unique: false,
            pairs: vec![Pair { emoji: None, role: 10, label: "Üye".to_owned() }],
            behavior: Behavior::Normal,
            temp_minutes: 0,
        };
        let json = serde_json::to_string(&menu)?;
        let back: Menu = serde_json::from_str(&json)?;
        assert_eq!(menu, back);
        Ok(())
    }
}
