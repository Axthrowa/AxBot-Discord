//! Profil komutları (`/kartım profil · rep · evlen · boşan`). Slash komut sınırı dolu olduğu için
//! `/kartım` üst komutunun alt komutlarıdır.

use std::time::Duration;

use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteractionCollector, CreateActionRow, CreateAllowedMentions, CreateAttachment, CreateButton, CreateEmbed,
    CreateInteractionResponse, CreateInteractionResponseMessage, Mentionable,
};
use poise::CreateReply;

use super::moderation::reply_private;
use crate::data::Context;
use crate::economy;
use crate::error::Error;
use crate::leveling::formula;
use crate::profile::{self, RepOutcome};

const PROPOSAL_TIMEOUT: Duration = Duration::from_secs(90);

/// Bir üyenin profilini gösterir: seviye kartı, bakiye, itibar, başarımlar ve eşi.
#[poise::command(slash_command, guild_only, rename = "profil", user_cooldown = 5)]
pub async fn profil(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Profili gösterilecek üye (boşsa sen)"]
    user: Option<serenity::User>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let user = user.unwrap_or_else(|| ctx.author().clone());
    if user.bot {
        return reply_private(ctx, "Botların profili olmaz.").await;
    }
    let data = ctx.data();
    ctx.defer().await?;

    let rank = data.leveling.rank(&data.db, &data.redis, guild_id, user.id).await?;
    let progress = formula::progress(rank.total_xp);
    let account = economy::account(&data.db, guild_id, user.id).await?;
    let rep = profile::reputation(&data.db, guild_id, user.id).await?;
    let spouse = profile::spouse(&data.db, guild_id, user.id).await?;
    let earned = crate::achievements::progress(data, guild_id, user.id).await.map(|(_, earned)| earned.len()).unwrap_or(0);
    let personal = crate::rankcard::get(&data.db, guild_id, user.id).await.unwrap_or_default();
    let accent = personal.accent.as_deref().and_then(crate::cards::parse_hex).unwrap_or([88, 101, 242]);
    let joined = ctx.guild().and_then(|g| g.members.get(&user.id).and_then(|m| m.joined_at)).map(|t| t.unix_timestamp());

    let name = user.global_name.clone().unwrap_or_else(|| user.name.clone());
    let mut embed = CreateEmbed::new()
        .title(format!("👤 {name}"))
        .colour(serenity::Colour::from_rgb(accent[0], accent[1], accent[2]))
        .field("Seviye", format!("**{}** · {} XP", progress.level, rank.total_xp), true)
        .field("Sıra", rank.position.map_or_else(|| "—".to_owned(), |p| format!("#{p}")), true)
        .field("İtibar", format!("⭐ {rep}"), true)
        .field("Cüzdan", economy::money(account.wallet), true)
        .field("Banka", economy::money(account.bank), true)
        .field("Başarım", format!("🏅 {earned}"), true)
        .field("Eşi", spouse.map_or_else(|| "—".to_owned(), |(other, since)| format!("💍 {} (<t:{since}:D>)", other.mention())), true);
    if let Some(joined) = joined {
        embed = embed.field("Sunucuya katılım", format!("<t:{joined}:D>"), true);
    }

    // Seviye kartı profilin görseli olur; çizilemezse profil yine gösterilir.
    let card = crate::cards::RankCard {
        name,
        level: progress.level,
        rank: rank.position,
        current: progress.current,
        needed: progress.needed,
        total: rank.total_xp,
        accent,
        font: personal.font.clone().unwrap_or_else(|| "noto".to_owned()),
    };
    let backdrop = async {
        match personal.background.as_deref() {
            Some(url) => crate::cards::fetch_image(url).await,
            None => None,
        }
    };
    let (avatar, backdrop) = tokio::join!(crate::members::avatar_image(&user), backdrop);
    let drawn = tokio::task::spawn_blocking(move || crate::cards::render_rank(&card, avatar.as_ref(), backdrop.as_ref())).await;
    let mut reply = CreateReply::default().allowed_mentions(CreateAllowedMentions::new());
    if let Ok(Ok(bytes)) = drawn {
        reply = reply.attachment(CreateAttachment::bytes(bytes, "profil.jpg"));
        embed = embed.image("attachment://profil.jpg");
    } else {
        embed = embed.thumbnail(user.face());
    }
    ctx.send(reply.embed(embed)).await?;
    Ok(())
}

