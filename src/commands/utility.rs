//! Genel araçlar: `/hatırlat`, `/bilgi kullanıcı`, `/bilgi sunucu`, `/bilgi avatar`.

use poise::serenity_prelude::{self as serenity, CreateEmbed, CreateEmbedFooter, Mentionable, User};
use poise::CreateReply;

use super::moderation::reply_private;
use crate::data::Context;
use crate::error::Error;
use crate::moderation::duration;
use crate::scheduler::Task;

const MAX_REMINDERS_PER_USER: i64 = 25;
const MAX_REMINDER: std::time::Duration = std::time::Duration::from_secs(365 * 24 * 3600);

/// Belirli bir süre sonra sana hatırlatma yapar.
#[poise::command(slash_command, rename = "hatırlat", category = "Genel")]
pub async fn hatirlat(
    ctx: Context<'_>,
    #[rename = "süre"]
    #[description = "Ne zaman (örn. 10dk, 2sa, 1g)"]
    length: String,
    #[description = "Neyi hatırlatayım?"]
    #[max_length = 1000]
    mesaj: String,
) -> Result<(), Error> {
    let length = match duration::parse(&length) {
        Ok(d) if d > MAX_REMINDER => return reply_private(ctx, "En fazla 1 yıl sonrası için hatırlatma kurulabilir.").await,
        Ok(d) => d,
        Err(e) => return reply_private(ctx, format!("Geçersiz süre: {e}")).await,
    };
    let scheduler = &ctx.data().scheduler;
    if scheduler.pending_reminders(ctx.author().id).await? >= MAX_REMINDERS_PER_USER {
        return reply_private(ctx, format!("En fazla {MAX_REMINDERS_PER_USER} bekleyen hatırlatman olabilir.")).await;
    }

    let now = serenity::Timestamp::now().unix_timestamp();
    let at = now + i64::try_from(length.as_secs())?;
    scheduler
        .schedule(
            at,
            &Task::Reminder {
                user_id: ctx.author().id.get(),
                channel_id: ctx.channel_id().get(),
                text: mesaj,
                created_unix: now,
            },
        )
        .await?;
    reply_private(ctx, format!("⏰ Tamam, <t:{at}:R> (<t:{at}:f>) hatırlatacağım.")).await
}

