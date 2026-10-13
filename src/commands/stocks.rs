//! `/borsa`: sunucu parasıyla oynanan sahte borsa.

use poise::serenity_prelude::{self as serenity, CreateEmbed, CreateEmbedFooter};
use poise::CreateReply;

use super::moderation::reply_private;
use crate::data::Context;
use crate::economy::stocks::{self, Stock, Trade, STOCKS};
use crate::economy::{self, money};
use crate::error::Error;

/// Sahte borsa: hayali şirketlerin hisselerini al, sat.
#[poise::command(slash_command, guild_only, rename = "borsa", subcommands("fiyatlar", "al", "sat", "portfoy"), subcommand_required, category = "Ekonomi")]
pub async fn borsa(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

fn now() -> i64 {
    serenity::Timestamp::now().unix_timestamp()
}

fn arrow(change: f64) -> &'static str {
    if change > 0.05 {
        "🟢"
    } else if change < -0.05 {
        "🔴"
    } else {
        "⚪"
    }
}

fn pick(symbol: &str) -> Result<&'static Stock, String> {
    stocks::find(symbol).ok_or_else(|| format!("Böyle bir hisse yok. Semboller: {}", STOCKS.iter().map(|s| s.symbol).collect::<Vec<_>>().join(", ")))
}

/// Hisselerin şu anki fiyatlarını gösterir (fiyatlar saatte bir değişir).
#[poise::command(slash_command, guild_only, rename = "fiyatlar")]
async fn fiyatlar(ctx: Context<'_>) -> Result<(), Error> {
    let unix = now();
    let lines: Vec<String> = STOCKS
        .iter()
        .map(|stock| {
            let (price, change) = stocks::quote(stock, unix);
            format!("{} {} **{}** — {} · {} · {change:+.1}%", arrow(change), stock.emoji, stock.symbol, stock.name, money(price))
        })
        .collect();
    let next = (stocks::hour_of(unix) + 1) * 3600;
    let embed = CreateEmbed::new()
        .title("📈 Borsa")
        .description(format!("{}\n\nSonraki fiyat değişimi: <t:{next}:R>", lines.join("\n")))
        .colour(serenity::Colour::DARK_GREEN)
        .footer(CreateEmbedFooter::new("Hayali şirketler, sunucu parası. Al: /borsa al · Sat: /borsa sat"));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

async fn trade(ctx: Context<'_>, symbol: &str, count: i64, buying: bool) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let stock = match pick(symbol) {
        Ok(stock) => stock,
        Err(problem) => return reply_private(ctx, problem).await,
    };
    let db = &ctx.data().db;
    let outcome = if buying { stocks::buy(db, guild_id, ctx.author().id, stock, count, now()).await? } else { stocks::sell(db, guild_id, ctx.author().id, stock, count, now()).await? };
    match outcome {
        Trade::Done { total, wallet, shares } => {
            let verb = if buying { "aldın" } else { "sattın" };
            let text = format!("{} **{count}** adet **{}** hissesi {verb}: toplam **{}**.\nElindeki {}: **{shares}** · Cüzdan: {}", stock.emoji, stock.symbol, money(total), stock.symbol, money(wallet));
            ctx.send(CreateReply::default().embed(CreateEmbed::new().title("📈 Borsa işlemi").description(text).colour(serenity::Colour::DARK_GREEN))).await?;
            Ok(())
        }
        Trade::NotEnoughMoney => reply_private(ctx, "Cüzdanındaki para yetmiyor.").await,
        Trade::NotEnoughShares(owned) => reply_private(ctx, format!("O kadar hissen yok (elinde **{owned}** {} var).", stock.symbol)).await,
        Trade::TooMany => reply_private(ctx, format!("Bir hisseden en fazla {} adet tutabilirsin.", stocks::MAX_SHARES)).await,
    }
}

/// Hisse satın alır (cüzdandaki parayla, şu anki fiyattan).
#[poise::command(slash_command, guild_only, rename = "al", user_cooldown = 3)]
async fn al(
    ctx: Context<'_>,
    #[description = "Hisse sembolü (ör. AXT)"]
    #[max_length = 5]
    hisse: String,
    #[description = "Kaç adet"]
    #[min = 1]
    #[max = 100000]
    adet: i64,
) -> Result<(), Error> {
    trade(ctx, &hisse, adet, true).await
}

/// Elindeki hisseyi şu anki fiyattan satar.
#[poise::command(slash_command, guild_only, rename = "sat", user_cooldown = 3)]
async fn sat(
    ctx: Context<'_>,
    #[description = "Hisse sembolü (ör. AXT)"]
    #[max_length = 5]
    hisse: String,
    #[description = "Kaç adet"]
    #[min = 1]
    #[max = 100000]
    adet: i64,
) -> Result<(), Error> {
    trade(ctx, &hisse, adet, false).await
}

/// Elindeki hisseleri ve şu anki değerlerini gösterir.
#[poise::command(slash_command, guild_only, rename = "portföy")]
async fn portfoy(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let db = &ctx.data().db;
    let owned = stocks::holdings(db, guild_id, ctx.author().id).await?;
    if owned.is_empty() {
        return reply_private(ctx, "Hiç hissen yok. Fiyatlara bak: `/borsa fiyatlar`").await;
    }
    let unix = now();
    let mut total = 0;
    let lines: Vec<String> = owned
        .iter()
        .filter_map(|(symbol, shares)| {
            let stock = stocks::find(symbol)?;
            let (price, change) = stocks::quote(stock, unix);
            total += price * shares;
            Some(format!("{} {} **{}** × {shares} = {} ({change:+.1}%)", arrow(change), stock.emoji, stock.symbol, money(price * shares)))
        })
        .collect();
    let account = economy::account(db, guild_id, ctx.author().id).await?;
    let embed = CreateEmbed::new()
        .title("💼 Portföyün")
        .description(format!("{}\n\nHisselerin değeri: **{}**\nCüzdan: {}", lines.join("\n"), money(total), money(account.wallet)))
        .colour(serenity::Colour::DARK_GREEN);
    ctx.send(CreateReply::default().embed(embed).ephemeral(true)).await?;
    Ok(())
}
