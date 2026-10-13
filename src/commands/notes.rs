//! `/üye-notu`: yalnızca yetkililerin gördüğü, üyeye bildirilmeyen moderatör notları.

use poise::serenity_prelude::{self as serenity, CreateEmbed};
use poise::CreateReply;

use super::moderation::reply_private;
use crate::data::Context;
use crate::db::snowflake;
use crate::error::Error;

pub const MAX_NOTES_PER_USER: i64 = 50;

/// Üyeler hakkında yetkili notları (üye görmez, ceza değildir).
#[poise::command(
    slash_command,
    guild_only,
    rename = "üye-notu",
    subcommands("not_ekle", "not_liste", "not_sil"),
    subcommand_required,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS",
    category = "Moderasyon"
)]
pub async fn uye_notu(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Bir üyeye not ekler.
#[poise::command(slash_command, guild_only, rename = "ekle")]
async fn not_ekle(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Not eklenecek üye"]
    user: serenity::User,
    #[rename = "not"]
    #[description = "Not"]
    #[max_length = 500]
    note: String,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let db = &ctx.data().db;
    let (guild, target) = (snowflake(guild_id.get())?, snowflake(user.id.get())?);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mod_notes WHERE guild_id = $1 AND user_id = $2").bind(guild).bind(target).fetch_one(db).await?;
    if count >= MAX_NOTES_PER_USER {
        return reply_private(ctx, format!("Bir üyeye en fazla {MAX_NOTES_PER_USER} not eklenebilir; eskilerden sil.")).await;
    }
    let id: i64 = sqlx::query_scalar("INSERT INTO mod_notes (guild_id, user_id, author_id, note) VALUES ($1, $2, $3, $4) RETURNING id")
        .bind(guild)
        .bind(target)
        .bind(snowflake(ctx.author().id.get())?)
        .bind(note.trim())
        .fetch_one(db)
        .await?;
    reply_private(ctx, format!("📝 Not #{id} eklendi ({}).", user.tag())).await
}

/// Bir üyenin notlarını gösterir.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn not_liste(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Notları gösterilecek üye"]
    user: serenity::User,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let rows: Vec<(i64, i64, String, i64)> = sqlx::query_as(
        "SELECT id, author_id, note, EXTRACT(EPOCH FROM created_at)::BIGINT FROM mod_notes
         WHERE guild_id = $1 AND user_id = $2 ORDER BY id DESC LIMIT 15",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(user.id.get())?)
    .fetch_all(&ctx.data().db)
    .await?;
    if rows.is_empty() {
        return reply_private(ctx, format!("{} için not yok.", user.tag())).await;
    }
    let lines: Vec<String> = rows.iter().map(|(id, author, note, at)| format!("**#{id}** · <@{author}> · <t:{at}:d>\n{note}")).collect();
    let embed = CreateEmbed::new()
        .title(format!("📝 {} — notlar", user.tag()))
        .description(lines.join("\n\n").chars().take(4000).collect::<String>())
        .colour(serenity::Colour::GOLD);
    ctx.send(CreateReply::default().embed(embed).ephemeral(true)).await?;
    Ok(())
}

/// Bir notu siler.
#[poise::command(slash_command, guild_only, rename = "sil")]
async fn not_sil(ctx: Context<'_>, #[description = "Notun numarası (/üye-notu liste)"] #[min = 1] numara: i64) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    // Yalnızca bu sunucunun notu silinebilir.
    let deleted = sqlx::query("DELETE FROM mod_notes WHERE id = $1 AND guild_id = $2")
        .bind(numara)
        .bind(snowflake(guild_id.get())?)
        .execute(&ctx.data().db)
        .await?
        .rows_affected();
    reply_private(ctx, if deleted > 0 { "🗑️ Not silindi." } else { "Bu numarada bir not yok." }).await
}