/// Bir üyeye itibar puanı verir (günde bir kez).
#[poise::command(slash_command, guild_only, rename = "rep")]
pub async fn rep(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "İtibar vereceğin üye"]
    user: serenity::User,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    if user.id == ctx.author().id {
        return reply_private(ctx, "Kendine itibar veremezsin. 🙂").await;
    }
    if user.bot {
        return reply_private(ctx, "Botlara itibar verilmez.").await;
    }
    let data = ctx.data();
    match profile::give_rep(&data.db, &data.redis, guild_id, ctx.author().id, user.id).await? {
        RepOutcome::Given(total) => {
            let text = format!("⭐ {} → {} itibar verdi! (toplam **{total}**)", ctx.author().id.mention(), user.id.mention());
            ctx.send(CreateReply::default().content(text).allowed_mentions(CreateAllowedMentions::new().users([user.id]))).await?;
            Ok(())
        }
        RepOutcome::Wait(seconds) => reply_private(ctx, format!("⏳ Bugünkü itibar hakkını kullandın. **{}** sonra tekrar verebilirsin.", economy::format_wait(Duration::from_secs(seconds)))).await,
    }
}

/// Bir üyeye evlenme teklif eder; kabul ederse profillerinizde görünür.
#[poise::command(slash_command, guild_only, rename = "evlen", user_cooldown = 30)]
pub async fn evlen(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Teklif edeceğin üye"]
    user: serenity::User,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let author = ctx.author().id;
    if user.id == author || user.bot {
        return reply_private(ctx, "Kendinle ya da bir botla evlenemezsin.").await;
    }
    let db = &ctx.data().db;
    if profile::spouse(db, guild_id, author).await?.is_some() {
        return reply_private(ctx, "Zaten evlisin. Önce `/kartım boşan`.").await;
    }
    if profile::spouse(db, guild_id, user.id).await?.is_some() {
        return reply_private(ctx, "Bu üye zaten evli.").await;
    }
    let id = ctx.id();
    let buttons = |disabled: bool| {
        vec![CreateActionRow::Buttons(vec![
            CreateButton::new(format!("evlen:{id}:evet")).label("Kabul et").emoji('💍').style(ButtonStyle::Success).disabled(disabled),
            CreateButton::new(format!("evlen:{id}:hayir")).label("Reddet").style(ButtonStyle::Secondary).disabled(disabled),
        ])]
    };
    let ask = format!("💍 {}, {} sana evlenme teklif ediyor! Kabul ediyor musun?", user.id.mention(), author.mention());
    let reply = ctx.send(CreateReply::default().content(ask).components(buttons(false)).allowed_mentions(CreateAllowedMentions::new().users([user.id]))).await?;
    let message = reply.message().await?.id;

    let mut outcome = "⌛ Teklif cevapsız kaldı.".to_owned();
    let deadline = std::time::Instant::now() + PROPOSAL_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(press) = ComponentInteractionCollector::new(ctx.serenity_context()).message_id(message).timeout(left).await else {
            break;
        };
        // Yalnızca teklif edilen kişi cevaplayabilir.
        if press.user.id != user.id {
            let note = CreateInteractionResponseMessage::new().content("Bu teklif sana yapılmadı.").ephemeral(true);
            press.create_response(ctx, CreateInteractionResponse::Message(note)).await?;
            continue;
        }
        press.create_response(ctx, CreateInteractionResponse::Acknowledge).await?;
        outcome = if press.data.custom_id.ends_with(":evet") {
            // Bu arada biri başkasıyla evlenmiş olabilir; kayıt tek sorguda denetlenir.
            if profile::marry(db, guild_id, author, user.id).await? {
                format!("🎉 {} ve {} artık evli! Mutluluklar! 💍", author.mention(), user.id.mention())
            } else {
                "Bu arada ikinizden biri evlenmiş; teklif geçersiz.".to_owned()
            }
        } else {
            format!("💔 {} teklifi reddetti.", user.id.mention())
        };
        break;
    }
    reply.edit(ctx, CreateReply::default().content(outcome).components(buttons(true)).allowed_mentions(CreateAllowedMentions::new())).await?;
    Ok(())
}

/// Evliliğini bitirir.
#[poise::command(slash_command, guild_only, rename = "boşan")]
pub async fn bosan(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let text = if profile::divorce(&ctx.data().db, guild_id, ctx.author().id).await? { "💔 Evliliğin sona erdi." } else { "Evli değilsin." };
    reply_private(ctx, text).await
}
