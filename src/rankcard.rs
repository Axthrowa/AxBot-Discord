//! Kişisel rütbe kartı: üyenin kendi seçtiği vurgu rengi, arka plan resmi ve yazı tipi (`/kartım`).
//!
//! Yalnızca `/seviye` çağrıldığında okunur (mesaj başına değil), bu yüzden doğrudan veritabanındadır.

use poise::serenity_prelude::{GuildId, UserId};
use sqlx::PgPool;

use crate::db::snowflake;

pub const MAX_URL_LEN: usize = 400;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Personal {
    /// `#RRGGBB`
    pub accent: Option<String>,
    /// https resim adresi.
    pub background: Option<String>,
    /// `cards::FONTS` anahtarlarından biri.
    pub font: Option<String>,
}

pub async fn get(db: &PgPool, guild_id: GuildId, user_id: UserId) -> anyhow::Result<Personal> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT accent, background, font FROM rank_cards WHERE guild_id = $1 AND user_id = $2")
            .bind(snowflake(guild_id.get())?)
            .bind(snowflake(user_id.get())?)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(accent, background, font)| Personal { accent, background, font }).unwrap_or_default())
}

pub async fn set(db: &PgPool, guild_id: GuildId, user_id: UserId, card: &Personal) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO rank_cards (guild_id, user_id, accent, background, font) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (guild_id, user_id) DO UPDATE
             SET accent = EXCLUDED.accent, background = EXCLUDED.background, font = EXCLUDED.font",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(user_id.get())?)
    .bind(&card.accent)
    .bind(&card.background)
    .bind(&card.font)
    .execute(db)
    .await?;
    Ok(())
}

/// Kayıt vardıysa `true`.
pub async fn reset(db: &PgPool, guild_id: GuildId, user_id: UserId) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM rank_cards WHERE guild_id = $1 AND user_id = $2")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user_id.get())?)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}
