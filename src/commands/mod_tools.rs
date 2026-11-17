//! Kanal ve geçmiş araçları: `/kanal temizle`, `/kanal yavaşmod`, `/kanal kilitle`, `/kanal kilidi-aç`, `/sicil`,
//! `/mod uyarı-cezası`.

use poise::serenity_prelude::{
    self as serenity, CreateEmbed, CreateEmbedFooter, EditChannel, GetMessages, Mentionable, MessageId,
    PermissionOverwrite, PermissionOverwriteType, Permissions, User,
};
use poise::CreateReply;

use super::moderation::{discord_error, guild_id, reply_private};
use crate::data::Context;
use crate::error::Error;
use crate::moderation::{self, duration, Punishment, PunishmentRule};

/// Discord toplu silmede 14 günden eski mesajları kabul etmez (1 dk emniyet payı).
const BULK_DELETE_MAX_AGE_SECS: i64 = 14 * 24 * 3600 - 60;

/// Kanaldaki son mesajları siler (filtrelenebilir; 14 günden eski mesajlar silinemez).
#[poise::command(
    slash_command,
    guild_only,
    rename = "temizle",
    default_member_permissions = "MANAGE_MESSAGES", required_permissions = "MANAGE_MESSAGES",
    required_bot_permissions = "MANAGE_MESSAGES | READ_MESSAGE_HISTORY",
    category = "Moderasyon"
)]
pub async fn temizle(
    ctx: Context<'_>,
    #[description = "Silinecek mesaj sayısı (1-100)"]
    #[min = 1]
    #[max = 100]
    miktar: u8,
    #[rename = "kullanıcı"]
    #[description = "Sadece bu kullanıcının mesajları"]
    user: Option<User>,
    #[rename = "içeren"]
    #[description = "Sadece bu metni içeren mesajlar"]
    contains: Option<String>,
    #[description = "Sadece botların mesajları"] botlar: Option<bool>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let channel = ctx.channel_id();
    let cutoff = serenity::Timestamp::now().unix_timestamp() - BULK_DELETE_MAX_AGE_SECS;
    let needle = contains.map(|c| c.to_lowercase());

    let messages = channel.messages(ctx, GetMessages::new().limit(100)).await?;
    let mut too_old = 0;
    let ids: Vec<MessageId> = messages
        .iter()
        .filter(|m| user.as_ref().is_none_or(|u| m.author.id == u.id))
        .filter(|m| !botlar.unwrap_or(false) || m.author.bot)
        .filter(|m| needle.as_ref().is_none_or(|n| m.content.to_lowercase().contains(n)))
        .filter(|m| {
            let fresh = m.timestamp.unix_timestamp() > cutoff;
            if !fresh {
                too_old += 1;
            }
            fresh
        })
        .take(usize::from(miktar))
        .map(|m| m.id)
        .collect();

    let result = match ids.len() {
        0 => return reply_private(ctx, "Filtreye uyan silinebilir mesaj bulunamadı.").await,
        1 => channel.delete_message(ctx, ids[0]).await,
        _ => channel.delete_messages(ctx, &ids).await,
    };
    if let Err(e) = result {
        return reply_private(ctx, format!("❌ Mesajlar silinemedi: {}", discord_error(&e))).await;
    }

    let mut text = format!("🧹 **{}** mesaj silindi.", ids.len());
    if too_old > 0 {
        text.push_str(" (14 günden eski mesajları Discord toplu silmeye izin vermiyor.)");
    }
    reply_private(ctx, text).await
}

/// Kanala yavaş mod koyar (0 = kapat).
#[poise::command(
    slash_command,
    guild_only,
    rename = "yavaşmod",
    default_member_permissions = "MANAGE_CHANNELS", required_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_CHANNELS",
    category = "Moderasyon"
)]
pub async fn yavasmod(
    ctx: Context<'_>,
    #[description = "Mesajlar arası bekleme (saniye, 0-21600; 0 = kapalı)"]
    #[min = 0]
    #[max = 21600]
    saniye: u16,
    #[description = "Kanal (boşsa bu kanal)"]
    #[channel_types("Text")]
    kanal: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let channel = kanal.map_or_else(|| ctx.channel_id(), |c| c.id);
    if let Err(e) = channel.edit(ctx, EditChannel::new().rate_limit_per_user(saniye)).await {
        return reply_private(ctx, format!("❌ {}", discord_error(&e))).await;
    }
    let text = if saniye == 0 {
        format!("🐇 {} kanalında yavaş mod kapatıldı.", channel.mention())
    } else {
        format!("🐢 {} kanalında yavaş mod: **{saniye} saniye**.", channel.mention())
    };
    ctx.say(text).await?;
    Ok(())
}

use crate::lockdown::send_permissions;

/// Kanalı kilitler: @everyone mesaj gönderemez (diğer izinler korunur).
#[poise::command(
    slash_command,
    guild_only,
    rename = "kilitle",
    default_member_permissions = "MANAGE_CHANNELS", required_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_CHANNELS | MANAGE_ROLES",
    category = "Moderasyon"
)]
pub async fn kilitle(
    ctx: Context<'_>,
    #[description = "Kanal (boşsa bu kanal)"]
    #[channel_types("Text")]
    kanal: Option<serenity::GuildChannel>,
    #[description = "Sebep"] sebep: Option<String>,
) -> Result<(), Error> {
    set_lock(ctx, kanal, true, sebep).await
}

