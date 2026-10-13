//! `/çekiliş` — butonlu çekilişler.

use poise::serenity_prelude::{self as serenity, Mentionable};

use super::moderation::{guild_id, reply_private};
use crate::data::Context;
use crate::error::Error;
use crate::giveaway::{self, NewGiveaway, MAX_DURATION, MAX_WINNERS};
use crate::moderation::duration;

/// Çekiliş başlatma ve yönetme.
#[poise::command(
    slash_command,
    guild_only,
    rename = "çekiliş",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("baslat", "bitir", "yeniden_cek"),
    subcommand_required,
    category = "Topluluk"
)]
pub async fn cekilis(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Yeni bir çekiliş başlatır.
#[poise::command(slash_command, guild_only, rename = "başlat", required_bot_permissions = "SEND_MESSAGES | EMBED_LINKS")]
async fn baslat(
    ctx: Context<'_>,
    #[rename = "süre"]
    #[description = "Ne kadar sürsün (örn. 1sa, 2g, 1hf; en fazla 30g)"]
    length: String,
    #[rename = "ödül"]
    #[description = "Ödül"]
    #[max_length = 200]
    prize: String,
    #[rename = "kazanan"]
    #[description = "Kazanan sayısı (varsayılan 1)"]
    #[min = 1]
    #[max = 20]
    winners: Option<u32>,
    #[description = "Kanal (boşsa bu kanal)"]
    #[channel_types("Text", "News")]
    kanal: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let length = match duration::parse(&length) {
        Ok(d) if d > MAX_DURATION => return reply_private(ctx, "Çekiliş en fazla 30 gün sürebilir.").await,
        Ok(d) if d.as_secs() < 10 => return reply_private(ctx, "Çekiliş en az 10 saniye sürmeli.").await,
        Ok(d) => d,
        Err(e) => return reply_private(ctx, format!("Geçersiz süre: {e}")).await,
    };
    let channel = kanal.map_or_else(|| ctx.channel_id(), |c| c.id);
    let data = ctx.data();

    let id = giveaway::create(
        ctx.http(),
        &data.db,
        &data.scheduler,
        NewGiveaway {
            guild_id,
            channel_id: channel,
            host: ctx.author().id,
            prize: prize.trim(),
            winners: winners.unwrap_or(1).clamp(1, MAX_WINNERS),
            duration: length,
        },
    )
    .await?;
    reply_private(
        ctx,
        format!("🎉 Çekiliş **#{id}** {} kanalında başladı ({}).", channel.mention(), duration::format(length)),
    )
    .await
}

/// Çekilişi hemen bitirir ve kazananları seçer.
#[poise::command(slash_command, guild_only, rename = "bitir")]
async fn bitir(
    ctx: Context<'_>,
    #[description = "Çekiliş numarası (mesajın altında yazar)"]
    #[min = 1]
    numara: i64,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    if !giveaway::is_active(&data.db, guild_id, numara).await? {
        return reply_private(ctx, format!("#{numara} numaralı aktif bir çekiliş yok.")).await;
    }
    giveaway::end(ctx.http(), &data.db, numara).await?;
    reply_private(ctx, format!("✅ Çekiliş #{numara} bitirildi.")).await
}

/// Bitmiş çekiliş için yeni kazanan seçer.
#[poise::command(slash_command, guild_only, rename = "yeniden-çek")]
async fn yeniden_cek(
    ctx: Context<'_>,
    #[description = "Çekiliş numarası"]
    #[min = 1]
    numara: i64,
    #[rename = "kazanan"]
    #[description = "Kaç yeni kazanan (varsayılan 1)"]
    #[min = 1]
    #[max = 20]
    count: Option<u32>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    match giveaway::reroll(ctx.http(), &ctx.data().db, guild_id, numara, count.unwrap_or(1)).await {
        Ok(winners) if winners.is_empty() => reply_private(ctx, "Seçilebilecek başka katılımcı yok.").await,
        Ok(_) => reply_private(ctx, "✅ Yeni kazanan duyuruldu.").await,
        Err(e) => reply_private(ctx, format!("❌ {e}")).await,
    }
}
