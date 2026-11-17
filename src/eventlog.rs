//! Sunucu olay kayıtları (denetim kaydı): silinen/düzenlenen mesajlar, üye giriş/çıkış,
//! takma ad ve rol değişiklikleri, ses kanalı hareketleri, yasaklar; rol, kanal, sunucu ve emoji
//! değişiklikleri.
//!
//! Kayıt kanalı ayarlı değilse ya da olay panelden kapatılmışsa hiçbir embed oluşturulmadan çıkılır.
//! Hangi olayların yazılacağı `ModSettings::log` içindedir (bellekte önbellekli).

use std::sync::Arc;

use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateEmbed, CreateEmbedFooter, CreateMessage, GuildChannel, GuildId,
    GuildMemberUpdateEvent, Member, Mentionable, Message, MessageId, MessageUpdateEvent, PartialGuild, Role,
    RoleId, User, VoiceState,
};
use tracing::warn;

use crate::automod::rules::ModSettings;
use crate::data::Data;
use crate::modlog::user_line;

const FIELD_MAX: usize = 1000;

/// Bir olayın yazılacağı yer ve geçerli kayıt ayarları.
struct Target {
    channel: ChannelId,
    settings: Arc<ModSettings>,
}

impl Target {
    /// Profil resmi (ayar izin veriyorsa) eklenmiş embed.
    fn avatar(&self, embed: CreateEmbed, user: &User) -> CreateEmbed {
        if self.settings.log.hide_avatars {
            embed
        } else {
            embed.thumbnail(user.face())
        }
    }

    fn skips_bot(&self, user: &User) -> bool {
        user.bot && self.settings.log.skip_bots
    }
}

/// Kayıt kanalı ayarlıysa ve `event` açıksa hedefi döner (çağıran yoksa hemen çıkar).
async fn target(data: &Data, guild_id: GuildId, event: &str) -> Option<Target> {
    let channel = match data.guild_configs.get(&data.db, guild_id).await {
        Ok(cfg) => cfg.event_log_channel?,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Sunucu ayarları okunamadı");
            return None;
        }
    };
    let settings = match data.settings.get::<ModSettings>(&data.db, guild_id).await {
        Ok(settings) => settings,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Kayıt ayarları okunamadı, varsayılan kullanılıyor");
            Arc::new(ModSettings::default())
        }
    };
    settings.log.logs(event).then_some(Target { channel, settings })
}

async fn send(ctx: &serenity::Context, channel: ChannelId, embed: CreateEmbed) {
    let embed = embed.timestamp(serenity::Timestamp::now());
    if let Err(e) = channel.send_message(&ctx.http, CreateMessage::new().embed(embed)).await {
        warn!(channel = %channel, error = %e, "Olay logu gönderilemedi");
    }
}

/// Baskın/giriş filtresi gibi uyarılar (olay seçiminden bağımsız, her zaman yazılır).
pub async fn alert(ctx: &serenity::Context, data: &Data, guild_id: GuildId, title: &str, description: String) {
    let channel = match data.guild_configs.get(&data.db, guild_id).await {
        Ok(cfg) => cfg.event_log_channel,
        Err(_) => None,
    };
    let Some(channel) = channel else { return };
    let embed = CreateEmbed::new()
        .title(title)
        .description(description)
        .colour(serenity::Colour::RED);
    send(ctx, channel, embed).await;
}