/// Kilitli kanalı tekrar açar.
#[poise::command(
    slash_command,
    guild_only,
    rename = "kilidi-aç",
    default_member_permissions = "MANAGE_CHANNELS", required_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_CHANNELS | MANAGE_ROLES",
    category = "Moderasyon"
)]
pub async fn kilidi_ac(
    ctx: Context<'_>,
    #[description = "Kanal (boşsa bu kanal)"]
    #[channel_types("Text")]
    kanal: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    set_lock(ctx, kanal, false, None).await
}

async fn set_lock(ctx: Context<'_>, kanal: Option<serenity::GuildChannel>, lock: bool, reason: Option<String>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let channel = kanal.map_or_else(|| ctx.channel_id(), |c| c.id);
    let everyone = guild_id.everyone_role();
    let bot_id = ctx.serenity_context().cache.current_user().id;

    // Mevcut @everyone izinlerini koru, sadece gönderme izinlerini değiştir.
    let (allow, deny) = ctx
        .guild()
        .and_then(|g| {
            g.channels.get(&channel).and_then(|c| {
                c.permission_overwrites
                    .iter()
                    .find(|o| o.kind == PermissionOverwriteType::Role(everyone))
                    .map(|o| (o.allow, o.deny))
            })
        })
        .unwrap_or((Permissions::empty(), Permissions::empty()));

    let (allow, deny) = if lock {
        (allow - send_permissions(), deny | send_permissions())
    } else {
        (allow, deny - send_permissions())
    };
    let overwrite = PermissionOverwrite {
        allow,
        deny,
        kind: PermissionOverwriteType::Role(everyone),
    };
    if let Err(e) = channel.create_permission(ctx, overwrite).await {
        return reply_private(ctx, format!("❌ {}", discord_error(&e))).await;
    }
    if lock {
        // Bot kilitli kanalda da yazabilsin (kilit duyurusu, moderasyon).
        let bot_overwrite = PermissionOverwrite {
            allow: Permissions::SEND_MESSAGES | Permissions::VIEW_CHANNEL,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(bot_id),
        };
        let _ = channel.create_permission(ctx, bot_overwrite).await;
    }

    let text = if lock {
        let mut t = format!("🔒 {} kanalı kilitlendi.", channel.mention());
        if let Some(r) = reason.filter(|r| !r.trim().is_empty()) {
            t.push_str(&format!("\n> {r}"));
        }
        t
    } else {
        format!("🔓 {} kanalının kilidi açıldı.", channel.mention())
    };
    ctx.say(text).await?;
    Ok(())
}

