//! Doğum günleri: üyeler gün/ay kaydeder; o gün (Türkiye saatiyle) kanalda kutlanır ve
//! isteğe bağlı doğum günü rolü verilir, ertesi gün geri alınır.
//!
//! Arka plan görevi 15 dakikada bir çalışır; her kutlama yıl başına bir kez yapılır
//! (`last_announced`), bu yüzden bot o gün geç açılsa bile kutlama kaçmaz.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, Utc};
use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateAllowedMentions, CreateEmbed, CreateMessage, GuildId,
    Mentionable, RoleId, UserId,
};
use sqlx::PgPool;
use tracing::{debug, info, warn};

use crate::db::snowflake;
use crate::guild_config::to_id;

const CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// Türkiye saati (UTC+3, yaz saati yok).
const TURKEY_OFFSET_SECS: i32 = 3 * 3600;

pub const MONTHS: [&str; 12] = [
    "Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık",
];

pub fn today() -> NaiveDate {
    (Utc::now() + chrono::Duration::seconds(i64::from(TURKEY_OFFSET_SECS))).date_naive()
}

/// 29 Şubat da geçerlidir (artık olmayan yıllarda 28 Şubat'ta kutlanır).
pub fn is_valid(month: u32, day: u32) -> bool {
    NaiveDate::from_ymd_opt(2024, month, day).is_some()
}

pub fn format(month: u32, day: u32) -> String {
    let name = MONTHS.get(month.saturating_sub(1) as usize).copied().unwrap_or("?");
    format!("{day} {name}")
}

/// Bugünden itibaren kaç gün sonra (bugünse 0).
pub fn days_until(today: NaiveDate, month: u32, day: u32) -> i64 {
    let this_year = celebration_date(today.year(), month, day);
    let next = match this_year {
        Some(d) if d >= today => Some(d),
        _ => celebration_date(today.year() + 1, month, day),
    };
    next.map_or(i64::MAX, |d| (d - today).num_days())
}

/// O yıl kutlanacağı gün (29 Şubat → artık olmayan yılda 28 Şubat).
fn celebration_date(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, day).or_else(|| {
        (month == 2 && day == 29)
            .then(|| NaiveDate::from_ymd_opt(year, 2, 28))
            .flatten()
    })
}

/// Sunucunun doğum günü ayarları (kanal ve rol `guild_settings` tablosundadır).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct BirthdaySettings {
    /// Kutlamanın yapılacağı saat (Türkiye saati, 0-23).
    pub hour: u8,
    /// Boşsa `DEFAULT_MESSAGE`. Değişken: `{kullanıcı}` (bugün doğanların etiketleri).
    pub message: Option<String>,
    /// Doğum yılını yazmış üyeler için; değişkenler `{kullanıcı}` ve `{yaş}`.
    pub age_message: Option<String>,
    /// Üyeler doğum yılını da kaydedebilir.
    pub allow_year: bool,
}

impl crate::settings::PluginConfig for BirthdaySettings {
    const KEY: &'static str = "birthdays";
}

pub const DEFAULT_MESSAGE: &str = "Bugün {kullanıcı} doğdu! 🎉\nİyi ki doğdun, nice mutlu yıllara! 🥳🎈";
pub const DEFAULT_AGE_MESSAGE: &str = "{kullanıcı} bugün **{yaş}** yaşına girdi! 🎂";

/// Kutlama metni: yılı bilinenler için yaşlı satır, diğerleri için ortak mesaj.
pub fn celebration_text(settings: &BirthdaySettings, people: &[(UserId, Option<i32>)], this_year: i32) -> String {
    let pick = |custom: &Option<String>, fallback: &'static str| custom.as_deref().filter(|m| !m.trim().is_empty()).unwrap_or(fallback).to_owned();
    let mut lines = Vec::new();
    let plain: Vec<String> = people
        .iter()
        .filter(|(_, year)| !settings.allow_year || year.is_none())
        .map(|(user, _)| user.mention().to_string())
        .collect();
    if !plain.is_empty() {
        lines.push(pick(&settings.message, DEFAULT_MESSAGE).replace("{kullanıcı}", &plain.join(", ")).replace("{kullanici}", &plain.join(", ")));
    }
    if settings.allow_year {
        for (user, year) in people {
            if let Some(age) = year.map(|y| this_year - y).filter(|age| (1..=120).contains(age)) {
                lines.push(
                    pick(&settings.age_message, DEFAULT_AGE_MESSAGE)
                        .replace("{kullanıcı}", &user.mention().to_string())
                        .replace("{kullanici}", &user.mention().to_string())
                        .replace("{yaş}", &age.to_string())
                        .replace("{yas}", &age.to_string()),
                );
            } else if year.is_some() {
                // Yıl saçmaysa (gelecek, 120+) yaşsız kutlanır.
                lines.push(pick(&settings.message, DEFAULT_MESSAGE).replace("{kullanıcı}", &user.mention().to_string()).replace("{kullanici}", &user.mention().to_string()));
            }
        }
    }
    lines.join("\n")
}

