//! Ek starboard'lar: ⭐ dışındaki emojiler için ayrı panolar (ör. 😂 → #komik-anlar).
//!
//! Asıl starboard (`starboard.rs`, ⭐) olduğu gibi durur; buradakiler onun yanında çalışır.
//! Ayar `PluginSettings` deposunda; kopyalar `starboard_extra_posts` tablosunda (mesaj + pano başına bir kayıt).

use poise::serenity_prelude::{self as serenity, ChannelId, CreateMessage, EditMessage, GuildId, Mentionable, Message, MessageId, ReactionType};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::data::Data;
use crate::db::snowflake;
use crate::guild_config::to_id;
use crate::settings::PluginConfig;

/// Asıl starboard ile birlikte en fazla 10 pano.
pub const MAX_EXTRA: usize = 9;
const MAX_REACTORS: u8 = 100;

static LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub channel: u64,
    /// Standart (Unicode) emoji.
    pub emoji: String,
    pub threshold: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StarboardSettings {
    pub boards: Vec<Board>,
}

impl PluginConfig for StarboardSettings {
    const KEY: &'static str = "starboards";
}

/// Karşılaştırma için: görünüm seçici (U+FE0F) ve boşluklar atılır.
pub fn normalize_emoji(raw: &str) -> String {
    raw.trim().chars().filter(|c| *c != '\u{FE0F}').collect()
}

/// Panelden girilen emoji uygun mu? Standart tek emoji olmalı; ⭐ asıl starboard'a ayrılmıştır.
pub fn valid_emoji(raw: &str) -> bool {
    let emoji = normalize_emoji(raw);
    let count = emoji.chars().count();
    (1..=8).contains(&count) && emoji != normalize_emoji(crate::starboard::STAR) && !emoji.chars().any(|c| c.is_alphanumeric() || c.is_whitespace() || c == '<' || c == ':')
}

/// Bir tepki eklendi / kaldırıldı: emojisi bir ek panoya aitse kopyayı oluşturur, günceller ya da siler.
pub async fn on_reaction(ctx: &serenity::Context, data: &Data, guild_id: GuildId, channel_id: ChannelId, message_id: MessageId, emoji: &ReactionType) {
    let ReactionType::Unicode(raw) = emoji else { return };
    let Ok(settings) = data.settings.get::<StarboardSettings>(&data.db, guild_id).await else { return };
    if settings.boards.is_empty() {
        return;
    }
    let wanted = normalize_emoji(raw);
    for board in settings.boards.iter().filter(|b| b.emoji == wanted) {
        // Panodaki kopyalara verilen tepkiler yeniden kopyalanmaz.
        if board.channel == channel_id.get() || board.channel == 0 {
            continue;
        }
        let _guard = LOCK.lock().await;
        if let Err(e) = sync(ctx, &data.db, guild_id, board, channel_id, message_id).await {
            warn!(guild = %guild_id, message = %message_id, error = ?e, "Ek starboard güncellenemedi");
        }
    }
}

async fn sync(ctx: &serenity::Context, db: &PgPool, guild_id: GuildId, board: &Board, channel_id: ChannelId, message_id: MessageId) -> anyhow::Result<()> {
    let target = ChannelId::new(board.channel);
    let message = channel_id.message(ctx, message_id).await?;
    let count = count(ctx, &message, &board.emoji).await?;
    let existing: Option<i64> = sqlx::query_scalar("SELECT star_message_id FROM starboard_extra_posts WHERE message_id = $1 AND board_channel_id = $2")
        .bind(snowflake(message_id.get())?)
        .bind(snowflake(board.channel)?)
        .fetch_optional(db)
        .await?;
    let existing = to_id(existing).map(MessageId::new);
    let header = format!("{} **{count}** | {}", board.emoji, channel_id.mention());

    match (existing, count >= board.threshold.max(1)) {
        (None, false) => {}
        (None, true) => {
            let post = target
                .send_message(ctx, CreateMessage::new().content(header).embed(crate::starboard::embed(&message)).components(crate::starboard::link(&message, guild_id)))
                .await?;
            sqlx::query(
                "INSERT INTO starboard_extra_posts (message_id, board_channel_id, guild_id, star_message_id) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (message_id, board_channel_id) DO NOTHING",
            )
            .bind(snowflake(message_id.get())?)
            .bind(snowflake(board.channel)?)
            .bind(snowflake(guild_id.get())?)
            .bind(snowflake(post.id.get())?)
            .execute(db)
            .await?;
        }
        (Some(post), true) => {
            target.edit_message(ctx, post, EditMessage::new().content(header)).await?;
        }
        (Some(post), false) => {
            if let Err(e) = target.delete_message(ctx, post).await {
                debug!(error = %e, "Ek starboard kopyası silinemedi (elle silinmiş olabilir)");
            }
            sqlx::query("DELETE FROM starboard_extra_posts WHERE message_id = $1 AND board_channel_id = $2")
                .bind(snowflake(message_id.get())?)
                .bind(snowflake(board.channel)?)
                .execute(db)
                .await?;
        }
    }
    Ok(())
}

/// Mesaj sahibi ve botlar hariç bu emojiyi verenlerin sayısı.
async fn count(ctx: &serenity::Context, message: &Message, emoji: &str) -> anyhow::Result<u32> {
    let Some(reaction) = message.reactions.iter().find_map(|r| match &r.reaction_type {
        ReactionType::Unicode(e) if normalize_emoji(e) == emoji => Some(r.reaction_type.clone()),
        _ => None,
    }) else {
        return Ok(0);
    };
    let users = message.channel_id.reaction_users(ctx, message.id, reaction, Some(MAX_REACTORS), None).await?;
    Ok(u32::try_from(users.iter().filter(|u| !u.bot && u.id != message.author.id).count())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_board_emojis() {
        assert!(valid_emoji("😂") && valid_emoji("❤️") && valid_emoji(" 🔥 "));
        assert_eq!(normalize_emoji("❤️"), "❤");
        // ⭐ asıl starboard'ındır; yazı ve sunucu emojisi kabul edilmez.
        for bad in ["⭐", "⭐️", "", "abc", "<:ad:123>", "😂 😂", ":joy:"] {
            assert!(!valid_emoji(bad), "{bad}");
        }
    }
}