/// Bir kullanıcının tüm moderasyon geçmişini gösterir.
#[poise::command(
    slash_command,
    context_menu_command = "Sicil",
    guild_only,
    rename = "sicil",
    default_member_permissions = "MODERATE_MEMBERS",
    category = "Moderasyon"
)]
pub async fn sicil(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Sicili gösterilecek kullanıcı"]
    user: User,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let (cases, total) = moderation::recent_cases(&ctx.data().db, guild_id, user.id, 15).await?;

    let description = if cases.is_empty() {
        "Temiz sicil. ✨".to_owned()
    } else {
        cases
            .iter()
            .map(|c| {
                let mut line = format!(
                    "**#{}** {} • <t:{}:R> • <@{}>",
                    c.case_number,
                    c.title(),
                    c.created_unix,
                    c.moderator_id
                );
                if let Some(secs) = c.duration_secs.and_then(|s| u64::try_from(s).ok()) {
                    line.push_str(&format!(" • {}", duration::format(std::time::Duration::from_secs(secs))));
                }
                if c.action == "warn" && !c.active {
                    line.push_str(" • ~~kaldırıldı~~");
                }
                if let Some(reason) = &c.reason {
                    line.push_str(&format!("\n> {}", reason.chars().take(150).collect::<String>()));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let embed = CreateEmbed::new()
        .title(format!("📁 {} — sicil ({total} kayıt)", user.tag()))
        .description(description)
        .thumbnail(user.face())
        .colour(serenity::Colour::DARK_GREY)
        .footer(CreateEmbedFooter::new(format!("Kullanıcı ID: {}{}", user.id, if total > 15 { " • son 15 kayıt" } else { "" })));
    ctx.send(CreateReply::default().embed(embed).ephemeral(true)).await?;
    Ok(())
}

/// Uyarı sayısına göre otomatik cezalar.
#[poise::command(
    slash_command,
    guild_only,
    rename = "uyarı-cezası",
    default_member_permissions = "MANAGE_GUILD", required_permissions = "MANAGE_GUILD",
    subcommands("ceza_ekle", "ceza_sil", "ceza_liste"),
    subcommand_required,
    category = "Moderasyon"
)]
pub async fn uyari_cezasi(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Belirli uyarı sayısına ulaşınca uygulanacak cezayı ayarlar.
#[poise::command(slash_command, guild_only, rename = "ekle")]
async fn ceza_ekle(
    ctx: Context<'_>,
    #[rename = "uyarı_sayısı"]
    #[description = "Kaçıncı uyarıda uygulansın"]
    #[min = 1]
    #[max = 50]
    count: u8,
    #[description = "Ceza"] ceza: Punishment,
    #[rename = "süre"]
    #[description = "Susturma süresi veya süreli ban (örn. 1sa, 1g); ban için boşsa kalıcı"]
    length: Option<String>,
    #[rename = "pencere"]
    #[description = "Sadece bu süre içindeki uyarılar sayılsın (örn. 1sa, 7g); boşsa tüm aktif uyarılar"]
    window: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let window = match window.as_deref().map(duration::parse) {
        None => None,
        Some(Ok(d)) => Some(d),
        Some(Err(e)) => return reply_private(ctx, format!("Geçersiz pencere süresi: {e}")).await,
    };
    let length = match length.as_deref().map(duration::parse) {
        None => None,
        Some(Ok(d)) if ceza == Punishment::Timeout && d > duration::MAX_TIMEOUT => {
            return reply_private(ctx, "Susturma en fazla 28 gün olabilir.").await;
        }
        Some(Ok(d)) => Some(d),
        Some(Err(e)) => return reply_private(ctx, format!("Geçersiz süre: {e}")).await,
    };
    if ceza == Punishment::Kick && length.is_some() {
        return reply_private(ctx, "Atma cezasının süresi olmaz.").await;
    }
    let length = if ceza == Punishment::Timeout { length.or(Some(std::time::Duration::from_secs(3600))) } else { length };

    let rule = PunishmentRule {
        warn_count: i32::from(count),
        punishment: ceza,
        duration: length,
        window,
    };
    moderation::set_punishment(&ctx.data().db, guild_id, &rule).await?;
    reply_private(ctx, format!("✅ **{count}. uyarıda**: {}", describe(&rule))).await
}

/// Bir uyarı eşiği cezasını kaldırır.
#[poise::command(slash_command, guild_only, rename = "sil")]
async fn ceza_sil(
    ctx: Context<'_>,
    #[rename = "uyarı_sayısı"]
    #[description = "Cezası kaldırılacak uyarı sayısı"]
    #[min = 1]
    #[max = 50]
    count: u8,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let text = if moderation::remove_punishment(&ctx.data().db, guild_id, i32::from(count)).await? {
        format!("✅ {count}. uyarı cezası kaldırıldı.")
    } else {
        format!("ℹ️ {count}. uyarı için ceza yok.")
    };
    reply_private(ctx, text).await
}

/// Tanımlı uyarı cezalarını listeler.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn ceza_liste(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let rules = moderation::punishments(&ctx.data().db, guild_id).await?;
    let text = if rules.is_empty() {
        "Henüz otomatik ceza yok. Örnek: `/mod uyarı-cezası ekle uyarı_sayısı:3 ceza:Susturma süre:1sa`".to_owned()
    } else {
        rules
            .iter()
            .map(|r| format!("**{}. uyarı** → {}", r.warn_count, describe(r)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    reply_private(ctx, text).await
}

fn describe(rule: &PunishmentRule) -> String {
    let what = moderation::describe_punishment(rule);
    match rule.window {
        Some(window) => format!("{what} (son {} içindeki uyarılar sayılır)", duration::format(window)),
        None => what,
    }
}

/// Bir üyenin ya da herkesin aktif uyarılarını temizler.
#[poise::command(
    slash_command,
    guild_only,
    rename = "uyarıları-sıfırla",
    default_member_permissions = "MANAGE_GUILD", required_permissions = "MANAGE_GUILD",
    category = "Moderasyon"
)]
pub async fn uyarilari_sifirla(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Uyarıları temizlenecek üye (boşsa sunucudaki herkes)"]
    user: Option<serenity::User>,
    #[rename = "herkes"]
    #[description = "Üye seçmediysen: sunucudaki TÜM uyarıları temizlemeyi onayla"]
    everyone: Option<bool>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    if user.is_none() && everyone != Some(true) {
        return reply_private(ctx, "Bir üye seç ya da sunucudaki tüm uyarıları temizlemek için `herkes: True` ver.").await;
    }
    let cleared = moderation::clear_warnings(&ctx.data().db, guild_id, user.as_ref().map(|u| u.id)).await?;
    let whose = user.map_or_else(|| "Sunucudaki".to_owned(), |u| format!("{} üyesinin", u.id.mention()));
    reply_private(ctx, format!("🧹 {whose} **{cleared}** aktif uyarısı temizlendi. (Kayıtlar sicilde durur.)")).await
}

/// Kanal araçları: toplu silme, yavaş mod, kilitleme.
#[poise::command(
    slash_command,
    guild_only,
    rename = "kanal",
    default_member_permissions = "MANAGE_MESSAGES",
    subcommands("temizle", "yavasmod", "kilitle", "kilidi_ac"),
    subcommand_required,
    category = "Moderasyon"
)]
pub async fn kanal(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}
