//! Starboard: ⭐ ve diğer emojiler için öne çıkarılan mesaj panoları.
//!
//! - Asıl ⭐ panosu: `starboard_posts` tablosunda ve `guild_configs` ayarlarında tutulur.
//! - Ek emoji panoları (😂, ❤️ vb.): `starboard_extra_posts` tablosunda ve `StarboardSettings` ("starboards") ayarlarında tutulur.
//!
//! Mesaj bazlı eşzamanlılık kilidi (`MESSAGE_LOCKS`) ile aynı mesaja gelen hızlı tepkiler
//! çift kopya oluşturmazken farklı mesajların birbirini bloklaması engellenir.

use std::sync::{Arc, LazyLock};

use dashmap::DashMap;
use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateActionRow, CreateButton, CreateEmbed,
    CreateEmbedAuthor, CreateEmbedFooter, CreateMessage, EditMessage, GuildId, Mentionable,
    Message, MessageId, ReactionType,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::data::Data;
use crate::db::snowflake;
use crate::guild_config::to_id;
use crate::settings::PluginConfig;

pub const STAR: &str = "⭐";
/// Asıl starboard ile birlikte en fazla 10 pano (1 asıl + 9 ek).
pub const MAX_EXTRA: usize = 9;
/// Bir mesajın en fazla bu kadar tepki vereni okunur (Discord sınırı).
const MAX_REACTORS: u8 = 100;

static MESSAGE_LOCKS: LazyLock<DashMap<MessageId, Arc<Mutex<()>>>> = LazyLock::new(DashMap::new);

async fn with_message_lock<Fut, R>(message_id: MessageId, fut: Fut) -> R
where
    Fut: std::future::Future<Output = R>,
{
    let lock = {
        let entry = MESSAGE_LOCKS.entry(message_id).or_insert_with(|| Arc::new(Mutex::new(())));
        Arc::clone(entry.value())
    };
    let _guard = lock.lock().await;
    let res = fut.await;
    drop(_guard);
    if Arc::strong_count(&lock) <= 2 {
        MESSAGE_LOCKS.remove_if(&message_id, |_, v| Arc::ptr_eq(v, &lock) && Arc::strong_count(v) <= 2);
    }
    res
}

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

/// Emojinin asıl yıldız emojisi (⭐) olup olmadığını kontrol eder.
pub fn is_star(emoji: &ReactionType) -> bool {
    match emoji {
        ReactionType::Unicode(e) => normalize_emoji(e) == normalize_emoji(STAR),
        _ => false,
    }
}

/// Panelden girilen emoji uygun mu? Standart tek emoji olmalı; ⭐ asıl starboard'a ayrılmıştır.
pub fn valid_emoji(raw: &str) -> bool {
    let emoji = normalize_emoji(raw);
    let count = emoji.chars().count();
    (1..=8).contains(&count)
        && emoji != normalize_emoji(STAR)
        && !emoji.chars().any(|c| c.is_alphanumeric() || c.is_whitespace() || c == '<' || c == ':')
}

