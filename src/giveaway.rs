//! Butonlu çekilişler. Katılımlar PostgreSQL'de tutulur; bitiş zamanlayıcı ile otomatik.

use std::time::Duration;

use anyhow::{anyhow, bail};
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow,
    CreateAllowedMentions, CreateButton, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditMessage,
    GuildId, Http, Mentionable, MessageId, RoleId, UserId,
};
use sqlx::PgPool;
use tracing::info;

use crate::data::Data;
use crate::db::snowflake;
use crate::guild_config::to_id;
use crate::rand_util;
use crate::scheduler::{Scheduler, Task};

pub const BUTTON_PREFIX: &str = "gw:";
pub const MAX_WINNERS: u32 = 20;
pub const MAX_DURATION: Duration = Duration::from_secs(30 * 24 * 3600);

/// Ek şans rolündekilerin katılımı bu kadar sayılır.
pub const BONUS_WEIGHT: i32 = 2;
pub const MAX_MIN_LEVEL: u32 = 500;
pub const MAX_ACCOUNT_DAYS: u32 = 3650;

/// Katılım şartları (hepsi isteğe bağlı) ve ek şans rolü.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Requirements {
    pub role: Option<RoleId>,
    pub min_level: u32,
    pub min_account_days: u32,
    pub bonus_role: Option<RoleId>,
}

impl Requirements {
    /// Çekiliş mesajında gösterilecek satırlar.
    fn describe(&self) -> String {
        let mut lines = Vec::new();
        if let Some(role) = self.role {
            lines.push(format!("🎭 Gereken rol: {}", role.mention()));
        }
        if self.min_level > 0 {
            lines.push(format!("⭐ En az seviye: **{}**", self.min_level));
        }
        if self.min_account_days > 0 {
            lines.push(format!("📅 Hesap en az **{}** günlük olmalı", self.min_account_days));
        }
        if let Some(role) = self.bonus_role {
            lines.push(format!("🍀 {} rolündekiler **{BONUS_WEIGHT} kat** şanslı", role.mention()));
        }
        if lines.is_empty() { String::new() } else { format!("\n\n**Şartlar**\n{}", lines.join("\n")) }
    }
}

/// Üye şartları karşılıyor mu? Karşılamıyorsa gösterilecek sebep.
pub fn unmet(req: &Requirements, roles: &[RoleId], level: u32, account_age_days: i64) -> Option<String> {
    if let Some(role) = req.role {
        if !roles.contains(&role) {
            return Some(format!("Bu çekilişe katılmak için {} rolün olmalı.", role.mention()));
        }
    }
    if level < req.min_level {
        return Some(format!("Bu çekilişe katılmak için en az **{}. seviye** olmalısın (şu an {level}).", req.min_level));
    }
    if account_age_days < i64::from(req.min_account_days) {
        return Some(format!("Bu çekilişe katılmak için hesabın en az **{}** günlük olmalı.", req.min_account_days));
    }
    None
}

pub struct NewGiveaway<'a> {
    pub guild_id: GuildId,
    pub channel_id: ChannelId,
    pub host: UserId,
    pub prize: &'a str,
    pub winners: u32,
    pub duration: Duration,
    pub requirements: Requirements,
}

fn role_col(role: Option<RoleId>) -> anyhow::Result<Option<i64>> {
    role.map(|r| snowflake(r.get())).transpose()
}

