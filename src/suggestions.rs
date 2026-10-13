//! Öneri kanalı: üyeler `/öneri` ile öneri gönderir, diğerleri butonla oylar,
//! yetkililer `/öneri-yanıtla` ile sonuçlandırır.
//!
//! Butonlar `sg:up:<id>` / `sg:down:<id>` biçimindedir; bot yeniden başlasa da çalışır.
//! Oy sayıları buton etiketlerinde gösterilir.

use anyhow::anyhow;
use poise::serenity_prelude::{
    self as serenity, AutoArchiveDuration, ButtonStyle, ChannelId, ComponentInteraction,
    CreateActionRow, CreateButton, CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateThread,
    EditMessage, GuildId, Http, MessageId, User, UserId,
};
use sqlx::PgPool;
use tracing::{debug, info};

use crate::db::snowflake;
use crate::guild_config::to_id;

pub const BUTTON_PREFIX: &str = "sg:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum Status {
    #[name = "Kabul edildi"]
    Approved,
    #[name = "Reddedildi"]
    Denied,
    #[name = "Değerlendiriliyor"]
    Considering,
    #[name = "Uygulandı"]
    Implemented,
}

impl Status {
    fn db(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Considering => "considering",
            Self::Implemented => "implemented",
        }
    }
}

/// Embed'de gösterilecek durum etiketi ve rengi.
fn status_look(status: &str) -> (&'static str, serenity::Colour) {
    match status {
        "approved" => ("✅ Kabul edildi", serenity::Colour::from_rgb(87, 242, 135)),
        "denied" => ("❌ Reddedildi", serenity::Colour::from_rgb(237, 66, 69)),
        "considering" => ("🤔 Değerlendiriliyor", serenity::Colour::from_rgb(254, 231, 92)),
        "implemented" => ("🚀 Uygulandı", serenity::Colour::from_rgb(26, 188, 156)),
        _ => ("🗳️ Oylamada", serenity::Colour::BLURPLE),
    }
}

/// Sonuçlanan öneriler artık oylanamaz.
fn voting_open(status: &str) -> bool {
    matches!(status, "open" | "considering")
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    number: i32,
    channel_id: i64,
    message_id: Option<i64>,
    author_id: i64,
    content: String,
    status: String,
    reviewer_id: Option<i64>,
    review_note: Option<String>,
}

struct Votes {
    up: i64,
    down: i64,
}

/// Öneriyi kanala gönderir, tartışma alt başlığı açar ve öneri numarasını döner.
pub async fn create(http: &Http, db: &PgPool, guild_id: GuildId, channel: ChannelId, author: &User, content: &str) -> anyhow::Result<i32> {
    let (id, number): (i64, i32) = sqlx::query_as(
        "INSERT INTO suggestions (guild_id, number, channel_id, author_id, content)
         VALUES ($1, (SELECT COALESCE(MAX(number), 0) + 1 FROM suggestions WHERE guild_id = $1), $2, $3, $4)
         RETURNING id, number",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(channel.get())?)
    .bind(snowflake(author.id.get())?)
    .bind(content)
    .fetch_one(db)
    .await?;

    let row = Row {
        id,
        number,
        channel_id: snowflake(channel.get())?,
        message_id: None,
        author_id: snowflake(author.id.get())?,
        content: content.to_owned(),
        status: "open".to_owned(),
        reviewer_id: None,
        review_note: None,
    };
    let votes = Votes { up: 0, down: 0 };
    let sent = channel
        .send_message(http, CreateMessage::new().embed(embed(&row, author)).components(buttons(&row, &votes)))
        .await;
    let message = match sent {
        Ok(m) => m,
        Err(e) => {
            sqlx::query("DELETE FROM suggestions WHERE id = $1").bind(id).execute(db).await?;
            return Err(e.into());
        }
    };
    sqlx::query("UPDATE suggestions SET message_id = $2 WHERE id = $1")
        .bind(id)
        .bind(snowflake(message.id.get())?)
        .execute(db)
        .await?;

    let thread = CreateThread::new(format!("Öneri #{number} — tartışma"))
        .auto_archive_duration(AutoArchiveDuration::OneWeek);
    if let Err(e) = channel.create_thread_from_message(http, message.id, thread).await {
        debug!(error = %e, "Öneri için alt başlık açılamadı (izin yok olabilir)");
    }

    info!(guild = %guild_id, suggestion = number, "Öneri gönderildi");
    Ok(number)
}

/// 👍/👎 butonu: aynı oya tekrar basmak oyu geri alır.
pub async fn handle_button(ctx: &serenity::Context, db: &PgPool, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let (direction, id) = parse_button(&interaction.data.custom_id).ok_or_else(|| anyhow!("geçersiz öneri butonu"))?;

    let row: Option<Row> = sqlx::query_as(
        "SELECT id, number, channel_id, message_id, author_id, content, status, reviewer_id, review_note
         FROM suggestions WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return ephemeral(ctx, interaction, "Bu öneri artık yok.").await;
    };
    if !voting_open(&row.status) {
        return ephemeral(ctx, interaction, "Bu öneri sonuçlandı, artık oylanamaz.").await;
    }

    let user = snowflake(interaction.user.id.get())?;
    let previous: Option<i16> = sqlx::query_scalar("SELECT vote FROM suggestion_votes WHERE suggestion_id = $1 AND user_id = $2")
        .bind(id)
        .bind(user)
        .fetch_optional(db)
        .await?;
    if previous == Some(direction) {
        sqlx::query("DELETE FROM suggestion_votes WHERE suggestion_id = $1 AND user_id = $2")
            .bind(id)
            .bind(user)
            .execute(db)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO suggestion_votes (suggestion_id, user_id, vote) VALUES ($1, $2, $3)
             ON CONFLICT (suggestion_id, user_id) DO UPDATE SET vote = EXCLUDED.vote",
        )
        .bind(id)
        .bind(user)
        .bind(direction)
        .execute(db)
        .await?;
    }

    let votes = votes(db, id).await?;
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::UpdateMessage(CreateInteractionResponseMessage::new().components(buttons(&row, &votes))),
        )
        .await?;
    Ok(())
}