pub async fn message_deleted(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    channel_id: ChannelId,
    message_id: MessageId,
) {
    // Auto-mod silmeleri zaten moderasyon loguna düşüyor.
    if data.automod.take_deleted(message_id) {
        return;
    }
    let Some(log) = target(data, guild_id, "message_delete").await else { return };
    if log.settings.log.ignores(channel_id.get()) {
        return;
    }
    let cached: Option<Message> = ctx.cache.message(channel_id, message_id).map(|m| m.clone());
    // Önbellekte yoksa (bot yeniden başladıysa) Redis'teki kısa ömürlü kopyaya bakılır.
    let saved = match cached {
        Some(msg) => {
            if log.skips_bot(&msg.author) {
                return;
            }
            Some((user_line(&msg.author), msg.author.id.get(), msg.content, msg.attachments.into_iter().map(|a| a.filename).collect()))
        }
        None => recall(data, channel_id, message_id).await.map(|r| (format!("<@{}> ({})", r.author_id, r.author), r.author_id, r.content, r.files)),
    };

    let mut embed = CreateEmbed::new()
        .title("🗑️ Mesaj silindi")
        .colour(serenity::Colour::from_rgb(237, 66, 69))
        .field("Kanal", channel_id.mention().to_string(), true);

    match saved {
        Some((author, author_id, content, files)) => {
            embed = embed
                .field("Yazar", author, true)
                .field("İçerik", quote(&content), false)
                .footer(CreateEmbedFooter::new(format!("Kullanıcı ID: {author_id} • Mesaj ID: {message_id}")));
            if !files.is_empty() {
                embed = embed.field("Ekler", truncate(&files.join(", ")), false);
            }
        }
        None => {
            // Bot açılmadan önce ya da saklama süresinden önce gönderilmiş mesaj; içerik bilinmiyor.
            embed = embed
                .field("İçerik", "_Mesajın içeriği bilinmiyor (bot açılmadan ya da 6 saatten önce gönderilmiş)._", false)
                .footer(CreateEmbedFooter::new(format!("Mesaj ID: {message_id}")));
        }
    }
    send(ctx, log.channel, embed).await;
}

/// Silinen mesaj kaydı için Redis'te tutulan kopyanın ömrü.
const REMEMBER_SECS: u64 = 6 * 3600;

#[derive(serde::Serialize, serde::Deserialize)]
struct Remembered {
    author_id: u64,
    author: String,
    content: String,
    files: Vec<String>,
}

fn remembered_key(channel: ChannelId, message: MessageId) -> String {
    format!("silinen:{channel}:{message}")
}

/// Kayıt kanalı ve "mesaj silindi" olayı açıksa mesajın kopyası 6 saat Redis'te tutulur; bot yeniden başlasa
/// da silinen mesajın içeriği yazılabilir (bellek önbelleği yeniden başlatmada boşalır).
pub async fn remember(data: &Data, guild_id: GuildId, msg: &Message) {
    let Some(log) = target(data, guild_id, "message_delete").await else { return };
    if log.settings.log.ignores(msg.channel_id.get()) {
        return;
    }
    let copy = Remembered {
        author_id: msg.author.id.get(),
        author: msg.author.tag(),
        content: msg.content.chars().take(FIELD_MAX).collect(),
        files: msg.attachments.iter().map(|a| a.filename.clone()).collect(),
    };
    let Ok(json) = serde_json::to_string(&copy) else { return };
    let mut conn = data.redis.clone();
    let saved: redis::RedisResult<()> =
        redis::cmd("SET").arg(remembered_key(msg.channel_id, msg.id)).arg(json).arg("EX").arg(REMEMBER_SECS).query_async(&mut conn).await;
    if let Err(e) = saved {
        tracing::debug!(error = %e, "Mesaj kopyası Redis'e yazılamadı");
    }
}

/// Redis'teki kopyayı okur ve siler (bir kez yazılır).
async fn recall(data: &Data, channel: ChannelId, message: MessageId) -> Option<Remembered> {
    let mut conn = data.redis.clone();
    let raw: Option<String> = redis::cmd("GETDEL").arg(remembered_key(channel, message)).query_async(&mut conn).await.ok()?;
    serde_json::from_str(&raw?).ok()
}

pub async fn messages_bulk_deleted(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    channel_id: ChannelId,
    count: usize,
) {
    let Some(log) = target(data, guild_id, "message_delete").await else { return };
    if log.settings.log.ignores(channel_id.get()) {
        return;
    }
    let embed = CreateEmbed::new()
        .title("🧹 Toplu mesaj silme")
        .description(format!("{} kanalında **{count}** mesaj silindi.", channel_id.mention()))
        .colour(serenity::Colour::from_rgb(237, 66, 69));
    send(ctx, log.channel, embed).await;
}

