//! `/bilet` — destek bileti sisteminin kurulumu ve yetkili işlemleri (üstlen, üye ekle/çıkar).
//!
//! Üst komut herkese görünür (Discord izinleri alt komut bazında verilemiyor); yetki kontrolü
//! her alt komutta çalışma anında yapılır.

use anyhow::anyhow;
use poise::serenity_prelude::{self as serenity, Mentionable};
use poise::CreateReply;

use super::moderation::{discord_error, reply_private};
use crate::data::Context;
use crate::error::Error;
use crate::tickets::{self, ClaimOutcome, OpenTicket, TicketSettings};

/// Destek bileti sistemi.
#[poise::command(
    slash_command,
    guild_only,
    rename = "bilet",
    subcommands("kur", "ustlen", "ekle", "cikar", "kapat", "yeniden_ac", "sil"),
    subcommand_required,
    category = "Destek"
)]
pub async fn ticket(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Bilet panelini bir kanala gönderir ve ayarları kaydeder (tekrar çalıştırmak ayarları günceller).
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    required_bot_permissions = "MANAGE_CHANNELS | MANAGE_ROLES | SEND_MESSAGES | EMBED_LINKS"
)]
async fn kur(
    ctx: Context<'_>,
    #[description = "Panelin gönderileceği kanal"]
    #[channel_types("Text")]
    kanal: serenity::GuildChannel,
    #[description = "Biletlerin açılacağı kategori (opsiyonel)"]
    #[channel_types("Category")]
    kategori: Option<serenity::GuildChannel>,
    #[rename = "destek_rolü"]
    #[description = "Biletleri görebilecek destek ekibi rolü (opsiyonel)"]
    destek_rolu: Option<serenity::Role>,
) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow!("guild_only komut sunucu dışında çalıştı"))?;
    let data = ctx.data();

    let settings = TicketSettings {
        category_id: kategori.as_ref().map(|c| c.id),
        support_role_id: destek_rolu.as_ref().map(|r| r.id),
    };
    tickets::save_settings(&data.db, guild_id, &settings).await?;

    let extras = data.settings.get::<tickets::TicketExtras>(&data.db, guild_id).await?;
    if let Err(e) = kanal.id.send_message(ctx.http(), tickets::panel_message(&extras)).await {
        ctx.send(
            CreateReply::default()
                .content(format!(
                    "❌ Ayarlar kaydedildi ama panel {} kanalına gönderilemedi: {e}",
                    kanal.id.mention()
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let mut summary = format!("✅ Bilet paneli {} kanalına gönderildi.", kanal.id.mention());
    match &kategori {
        Some(c) => summary.push_str(&format!("\n• Kategori: **{}**", c.name)),
        None => summary.push_str("\n• Kategori: _yok (biletler kategorisiz açılır)_"),
    }
    match &destek_rolu {
        Some(r) => summary.push_str(&format!("\n• Destek rolü: {}", r.id.mention())),
        None => summary.push_str("\n• Destek rolü: _yok (sadece yöneticiler görür)_"),
    }

    ctx.send(CreateReply::default().content(summary).ephemeral(true))
        .await?;
    Ok(())
}

/// Bu bileti üstlenirsin (destek ekibi).
#[poise::command(slash_command, guild_only, rename = "üstlen")]
async fn ustlen(ctx: Context<'_>) -> Result<(), Error> {
    if !staff_check(ctx).await? {
        return Ok(());
    }
    let text = match tickets::claim(&ctx.data().db, ctx.channel_id(), ctx.author().id).await? {
        ClaimOutcome::Claimed(number) => {
            ctx.say(format!("🙋 Bilet #{number:04} {} tarafından üstlenildi.", ctx.author().id.mention()))
                .await?;
            return Ok(());
        }
        ClaimOutcome::AlreadyClaimed(other) => format!("Bu bileti zaten {} üstlendi.", other.mention()),
        ClaimOutcome::NotATicket => "Bu komutu açık bir bilet kanalında kullanmalısın.".to_owned(),
    };
    reply_private(ctx, text).await
}

/// Bilete bir üye ekler (destek ekibi).
#[poise::command(slash_command, guild_only, rename = "ekle", required_bot_permissions = "MANAGE_ROLES")]
async fn ekle(
    ctx: Context<'_>,
    #[rename = "üye"]
    #[description = "Bilete eklenecek üye"]
    member: serenity::Member,
) -> Result<(), Error> {
    if !staff_check(ctx).await? {
        return Ok(());
    }
    let Some(ticket) = current_ticket(ctx).await? else {
        return Ok(());
    };
    if member.user.bot {
        return reply_private(ctx, "Botlar bilete eklenemez.").await;
    }
    if let Err(e) = tickets::add_member(ctx.http(), ctx.channel_id(), member.user.id).await {
        return reply_private(ctx, format!("❌ Üye eklenemedi: {}", explain(&e))).await;
    }
    ctx.say(format!("➕ {} bilet #{:04}'e eklendi.", member.user.id.mention(), ticket.number))
        .await?;
    Ok(())
}

/// Bir üyeyi biletten çıkarır (destek ekibi).
#[poise::command(slash_command, guild_only, rename = "çıkar", required_bot_permissions = "MANAGE_ROLES")]
async fn cikar(
    ctx: Context<'_>,
    #[rename = "üye"]
    #[description = "Biletten çıkarılacak üye"]
    member: serenity::Member,
) -> Result<(), Error> {
    if !staff_check(ctx).await? {
        return Ok(());
    }
    let Some(ticket) = current_ticket(ctx).await? else {
        return Ok(());
    };
    if ticket.opener == Some(member.user.id) {
        return reply_private(ctx, "Bileti açan kişi çıkarılamaz; gerekirse bileti kapat.").await;
    }
    if let Err(e) = tickets::remove_member(ctx.http(), ctx.channel_id(), member.user.id).await {
        return reply_private(ctx, format!("❌ Üye çıkarılamadı: {}", explain(&e))).await;
    }
    ctx.say(format!("➖ {} bilet #{:04}'ten çıkarıldı.", member.user.id.mention(), ticket.number))
        .await?;
    Ok(())
}

fn explain(e: &anyhow::Error) -> String {
    e.downcast_ref::<serenity::Error>()
        .map_or_else(|| e.to_string(), discord_error)
}

/// Yetkili değilse kullanıcıya söyler ve `false` döner.
async fn staff_check(ctx: Context<'_>) -> Result<bool, Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow!("guild_only komut sunucu dışında çalıştı"))?;
    let staff = match ctx.author_member().await {
        Some(member) => tickets::is_staff(&ctx.data().db, guild_id, &member).await?,
        None => false,
    };
    if !staff {
        reply_private(ctx, "Bu komutu sadece destek ekibi kullanabilir.").await?;
    }
    Ok(staff)
}

/// Komutun kullanıldığı kanal açık bir bilet değilse kullanıcıya söyler.
async fn current_ticket(ctx: Context<'_>) -> Result<Option<OpenTicket>, Error> {
    let ticket = tickets::open_ticket_in(&ctx.data().db, ctx.channel_id()).await?;
    if ticket.is_none() {
        reply_private(ctx, "Bu komutu açık bir bilet kanalında kullanmalısın.").await?;
    }
    Ok(ticket)
}

/// Bulunduğun destek biletini kapatır.
#[poise::command(slash_command, guild_only)]
async fn kapat(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or_else(|| anyhow!("guild_only komut sunucu dışında çalıştı"))?;
    let data = ctx.data();
    let member = ctx.author_member().await;
    let closing = match tickets::begin_close(&data.db, guild_id, ctx.channel_id(), ctx.author().id, member.as_deref()).await? {
        Ok(closing) => closing,
        Err(reason) => return reply_private(ctx, reason).await,
    };
    let extras = data.settings.get::<tickets::TicketExtras>(&data.db, guild_id).await?;
    ctx.say(tickets::closing_notice(ctx.author().id, extras.keep_closed)).await?;
    tickets::finish_close(ctx.serenity_context(), data, guild_id, ctx.channel_id(), closing, ctx.author(), &extras).await;
    Ok(())
}

/// Kapatılıp saklanmış bileti yeniden açar (destek ekibi).
#[poise::command(slash_command, guild_only, rename = "yeniden-aç")]
async fn yeniden_ac(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or_else(|| anyhow!("guild_only komut sunucu dışında çalıştı"))?;
    let Some(member) = ctx.author_member().await else {
        return reply_private(ctx, "Üyelik bilgin alınamadı.").await;
    };
    match tickets::reopen_ticket(ctx.serenity_context(), ctx.data(), guild_id, ctx.channel_id(), &member).await? {
        Ok(text) => {
            ctx.say(text).await?;
            Ok(())
        }
        Err(reason) => reply_private(ctx, reason).await,
    }
}

/// Kapatılıp saklanmış bileti kalıcı olarak siler (destek ekibi).
#[poise::command(slash_command, guild_only)]
async fn sil(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or_else(|| anyhow!("guild_only komut sunucu dışında çalıştı"))?;
    let Some(member) = ctx.author_member().await else {
        return reply_private(ctx, "Üyelik bilgin alınamadı.").await;
    };
    // Kanal silineceği için cevap önce gönderilir.
    ctx.defer_ephemeral().await?;
    match tickets::delete_ticket(ctx.serenity_context(), ctx.data(), guild_id, ctx.channel_id(), &member).await? {
        Ok(()) => Ok(()),
        Err(reason) => reply_private(ctx, reason).await,
    }
}