/// Türkiye saatiyle şu anki saat (0-23).
pub fn current_hour() -> u32 {
    use chrono::Timelike as _;
    match chrono::FixedOffset::east_opt(TURKEY_OFFSET_SECS) {
        Some(tr) => chrono::Utc::now().with_timezone(&tr).hour(),
        None => chrono::Utc::now().hour(),
    }
}

pub async fn set(db: &PgPool, guild_id: GuildId, user: UserId, month: u32, day: u32, year: Option<i32>) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO birthdays (guild_id, user_id, month, day, year) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (guild_id, user_id) DO UPDATE SET month = EXCLUDED.month, day = EXCLUDED.day, year = EXCLUDED.year",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(user.get())?)
    .bind(i16::try_from(month)?)
    .bind(i16::try_from(day)?)
    .bind(year.map(i16::try_from).transpose()?)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn remove(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM birthdays WHERE guild_id = $1 AND user_id = $2")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Sunucudaki tüm doğum günleri: (üye, ay, gün).
pub async fn all(db: &PgPool, guild_id: GuildId) -> anyhow::Result<Vec<(UserId, u32, u32)>> {
    let rows: Vec<(i64, i16, i16)> = sqlx::query_as("SELECT user_id, month, day FROM birthdays WHERE guild_id = $1")
        .bind(snowflake(guild_id.get())?)
        .fetch_all(db)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(user, month, day)| {
            Some((UserId::new(to_id(Some(user))?), u32::try_from(month).ok()?, u32::try_from(day).ok()?))
        })
        .collect())
}

pub fn spawn_checker(ctx: serenity::Context, db: PgPool, settings: crate::settings::PluginSettings) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CHECK_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            crate::supervise::round("doğum günleri", std::time::Duration::from_secs(300), async {
                if let Err(e) = check(&ctx, &db, &settings).await {
                    warn!(error = ?e, "Doğum günü kontrolü başarısız");
                }
            })
            .await;
        }
    });
}

/// Sunucunun kutlama kanalı, rolü ve bugün doğanlar (doğum yılıyla).
type GuildBirthdays = (Option<ChannelId>, Option<RoleId>, Vec<(UserId, Option<i32>)>);

#[derive(sqlx::FromRow)]
struct DueRow {
    guild_id: i64,
    user_id: i64,
    birthday_channel_id: Option<i64>,
    birthday_role_id: Option<i64>,
    year: Option<i16>,
}

async fn check(ctx: &serenity::Context, db: &PgPool, settings: &crate::settings::PluginSettings) -> anyhow::Result<()> {
    let today = today();
    remove_expired_roles(ctx, db, today).await?;

    // 28 Şubat artık yıl değilse 29 Şubat doğumlular da bugün kutlanır.
    let leap_extra = today.month() == 2 && today.day() == 28 && NaiveDate::from_ymd_opt(today.year(), 2, 29).is_none();
    let due: Vec<DueRow> = sqlx::query_as(
        "SELECT b.guild_id, b.user_id, g.birthday_channel_id, g.birthday_role_id, b.year
         FROM birthdays b JOIN guild_settings g ON g.guild_id = b.guild_id
         WHERE ((b.month = $1 AND b.day = $2) OR ($4 AND b.month = 2 AND b.day = 29))
           AND (b.last_announced IS NULL OR b.last_announced < $3)
           AND (g.birthday_channel_id IS NOT NULL OR g.birthday_role_id IS NOT NULL)",
    )
    .bind(i16::try_from(today.month())?)
    .bind(i16::try_from(today.day())?)
    .bind(today.year())
    .bind(leap_extra)
    .fetch_all(db)
    .await?;

    // Sunucu başına tek kutlama mesajı.
    let mut by_guild: BTreeMap<u64, GuildBirthdays> = BTreeMap::new();
    for row in &due {
        let (Some(guild), Some(user)) = (to_id(Some(row.guild_id)), to_id(Some(row.user_id))) else {
            continue;
        };
        let entry = by_guild.entry(guild).or_insert_with(|| {
            (
                to_id(row.birthday_channel_id).map(ChannelId::new),
                to_id(row.birthday_role_id).map(RoleId::new),
                Vec::new(),
            )
        });
        entry.2.push((UserId::new(user), row.year.map(i32::from)));
    }

    let hour = current_hour();
    for (guild, (channel, role, people)) in by_guild {
        let guild_id = GuildId::new(guild);
        let guild_settings = settings.get::<BirthdaySettings>(db, guild_id).await.unwrap_or_default();
        // Kutlama saati gelmediyse bu sunucu bir sonraki kontrolde yeniden denenir.
        if hour < u32::from(guild_settings.hour) {
            continue;
        }
        let users: Vec<UserId> = people.iter().map(|(user, _)| *user).collect();
        // Sunucudan ayrılmış üyeler kutlanmaz.
        let present: Vec<UserId> = match ctx.cache.guild(guild_id) {
            Some(g) => users.iter().copied().filter(|u| g.members.contains_key(u)).collect(),
            None => Vec::new(),
        };
        for user in &users {
            sqlx::query(
                "UPDATE birthdays SET last_announced = $3, role_given_on = CASE WHEN $4 THEN $5 ELSE role_given_on END
                 WHERE guild_id = $1 AND user_id = $2",
            )
            .bind(snowflake(guild)?)
            .bind(snowflake(user.get())?)
            .bind(today.year())
            .bind(role.is_some() && present.contains(user))
            .bind(today)
            .execute(db)
            .await?;
        }
        if present.is_empty() {
            continue;
        }
        if let Some(role) = role {
            for user in &present {
                if let Err(e) = ctx.http.add_member_role(guild_id, *user, role, Some("Doğum günü")).await {
                    warn!(guild = %guild_id, role = %role, error = %e, "Doğum günü rolü verilemedi");
                }
            }
        }
        if let Some(channel) = channel {
            let mentions = present.iter().map(|u| u.mention().to_string()).collect::<Vec<_>>().join(", ");
            let embed = CreateEmbed::new()
                .title("🎂 Doğum günü kutlaması!")
                .description(celebration_text(
                    &guild_settings,
                    &people.iter().copied().filter(|(user, _)| present.contains(user)).collect::<Vec<_>>(),
                    today.year(),
                ))
                .colour(serenity::Colour::from_rgb(255, 115, 250));
            let message = CreateMessage::new()
                .content(mentions)
                .embed(embed)
                .allowed_mentions(CreateAllowedMentions::new().users(present.clone()));
            if let Err(e) = channel.send_message(&ctx.http, message).await {
                warn!(channel = %channel, error = %e, "Doğum günü mesajı gönderilemedi");
            }
        }
        info!(guild = %guild_id, count = present.len(), "Doğum günü kutlandı");
    }
    Ok(())
}

