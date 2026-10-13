//! Hayvan yakalama: ayarlanan kanalda her 10-20 mesajda bir hayvan belirir; butona ilk basan
//! yakalar, koleksiyonuna eklenir ve nadirliğine göre para kazanır.
//!
//! Mesaj sayaçları ve aktif hayvanlar bellektedir (mesaj başına veritabanı yok); sadece
//! yakalama anında veritabanına yazılır. Aynı hayvanı iki kişi yakalayamaz (DashMap'ten
//! tek seferlik çıkarma).

use std::time::{Duration, Instant};

use dashmap::DashMap;
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow, CreateButton,
    CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreateMessage, EditMessage, GuildId, Mentionable, MessageId, UserId,
};
use sqlx::PgPool;
use tracing::{debug, info, warn};

use super::content::{self, Animal};
use crate::data::Data;
use crate::db::snowflake;
use crate::guild_config::to_id;
use crate::{economy, rand_util};

pub const PREFIX: &str = "animal:";
const MIN_MESSAGES: u64 = 10;
const MAX_MESSAGES: u64 = 20;
/// Yakalanmayan hayvan bu süre sonra kaçar.
const ESCAPE_AFTER: Duration = Duration::from_secs(5 * 60);
/// İki hayvan arasında en az bu kadar süre geçer.
const MIN_GAP: Duration = Duration::from_secs(60);

struct Counter {
    count: u64,
    target: u64,
    last_spawn: Option<Instant>,
}

struct Spawn {
    id: u64,
    animal: &'static Animal,
    message: MessageId,
    appeared: Instant,
}

#[derive(Default)]
pub struct Animals {
    counters: DashMap<ChannelId, Counter>,
    active: DashMap<ChannelId, Spawn>,
}

fn new_target() -> u64 {
    MIN_MESSAGES + rand_util::below(MAX_MESSAGES - MIN_MESSAGES + 1)
}

impl Animals {
    /// Oyun kanalındaki her insan mesajında çağrılır.
    pub async fn on_message(&self, ctx: &serenity::Context, channel: ChannelId) {
        let due = {
            let mut counter = self.counters.entry(channel).or_insert_with(|| Counter { count: 0, target: new_target(), last_spawn: None });
            counter.count += 1;
            let gap_ok = counter.last_spawn.is_none_or(|t| t.elapsed() >= MIN_GAP);
            if counter.count >= counter.target && gap_ok {
                counter.count = 0;
                counter.target = new_target();
                counter.last_spawn = Some(Instant::now());
                true
            } else {
                false
            }
        };
        if !due {
            return;
        }
        // Önceki hayvan hâlâ bekliyorsa yenisi çıkmaz; süresi dolduysa kaçar.
        if let Some(old) = self.active.get(&channel).map(|s| (s.message, s.appeared, s.animal)) {
            if old.1.elapsed() < ESCAPE_AFTER {
                return;
            }
            self.active.remove(&channel);
            escaped(ctx, channel, old.0, old.2).await;
        }
        self.spawn(ctx, channel).await;
    }

    async fn spawn(&self, ctx: &serenity::Context, channel: ChannelId) {
        let animal = content::random_animal();
        let id = rand_util::random_u64();
        let (r, g, b) = animal.rarity.colour();
        let embed = CreateEmbed::new()
            .title(format!("{} Vahşi bir {} belirdi!", animal.emoji, animal.name))
            .description(format!("**{}** · Yakalayana **{}**\nİlk basan kapar!", animal.rarity.label(), economy::money(animal.rarity.reward())))
            .colour(serenity::Colour::from_rgb(r, g, b))
            .footer(CreateEmbedFooter::new("5 dakika içinde yakalanmazsa kaçar."));
        let button = CreateButton::new(format!("{PREFIX}catch:{id}")).label("Yakala!").emoji('🫳').style(ButtonStyle::Success);
        match channel
            .send_message(ctx, CreateMessage::new().embed(embed).components(vec![CreateActionRow::Buttons(vec![button])]))
            .await
        {
            Ok(message) => {
                self.active.insert(channel, Spawn { id, animal, message: message.id, appeared: Instant::now() });
            }
            Err(e) => debug!(channel = %channel, error = %e, "Hayvan gönderilemedi"),
        }
    }