pub async fn message_edited(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    old: Option<&Message>,
    event: &MessageUpdateEvent,
) {
    // İçerik değişmediyse (ör. sadece link önizlemesi eklendi) loglanmaz.
    let (Some(old), Some(new_content)) = (old, event.content.as_ref()) else { return };
    if &old.content == new_content {
        return;
    }
    let Some(log) = target(data, guild_id, "message_edit").await else { return };
    if log.skips_bot(&old.author) || log.settings.log.ignores(event.channel_id.get()) {
        return;
    }

    let link = format!("https://discord.com/channels/{guild_id}/{}/{}", event.channel_id, event.id);
    let embed = CreateEmbed::new()
        .title("✏️ Mesaj düzenlendi")
        .url(link)
        .colour(serenity::Colour::from_rgb(254, 231, 92))
        .field("Yazar", user_line(&old.author), true)
        .field("Kanal", event.channel_id.mention().to_string(), true)
        .field("Önce", quote(&old.content), false)
        .field("Sonra", quote(new_content), false)
        .footer(CreateEmbedFooter::new(format!("Kullanıcı ID: {}", old.author.id)));
    send(ctx, log.channel, embed).await;
}

/// Davet bağlantısı içeren (ve otomatik moderasyonun silmediği) mesaj.
pub async fn invite_posted(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message) {
    let Some(log) = target(data, guild_id, "invite_posted").await else { return };
    if log.skips_bot(&msg.author) || log.settings.log.ignores(msg.channel_id.get()) {
        return;
    }
    let embed = CreateEmbed::new()
        .title("🔗 Davet bağlantısı paylaşıldı")
        .url(msg.link())
        .colour(serenity::Colour::from_rgb(254, 231, 92))
        .field("Yazar", user_line(&msg.author), true)
        .field("Kanal", msg.channel_id.mention().to_string(), true)
        .field("İçerik", quote(&msg.content), false)
        .footer(CreateEmbedFooter::new(format!("Kullanıcı ID: {}", msg.author.id)));
    send(ctx, log.channel, embed).await;
}

pub async fn member_joined(
    ctx: &serenity::Context,
    data: &Data,
    member: &Member,
    account_age_days: i64,
    invite: Option<&crate::invites::UsedInvite>,
) {
    let Some(log) = target(data, member.guild_id, "member_join").await else { return };
    let created = member.user.created_at().unix_timestamp();
    let mut embed = CreateEmbed::new()
        .title("📥 Üye katıldı")
        .colour(serenity::Colour::from_rgb(87, 242, 135))
        .field("Üye", user_line(&member.user), false)
        .field("Hesap oluşturma", format!("<t:{created}:R>"), true)
        .footer(CreateEmbedFooter::new(format!("Kullanıcı ID: {}", member.user.id)));
    embed = log.avatar(embed, &member.user);
    if account_age_days < 7 {
        embed = embed.field("⚠️ Dikkat", format!("Yeni hesap ({account_age_days} gün)"), true);
    }
    let invited = match invite {
        Some(crate::invites::UsedInvite { code, inviter: Some(inviter) }) => format!("{} (`{code}`)", inviter.mention()),
        Some(crate::invites::UsedInvite { code, inviter: None }) => format!("`{code}`"),
        None => "Bilinmiyor".to_owned(),
    };
    embed = embed.field("Davet", invited, true);
    send(ctx, log.channel, embed).await;
}

