//! `/istatistik` — Sunucu ve üye analitikleri: mesaj, ses ve sıralama verileri.

use anyhow::anyhow;
use poise::serenity_prelude::{self as serenity, CreateEmbed, CreateEmbedFooter, GuildId};
use poise::CreateReply;

use crate::data::Context;
use crate::db::snowflake;
use crate::error::Error;

fn guild_id(ctx: Context<'_>) -> Result<GuildId, Error> {
    ctx.guild_id().ok_or_else(|| anyhow!("Bu komut yalnızca sunucularda kullanılabilir"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum PeriodDays {
    #[name = "Son 7 gün"]
    Seven,
    #[name = "Son 14 gün"]
    Fourteen,
    #[name = "Son 30 gün"]
    Thirty,
}

impl PeriodDays {
    #[must_use]
    pub fn days(self) -> i64 {
        match self {
            Self::Seven => 7,
            Self::Fourteen => 14,
            Self::Thirty => 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum RankingType {
    #[name = "Mesaj"]
    Messages,
    #[name = "Ses"]
    Voice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum RankingPeriod {
    #[name = "Son 7 gün"]
    Seven,
    #[name = "Son 30 gün"]
    Thirty,
}

impl RankingPeriod {
    #[must_use]
    pub fn days(self) -> i64 {
        match self {
            Self::Seven => 7,
            Self::Thirty => 30,
        }
    }
}

#[must_use]
pub fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let mins = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}s {mins}dk")
    } else {
        format!("{mins}dk")
    }
}

/// Sunucunun seçili dönemdeki genel istatistiklerini (mesaj, ses, üye değişimi, kanallar) gösterir.
#[poise::command(slash_command, guild_only, category = "İstatistik")]
pub async fn genel(
    ctx: Context<'_>,
    #[description = "İstatistik zaman aralığı (varsayılan: 7 gün)"]
    #[rename = "gün"]
    gun: Option<PeriodDays>,
) -> Result<(), Error> {
    ctx.defer().await?;
    let guild_id = guild_id(ctx)?;
    let days = gun.unwrap_or(PeriodDays::Seven).days();
    let data = ctx.data();

    let (messages, voice_sec, joined, left): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(messages_count), 0)::BIGINT,
                COALESCE(SUM(voice_seconds), 0)::BIGINT,
                COALESCE(SUM(members_joined), 0)::BIGINT,
                COALESCE(SUM(members_left), 0)::BIGINT
         FROM server_daily_stats
         WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(i32::try_from(days).unwrap_or(7))
    .fetch_one(&data.db)
    .await?;

    let grid = crate::community::heatmap(&data.redis, guild_id).await;
    let mut hour_totals = [0_u64; 24];
    for day_row in &grid {
        for (hour, &count) in day_row.iter().enumerate() {
            hour_totals[hour] += count;
        }
    }
    let (peak_hour, peak_count) = hour_totals
        .iter()
        .enumerate()
        .max_by_key(|&(_, &count)| count)
        .map(|(hour, &count)| (hour, count))
        .unwrap_or((0, 0));
    let peak_str = if peak_count > 0 {
        format!("{:02}:00 - {:02}:00 ({} mesaj)", peak_hour, (peak_hour + 1) % 24, peak_count)
    } else {
        "Veri yok".to_owned()
    };

    let text_channels: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT channel_id, COALESCE(SUM(messages_count), 0)::BIGINT AS total
         FROM channel_daily_stats
         WHERE guild_id = $1 AND is_voice = FALSE AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)
         GROUP BY channel_id
         HAVING SUM(messages_count) > 0
         ORDER BY total DESC
         LIMIT 3",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(i32::try_from(days).unwrap_or(7))
    .fetch_all(&data.db)
    .await?;

    let voice_channels: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT channel_id, COALESCE(SUM(voice_seconds), 0)::BIGINT AS total
         FROM channel_daily_stats
         WHERE guild_id = $1 AND is_voice = TRUE AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)
         GROUP BY channel_id
         HAVING SUM(voice_seconds) > 0
         ORDER BY total DESC
         LIMIT 3",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(i32::try_from(days).unwrap_or(7))
    .fetch_all(&data.db)
    .await?;

    let text_str = if text_channels.is_empty() {
        "—".to_owned()
    } else {
        text_channels
            .into_iter()
            .enumerate()
            .map(|(i, (id, count))| {
                let medal = ["🥇", "🥈", "🥉"].get(i).copied().unwrap_or("•");
                format!("{medal} <#{id}> — **{count}** mesaj")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let voice_str = if voice_channels.is_empty() {
        "—".to_owned()
    } else {
        voice_channels
            .into_iter()
            .enumerate()
            .map(|(i, (id, sec))| {
                let medal = ["🥇", "🥈", "🥉"].get(i).copied().unwrap_or("•");
                format!("{medal} <#{id}> — **{}**", format_duration(u64::try_from(sec.max(0)).unwrap_or(0)))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let net_growth = joined - left;
    let net_sign = if net_growth > 0 { format!("+{net_growth}") } else { net_growth.to_string() };
    let guild_name = ctx.guild().map(|g| g.name.clone()).unwrap_or_else(|| "Sunucu".to_owned());
    let lang = crate::i18n::resolve_discord_lang(&ctx);
    let title_text = crate::i18n::t(lang, "analytics.overview_title", &[]);
    let msg_title = crate::i18n::t(lang, "analytics.messages_count", &[("count", &messages.to_string())]);

    let embed = CreateEmbed::new()
        .title(format!("📊 {guild_name} — {title_text} (Son {days} Gün)"))
        .colour(serenity::Colour::BLURPLE)
        .field("💬 Toplam Mesaj", messages.to_string(), true)
        .field("🎙️ Toplam Ses", format_duration(u64::try_from(voice_sec.max(0)).unwrap_or(0)), true)
        .field("👥 Üye Akışı", format!("+{joined} / -{left} (Net: **{net_sign}**)"), true)
        .field("⏰ En Yoğun Saat", peak_str, false)
        .field("📝 En Aktif Metin Kanalları", text_str, false)
        .field("🔊 En Aktif Ses Kanalları", voice_str, false)
        .footer(CreateEmbedFooter::new(format!("{msg_title} · Veriler son {days} günlüktür")));

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Hedef üyenin veya tüm sunucunun mesaj istatistiklerini gösterir.
#[poise::command(slash_command, guild_only, category = "İstatistik")]
pub async fn mesaj(
    ctx: Context<'_>,
    #[description = "Hedef kullanıcı (belirtilmezse tüm sunucu)"]
    #[rename = "kullanıcı"]
    kullanici: Option<serenity::User>,
    #[description = "İstatistik zaman aralığı (varsayılan: 7 gün)"]
    #[rename = "gün"]
    gun: Option<PeriodDays>,
) -> Result<(), Error> {
    ctx.defer().await?;
    let guild_id = guild_id(ctx)?;
    let days = gun.unwrap_or(PeriodDays::Seven).days();
    let data = ctx.data();

    let embed = if let Some(user) = kullanici {
        let (total_msgs, active_days): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(messages_count), 0)::BIGINT,
                    COUNT(DISTINCT stat_date) FILTER (WHERE messages_count > 0)::BIGINT
             FROM member_daily_stats
             WHERE guild_id = $1 AND user_id = $2 AND stat_date >= CURRENT_DATE - make_interval(days => $3::int)",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.id.get())?)
        .bind(i32::try_from(days).unwrap_or(7))
        .fetch_one(&data.db)
        .await?;

        let daily_avg = total_msgs as f64 / days as f64;
        let name = user.global_name.as_deref().unwrap_or(&user.name);

        CreateEmbed::new()
            .author(serenity::CreateEmbedAuthor::new(user.tag()).icon_url(user.face()))
            .title(format!("💬 {name} — Mesaj İstatistikleri (Son {days} Gün)"))
            .colour(serenity::Colour::BLURPLE)
            .field("Toplam Mesaj", total_msgs.to_string(), true)
            .field("Günlük Ortalama", format!("{daily_avg:.1}"), true)
            .field("Aktif Gün Sayısı", format!("{active_days} / {days}"), true)
            .footer(CreateEmbedFooter::new(format!("Son {days} günlük veriler")))
    } else {
        let (total_msgs,): (i64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(messages_count), 0)::BIGINT
             FROM server_daily_stats
             WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(i32::try_from(days).unwrap_or(7))
        .fetch_one(&data.db)
        .await?;

        let daily_avg = total_msgs as f64 / days as f64;

        let top_channels: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT channel_id, COALESCE(SUM(messages_count), 0)::BIGINT AS total
             FROM channel_daily_stats
             WHERE guild_id = $1 AND is_voice = FALSE AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)
             GROUP BY channel_id
             HAVING SUM(messages_count) > 0
             ORDER BY total DESC
             LIMIT 5",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(i32::try_from(days).unwrap_or(7))
        .fetch_all(&data.db)
        .await?;

        let channels_str = if top_channels.is_empty() {
            "—".to_owned()
        } else {
            top_channels
                .into_iter()
                .enumerate()
                .map(|(i, (id, count))| {
                    let medal = ["🥇", "🥈", "🥉"].get(i).copied().unwrap_or("•");
                    format!("{medal} <#{id}> — **{count}** mesaj")
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let guild_name = ctx.guild().map(|g| g.name.clone()).unwrap_or_else(|| "Sunucu".to_owned());

        CreateEmbed::new()
            .title(format!("💬 {guild_name} — Mesaj İstatistikleri (Son {days} Gün)"))
            .colour(serenity::Colour::BLURPLE)
            .field("Toplam Mesaj", total_msgs.to_string(), true)
            .field("Günlük Ortalama", format!("{daily_avg:.1}"), true)
            .field("En Aktif 5 Metin Kanalı", channels_str, false)
            .footer(CreateEmbedFooter::new(format!("Son {days} günlük veriler")))
    };

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Hedef üyenin veya tüm sunucunun ses istatistiklerini ve ses XP'sini gösterir.
#[poise::command(slash_command, guild_only, category = "İstatistik")]
pub async fn ses(
    ctx: Context<'_>,
    #[description = "Hedef kullanıcı (belirtilmezse tüm sunucu)"]
    #[rename = "kullanıcı"]
    kullanici: Option<serenity::User>,
    #[description = "İstatistik zaman aralığı (varsayılan: 7 gün)"]
    #[rename = "gün"]
    gun: Option<PeriodDays>,
) -> Result<(), Error> {
    ctx.defer().await?;
    let guild_id = guild_id(ctx)?;
    let days = gun.unwrap_or(PeriodDays::Seven).days();
    let data = ctx.data();

    let embed = if let Some(user) = kullanici {
        let (period_sec, period_xp): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(voice_seconds), 0)::BIGINT,
                    COALESCE(SUM(voice_xp_earned), 0)::BIGINT
             FROM member_daily_stats
             WHERE guild_id = $1 AND user_id = $2 AND stat_date >= CURRENT_DATE - make_interval(days => $3::int)",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.id.get())?)
        .bind(i32::try_from(days).unwrap_or(7))
        .fetch_one(&data.db)
        .await?;

        let lifetime: Option<(i64, i64)> = sqlx::query_as(
            "SELECT total_voice_seconds, total_voice_xp
             FROM member_voice_stats
             WHERE guild_id = $1 AND user_id = $2",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.id.get())?)
        .fetch_optional(&data.db)
        .await?;

        let (total_sec, total_xp) = lifetime.unwrap_or((0, 0));
        let name = user.global_name.as_deref().unwrap_or(&user.name);

        CreateEmbed::new()
            .author(serenity::CreateEmbedAuthor::new(user.tag()).icon_url(user.face()))
            .title(format!("🎙️ {name} — Ses İstatistikleri (Son {days} Gün)"))
            .colour(serenity::Colour::BLURPLE)
            .field("Dönemlik Ses Süresi", format_duration(u64::try_from(period_sec.max(0)).unwrap_or(0)), true)
            .field("Dönemlik Ses XP'si", format!("{period_xp} XP"), true)
            .field("Toplam Ses Süresi", format_duration(u64::try_from(total_sec.max(0)).unwrap_or(0)), true)
            .field("Toplam Ses XP'si", format!("{total_xp} XP"), true)
            .footer(CreateEmbedFooter::new(format!("Dönemlik veriler son {days} günlüktür")))
    } else {
        let (total_sec,): (i64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(voice_seconds), 0)::BIGINT
             FROM server_daily_stats
             WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(i32::try_from(days).unwrap_or(7))
        .fetch_one(&data.db)
        .await?;

        let (total_xp,): (i64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(voice_xp_earned), 0)::BIGINT
             FROM member_daily_stats
             WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(i32::try_from(days).unwrap_or(7))
        .fetch_one(&data.db)
        .await?;

        let top_channels: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT channel_id, COALESCE(SUM(voice_seconds), 0)::BIGINT AS total
             FROM channel_daily_stats
             WHERE guild_id = $1 AND is_voice = TRUE AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)
             GROUP BY channel_id
             HAVING SUM(voice_seconds) > 0
             ORDER BY total DESC
             LIMIT 5",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(i32::try_from(days).unwrap_or(7))
        .fetch_all(&data.db)
        .await?;

        let channels_str = if top_channels.is_empty() {
            "—".to_owned()
        } else {
            top_channels
                .into_iter()
                .enumerate()
                .map(|(i, (id, sec))| {
                    let medal = ["🥇", "🥈", "🥉"].get(i).copied().unwrap_or("•");
                    format!("{medal} <#{id}> — **{}**", format_duration(u64::try_from(sec.max(0)).unwrap_or(0)))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let guild_name = ctx.guild().map(|g| g.name.clone()).unwrap_or_else(|| "Sunucu".to_owned());

        CreateEmbed::new()
            .title(format!("🎙️ {guild_name} — Ses İstatistikleri (Son {days} Gün)"))
            .colour(serenity::Colour::BLURPLE)
            .field("Toplam Ses Süresi", format_duration(u64::try_from(total_sec.max(0)).unwrap_or(0)), true)
            .field("Kazanılan Ses XP'si", format!("{total_xp} XP"), true)
            .field("En Aktif 5 Ses Kanalı", channels_str, false)
            .footer(CreateEmbedFooter::new(format!("Son {days} günlük veriler")))
    };

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Seçili dönemde en aktif ilk 10 üyeyi (mesaj veya ses) listeler.
#[poise::command(slash_command, guild_only, rename = "sıralama", category = "İstatistik")]
pub async fn siralama(
    ctx: Context<'_>,
    #[description = "Sıralama ölçütü (Mesaj veya Ses)"]
    #[rename = "tür"]
    tur: RankingType,
    #[description = "İstatistik zaman aralığı (varsayılan: 7 gün)"]
    #[rename = "gün"]
    gun: Option<RankingPeriod>,
) -> Result<(), Error> {
    ctx.defer().await?;
    let guild_id = guild_id(ctx)?;
    let days = gun.unwrap_or(RankingPeriod::Seven).days();
    let data = ctx.data();

    let (title, description) = match tur {
        RankingType::Messages => {
            let rows: Vec<(i64, i64)> = sqlx::query_as(
                "SELECT user_id, COALESCE(SUM(messages_count), 0)::BIGINT AS total
                 FROM member_daily_stats
                 WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)
                 GROUP BY user_id
                 HAVING SUM(messages_count) > 0
                 ORDER BY total DESC
                 LIMIT 10",
            )
            .bind(snowflake(guild_id.get())?)
            .bind(i32::try_from(days).unwrap_or(7))
            .fetch_all(&data.db)
            .await?;

            let desc = if rows.is_empty() {
                "Bu dönemde kayıtlı mesaj bulunmuyor.".to_owned()
            } else {
                rows.into_iter()
                    .enumerate()
                    .map(|(i, (user_id, count))| {
                        let medal = ["🥇", "🥈", "🥉"].get(i).copied().unwrap_or("•");
                        let user_id = u64::try_from(user_id).unwrap_or(0);
                        format!("{medal} <@{user_id}> — **{count}** mesaj")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            ("💬 En Çok Mesaj Yazanlar", desc)
        }
        RankingType::Voice => {
            let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
                "SELECT user_id,
                        COALESCE(SUM(voice_seconds), 0)::BIGINT AS total_sec,
                        COALESCE(SUM(voice_xp_earned), 0)::BIGINT AS total_xp
                 FROM member_daily_stats
                 WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)
                 GROUP BY user_id
                 HAVING SUM(voice_seconds) > 0
                 ORDER BY total_sec DESC
                 LIMIT 10",
            )
            .bind(snowflake(guild_id.get())?)
            .bind(i32::try_from(days).unwrap_or(7))
            .fetch_all(&data.db)
            .await?;

            let desc = if rows.is_empty() {
                "Bu dönemde kayıtlı ses etkinliği bulunmuyor.".to_owned()
            } else {
                rows.into_iter()
                    .enumerate()
                    .map(|(i, (user_id, sec, xp))| {
                        let medal = ["🥇", "🥈", "🥉"].get(i).copied().unwrap_or("•");
                        let user_id = u64::try_from(user_id).unwrap_or(0);
                        let dur = format_duration(u64::try_from(sec.max(0)).unwrap_or(0));
                        format!("{medal} <@{user_id}> — **{dur}** • {xp} XP")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            ("🎙️ En Çok Seste Duranlar", desc)
        }
    };

    let guild_name = ctx.guild().map(|g| g.name.clone()).unwrap_or_else(|| "Sunucu".to_owned());
    let embed = CreateEmbed::new()
        .title(format!("{title} — {guild_name} (Son {days} Gün)"))
        .description(description)
        .colour(serenity::Colour::GOLD)
        .footer(CreateEmbedFooter::new(format!("Sıralama son {days} günlüktür")));

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Sunucu ve üye analitikleri: mesaj, ses ve sıralama verileri.
#[poise::command(
    slash_command,
    guild_only,
    category = "İstatistik",
    subcommands("genel", "mesaj", "ses", "siralama")
)]
pub async fn istatistik(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_days_values_and_bounds() {
        assert_eq!(PeriodDays::Seven.days(), 7);
        assert_eq!(PeriodDays::Fourteen.days(), 14);
        assert_eq!(PeriodDays::Thirty.days(), 30);
    }

    #[test]
    fn ranking_period_values_and_bounds() {
        assert_eq!(RankingPeriod::Seven.days(), 7);
        assert_eq!(RankingPeriod::Thirty.days(), 30);
    }

    #[test]
    fn format_duration_formats_properly() {
        assert_eq!(format_duration(0), "0dk");
        assert_eq!(format_duration(59), "0dk");
        assert_eq!(format_duration(60), "1dk");
        assert_eq!(format_duration(3599), "59dk");
        assert_eq!(format_duration(3600), "1s 0dk");
        assert_eq!(format_duration(3665), "1s 1dk");
        assert_eq!(format_duration(45600), "12s 40dk");
    }
}