    pub async fn handle_button(&self, ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
        let Some(guild_id) = interaction.guild_id else {
            return Ok(());
        };
        let Some(id) = interaction
            .data
            .custom_id
            .strip_prefix(PREFIX)
            .and_then(|s| s.strip_prefix("catch:"))
            .and_then(|s| s.parse::<u64>().ok())
        else {
            return Ok(());
        };
        let channel = interaction.channel_id;
        // Tek seferlik: sadece ilk tıklayan çıkarabilir.
        let caught = self
            .active
            .remove_if(&channel, |_, s| s.id == id && s.appeared.elapsed() < ESCAPE_AFTER)
            .map(|(_, s)| s.animal);
        let Some(animal) = caught else {
            interaction
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new().content("💨 Geç kaldın, bu hayvan yakalandı ya da kaçtı.").ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        };

        let user = interaction.user.id;
        let count = record_catch(&data.db, guild_id, user, animal.id).await?;
        let reward = animal.rarity.reward();
        if let Err(e) = economy::add(&data.db, guild_id, user, reward).await {
            warn!(error = ?e, "Yakalama ödülü verilemedi");
        }
        let (r, g, b) = animal.rarity.colour();
        let embed = CreateEmbed::new()
            .title(format!("{} {} yakalandı!", animal.emoji, animal.name))
            .description(format!(
                "{} yakaladı! **+{}**\nKoleksiyonunda {} tane {} var.",
                user.mention(),
                economy::money(reward),
                count,
                animal.name
            ))
            .colour(serenity::Colour::from_rgb(r, g, b));
        interaction
            .create_response(
                ctx,
                CreateInteractionResponse::UpdateMessage(CreateInteractionResponseMessage::new().embed(embed).components(Vec::new())),
            )
            .await?;
        info!(guild = %guild_id, user = %user, animal = animal.id, "Hayvan yakalandı");
        Ok(())
    }
}

async fn escaped(ctx: &serenity::Context, channel: ChannelId, message: MessageId, animal: &Animal) {
    let embed = CreateEmbed::new()
        .title(format!("{} {} kaçtı…", animal.emoji, animal.name))
        .description("Kimse yakalayamadı. Bir dahakine daha hızlı ol!")
        .colour(serenity::Colour::DARK_GREY);
    if let Err(e) = channel
        .edit_message(ctx, message, EditMessage::new().embed(embed).components(Vec::new()))
        .await
    {
        debug!(error = %e, "Kaçan hayvan mesajı güncellenemedi");
    }
}

/// Yakalamayı kaydeder; o hayvandan kaç tane olduğunu döner.
pub async fn record_catch(db: &PgPool, guild_id: GuildId, user: UserId, animal: &str) -> anyhow::Result<i32> {
    Ok(sqlx::query_scalar(
        "INSERT INTO animal_catches (guild_id, user_id, animal, count) VALUES ($1, $2, $3, 1)
         ON CONFLICT (guild_id, user_id, animal) DO UPDATE SET count = animal_catches.count + 1
         RETURNING count",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(user.get())?)
    .bind(animal)
    .fetch_one(db)
    .await?)
}

/// (hayvan kimliği, adet)
pub async fn collection(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<Vec<(String, i32)>> {
    Ok(sqlx::query_as("SELECT animal, count FROM animal_catches WHERE guild_id = $1 AND user_id = $2")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .fetch_all(db)
        .await?)
}

/// (üye, toplam yakalama, farklı tür sayısı)
pub async fn leaderboard(db: &PgPool, guild_id: GuildId, limit: i64) -> anyhow::Result<Vec<(UserId, i64, i64)>> {
    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT user_id, SUM(count)::BIGINT, COUNT(*) FROM animal_catches WHERE guild_id = $1
         GROUP BY user_id ORDER BY COUNT(*) DESC, SUM(count) DESC LIMIT $2",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().filter_map(|(u, total, unique)| to_id(Some(u)).map(|u| (UserId::new(u), total, unique))).collect())
}

/// Gerçek PostgreSQL (sahte sunucu ID'si 1). Elle: `cargo test live_animals_sql -- --ignored --nocapture`
#[cfg(test)]
mod live_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "gerçek PostgreSQL gerektirir"]
    async fn live_animals_sql() -> anyhow::Result<()> {
        let _ = dotenvy::dotenv();
        let config = crate::config::DatabaseConfig { url: std::env::var("DATABASE_URL")?, max_connections: 2 };
        let db = crate::db::connect(&config).await?;
        let guild = GuildId::new(1);
        sqlx::query("DELETE FROM animal_catches WHERE guild_id = 1").execute(&db).await?;
        assert_eq!(record_catch(&db, guild, UserId::new(1), "kedi").await?, 1);
        assert_eq!(record_catch(&db, guild, UserId::new(1), "kedi").await?, 2);
        record_catch(&db, guild, UserId::new(1), "tilki").await?;
        record_catch(&db, guild, UserId::new(2), "ejderha").await?;
        let mut mine = collection(&db, guild, UserId::new(1)).await?;
        mine.sort();
        assert_eq!(mine, vec![("kedi".to_owned(), 2), ("tilki".to_owned(), 1)]);
        let top = leaderboard(&db, guild, 10).await?;
        assert_eq!(top.first().map(|(u, total, unique)| (u.get(), *total, *unique)), Some((1, 3, 2)));
        sqlx::query("DELETE FROM animal_catches WHERE guild_id = 1").execute(&db).await?;
        Ok(())
    }
}