pub async fn member_left(ctx: &serenity::Context, data: &Data, guild_id: GuildId, user: &User, member: Option<&Member>) {
    let Some(log) = target(data, guild_id, "member_leave").await else { return };
    let mut embed = CreateEmbed::new()
        .title("📤 Üye ayrıldı")
        .colour(serenity::Colour::from_rgb(237, 66, 69))
        .field("Üye", user_line(user), false)
        .footer(CreateEmbedFooter::new(format!("Kullanıcı ID: {}", user.id)));
    embed = log.avatar(embed, user);
    if let Some(member) = member {
        if let Some(joined) = member.joined_at {
            embed = embed.field("Katılma", format!("<t:{}:R>", joined.unix_timestamp()), true);
        }
        if !member.roles.is_empty() {
            let roles = member.roles.iter().map(|r| r.mention().to_string()).collect::<Vec<_>>().join(" ");
            embed = embed.field("Roller", truncate(&roles), false);
        }
    }
    send(ctx, log.channel, embed).await;
}

/// Takma ad ve rol değişiklikleri ayrı olaylardır; ikisi ayrı ayrı açılıp kapatılabilir.
pub async fn member_updated(ctx: &serenity::Context, data: &Data, old: Option<&Member>, event: &GuildMemberUpdateEvent) {
    let Some(old) = old else { return };
    let nick_changed = old.nick != event.nick;
    let added: Vec<_> = event.roles.iter().filter(|r| !old.roles.contains(r)).collect();
    let removed: Vec<_> = old.roles.iter().filter(|r| !event.roles.contains(r)).collect();
    let roles_changed = !added.is_empty() || !removed.is_empty();
    if !nick_changed && !roles_changed {
        return;
    }

    let footer = || CreateEmbedFooter::new(format!("Kullanıcı ID: {}", event.user.id));
    if nick_changed {
        if let Some(log) = target(data, event.guild_id, "nickname").await {
            if !log.skips_bot(&event.user) {
                let show = |n: &Option<String>| n.clone().unwrap_or_else(|| "_yok_".to_owned());
                let embed = CreateEmbed::new()
                    .title("👤 Takma ad değişti")
                    .colour(serenity::Colour::BLURPLE)
                    .field("Üye", user_line(&event.user), false)
                    .field("Takma ad", format!("{} → {}", show(&old.nick), show(&event.nick)), false)
                    .footer(footer());
                send(ctx, log.channel, embed).await;
            }
        }
    }
    if roles_changed {
        if let Some(log) = target(data, event.guild_id, "member_roles").await {
            if !log.skips_bot(&event.user) {
                let mut embed = CreateEmbed::new()
                    .title("👤 Üyenin rolleri değişti")
                    .colour(serenity::Colour::BLURPLE)
                    .field("Üye", user_line(&event.user), false)
                    .footer(footer());
                if !added.is_empty() {
                    let roles = added.iter().map(|r| r.mention().to_string()).collect::<Vec<_>>().join(" ");
                    embed = embed.field("➕ Eklenen roller", truncate(&roles), false);
                }
                if !removed.is_empty() {
                    let roles = removed.iter().map(|r| r.mention().to_string()).collect::<Vec<_>>().join(" ");
                    embed = embed.field("➖ Alınan roller", truncate(&roles), false);
                }
                send(ctx, log.channel, embed).await;
            }
        }
    }
}

