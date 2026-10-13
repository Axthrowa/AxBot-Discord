//! Butonlu çekilişler. Katılımlar PostgreSQL'de tutulur; bitiş zamanlayıcı ile otomatik.

use std::time::Duration;

use anyhow::{anyhow, bail};
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow,
    CreateAllowedMentions, CreateButton, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditMessage,
    GuildId, Http, Mentionable, MessageId, UserId,
};
use sqlx::PgPool;
use tracing::info;

use crate::db::snowflake;
use crate::guild_config::to_id;
use crate::rand_util;
use crate::scheduler::{Scheduler, Task};

pub const BUTTON_PREFIX: &str = "gw:";
pub const MAX_WINNERS: u32 = 20;
pub const MAX_DURATION: Duration = Duration::from_secs(30 * 24 * 3600);

pub struct NewGiveaway<'a> {
    pub guild_id: GuildId,
    pub channel_id: ChannelId,
    pub host: UserId,
    pub prize: &'a str,
    pub winners: u32,
    pub duration: Duration,
}

/// Çekilişi oluşturur, mesajını gönderir ve bitişini zamanlar. Çekiliş numarasını döner.
pub async fn create(http: &Http, db: &PgPool, scheduler: &Scheduler, g: NewGiveaway<'_>) -> anyhow::Result<i64> {
    let ends_unix = serenity::Timestamp::now().unix_timestamp() + i64::try_from(g.duration.as_secs())?;
    let ends_at = chrono::DateTime::<chrono::Utc>::from_timestamp(ends_unix, 0).ok_or_else(|| anyhow!("geçersiz süre"))?;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO giveaways (guild_id, channel_id, host_id, prize, winner_count, ends_at)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(snowflake(g.guild_id.get())?)
    .bind(snowflake(g.channel_id.get())?)
    .bind(snowflake(g.host.get())?)
    .bind(g.prize)
    .bind(i32::try_from(g.winners)?)
    .bind(ends_at)
    .fetch_one(db)
    .await?;

    let message = g
        .channel_id
        .send_message(
            http,
            CreateMessage::new()
                .embed(active_embed(id, g.prize, g.host, g.winners, ends_unix))
                .components(vec![button(id, false)]),
        )
        .await;
    let message = match message {
        Ok(m) => m,
        Err(e) => {
            sqlx::query("DELETE FROM giveaways WHERE id = $1").bind(id).execute(db).await?;
            return Err(e.into());
        }
    };

    sqlx::query("UPDATE giveaways SET message_id = $1 WHERE id = $2")
        .bind(snowflake(message.id.get())?)
        .bind(id)
        .execute(db)
        .await?;
    scheduler.schedule(ends_unix, &Task::GiveawayEnd { giveaway_id: id }).await?;

    info!(guild = %g.guild_id, giveaway = id, prize = g.prize, "Çekiliş başladı");
    Ok(id)
}

/// Katıl/ayrıl butonu.
pub async fn handle_button(
    ctx: &serenity::Context,
    db: &PgPool,
    interaction: &ComponentInteraction,
) -> anyhow::Result<()> {
    let id: i64 = interaction
        .data
        .custom_id
        .strip_prefix(BUTTON_PREFIX)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("geçersiz çekiliş butonu"))?;
    let user = snowflake(interaction.user.id.get())?;

    let ended: Option<bool> = sqlx::query_scalar("SELECT ended OR ends_at <= now() FROM giveaways WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    let text = match ended {
        None => "Bu çekiliş artık yok.".to_owned(),
        Some(true) => "⏰ Bu çekiliş sona erdi.".to_owned(),
        Some(false) => {
            let joined = sqlx::query(
                "INSERT INTO giveaway_entries (giveaway_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .bind(user)
            .execute(db)
            .await?
            .rows_affected()
                > 0;
            if !joined {
                sqlx::query("DELETE FROM giveaway_entries WHERE giveaway_id = $1 AND user_id = $2")
                    .bind(id)
                    .bind(user)
                    .execute(db)
                    .await?;
            }
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM giveaway_entries WHERE giveaway_id = $1")
                .bind(id)
                .fetch_one(db)
                .await?;
            if joined {
                format!("🎉 Çekilişe katıldın! Toplam katılımcı: **{count}**")
            } else {
                format!("Çekilişten ayrıldın. Toplam katılımcı: **{count}**")
            }
        }
    };

    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true)),
        )
        .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Row {
    guild_id: i64,
    channel_id: i64,
    message_id: Option<i64>,
    host_id: i64,
    prize: String,
    winner_count: i32,
    ends_unix: i64,
}