/// Dünden kalan doğum günü rollerini geri alır.
async fn remove_expired_roles(ctx: &serenity::Context, db: &PgPool, today: NaiveDate) -> anyhow::Result<()> {
    let rows: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT b.guild_id, b.user_id, g.birthday_role_id
         FROM birthdays b JOIN guild_settings g ON g.guild_id = b.guild_id
         WHERE b.role_given_on IS NOT NULL AND b.role_given_on < $1",
    )
    .bind(today)
    .fetch_all(db)
    .await?;
    for (guild, user, role) in rows {
        if let (Some(g), Some(u), Some(r)) = (to_id(Some(guild)), to_id(Some(user)), to_id(role)) {
            if let Err(e) = ctx
                .http
                .remove_member_role(GuildId::new(g), UserId::new(u), RoleId::new(r), Some("Doğum günü bitti"))
                .await
            {
                debug!(error = %e, "Doğum günü rolü geri alınamadı (üye ayrılmış olabilir)");
            }
        }
        sqlx::query("UPDATE birthdays SET role_given_on = NULL WHERE guild_id = $1 AND user_id = $2")
            .bind(guild)
            .bind(user)
            .execute(db)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> anyhow::Result<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, d).ok_or_else(|| anyhow::anyhow!("geçersiz tarih"))
    }

    #[test]
    fn celebration_text_uses_age_only_when_allowed() {
        let people = [(UserId::new(1), Some(2000)), (UserId::new(2), None)];
        let plain = celebration_text(&BirthdaySettings::default(), &people, 2026);
        assert!(plain.contains("<@1>, <@2>") && !plain.contains("26"));
        let with_age = BirthdaySettings { allow_year: true, message: Some("Kutlu olsun {kullanıcı}".to_owned()), ..BirthdaySettings::default() };
        let text = celebration_text(&with_age, &people, 2026);
        assert_eq!(text, "Kutlu olsun <@2>\n<@1> bugün **26** yaşına girdi! 🎂");
        // Saçma yıl (gelecek) yaşsız kutlanır.
        let odd = celebration_text(&with_age, &[(UserId::new(3), Some(2090))], 2026);
        assert_eq!(odd, "Kutlu olsun <@3>");
    }

    #[test]
    fn validates_dates() {
        assert!(is_valid(2, 29));
        assert!(is_valid(12, 31));
        assert!(!is_valid(2, 30));
        assert!(!is_valid(13, 1));
        assert!(!is_valid(4, 31));
    }

    #[test]
    fn counts_days_until_birthday() -> anyhow::Result<()> {
        let today = date(2026, 10, 8)?;
        assert_eq!(days_until(today, 10, 8), 0);
        assert_eq!(days_until(today, 10, 9), 1);
        assert_eq!(days_until(today, 10, 7), 364);
        // 2027 artık yıl değil → 28 Şubat'ta kutlanır.
        assert_eq!(days_until(today, 2, 29), (date(2027, 2, 28)? - today).num_days());
        Ok(())
    }

    #[test]
    fn formats_in_turkish() {
        assert_eq!(format(10, 8), "8 Ekim");
        assert_eq!(format(2, 29), "29 Şubat");
    }
}