pub async fn voice_changed(ctx: &serenity::Context, data: &Data, old: Option<&VoiceState>, new: &VoiceState) {
    let Some(guild_id) = new.guild_id else { return };
    let before = old.and_then(|o| o.channel_id);
    let after = new.channel_id;
    let description = match (before, after) {
        (None, Some(to)) => format!("{} → {} kanalına katıldı", new.user_id.mention(), to.mention()),
        (Some(from), None) => format!("{} → {} kanalından ayrıldı", new.user_id.mention(), from.mention()),
        (Some(from), Some(to)) if from != to => {
            format!("{} → {} kanalından {} kanalına geçti", new.user_id.mention(), from.mention(), to.mention())
        }
        // Aynı kanalda kalan üyede yalnızca yetkilinin sesi kapatması / sağırlaştırması yazılır.
        _ => {
            if let Some(old) = old {
                let mut changes = Vec::new();
                if old.mute != new.mute {
                    changes.push(if new.mute { "sesi yetkilice kapatıldı" } else { "sesi yetkilice açıldı" });
                }
                if old.deaf != new.deaf {
                    changes.push(if new.deaf { "yetkilice sağırlaştırıldı" } else { "sağırlaştırması kaldırıldı" });
                }
                if !changes.is_empty() {
                    let text = format!("{}: {}", new.user_id.mention(), changes.join(", "));
                    note(ctx, data, guild_id, "voice_mod", "🔇 Ses yetkisi", text, serenity::Colour::from_rgb(88, 101, 242)).await;
                }
            }
            return;
        }
    };
    let Some(log) = target(data, guild_id, "voice").await else { return };
    if new.member.as_ref().is_some_and(|m| log.skips_bot(&m.user)) {
        return;
    }
    let embed = CreateEmbed::new()
        .title("🔊 Ses kanalı")
        .description(description)
        .colour(serenity::Colour::from_rgb(88, 101, 242));
    send(ctx, log.channel, embed).await;
}

pub async fn ban_changed(ctx: &serenity::Context, data: &Data, guild_id: GuildId, user: &User, banned: bool) {
    let event = if banned { "member_ban" } else { "member_unban" };
    let Some(log) = target(data, guild_id, event).await else { return };
    let (title, colour) = if banned {
        ("🔨 Üye yasaklandı", serenity::Colour::DARK_RED)
    } else {
        ("🔓 Yasak kaldırıldı", serenity::Colour::DARK_GREEN)
    };
    let embed = CreateEmbed::new()
        .title(title)
        .colour(colour)
        .field("Kullanıcı", user_line(user), false)
        .footer(CreateEmbedFooter::new(format!("Kullanıcı ID: {}", user.id)));
    send(ctx, log.channel, embed).await;
}

// ---------------------------------------------------------------- Rol, kanal, sunucu, emoji

/// Basit "başlık + açıklama" kaydı.
async fn note(ctx: &serenity::Context, data: &Data, guild_id: GuildId, event: &str, title: &str, description: String, colour: serenity::Colour) {
    let Some(log) = target(data, guild_id, event).await else { return };
    let embed = CreateEmbed::new().title(title).description(truncate(&description)).colour(colour);
    send(ctx, log.channel, embed).await;
}

const GREEN: serenity::Colour = serenity::Colour::from_rgb(87, 242, 135);
const RED: serenity::Colour = serenity::Colour::from_rgb(237, 66, 69);

pub async fn role_created(ctx: &serenity::Context, data: &Data, role: &Role) {
    note(ctx, data, role.guild_id, "role_create", "🏷️ Rol oluşturuldu", format!("{} (`{}`)", role.mention(), role.name), GREEN).await;
}

pub async fn role_deleted(ctx: &serenity::Context, data: &Data, guild_id: GuildId, role_id: RoleId, role: Option<&Role>) {
    let name = role.map_or_else(|| format!("ID {role_id}"), |r| format!("`{}`", r.name));
    note(ctx, data, guild_id, "role_delete", "🏷️ Rol silindi", name, RED).await;
}

pub async fn role_updated(ctx: &serenity::Context, data: &Data, old: Option<&Role>, new: &Role) {
    // Eski hâli önbellekte yoksa neyin değiştiği bilinemez; boş kayıt yazılmaz.
    let Some(old) = old else { return };
    let changes = role_changes(old, new);
    if changes.is_empty() {
        return;
    }
    note(
        ctx,
        data,
        new.guild_id,
        "role_update",
        "🏷️ Rol güncellendi",
        format!("{}\n{}", new.mention(), changes.join("\n")),
        serenity::Colour::BLURPLE,
    )
    .await;
}

