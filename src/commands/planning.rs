//! `/etkinlik` (etkinlik planlama) ve `/yedek` (sunucu yapısı yedeği).

use poise::serenity_prelude::{self as serenity, CreateEmbed, Mentionable};
use poise::CreateReply;

use super::moderation::reply_private;
use crate::backup;
use crate::data::Context;
use crate::error::Error;
use crate::rsvp::{self, EventSettings};

/// Etkinlik planla: üyeler butonla katılıp katılmayacağını bildirir.
#[poise::command(
    slash_command,
    guild_only,
    rename = "etkinlik",
    subcommands("etkinlik_olustur", "etkinlik_liste", "etkinlik_iptal"),
    subcommand_required,
    default_member_permissions = "MANAGE_EVENTS",
    required_permissions = "MANAGE_EVENTS",
    category = "Topluluk"
)]
pub async fn etkinlik(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Bu kanala bir etkinlik duyurusu gönderir.
#[poise::command(slash_command, guild_only, rename = "oluştur", required_bot_permissions = "SEND_MESSAGES | EMBED_LINKS")]
async fn etkinlik_olustur(
    ctx: Context<'_>,
    #[rename = "başlık"]
    #[description = "Etkinliğin adı"]
    #[max_length = 100]
    title: String,
    #[rename = "zaman"]
    #[description = "Türkiye saatiyle: 25.12.2026 21:30 (yıl yazmazsan en yakın tarih)"]
    #[max_length = 20]
    when: String,
    #[rename = "açıklama"]
    #[description = "Ayrıntılar (isteğe bağlı)"]
    #[max_length = 1000]
    description: Option<String>,
    #[rename = "discord_etkinliği"]
    #[description = "Discord'un Etkinlikler listesine de eklensin mi? (varsayılan: evet)"]
    discord_event: Option<bool>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let now = serenity::Timestamp::now().unix_timestamp();
    let Some(start) = rsvp::parse_time(&when, rsvp::turkey_today(now)) else {
        return reply_private(ctx, "Zamanı `GG.AA.YYYY SS:DD` biçiminde yaz (ör. `25.12.2026 21:30`).").await;
    };
    if !rsvp::valid_start(start, now) {
        return reply_private(ctx, "Etkinlik gelecekte ve en fazla bir yıl sonra olmalı.").await;
    }
    let data = ctx.data();
    // Başlık ve açıklama herkese gösterileceği için sunucunun kelime filtresinden geçer.
    let text = format!("{title} {}", description.as_deref().unwrap_or_default());
    if data.automod.has_profanity(&data.db, &data.redis, guild_id, &text).await {
        return reply_private(ctx, "Başlık ya da açıklama sunucunun yasaklı kelimelerini içeriyor.").await;
    }
    ctx.defer_ephemeral().await?;
    let discord_event = discord_event.unwrap_or(true);
    match rsvp::create(ctx.http(), &data.db, &data.settings, guild_id, ctx.channel_id(), ctx.author().id, title, description, start, discord_event).await {
        Ok((id, note)) => {
            let listed = match (&note, discord_event) {
                (Some(note), _) => format!("\n⚠️ {note}"),
                (None, true) => "\n📅 Sunucunun **Etkinlikler** listesine de eklendi.".to_owned(),
                (None, false) => String::new(),
            };
            reply_private(ctx, format!("✅ Etkinlik #{id} oluşturuldu (<t:{start}:F>). İptal için: `/etkinlik iptal numara:{id}`{listed}")).await
        }
        Err(problem) => reply_private(ctx, format!("❌ {problem}")).await,
    }
}

/// Yaklaşan etkinlikleri listeler.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn etkinlik_liste(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let data = ctx.data();
    let settings = data.settings.get::<EventSettings>(&data.db, guild_id).await?;
    if settings.events.is_empty() {
        return reply_private(ctx, "Planlanmış etkinlik yok. `/etkinlik oluştur` ile ekleyebilirsin.").await;
    }
    let mut events: Vec<_> = settings.events.iter().collect();
    events.sort_by_key(|e| e.start);
    let lines: Vec<String> = events
        .iter()
        .map(|e| format!("`#{}` **{}** — <t:{}:F> · ✅ {} · 🤔 {} · <#{}>", e.id, e.title, e.start, e.going.len(), e.maybe.len(), e.channel))
        .collect();
    reply_private(ctx, lines.join("\n").chars().take(1900).collect::<String>()).await
}

/// Bir etkinliği iptal eder.
#[poise::command(slash_command, guild_only, rename = "iptal")]
async fn etkinlik_iptal(ctx: Context<'_>, #[description = "Etkinlik numarası (/etkinlik liste)"] #[min = 1] numara: u32) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let data = ctx.data();
    let text = if rsvp::cancel(ctx.http(), &data.db, &data.settings, guild_id, numara).await? { "🚫 Etkinlik iptal edildi." } else { "Bu numarada bir etkinlik yok." };
    reply_private(ctx, text).await
}