/// Bir kullanıcının hesap ve sunucu bilgilerini gösterir.
#[poise::command(slash_command, guild_only, rename = "kullanıcı", category = "Genel")]
pub async fn kullanici_bilgi(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Kullanıcı (boşsa sen)"]
    user: Option<User>,
) -> Result<(), Error> {
    let user = user.unwrap_or_else(|| ctx.author().clone());
    let created = user.created_at().unix_timestamp();
    let member = match ctx.guild_id() {
        Some(g) => g.member(ctx, user.id).await.ok(),
        None => None,
    };

    let mut embed = CreateEmbed::new()
        .author(serenity::CreateEmbedAuthor::new(user.tag()).icon_url(user.face()))
        .thumbnail(user.face())
        .colour(serenity::Colour::BLURPLE)
        .field("Kullanıcı", user.id.mention().to_string(), true)
        .field("Bot mu?", if user.bot { "🤖 Evet" } else { "Hayır" }, true)
        .field("Hesap oluşturma", format!("<t:{created}:D> (<t:{created}:R>)"), false)
        .footer(CreateEmbedFooter::new(format!("ID: {}", user.id)));

    if let Some(member) = &member {
        if let Some(joined) = member.joined_at {
            let j = joined.unix_timestamp();
            embed = embed.field("Sunucuya katılma", format!("<t:{j}:D> (<t:{j}:R>)"), false);
        }
        if let Some(nick) = &member.nick {
            embed = embed.field("Takma ad", nick, true);
        }
        if !member.roles.is_empty() {
            let roles = member.roles.iter().map(|r| r.mention().to_string()).collect::<Vec<_>>().join(" ");
            embed = embed.field(format!("Roller ({})", member.roles.len()), roles.chars().take(1000).collect::<String>(), false);
        }
        if let Some(until) = member.communication_disabled_until {
            if until.unix_timestamp() > serenity::Timestamp::now().unix_timestamp() {
                embed = embed.field("🔇 Susturulmuş", format!("<t:{}:R> bitiyor", until.unix_timestamp()), true);
            }
        }
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Sunucu hakkında bilgi gösterir.
#[poise::command(slash_command, guild_only, rename = "sunucu", category = "Genel")]
pub async fn sunucu_bilgi(ctx: Context<'_>) -> Result<(), Error> {
    // Önbellekten tek seferde topla; await öncesinde kilit bırakılır.
    let info = ctx.guild().map(|g| {
        let text = g.channels.values().filter(|c| c.kind == serenity::ChannelType::Text).count();
        let voice = g.channels.values().filter(|c| c.kind == serenity::ChannelType::Voice).count();
        (
            g.name.clone(),
            g.id,
            g.icon_url(),
            g.owner_id,
            g.member_count,
            g.roles.len(),
            text,
            voice,
            g.premium_subscription_count.unwrap_or(0),
            u8::from(g.premium_tier),
        )
    });
    let Some((name, id, icon, owner, members, roles, text, voice, boosts, tier)) = info else {
        return reply_private(ctx, "Sunucu bilgisi henüz yüklenmedi.").await;
    };
    let created = id.created_at().unix_timestamp();

    let mut embed = CreateEmbed::new()
        .title(name)
        .colour(serenity::Colour::BLURPLE)
        .field("Sahip", owner.mention().to_string(), true)
        .field("Üye", members.to_string(), true)
        .field("Roller", roles.to_string(), true)
        .field("Kanallar", format!("💬 {text} · 🔊 {voice}"), true)
        .field("Boost", format!("Seviye {tier} · {boosts} boost"), true)
        .field("Oluşturma", format!("<t:{created}:D> (<t:{created}:R>)"), false)
        .footer(CreateEmbedFooter::new(format!("ID: {id}")));
    if let Some(icon) = icon {
        embed = embed.thumbnail(icon);
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Bir kullanıcının profil resmini büyük gösterir.
#[poise::command(slash_command, rename = "avatar", category = "Genel")]
pub async fn avatar(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Kullanıcı (boşsa sen)"]
    user: Option<User>,
) -> Result<(), Error> {
    let user = user.unwrap_or_else(|| ctx.author().clone());
    let url = user
        .avatar_url()
        .map(|u| u.replace("size=1024", "size=4096"))
        .unwrap_or_else(|| user.default_avatar_url());
    let embed = CreateEmbed::new()
        .title(format!("🖼️ {}", user.tag()))
        .url(&url)
        .image(&url)
        .colour(serenity::Colour::BLURPLE);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Bir rol hakkında bilgi gösterir.
#[poise::command(slash_command, guild_only, rename = "bilgi")]
pub async fn rol_bilgi(ctx: Context<'_>, #[description = "Bakılacak rol"] rol: serenity::Role) -> Result<(), Error> {
    // Üye sayısı önbellekten; büyük sunucularda önbellekte olmayan üyeler sayılmaz.
    let members = ctx.guild().map_or(0, |g| g.members.values().filter(|m| m.roles.contains(&rol.id)).count());
    let created = rol.id.created_at().unix_timestamp();
    let permissions = if rol.permissions.is_empty() {
        "Yok".to_owned()
    } else if rol.permissions.administrator() {
        "Yönetici (tüm izinler)".to_owned()
    } else {
        crate::error::permission_names(rol.permissions)
    };
    let yes_no = |on: bool| if on { "Evet" } else { "Hayır" };

    let embed = CreateEmbed::new()
        .title(format!("🏷️ {}", rol.name))
        .colour(rol.colour)
        .field("Etiket", rol.id.mention().to_string(), true)
        .field("Üye", members.to_string(), true)
        .field("Renk", format!("#{:06X}", rol.colour.0), true)
        .field("Sıra", rol.position.to_string(), true)
        .field("Ayrı gösterilir", yes_no(rol.hoist), true)
        .field("Etiketlenebilir", yes_no(rol.mentionable), true)
        .field("Bot/entegrasyon rolü", yes_no(rol.managed), true)
        .field("Oluşturma", format!("<t:{created}:D> (<t:{created}:R>)"), false)
        .field("İzinler", permissions.chars().take(1000).collect::<String>(), false)
        .footer(CreateEmbedFooter::new(format!("ID: {}", rol.id)));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Kullanıcı, sunucu ve avatar bilgisi.
#[poise::command(slash_command, rename = "bilgi", subcommands("kullanici_bilgi", "sunucu_bilgi", "avatar", "gizlilik", "verilerimi_sil", "sunucu_verisini_sil"), subcommand_required, category = "Genel")]
pub async fn bilgi(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

// ---------------------------------------------------------------- Gizlilik

/// Botun hangi verileri tuttuğunu ve nasıl silineceğini gösterir.
#[poise::command(slash_command, rename = "gizlilik")]
pub async fn gizlilik(ctx: Context<'_>) -> Result<(), Error> {
    let link = ctx.data().public_url.public().map(|base| format!("\n\n📄 Tam metin: {}/gizlilik · English: {}/privacy", base.trim_end_matches('/'), base.trim_end_matches('/')));
    let text = format!(
        "**Tutulanlar:** Discord kimlikleri, XP/seviye, ekonomi, başarımlar, doğum günü (verirsen), çalma listelerin, moderasyon ve bilet kayıtları, sunucu ayarları.\n\
         **Mesajlar saklanmaz.** İstisna: silinen mesaj kaydı açık sunucularda kopya en fazla 6 saat, yapay zekâ sohbetinde son mesajlar 1 saat tutulur. \
         Yapay zekâya yazdığın mesaj cevap için yapay zekâ sağlayıcısına gönderilir.\n\
         **Silme:** `/bilgi verilerimi-sil` kendi verini, sunucu sahibi `/bilgi sunucu-verisini-sil` sunucunun verisini siler. \
         Bot sunucudan çıkarılıp {} gün içinde geri eklenmezse sunucu verisi kendiliğinden silinir.{}",
        crate::privacy::PURGE_AFTER_DAYS,
        link.unwrap_or_default()
    );
    let embed = CreateEmbed::new().title("🔒 Gizlilik").description(text).colour(serenity::Colour::DARK_GREEN);
    ctx.send(CreateReply::default().embed(embed).ephemeral(true)).await?;
    Ok(())
}

/// Bütün sunuculardaki kendi verini (XP, ekonomi, profil, doğum günü…) kalıcı olarak siler.
#[poise::command(slash_command, rename = "verilerimi-sil")]
pub async fn verilerimi_sil(
    ctx: Context<'_>,
    #[description = "Geri alınamaz; silmek için Evet seç"] onay: Option<bool>,
) -> Result<(), Error> {
    if onay != Some(true) {
        return reply_private(
            ctx,
            "Bu komut bütün sunuculardaki **XP ve seviyeni, ekonomi hesabını, hisselerini, başarımlarını, prestijini, itibarını, \
             rütbe kartını, doğum gününü, çalma listelerini, haber tercihlerini ve vurgu kelimelerini** kalıcı olarak siler.\n\
             Moderasyon kayıtları, biletler, itirazlar, başvurular ve öneriler sunucuların güvenliği için silinmez.\n\n\
             Emin isen: `/bilgi verilerimi-sil onay:True`",
        )
        .await;
    }
    ctx.defer_ephemeral().await?;
    let guilds = ctx.cache().guilds();
    let rows = crate::privacy::delete_user(&ctx.data().db, &ctx.data().redis, &guilds, ctx.author().id).await?;
    reply_private(ctx, format!("🗑️ Verin silindi ({rows} kayıt). Botu kullanmaya devam edersen yeni kayıtlar baştan oluşur.")).await
}

/// (Sunucu sahibi) Bu sunucunun bottaki bütün verisini siler ve bot sunucudan çıkar.
#[poise::command(slash_command, guild_only, rename = "sunucu-verisini-sil")]
pub async fn sunucu_verisini_sil(
    ctx: Context<'_>,
    #[description = "Geri alınamaz; silmek için Evet seç"] onay: Option<bool>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let owner = ctx.guild().map(|g| g.owner_id);
    if owner != Some(ctx.author().id) {
        return reply_private(ctx, "❌ Bunu yalnızca sunucunun sahibi yapabilir.").await;
    }
    if onay != Some(true) {
        return reply_private(
            ctx,
            "Bu komut sunucunun bottaki **bütün** verisini kalıcı olarak siler: ayarlar, XP, ekonomi, moderasyon kayıtları, biletler, \
             yedekler… Ardından bot sunucudan çıkar. Geri alınamaz.\n\nEmin isen: `/bilgi sunucu-verisini-sil onay:True`",
        )
        .await;
    }
    ctx.defer_ephemeral().await?;
    let data = ctx.data();
    let rows = crate::privacy::delete_guild(&data.db, &data.redis, &data.settings, guild_id).await?;
    reply_private(ctx, format!("🗑️ Sunucu verisi silindi ({rows} kayıt). Bot şimdi sunucudan çıkıyor.")).await?;
    guild_id.leave(ctx.http()).await?;
    Ok(())
}