/// Çekilişi bitirir (zamanlayıcıdan veya `/çekiliş bitir` ile). Zaten bittiyse bir şey yapmaz.
pub async fn end(http: &Http, db: &PgPool, id: i64) -> anyhow::Result<()> {
    let row: Option<Row> = sqlx::query_as(
        "UPDATE giveaways SET ended = TRUE WHERE id = $1 AND NOT ended
         RETURNING guild_id, channel_id, message_id, host_id, prize, winner_count,
                   EXTRACT(EPOCH FROM ends_at)::BIGINT AS ends_unix",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return Ok(());
    };

    let winners = draw(db, id, usize::try_from(row.winner_count).unwrap_or(1), &[]).await?;
    let winner_ids: Vec<i64> = winners.iter().map(|w| snowflake(w.get())).collect::<Result<_, _>>()?;
    sqlx::query("UPDATE giveaways SET winners = $1 WHERE id = $2")
        .bind(&winner_ids)
        .bind(id)
        .execute(db)
        .await?;

    let entry_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM giveaway_entries WHERE giveaway_id = $1")
        .bind(id)
        .fetch_one(db)
        .await?;
    let channel = ChannelId::new(to_id(Some(row.channel_id)).ok_or_else(|| anyhow!("geçersiz kanal"))?);
    let host = UserId::new(to_id(Some(row.host_id)).ok_or_else(|| anyhow!("geçersiz yetkili"))?);

    if let Some(message_id) = to_id(row.message_id).map(MessageId::new) {
        let edit = EditMessage::new()
            .embed(ended_embed(id, &row.prize, host, &winners, entry_count, row.ends_unix))
            .components(vec![button(id, true)]);
        // Mesaj silinmiş olabilir; sonuç yine duyurulur.
        let _ = channel.edit_message(http, message_id, edit).await;
    }
    announce(http, channel, id, &row.prize, &winners, false).await?;
    info!(guild = row.guild_id, giveaway = id, winners = winners.len(), "Çekiliş bitti");
    Ok(())
}

/// Bitmiş çekiliş için yeni kazanan(lar) seçer; önceki kazananlar hariç tutulur.
pub async fn reroll(http: &Http, db: &PgPool, guild_id: GuildId, id: i64, count: u32) -> anyhow::Result<Vec<UserId>> {
    let row: Option<(i64, String, bool, Vec<i64>)> = sqlx::query_as(
        "SELECT channel_id, prize, ended, winners FROM giveaways WHERE id = $1 AND guild_id = $2",
    )
    .bind(id)
    .bind(snowflake(guild_id.get())?)
    .fetch_optional(db)
    .await?;
    let Some((channel, prize, ended, previous)) = row else {
        bail!("çekiliş bulunamadı");
    };
    if !ended {
        bail!("çekiliş henüz bitmedi");
    }

    let previous: Vec<UserId> = previous.into_iter().filter_map(|w| to_id(Some(w))).map(UserId::new).collect();
    let winners = draw(db, id, usize::try_from(count).unwrap_or(1), &previous).await?;
    let channel = ChannelId::new(to_id(Some(channel)).ok_or_else(|| anyhow!("geçersiz kanal"))?);
    announce(http, channel, id, &prize, &winners, true).await?;
    Ok(winners)
}