/// Sunucunun rol ve kanal yapısını yedekle / geri yükle.
#[poise::command(
    slash_command,
    guild_only,
    rename = "yedek",
    subcommands("yedek_al", "yedek_liste", "yedek_yukle", "yedek_sil"),
    subcommand_required,
    default_member_permissions = "ADMINISTRATOR",
    required_permissions = "ADMINISTRATOR",
    category = "Sunucu"
)]
pub async fn yedek(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Rollerin ve kanalların şu anki yapısını kaydeder.
#[poise::command(slash_command, guild_only, rename = "al")]
async fn yedek_al(ctx: Context<'_>, #[description = "Yedeğin adı (isteğe bağlı)"] #[max_length = 50] ad: Option<String>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let db = &ctx.data().db;
    if backup::count(db, guild_id).await? >= backup::MAX_BACKUPS {
        return reply_private(ctx, format!("En fazla {} yedek tutulur; önce `/yedek sil` ile birini kaldır.", backup::MAX_BACKUPS)).await;
    }
    let Some(snapshot) = backup::take(&ctx.serenity_context().cache, guild_id) else {
        return reply_private(ctx, "Sunucu bilgisi okunamadı, biraz sonra tekrar dene.").await;
    };
    let name: String = ad.map(|a| a.trim().to_owned()).filter(|a| !a.is_empty()).unwrap_or_else(|| "Yedek".to_owned()).chars().take(backup::MAX_NAME).collect();
    let id = backup::save(db, guild_id, &name, &snapshot).await?;
    reply_private(
        ctx,
        format!(
            "💾 Yedek #{id} alındı: **{}** rol, **{}** kanal.\n_Mesajlar, üyeler ve kanala özel izinler yedeklenmez; botlara ait roller dahil değildir._",
            snapshot.roles.len(),
            snapshot.channels.len()
        ),
    )
    .await
}

/// Alınmış yedekleri listeler.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn yedek_liste(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let backups = backup::list(&ctx.data().db, guild_id).await?;
    if backups.is_empty() {
        return reply_private(ctx, "Henüz yedek yok. `/yedek al` ile alabilirsin.").await;
    }
    let lines: Vec<String> = backups.iter().map(|b| format!("`#{}` **{}** — <t:{}:f> · {} rol · {} kanal", b.id, b.name, b.created_unix, b.roles, b.channels)).collect();
    let embed = CreateEmbed::new().title("💾 Sunucu yedekleri").description(lines.join("\n")).colour(serenity::Colour::BLURPLE);
    ctx.send(CreateReply::default().embed(embed).ephemeral(true)).await?;
    Ok(())
}

/// Yedekte olup sunucuda bulunmayan rolleri ve kanalları yeniden oluşturur (hiçbir şeyi silmez).
#[poise::command(slash_command, guild_only, rename = "yükle", required_bot_permissions = "MANAGE_ROLES | MANAGE_CHANNELS", guild_cooldown = 60)]
async fn yedek_yukle(
    ctx: Context<'_>,
    #[description = "Yedek numarası (/yedek liste)"]
    #[min = 1]
    numara: i64,
    #[description = "Gerçekten oluşturmak için 'Evet' seç; boşsa yalnızca ne olacağını gösterir"] onay: Option<bool>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(snapshot) = backup::load(&ctx.data().db, guild_id, numara).await? else {
        return reply_private(ctx, "Bu numarada bir yedek yok.").await;
    };
    let cache = &ctx.serenity_context().cache;
    let Some(current) = backup::take(cache, guild_id) else {
        return reply_private(ctx, "Sunucu bilgisi okunamadı, biraz sonra tekrar dene.").await;
    };
    let (roles, channels) = backup::missing(&snapshot, &current);
    if roles.is_empty() && channels.is_empty() {
        return reply_private(ctx, "✅ Yedekteki bütün roller ve kanallar sunucuda zaten var; yapılacak bir şey yok.").await;
    }
    if onay != Some(true) {
        let preview = |names: Vec<&str>| {
            let shown: Vec<&str> = names.iter().copied().take(15).collect();
            shown.join(", ") + if names.len() > 15 { " …" } else { "" }
        };
        return reply_private(
            ctx,
            format!(
                "Bu yedek yüklenirse **{}** rol ve **{}** kanal oluşturulacak; var olan hiçbir şey silinmez ya da değişmez.\n\
                 **Roller:** {}\n**Kanallar:** {}\n\nOnaylıyorsan komutu `onay: Evet` ile yeniden çalıştır.",
                roles.len(),
                channels.len(),
                preview(roles.iter().map(|r| r.name.as_str()).collect()),
                preview(channels.iter().map(|c| c.name.as_str()).collect()),
            )
            .chars()
            .take(1900)
            .collect::<String>(),
        )
        .await;
    }
    ctx.defer_ephemeral().await?;
    // Bot kendinde olmayan izni veremez: rol izinleri botun izinleriyle sınırlanır (kilit `await`'ten önce bırakılır).
    let bot_permissions = {
        let bot = cache.current_user().id;
        cache.guild(guild_id).and_then(|g| g.members.get(&bot).map(|me| g.member_permissions(me))).unwrap_or_else(serenity::Permissions::empty)
    };
    let Some(done) = backup::restore(ctx.http(), cache, guild_id, &snapshot, bot_permissions).await else {
        return reply_private(ctx, "Sunucu bilgisi okunamadı, biraz sonra tekrar dene.").await;
    };
    tracing::info!(guild = %guild_id, user = %ctx.author().id, backup = numara, roles = done.roles, channels = done.channels, failed = done.failed, "Yedek geri yüklendi");
    let failed = if done.failed > 0 { format!(" {} öğe oluşturulamadı (botun izinlerini ve rol sırasını kontrol et).", done.failed) } else { String::new() };
    reply_private(ctx, format!("♻️ {} tarafından geri yüklendi: **{}** rol, **{}** kanal oluşturuldu.{failed}", ctx.author().id.mention(), done.roles, done.channels)).await
}

/// Bir yedeği siler.
#[poise::command(slash_command, guild_only, rename = "sil")]
async fn yedek_sil(ctx: Context<'_>, #[description = "Yedek numarası (/yedek liste)"] #[min = 1] numara: i64) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let text = if backup::delete(&ctx.data().db, guild_id, numara).await? { "🗑️ Yedek silindi." } else { "Bu numarada bir yedek yok." };
    reply_private(ctx, text).await
}