pub enum ReviewOutcome {
    /// Öneren kişiye DM gönderildiyse onun ID'si.
    Done { notified: Option<UserId> },
    NotFound,
}

/// Öneriyi sonuçlandırır: mesajı günceller ve öneren kişiye DM atar.
pub async fn review(
    http: &Http,
    db: &PgPool,
    guild_id: GuildId,
    number: i32,
    status: Status,
    reviewer: UserId,
    note: Option<&str>,
) -> anyhow::Result<ReviewOutcome> {
    let row: Option<Row> = sqlx::query_as(
        "UPDATE suggestions SET status = $3, reviewer_id = $4, review_note = $5
         WHERE guild_id = $1 AND number = $2
         RETURNING id, number, channel_id, message_id, author_id, content, status, reviewer_id, review_note",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(number)
    .bind(status.db())
    .bind(snowflake(reviewer.get())?)
    .bind(note)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return Ok(ReviewOutcome::NotFound);
    };

    let author = match to_id(Some(row.author_id)) {
        Some(id) => UserId::new(id).to_user(http).await.ok(),
        None => None,
    };
    let votes = votes(db, row.id).await?;
    if let (Some(channel), Some(message)) = (to_id(Some(row.channel_id)), to_id(row.message_id)) {
        let fallback = User::default();
        let edit = EditMessage::new()
            .embed(embed(&row, author.as_ref().unwrap_or(&fallback)))
            .components(buttons(&row, &votes));
        ChannelId::new(channel).edit_message(http, MessageId::new(message), edit).await?;
    }

    let mut notified = None;
    if let Some(author) = &author {
        let (label, _) = status_look(&row.status);
        let mut text = format!("📬 **#{}** numaralı önerin güncellendi: **{label}**", row.number);
        if let Some(note) = note {
            text.push_str(&format!("\n> {note}"));
        }
        match author.direct_message(http, CreateMessage::new().content(text)).await {
            Ok(_) => notified = Some(author.id),
            Err(e) => debug!(user = %author.id, error = %e, "Öneri sonucu DM ile gönderilemedi"),
        }
    }
    Ok(ReviewOutcome::Done { notified })
}

async fn votes(db: &PgPool, id: i64) -> anyhow::Result<Votes> {
    let (up, down): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE vote > 0), COUNT(*) FILTER (WHERE vote < 0)
         FROM suggestion_votes WHERE suggestion_id = $1",
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(Votes { up, down })
}

fn embed(row: &Row, author: &User) -> CreateEmbed {
    let (label, colour) = status_look(&row.status);
    let name = author.global_name.clone().unwrap_or_else(|| author.name.clone());
    let mut embed = CreateEmbed::new()
        .author(CreateEmbedAuthor::new(name).icon_url(author.face()))
        .title(format!("💡 Öneri #{}", row.number))
        .description(&row.content)
        .colour(colour)
        .field("Durum", label, true);
    if let Some(reviewer) = to_id(row.reviewer_id) {
        embed = embed.field("Yanıtlayan", format!("<@{reviewer}>"), true);
    }
    if let Some(note) = &row.review_note {
        embed = embed.field("Not", note.as_str(), false);
    }
    embed.footer(CreateEmbedFooter::new("Oyunu butonlarla ver; tekrar basarsan geri alınır."))
}

fn buttons(row: &Row, votes: &Votes) -> Vec<CreateActionRow> {
    let open = voting_open(&row.status);
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("{BUTTON_PREFIX}up:{}", row.id))
            .label(votes.up.to_string())
            .emoji('👍')
            .style(ButtonStyle::Success)
            .disabled(!open),
        CreateButton::new(format!("{BUTTON_PREFIX}down:{}", row.id))
            .label(votes.down.to_string())
            .emoji('👎')
            .style(ButtonStyle::Danger)
            .disabled(!open),
    ])]
}

/// `sg:up:12` → (1, 12), `sg:down:12` → (-1, 12).
fn parse_button(custom_id: &str) -> Option<(i16, i64)> {
    let rest = custom_id.strip_prefix(BUTTON_PREFIX)?;
    let (kind, id) = rest.split_once(':')?;
    let direction = match kind {
        "up" => 1,
        "down" => -1,
        _ => return None,
    };
    Some((direction, id.parse().ok()?))
}

async fn ephemeral(ctx: &serenity::Context, interaction: &ComponentInteraction, text: &str) -> anyhow::Result<()> {
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true)),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vote_buttons() {
        assert_eq!(parse_button("sg:up:12"), Some((1, 12)));
        assert_eq!(parse_button("sg:down:7"), Some((-1, 7)));
        assert_eq!(parse_button("sg:left:7"), None);
        assert_eq!(parse_button("gw:7"), None);
    }

    #[test]
    fn only_open_suggestions_accept_votes() {
        assert!(voting_open("open"));
        assert!(voting_open("considering"));
        assert!(!voting_open("approved"));
        assert!(!voting_open("denied"));
    }
}
