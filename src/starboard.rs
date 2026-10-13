//! Starboard: ⭐ tepkisi eşiği geçen mesajlar ayrı bir kanalda öne çıkarılır.
//!
//! Sadece ⭐ tepkilerinde çalışır (diğer tepkiler hiçbir işlem başlatmaz). Mesaj sahibinin ve
//! botların yıldızları sayılmaz. Yıldız sayısı eşiğin altına düşerse kopya silinir.
//! Aynı mesaja aynı anda gelen tepkiler çift kopya oluşturmasın diye işlemler sıraya alınır.

use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateActionRow, CreateButton, CreateEmbed,
    CreateEmbedAuthor, CreateEmbedFooter, CreateMessage, EditMessage, GuildId, Mentionable,
    Message, MessageId, ReactionType,
};
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::data::Data;
use crate::db::snowflake;
use crate::guild_config::to_id;

pub const STAR: &str = "⭐";
/// Bir mesajın en fazla bu kadar yıldızlayanı okunur (Discord sınırı).
const MAX_REACTORS: u8 = 100;

static LOCK: Mutex<()> = Mutex::const_new(());

pub fn is_star(emoji: &ReactionType) -> bool {
    matches!(emoji, ReactionType::Unicode(e) if e == STAR)
}

/// ⭐ eklendi/kaldırıldı: yıldızları sayar, kopyayı oluşturur/günceller/siler.
/// Hatalar sadece loglanır; tepki akışını etkilemez.
pub async fn on_star(ctx: &serenity::Context, data: &Data, guild_id: GuildId, channel_id: ChannelId, message_id: MessageId) {
    let cfg = match data.guild_configs.get(&data.db, guild_id).await {
        Ok(cfg) => cfg,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Sunucu ayarları okunamadı");
            return;
        }
    };
    let Some(board) = cfg.starboard_channel else {
        return;
    };
    // Starboard kanalındaki kopyalar yıldızlanınca tekrar kopyalanmaz.
    if channel_id == board {
        return;
    }

    let _guard = LOCK.lock().await;
    if let Err(e) = sync(ctx, &data.db, guild_id, board, cfg.starboard_threshold, channel_id, message_id).await {
        warn!(guild = %guild_id, message = %message_id, error = ?e, "Starboard güncellenemedi");
    }
}

async fn sync(
    ctx: &serenity::Context,
    db: &PgPool,
    guild_id: GuildId,
    board: ChannelId,
    threshold: u32,
    channel_id: ChannelId,
    message_id: MessageId,
) -> anyhow::Result<()> {
    let message = channel_id.message(ctx, message_id).await?;
    let stars = count_stars(ctx, &message).await?;
    let existing = find(db, message_id).await?;

    match (existing, stars >= threshold) {
        (None, false) => {}
        (None, true) => {
            let post = board
                .send_message(ctx, CreateMessage::new().content(header(stars, channel_id)).embed(embed(&message)).components(link(&message, guild_id)))
                .await?;
            sqlx::query(
                "INSERT INTO starboard_posts (message_id, guild_id, channel_id, star_message_id, stars)
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT (message_id) DO NOTHING",
            )
            .bind(snowflake(message_id.get())?)
            .bind(snowflake(guild_id.get())?)
            .bind(snowflake(channel_id.get())?)
            .bind(snowflake(post.id.get())?)
            .bind(i32::try_from(stars)?)
            .execute(db)
            .await?;
            debug!(guild = %guild_id, message = %message_id, stars, "Starboard'a eklendi");
        }
        (Some(post), true) => {
            board
                .edit_message(ctx, post, EditMessage::new().content(header(stars, channel_id)))
                .await?;
            sqlx::query("UPDATE starboard_posts SET stars = $2 WHERE message_id = $1")
                .bind(snowflake(message_id.get())?)
                .bind(i32::try_from(stars)?)
                .execute(db)
                .await?;
        }
        (Some(post), false) => {
            if let Err(e) = board.delete_message(ctx, post).await {
                debug!(error = %e, "Starboard kopyası silinemedi (elle silinmiş olabilir)");
            }
            sqlx::query("DELETE FROM starboard_posts WHERE message_id = $1")
                .bind(snowflake(message_id.get())?)
                .execute(db)
                .await?;
        }
    }
    Ok(())
}

/// Mesaj sahibi ve botlar hariç ⭐ verenlerin sayısı.
async fn count_stars(ctx: &serenity::Context, message: &Message) -> anyhow::Result<u32> {
    let has_stars = message.reactions.iter().any(|r| is_star(&r.reaction_type));
    if !has_stars {
        return Ok(0);
    }
    let users = message
        .channel_id
        .reaction_users(ctx, message.id, ReactionType::Unicode(STAR.to_owned()), Some(MAX_REACTORS), None)
        .await?;
    let count = users.iter().filter(|u| !u.bot && u.id != message.author.id).count();
    Ok(u32::try_from(count)?)
}

async fn find(db: &PgPool, message_id: MessageId) -> anyhow::Result<Option<MessageId>> {
    let id: Option<i64> = sqlx::query_scalar("SELECT star_message_id FROM starboard_posts WHERE message_id = $1")
        .bind(snowflake(message_id.get())?)
        .fetch_optional(db)
        .await?;
    Ok(to_id(id).map(MessageId::new))
}

fn header(stars: u32, channel_id: ChannelId) -> String {
    let icon = match stars {
        0..=4 => "⭐",
        5..=9 => "🌟",
        _ => "💫",
    };
    format!("{icon} **{stars}** | {}", channel_id.mention())
}

pub(crate) fn embed(message: &Message) -> CreateEmbed {
    let content: String = message.content.chars().take(4000).collect();
    let mut embed = CreateEmbed::new()
        .author(CreateEmbedAuthor::new(message.author.global_name.clone().unwrap_or_else(|| message.author.name.clone())).icon_url(message.author.face()))
        .colour(serenity::Colour::GOLD)
        .footer(CreateEmbedFooter::new(format!("Mesaj ID: {}", message.id)))
        .timestamp(message.timestamp);
    if !content.is_empty() {
        embed = embed.description(content);
    }
    let image = message
        .attachments
        .iter()
        .find(|a| a.content_type.as_deref().is_some_and(|t| t.starts_with("image/")));
    if let Some(image) = image {
        embed = embed.image(&image.url);
    } else if let Some(file) = message.attachments.first() {
        embed = embed.field("Ek", format!("[{}]({})", file.filename, file.url), false);
    }
    embed
}

pub(crate) fn link(message: &Message, guild_id: GuildId) -> Vec<CreateActionRow> {
    let url = format!("https://discord.com/channels/{guild_id}/{}/{}", message.channel_id, message.id);
    vec![CreateActionRow::Buttons(vec![CreateButton::new_link(url).label("Mesaja git")])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_only_star() {
        assert!(is_star(&ReactionType::Unicode("⭐".to_owned())));
        assert!(!is_star(&ReactionType::Unicode("🌟".to_owned())));
    }

    #[test]
    fn header_icon_grows_with_stars() {
        let channel = ChannelId::new(1);
        assert!(header(3, channel).starts_with("⭐ **3**"));
        assert!(header(7, channel).starts_with("🌟"));
        assert!(header(12, channel).starts_with("💫"));
    }
}