/// Çekilişi oluşturur, mesajını gönderir ve bitişini zamanlar. Çekiliş numarasını döner.
pub async fn create(http: &Http, db: &PgPool, scheduler: &Scheduler, g: NewGiveaway<'_>) -> anyhow::Result<i64> {
    let ends_unix = serenity::Timestamp::now().unix_timestamp() + i64::try_from(g.duration.as_secs())?;
    let ends_at = chrono::DateTime::<chrono::Utc>::from_timestamp(ends_unix, 0).ok_or_else(|| anyhow!("geçersiz süre"))?;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO giveaways (guild_id, channel_id, host_id, prize, winner_count, ends_at, required_role, min_level, min_account_days, bonus_role)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
    )
    .bind(snowflake(g.guild_id.get())?)
    .bind(snowflake(g.channel_id.get())?)
    .bind(snowflake(g.host.get())?)
    .bind(g.prize)
    .bind(i32::try_from(g.winners)?)
    .bind(ends_at)
    .bind(role_col(g.requirements.role)?)
    .bind(i32::try_from(g.requirements.min_level)?)
    .bind(i32::try_from(g.requirements.min_account_days)?)
    .bind(role_col(g.requirements.bonus_role)?)
    .fetch_one(db)
    .await?;

    let message = g
        .channel_id
        .send_message(
            http,
            CreateMessage::new()
                .embed(active_embed(id, g.prize, g.host, g.winners, ends_unix, &g.requirements))
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
    data: &Data,
    interaction: &ComponentInteraction,
) -> anyhow::Result<()> {
    let db = &data.db;
    let id: i64 = interaction
        .data
        .custom_id
        .strip_prefix(BUTTON_PREFIX)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("geçersiz çekiliş butonu"))?;
    let user = snowflake(interaction.user.id.get())?;

    type State = (bool, Option<i64>, i32, i32, Option<i64>);
    let state: Option<State> = sqlx::query_as(
        "SELECT ended OR ends_at <= now(), required_role, min_level, min_account_days, bonus_role FROM giveaways WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let text = match state {
        None => "Bu çekiliş artık yok.".to_owned(),
        Some((true, ..)) => "⏰ Bu çekiliş sona erdi.".to_owned(),
        Some((false, role, min_level, min_days, bonus)) => 'join: {
            let req = Requirements {
                role: to_id(role).map(RoleId::new),
                min_level: u32::try_from(min_level).unwrap_or(0),
                min_account_days: u32::try_from(min_days).unwrap_or(0),
                bonus_role: to_id(bonus).map(RoleId::new),
            };
            let roles: Vec<RoleId> = interaction.member.as_ref().map(|m| m.roles.clone()).unwrap_or_default();
            let already: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM giveaway_entries WHERE giveaway_id = $1 AND user_id = $2)")
                .bind(id)
                .bind(user)
                .fetch_one(db)
                .await?;
            // Ayrılmak için şart aranmaz.
            if !already {
                let level = match (req.min_level > 0, interaction.guild_id) {
                    (true, Some(guild)) => {
                        let rank = data.leveling.rank(db, &data.redis, guild, interaction.user.id).await?;
                        crate::leveling::formula::level(rank.total_xp)
                    }
                    _ => 0,
                };
                let age_days = (serenity::Timestamp::now().unix_timestamp() - interaction.user.id.created_at().unix_timestamp()) / 86_400;
                if let Some(reason) = unmet(&req, &roles, level, age_days) {
                    break 'join format!("❌ {reason}");
                }
            }
            let weight = if req.bonus_role.is_some_and(|r| roles.contains(&r)) { BONUS_WEIGHT } else { 1 };
            let joined = sqlx::query(
                "INSERT INTO giveaway_entries (giveaway_id, user_id, weight) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .bind(user)
            .bind(weight)
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
            if joined && weight > 1 {
                format!("🎉 Çekilişe katıldın — ek şans rolün sayesinde **{weight} kat** şanslısın! Toplam katılımcı: **{count}**")
            } else if joined {
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
    let entries: Vec<(i64, i32)> = sqlx::query_as("SELECT user_id, weight FROM giveaway_entries WHERE giveaway_id = $1")
        .bind(id)
        .fetch_all(db)
        .await?;
    let pool: Vec<(UserId, u64)> = entries
        .into_iter()
        .filter_map(|(u, w)| to_id(Some(u)).map(|u| (UserId::new(u), u64::try_from(w.max(1)).unwrap_or(1))))
        .filter(|(u, _)| !exclude.contains(u))
        .collect();
    Ok(weighted_pick(pool, count, rand_util::below))
}

/// Ağırlıklı, tekrarsız seçim. `below(n)`: 0..n arasında rastgele sayı (testte sabitlenir).
fn weighted_pick(mut pool: Vec<(UserId, u64)>, count: usize, below: impl Fn(u64) -> u64) -> Vec<UserId> {
    let mut picked = Vec::new();
    while picked.len() < count && !pool.is_empty() {
        let total: u64 = pool.iter().map(|(_, w)| *w).sum();
        let mut ticket = below(total.max(1));
        let index = pool
            .iter()
            .position(|(_, w)| {
                if ticket < *w {
                    true
                } else {
                    ticket -= *w;
                    false
                }
            })
            .unwrap_or(0);
        picked.push(pool.swap_remove(index).0);
    }
    picked
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

fn active_embed(id: i64, prize: &str, host: UserId, winners: u32, ends_unix: i64, req: &Requirements) -> CreateEmbed {
    CreateEmbed::new()
        .title(format!("🎉 {prize}"))
        .description(format!(
            "Katılmak için **Katıl** butonuna bas!\n\n\
             ⏰ Bitiş: <t:{ends_unix}:R> (<t:{ends_unix}:f>)\n\
             🏆 Kazanan sayısı: **{winners}**\n\
             👤 Düzenleyen: {}{}",
            host.mention(),
            req.describe()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: u64) -> UserId {
        UserId::new(id)
    }

    #[test]
    fn weighted_pick_is_distinct_and_respects_weights() {
        let pool = vec![(user(1), 1), (user(2), 2), (user(3), 1)];
        // Bilet 1 → 2. üyenin aralığı (0 = 1. üye, 1-2 = 2. üye, 3 = 3. üye).
        let picked = weighted_pick(pool.clone(), 1, |_| 1);
        assert_eq!(picked, vec![user(2)]);
        let all = weighted_pick(pool, 10, |n| n - 1);
        assert_eq!(all.len(), 3);
        let mut sorted: Vec<u64> = all.iter().map(|u| u.get()).collect();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![1, 2, 3], "tekrar eden kazanan olmamalı");
    }

    #[test]
    fn requirements_are_checked() {
        let role = RoleId::new(10);
        let req = Requirements { role: Some(role), min_level: 5, min_account_days: 30, bonus_role: None };
        assert!(unmet(&req, &[], 10, 100).is_some_and(|r| r.contains("rol")));
        assert!(unmet(&req, &[role], 4, 100).is_some_and(|r| r.contains("seviye")));
        assert!(unmet(&req, &[role], 5, 29).is_some_and(|r| r.contains("günlük")));
        assert_eq!(unmet(&req, &[role], 5, 30), None);
        assert_eq!(unmet(&Requirements::default(), &[], 0, 0), None);
    }
}