pub fn embed(message: &Message) -> CreateEmbed {
    let content: String = message.content.chars().take(4000).collect();
    let author_name = message.author.global_name.clone().unwrap_or_else(|| message.author.name.clone());
    let mut embed = CreateEmbed::new()
        .author(CreateEmbedAuthor::new(author_name).icon_url(message.author.face()))
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

pub fn link(message: &Message, guild_id: GuildId) -> Vec<CreateActionRow> {
    let url = format!("https://discord.com/channels/{guild_id}/{}/{}", message.channel_id, message.id);
    vec![CreateActionRow::Buttons(vec![CreateButton::new_link(url).label("Mesaja git")])]
}

fn header_star(stars: u32, channel_id: ChannelId) -> String {
    let icon = match stars {
        0..=4 => "⭐",
        5..=9 => "🌟",
        _ => "💫",
    };
    format!("{icon} **{stars}** | {}", channel_id.mention())
}

fn header_board(emoji: &str, count: u32, channel_id: ChannelId) -> String {
    format!("{emoji} **{count}** | {}", channel_id.mention())
}

/// Mesaj sahibi ve botlar hariç bu emojiyi verenlerin sayısı.
pub async fn count_reactors(
    ctx: &serenity::Context,
    message: &Message,
    target_emoji: &str,
) -> anyhow::Result<u32> {
    let wanted = normalize_emoji(target_emoji);
    let Some(reaction) = message.reactions.iter().find_map(|r| match &r.reaction_type {
        ReactionType::Unicode(e) if normalize_emoji(e) == wanted => Some(r.reaction_type.clone()),
        _ => None,
    }) else {
        return Ok(0);
    };
    let users = message
        .channel_id
        .reaction_users(ctx, message.id, reaction, Some(MAX_REACTORS), None)
        .await?;
    let count = users.iter().filter(|u| !u.bot && u.id != message.author.id).count();
    Ok(u32::try_from(count)?)
}

async fn sync_star_board(
    ctx: &serenity::Context,
    db: &PgPool,
    guild_id: GuildId,
    board: ChannelId,
    threshold: u32,
    message: &Message,
) -> anyhow::Result<()> {
    let stars = count_reactors(ctx, message, STAR).await?;
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT star_message_id FROM starboard_posts WHERE message_id = $1",
    )
    .bind(snowflake(message.id.get())?)
    .fetch_optional(db)
    .await?;
    let existing = to_id(existing).map(MessageId::new);

    match (existing, stars >= threshold) {
        (None, false) => {}
        (None, true) => {
            let post = board
                .send_message(
                    ctx,
                    CreateMessage::new()
                        .content(header_star(stars, message.channel_id))
                        .embed(embed(message))
                        .components(link(message, guild_id)),
                )
                .await?;
            sqlx::query(
                "INSERT INTO starboard_posts (message_id, guild_id, channel_id, star_message_id, stars)
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT (message_id) DO NOTHING",
            )
            .bind(snowflake(message.id.get())?)
            .bind(snowflake(guild_id.get())?)
            .bind(snowflake(message.channel_id.get())?)
            .bind(snowflake(post.id.get())?)
            .bind(i32::try_from(stars)?)
            .execute(db)
            .await?;
            debug!(guild = %guild_id, message = %message.id, stars, "Starboard'a eklendi");
        }
        (Some(post), true) => {
            board
                .edit_message(
                    ctx,
                    post,
                    EditMessage::new().content(header_star(stars, message.channel_id)),
                )
                .await?;
            sqlx::query("UPDATE starboard_posts SET stars = $2 WHERE message_id = $1")
                .bind(snowflake(message.id.get())?)
                .bind(i32::try_from(stars)?)
                .execute(db)
                .await?;
        }
        (Some(post), false) => {
            if let Err(e) = board.delete_message(ctx, post).await {
                debug!(error = %e, "Starboard kopyası silinemedi (elle silinmiş olabilir)");
            }
            sqlx::query("DELETE FROM starboard_posts WHERE message_id = $1")
                .bind(snowflake(message.id.get())?)
                .execute(db)
                .await?;
        }
    }
    Ok(())
}

async fn sync_extra_board(
    ctx: &serenity::Context,
    db: &PgPool,
    guild_id: GuildId,
    board: &Board,
    message: &Message,
) -> anyhow::Result<()> {
    let target = ChannelId::new(board.channel);
    let count = count_reactors(ctx, message, &board.emoji).await?;
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT star_message_id FROM starboard_extra_posts WHERE message_id = $1 AND board_channel_id = $2",
    )
    .bind(snowflake(message.id.get())?)
    .bind(snowflake(board.channel)?)
    .fetch_optional(db)
    .await?;
    let existing = to_id(existing).map(MessageId::new);
    let header = header_board(&board.emoji, count, message.channel_id);

    match (existing, count >= board.threshold.max(1)) {
        (None, false) => {}
        (None, true) => {
            let post = target
                .send_message(
                    ctx,
                    CreateMessage::new()
                        .content(header)
                        .embed(embed(message))
                        .components(link(message, guild_id)),
                )
                .await?;
            sqlx::query(
                "INSERT INTO starboard_extra_posts (message_id, board_channel_id, guild_id, star_message_id)
                 VALUES ($1, $2, $3, $4) ON CONFLICT (message_id, board_channel_id) DO NOTHING",
            )
            .bind(snowflake(message.id.get())?)
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
                .bind(snowflake(message.id.get())?)
                .bind(snowflake(board.channel)?)
                .execute(db)
                .await?;
        }
    }
    Ok(())
}

/// Bir tepki eklendi veya kaldırıldı: ilgili pano(lar) kopyayı oluşturur, günceller ya da siler.
pub async fn on_reaction_event(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    channel_id: ChannelId,
    message_id: MessageId,
    emoji: &ReactionType,
) {
    let ReactionType::Unicode(raw) = emoji else { return };
    let normalized = normalize_emoji(raw);

    with_message_lock(message_id, async {
        let message = match channel_id.message(ctx, message_id).await {
            Ok(msg) => msg,
            Err(e) => {
                debug!(guild = %guild_id, message = %message_id, error = %e, "Mesaj okunamadı");
                return;
            }
        };

        // 1. Asıl ⭐ panosu
        if is_star(emoji) {
            if let Ok(cfg) = data.guild_configs.get(&data.db, guild_id).await {
                if let Some(board) = cfg.starboard_channel {
                    if channel_id != board {
                        if let Err(e) = sync_star_board(ctx, &data.db, guild_id, board, cfg.starboard_threshold, &message).await {
                            warn!(guild = %guild_id, message = %message_id, error = ?e, "Starboard güncellenemedi");
                        }
                    }
                }
            }
        }

        // 2. Ek panolar
        if let Ok(settings) = data.settings.get::<StarboardSettings>(&data.db, guild_id).await {
            for board in settings.boards.iter().filter(|b| b.emoji == normalized) {
                if board.channel == channel_id.get() || board.channel == 0 {
                    continue;
                }
                if let Err(e) = sync_extra_board(ctx, &data.db, guild_id, board, &message).await {
                    warn!(guild = %guild_id, message = %message_id, error = ?e, "Ek starboard güncellenemedi");
                }
            }
        }
    })
    .await;
}