/// Sıra (position) değişiklikleri sayılmaz: bir rol taşınınca diğerlerinin hepsi "güncellenir".
fn role_changes(old: &Role, new: &Role) -> Vec<String> {
    let mut changes = Vec::new();
    if old.name != new.name {
        changes.push(format!("**Ad:** `{}` → `{}`", old.name, new.name));
    }
    if old.colour != new.colour {
        changes.push(format!("**Renk:** #{:06X} → #{:06X}", old.colour.0, new.colour.0));
    }
    if old.hoist != new.hoist {
        changes.push(format!("**Ayrı göster:** {}", on_off(new.hoist)));
    }
    if old.mentionable != new.mentionable {
        changes.push(format!("**Etiketlenebilir:** {}", on_off(new.mentionable)));
    }
    if old.permissions != new.permissions {
        let added = new.permissions - old.permissions;
        let removed = old.permissions - new.permissions;
        if !added.is_empty() {
            changes.push(format!("**➕ İzinler:** {}", crate::error::permission_names(added)));
        }
        if !removed.is_empty() {
            changes.push(format!("**➖ İzinler:** {}", crate::error::permission_names(removed)));
        }
    }
    changes
}

fn on_off(on: bool) -> &'static str {
    if on {
        "açık"
    } else {
        "kapalı"
    }
}

pub async fn channel_created(ctx: &serenity::Context, data: &Data, channel: &GuildChannel) {
    note(
        ctx,
        data,
        channel.guild_id,
        "channel_create",
        "📁 Kanal oluşturuldu",
        format!("{} (`{}`)", channel.mention(), channel.name),
        GREEN,
    )
    .await;
}

pub async fn channel_deleted(ctx: &serenity::Context, data: &Data, channel: &GuildChannel) {
    note(ctx, data, channel.guild_id, "channel_create", "📁 Kanal silindi", format!("`{}`", channel.name), RED).await;
}

pub async fn channel_updated(ctx: &serenity::Context, data: &Data, old: Option<&GuildChannel>, new: &GuildChannel) {
    let Some(old) = old else { return };
    let mut changes = Vec::new();
    if old.name != new.name {
        changes.push(format!("**Ad:** `{}` → `{}`", old.name, new.name));
    }
    if old.topic != new.topic {
        let show = |t: &Option<String>| t.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "_yok_".to_owned());
        changes.push(format!("**Konu:** {} → {}", show(&old.topic), show(&new.topic)));
    }
    if old.nsfw != new.nsfw {
        changes.push(format!("**Yaş sınırı:** {}", on_off(new.nsfw)));
    }
    if old.rate_limit_per_user != new.rate_limit_per_user {
        changes.push(format!("**Yavaş mod:** {} sn", new.rate_limit_per_user.unwrap_or(0)));
    }
    if old.parent_id != new.parent_id {
        changes.push("**Kategori** değişti".to_owned());
    }
    if old.permission_overwrites.len() != new.permission_overwrites.len() {
        changes.push("**Kanal izinleri** değişti".to_owned());
    }
    // Yalnızca sıra değişmişse (kanal sürüklenince hepsi tetiklenir) kayıt yazılmaz.
    if changes.is_empty() {
        return;
    }
    note(
        ctx,
        data,
        new.guild_id,
        "channel_update",
        "📁 Kanal güncellendi",
        format!("{}\n{}", new.mention(), changes.join("\n")),
        serenity::Colour::BLURPLE,
    )
    .await;
}

pub async fn guild_updated(ctx: &serenity::Context, data: &Data, old: Option<(String, Option<String>)>, new: &PartialGuild) {
    let Some((old_name, old_icon)) = old else { return };
    let mut changes = Vec::new();
    if old_name != new.name {
        changes.push(format!("**Ad:** `{old_name}` → `{}`", new.name));
    }
    if old_icon != new.icon_url() {
        changes.push("**Simge** değişti".to_owned());
    }
    if changes.is_empty() {
        return;
    }
    note(ctx, data, new.id, "guild_update", "🏠 Sunucu düzenlendi", changes.join("\n"), serenity::Colour::BLURPLE).await;
}