/// Bu sunucudaki bitmemiş çekilişin numarası geçerli mi?
pub async fn is_active(db: &PgPool, guild_id: GuildId, id: i64) -> anyhow::Result<bool> {
    let active: Option<bool> = sqlx::query_scalar("SELECT NOT ended FROM giveaways WHERE id = $1 AND guild_id = $2")
        .bind(id)
        .bind(snowflake(guild_id.get())?)
        .fetch_optional(db)
        .await?;
    Ok(active.unwrap_or(false))
}

async fn draw(db: &PgPool, id: i64, count: usize, exclude: &[UserId]) -> anyhow::Result<Vec<UserId>> {
    let entries: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM giveaway_entries WHERE giveaway_id = $1")
        .bind(id)
        .fetch_all(db)
        .await?;
    let pool: Vec<UserId> = entries
        .into_iter()
        .filter_map(|u| to_id(Some(u)))
        .map(UserId::new)
        .filter(|u| !exclude.contains(u))
        .collect();
    Ok(rand_util::pick(&pool, count))
}

async fn announce(
    http: &Http,
    channel: ChannelId,
    id: i64,
    prize: &str,
    winners: &[UserId],
    reroll: bool,
) -> anyhow::Result<()> {
    let prefix = if reroll { "🔄 Yeniden çekildi! " } else { "" };
    let content = if winners.is_empty() {
        format!("{prefix}😔 **{prize}** çekilişinde (#{id}) seçilebilecek katılımcı yok.")
    } else {
        let mentions = winners.iter().map(|w| w.mention().to_string()).collect::<Vec<_>>().join(", ");
        let verb = if winners.len() > 1 { "kazandınız" } else { "kazandın" };
        format!("{prefix}🎉 Tebrikler {mentions}! **{prize}** çekilişini (#{id}) {verb}!")
    };
    channel
        .send_message(
            http,
            CreateMessage::new()
                .content(content)
                .allowed_mentions(CreateAllowedMentions::new().users(winners.to_vec())),
        )
        .await?;
    Ok(())
}

fn button(id: i64, ended: bool) -> CreateActionRow {
    let button = CreateButton::new(format!("{BUTTON_PREFIX}{id}"))
        .label(if ended { "Çekiliş bitti" } else { "Katıl" })
        .emoji('🎉')
        .style(if ended { ButtonStyle::Secondary } else { ButtonStyle::Success })
        .disabled(ended);
    CreateActionRow::Buttons(vec![button])
}

fn active_embed(id: i64, prize: &str, host: UserId, winners: u32, ends_unix: i64) -> CreateEmbed {
    CreateEmbed::new()
        .title(format!("🎉 {prize}"))
        .description(format!(
            "Katılmak için **Katıl** butonuna bas!\n\n\
             ⏰ Bitiş: <t:{ends_unix}:R> (<t:{ends_unix}:f>)\n\
             🏆 Kazanan sayısı: **{winners}**\n\
             👤 Düzenleyen: {}",
            host.mention()
        ))
        .colour(serenity::Colour::GOLD)
        .footer(CreateEmbedFooter::new(format!("Çekiliş #{id}")))
}

fn ended_embed(id: i64, prize: &str, host: UserId, winners: &[UserId], entries: i64, ends_unix: i64) -> CreateEmbed {
    let winner_text = if winners.is_empty() {
        "Kazanan yok (katılım olmadı)".to_owned()
    } else {
        winners.iter().map(|w| w.mention().to_string()).collect::<Vec<_>>().join(", ")
    };
    CreateEmbed::new()
        .title(format!("🎉 {prize}"))
        .description(format!(
            "**Çekiliş sona erdi.**\n\n\
             🏆 Kazanan: {winner_text}\n\
             👥 Katılımcı: **{entries}**\n\
             ⏰ Bitti: <t:{ends_unix}:f>\n\
             👤 Düzenleyen: {}",
            host.mention()
        ))
        .colour(serenity::Colour::DARK_GREY)
        .footer(CreateEmbedFooter::new(format!("Çekiliş #{id}")))
}