/// Bir mesaja ait belirli bir emojinin tüm tepkileri silindiğinde çağrılır.
pub async fn on_reaction_emoji_removed(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    channel_id: ChannelId,
    message_id: MessageId,
    emoji: &ReactionType,
) {
    on_reaction_event(ctx, data, guild_id, channel_id, message_id, emoji).await;
}

/// Bir mesajdaki TÜM tepkiler topluca temizlendiğinde çağrılır (hem ⭐ hem ek panolar senkronize edilir).
pub async fn on_reactions_cleared(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    channel_id: ChannelId,
    message_id: MessageId,
) {
    with_message_lock(message_id, async {
        let message = match channel_id.message(ctx, message_id).await {
            Ok(msg) => msg,
            Err(e) => {
                debug!(guild = %guild_id, message = %message_id, error = %e, "Mesaj okunamadı");
                return;
            }
        };

        // Asıl ⭐ panosu
        if let Ok(cfg) = data.guild_configs.get(&data.db, guild_id).await {
            if let Some(board) = cfg.starboard_channel {
                if channel_id != board {
                    if let Err(e) = sync_star_board(ctx, &data.db, guild_id, board, cfg.starboard_threshold, &message).await {
                        warn!(guild = %guild_id, message = %message_id, error = ?e, "Starboard güncellenemedi");
                    }
                }
            }
        }

        // Ek panolar
        if let Ok(settings) = data.settings.get::<StarboardSettings>(&data.db, guild_id).await {
            for board in &settings.boards {
                if board.channel == channel_id.get() || board.channel == 0 {
                    continue;
                }
                if let Err(e) = sync_extra_board(ctx, &data.db, guild_id, board, &message).await {
                    warn!(guild = %guild_id, message = %message_id, error = ?e, "Ek starboard güncellenemedi");
                }
            }
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_star_variants() {
        assert!(is_star(&ReactionType::Unicode("⭐".to_owned())));
        assert!(is_star(&ReactionType::Unicode("⭐\u{FE0F}".to_owned())));
        assert!(!is_star(&ReactionType::Unicode("🌟".to_owned())));
        assert!(!is_star(&ReactionType::Unicode("❤️".to_owned())));
        assert!(!is_star(&ReactionType::Custom {
            animated: false,
            id: serenity::EmojiId::new(12345),
            name: Some("star".to_owned()),
        }));
    }

    #[test]
    fn header_icon_grows_with_stars() {
        let channel = ChannelId::new(1);
        assert!(header_star(3, channel).starts_with("⭐ **3**"));
        assert!(header_star(7, channel).starts_with("🌟"));
        assert!(header_star(12, channel).starts_with("💫"));
    }

    #[test]
    fn validates_board_emojis() {
        assert!(valid_emoji("😂") && valid_emoji("❤️") && valid_emoji(" 🔥 "));
        assert_eq!(normalize_emoji("❤️"), "❤");
        // ⭐ asıl starboard'ındır; yazı ve sunucu emojisi kabul edilmez.
        for bad in ["⭐", "⭐️", "", "abc", "<:ad:123>", "😂 😂", ":joy:"] {
            assert!(!valid_emoji(bad), "{bad}");
        }
    }

    #[test]
    fn header_board_formatting() {
        let channel = ChannelId::new(10);
        let formatted = header_board("🔥", 5, channel);
        assert_eq!(formatted, "🔥 **5** | <#10>");
    }

    #[test]
    fn normalize_emoji_removes_variant_selector() {
        assert_eq!(normalize_emoji("❤️"), "❤");
        assert_eq!(normalize_emoji("⭐\u{FE0F}"), "⭐");
        assert_eq!(normalize_emoji(" 🔥 "), "🔥");
    }

    #[test]
    fn board_serialization_round_trip() -> anyhow::Result<()> {
        let board = Board {
            channel: 123456789,
            emoji: "🔥".to_owned(),
            threshold: 5,
        };
        let json = serde_json::to_string(&board)?;
        let parsed: Board = serde_json::from_str(&json)?;
        assert_eq!(board, parsed);

        let settings = StarboardSettings {
            boards: vec![board],
        };
        let json_settings = serde_json::to_string(&settings)?;
        let parsed_settings: StarboardSettings = serde_json::from_str(&json_settings)?;
        assert_eq!(settings, parsed_settings);
        Ok(())
    }
}