pub async fn emojis_updated(ctx: &serenity::Context, data: &Data, guild_id: GuildId, count: usize) {
    note(
        ctx,
        data,
        guild_id,
        "emoji_update",
        "😀 Emojiler güncellendi",
        format!("Sunucuda artık **{count}** emoji var."),
        serenity::Colour::BLURPLE,
    )
    .await;
}

pub async fn stickers_updated(ctx: &serenity::Context, data: &Data, guild_id: GuildId, count: usize) {
    let text = format!("Sunucuda artık **{count}** çıkartma var.");
    note(ctx, data, guild_id, "sticker_update", "🖼️ Çıkartmalar güncellendi", text, serenity::Colour::BLURPLE).await;
}

pub async fn webhooks_changed(ctx: &serenity::Context, data: &Data, guild_id: GuildId, channel: ChannelId) {
    let text = format!("{} kanalının webhook'ları değişti (eklendi, düzenlendi ya da silindi).", channel.mention());
    note(ctx, data, guild_id, "webhook_update", "🪝 Webhook değişti", text, serenity::Colour::ORANGE).await;
}

/// Davet oluşturuldu (`inviter` biliniyorsa) ya da silindi.
pub async fn invite_changed(ctx: &serenity::Context, data: &Data, guild_id: GuildId, code: &str, channel: ChannelId, inviter: Option<&User>, created: bool) {
    let Some(log) = target(data, guild_id, "invite_create").await else { return };
    if inviter.is_some_and(|u| log.skips_bot(u)) {
        return;
    }
    let mut text = format!("`{code}` → {}", channel.mention());
    if let Some(user) = inviter {
        text.push_str(&format!("\nOluşturan: {}", user_line(user)));
    }
    let (title, colour) = if created { ("✉️ Davet oluşturuldu", GREEN) } else { ("✉️ Davet silindi", RED) };
    send(ctx, log.channel, CreateEmbed::new().title(title).description(text).colour(colour)).await;
}

/// Alt başlık açıldı ya da silindi.
pub async fn thread_changed(ctx: &serenity::Context, data: &Data, guild_id: GuildId, name: Option<&str>, thread: ChannelId, parent: Option<ChannelId>, created: bool) {
    let mut text = match (created, name) {
        (true, _) => thread.mention().to_string(),
        (false, Some(name)) => format!("`{name}`"),
        (false, None) => format!("ID {thread}"),
    };
    if let Some(parent) = parent {
        text.push_str(&format!(" ({} içinde)", parent.mention()));
    }
    let (title, colour) = if created { ("🧵 Alt başlık açıldı", GREEN) } else { ("🧵 Alt başlık silindi", RED) };
    note(ctx, data, guild_id, "thread_create", title, text, colour).await;
}

pub async fn thread_updated(ctx: &serenity::Context, data: &Data, old: Option<&GuildChannel>, new: &GuildChannel) {
    let Some(old) = old else { return };
    let mut changes = Vec::new();
    if old.name != new.name {
        changes.push(format!("**Ad:** `{}` → `{}`", old.name, new.name));
    }
    let flags = |c: &GuildChannel| c.thread_metadata.map(|m| (m.archived, m.locked)).unwrap_or_default();
    let ((old_archived, old_locked), (archived, locked)) = (flags(old), flags(new));
    if old_archived != archived {
        changes.push(format!("**Arşiv:** {}", on_off(archived)));
    }
    if old_locked != locked {
        changes.push(format!("**Kilit:** {}", on_off(locked)));
    }
    if changes.is_empty() {
        return;
    }
    let text = format!("{}\n{}", new.mention(), changes.join("\n"));
    note(ctx, data, new.guild_id, "thread_update", "🧵 Alt başlık güncellendi", text, serenity::Colour::BLURPLE).await;
}

fn quote(content: &str) -> String {
    if content.is_empty() {
        return "_(boş — sadece ek veya embed)_".to_owned();
    }
    let safe = content.replace("```", "ʼʼʼ");
    format!("```\n{}\n```", truncate(&safe))
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= FIELD_MAX {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(FIELD_MAX - 1).collect();
    out.push('…');
    out
}
