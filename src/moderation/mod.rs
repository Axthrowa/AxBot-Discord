//! Moderasyon işlemleri: vaka kaydı (PostgreSQL), rol hiyerarşisi kontrolü ve süre ayrıştırma.

pub mod auto;
pub mod duration;
pub mod hierarchy;

use std::time::Duration;

use poise::serenity_prelude::{Colour, GuildId, UserId};
use sqlx::PgPool;

use crate::db::snowflake;

/// Discord'un denetim kaydı (audit log) sebep sınırı.
pub const MAX_REASON_LEN: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModAction {
    Ban,
    Unban,
    Kick,
    Timeout,
    Untimeout,
    Warn,
    Quarantine,
    Unquarantine,
}

impl ModAction {
    /// Veritabanında saklanan değer.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ban => "ban",
            Self::Unban => "unban",
            Self::Kick => "kick",
            Self::Timeout => "timeout",
            Self::Untimeout => "untimeout",
            Self::Warn => "warn",
            Self::Quarantine => "quarantine",
            Self::Unquarantine => "unquarantine",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Ban => "🔨 Yasaklama",
            Self::Unban => "🔓 Yasak kaldırma",
            Self::Kick => "👢 Atma",
            Self::Timeout => "🔇 Susturma",
            Self::Untimeout => "🔊 Susturma kaldırma",
            Self::Warn => "⚠️ Uyarı",
            Self::Quarantine => "🚧 Karantina",
            Self::Unquarantine => "✅ Karantinadan çıkarma",
        }
    }

    /// "… yasaklandı", "… uyarıldı" gibi onay cümleleri için.
    pub fn past_tense(self) -> &'static str {
        match self {
            Self::Ban => "yasaklandı",
            Self::Unban => "yasağı kaldırıldı",
            Self::Kick => "sunucudan atıldı",
            Self::Timeout => "susturuldu",
            Self::Untimeout => "susturması kaldırıldı",
            Self::Warn => "uyarıldı",
            Self::Quarantine => "karantinaya alındı",
            Self::Unquarantine => "karantinadan çıkarıldı",
        }
    }

    pub fn colour(self) -> Colour {
        match self {
            Self::Ban => Colour::DARK_RED,
            Self::Kick => Colour::ORANGE,
            Self::Timeout | Self::Quarantine => Colour::GOLD,
            Self::Warn => Colour::from_rgb(250, 200, 50),
            Self::Unban | Self::Untimeout | Self::Unquarantine => Colour::DARK_GREEN,
        }
    }
}

/// Kaydedilmiş bir vaka.
pub struct Case {
    pub number: i32,
}

/// Vakayı kaydeder ve sunucuya özel artan numarasını döner.
/// Numara artırma ve kayıt tek transaction'dadır; aynı anda gelen iki işlem aynı numarayı alamaz.
pub async fn record_case(
    db: &PgPool,
    guild_id: GuildId,
    action: ModAction,
    target: UserId,
    moderator: UserId,
    reason: Option<&str>,
    duration: Option<Duration>,
) -> anyhow::Result<Case> {
    let gid = snowflake(guild_id.get())?;
    let duration_secs = duration
        .map(|d| i64::try_from(d.as_secs()))
        .transpose()?;

    let mut tx = db.begin().await?;

    let number: i32 = sqlx::query_scalar(
        "INSERT INTO guild_settings (guild_id, case_counter) VALUES ($1, 1)
         ON CONFLICT (guild_id) DO UPDATE SET case_counter = guild_settings.case_counter + 1
         RETURNING case_counter",
    )
    .bind(gid)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO mod_cases
             (guild_id, case_number, action, target_id, moderator_id, reason, duration_secs)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(gid)
    .bind(number)
    .bind(action.as_str())
    .bind(snowflake(target.get())?)
    .bind(snowflake(moderator.get())?)
    .bind(reason)
    .bind(duration_secs)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Case { number })
}

#[derive(sqlx::FromRow)]
pub struct WarningRow {
    pub case_number: i32,
    pub moderator_id: i64,
    pub reason: Option<String>,
    pub created_unix: i64,
    /// Uyarı puanı (varsayılan 1); otomatik cezalar puan toplamına bakar.
    pub points: i32,
}

/// Uyarıya 1 dışında bir puan verir (kayıt `record_case` ile 1 puanla açılır).
pub async fn set_points(db: &PgPool, guild_id: GuildId, case_number: i32, points: i32) -> anyhow::Result<()> {
    sqlx::query("UPDATE mod_cases SET points = $3 WHERE guild_id = $1 AND case_number = $2")
        .bind(snowflake(guild_id.get())?)
        .bind(case_number)
        .bind(points)
        .execute(db)
        .await?;
    Ok(())
}

/// Üyenin aktif uyarıları, en yeniden eskiye.
pub async fn active_warnings(
    db: &PgPool,
    guild_id: GuildId,
    target: UserId,
) -> anyhow::Result<Vec<WarningRow>> {
    let rows = sqlx::query_as::<_, WarningRow>(
        "SELECT case_number, moderator_id, reason,
                EXTRACT(EPOCH FROM created_at)::BIGINT AS created_unix, points
         FROM mod_cases
         WHERE guild_id = $1 AND target_id = $2 AND action = 'warn' AND active
         ORDER BY created_at DESC, case_number DESC",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(target.get())?)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Uyarıyı pasifleştirir. Uyarı bulunduysa sahibinin ID'sini döner.
pub async fn deactivate_warning(
    db: &PgPool,
    guild_id: GuildId,
    case_number: i32,
) -> anyhow::Result<Option<UserId>> {
    let target: Option<i64> = sqlx::query_scalar(
        "UPDATE mod_cases SET active = FALSE
         WHERE guild_id = $1 AND case_number = $2 AND action = 'warn' AND active
         RETURNING target_id",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(case_number)
    .fetch_optional(db)
    .await?;

    Ok(target
        .and_then(|id| u64::try_from(id).ok())
        .filter(|id| *id != 0)
        .map(UserId::new))
}

/// Sebebi Discord'un sınırına kırpar; boşsa `None`.
pub fn clean_reason(reason: Option<String>) -> Option<String> {
    let reason = reason?.trim().to_owned();
    if reason.is_empty() {
        return None;
    }
    Some(reason.chars().take(MAX_REASON_LEN).collect())
}

/// En fazla bu kadar sebep kısayolu; kısa ad ve metin uzunluk sınırları.
pub const MAX_SHORTCUTS: usize = 25;
const MAX_SHORTCUT_NAME: usize = 20;

/// Sebep yalnızca bir kısayolun adıysa (büyük/küçük harf önemsiz) tam metni döner.
pub fn expand_reason(reason: Option<String>, shortcuts: &[(String, String)]) -> Option<String> {
    let reason = clean_reason(reason)?;
    let key = reason.to_lowercase();
    match shortcuts.iter().find(|(name, _)| name.to_lowercase() == key) {
        Some((_, text)) => clean_reason(Some(text.clone())),
        None => Some(reason),
    }
}

/// Panel metnindeki "kısa = tam metin" satırlarını okur; hatalı satırda `Err(satır numarası)`.
pub fn parse_shortcuts(raw: &str) -> Result<Vec<(String, String)>, usize> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (name, text) = line.split_once('=').ok_or(i + 1)?;
        let (name, text) = (name.trim(), text.trim());
        if name.is_empty() || text.is_empty() || name.chars().count() > MAX_SHORTCUT_NAME || text.chars().count() > MAX_REASON_LEN {
            return Err(i + 1);
        }
        out.retain(|(n, _)| n.to_lowercase() != name.to_lowercase());
        out.push((name.to_owned(), text.to_owned()));
    }
    if out.len() > MAX_SHORTCUTS {
        return Err(0);
    }
    Ok(out)
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn shortcuts_expand_only_exact_names() -> Result<(), usize> {
        let list = parse_shortcuts("spam = Spam yapmak (kural 3)

 Reklam=Sunucu reklamı
spam = Tekrarlayan mesaj")?;
        assert_eq!(list.len(), 2);
        assert_eq!(expand_reason(Some(" SPAM ".into()), &list).as_deref(), Some("Tekrarlayan mesaj"));
        assert_eq!(expand_reason(Some("reklam".into()), &list).as_deref(), Some("Sunucu reklamı"));
        assert_eq!(expand_reason(Some("spam yaptı".into()), &list).as_deref(), Some("spam yaptı"));
        assert_eq!(expand_reason(None, &list), None);
        assert_eq!(parse_shortcuts("eşittir yok"), Err(1));
        assert_eq!(parse_shortcuts("a = b
 = boş ad"), Err(2));
        Ok(())
    }
}

// ---------------------------------------------------------------- Uyarı eşiği cezaları

/// Belirli sayıda aktif uyarıya ulaşınca otomatik uygulanan ceza.
#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum Punishment {
    #[name = "Susturma (timeout)"]
    Timeout,
    #[name = "Atma (kick)"]
    Kick,
    #[name = "Yasaklama (ban)"]
    Ban,
}

impl Punishment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Kick => "kick",
            Self::Ban => "ban",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "timeout" => Some(Self::Timeout),
            "kick" => Some(Self::Kick),
            "ban" => Some(Self::Ban),
            _ => None,
        }
    }

    pub fn action(self) -> ModAction {
        match self {
            Self::Timeout => ModAction::Timeout,
            Self::Kick => ModAction::Kick,
            Self::Ban => ModAction::Ban,
        }
    }
}

pub struct PunishmentRule {
    pub warn_count: i32,
    pub punishment: Punishment,
    pub duration: Option<Duration>,
    /// Doluysa sadece bu süre içindeki uyarılar sayılır ("son 1 saatte 3 uyarı").
    pub window: Option<Duration>,
}

/// Cezanın kısa açıklaması (komut ve panel ortak kullanır).
pub fn describe_punishment(rule: &PunishmentRule) -> String {
    match (rule.punishment, rule.duration) {
        (Punishment::Timeout, Some(d)) => format!("🔇 {} susturma", duration::format(d)),
        (Punishment::Timeout, None) => "🔇 susturma".to_owned(),
        (Punishment::Kick, _) => "👢 sunucudan atma".to_owned(),
        (Punishment::Ban, Some(d)) => format!("🔨 {} yasak", duration::format(d)),
        (Punishment::Ban, None) => "🔨 kalıcı yasak".to_owned(),
    }
}

pub async fn set_punishment(db: &PgPool, guild_id: GuildId, rule: &PunishmentRule) -> anyhow::Result<()> {
    let duration_secs = rule.duration.map(|d| i64::try_from(d.as_secs())).transpose()?;
    let window_secs = rule.window.map(|d| i64::try_from(d.as_secs())).transpose()?;
    sqlx::query(
        "INSERT INTO warn_punishments (guild_id, warn_count, action, duration_secs, window_secs) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (guild_id, warn_count) DO UPDATE
             SET action = EXCLUDED.action, duration_secs = EXCLUDED.duration_secs, window_secs = EXCLUDED.window_secs",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(rule.warn_count)
    .bind(rule.punishment.as_str())
    .bind(duration_secs)
    .bind(window_secs)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn remove_punishment(db: &PgPool, guild_id: GuildId, warn_count: i32) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM warn_punishments WHERE guild_id = $1 AND warn_count = $2")
        .bind(snowflake(guild_id.get())?)
        .bind(warn_count)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn punishments(db: &PgPool, guild_id: GuildId) -> anyhow::Result<Vec<PunishmentRule>> {
    let rows: Vec<(i32, String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT warn_count, action, duration_secs, window_secs FROM warn_punishments WHERE guild_id = $1 ORDER BY warn_count",
    )
    .bind(snowflake(guild_id.get())?)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(warn_count, action, secs, window)| {
            let seconds = |raw: Option<i64>| raw.and_then(|s| u64::try_from(s).ok()).filter(|s| *s > 0).map(Duration::from_secs);
            Some(PunishmentRule {
                warn_count,
                punishment: Punishment::from_db(&action)?,
                duration: seconds(secs),
                window: seconds(window),
            })
        })
        .collect())
}

// ---------------------------------------------------------------- Sicil

#[derive(sqlx::FromRow)]
pub struct CaseRow {
    pub case_number: i32,
    pub action: String,
    pub moderator_id: i64,
    pub reason: Option<String>,
    pub duration_secs: Option<i64>,
    pub active: bool,
    pub created_unix: i64,
}

impl CaseRow {
    pub fn title(&self) -> &'static str {
        match self.action.as_str() {
            "ban" => ModAction::Ban.title(),
            "unban" => ModAction::Unban.title(),
            "kick" => ModAction::Kick.title(),
            "timeout" => ModAction::Timeout.title(),
            "untimeout" => ModAction::Untimeout.title(),
            "warn" => ModAction::Warn.title(),
            "quarantine" => ModAction::Quarantine.title(),
            "unquarantine" => ModAction::Unquarantine.title(),
            _ => "❔ Bilinmeyen",
        }
    }
}

pub async fn recent_cases(db: &PgPool, guild_id: GuildId, target: UserId, limit: i64) -> anyhow::Result<(Vec<CaseRow>, i64)> {
    let gid = snowflake(guild_id.get())?;
    let uid = snowflake(target.get())?;
    let rows = sqlx::query_as::<_, CaseRow>(
        "SELECT case_number, action, moderator_id, reason, duration_secs, active,
                EXTRACT(EPOCH FROM created_at)::BIGINT AS created_unix
         FROM mod_cases WHERE guild_id = $1 AND target_id = $2
         ORDER BY created_at DESC LIMIT $3",
    )
    .bind(gid)
    .bind(uid)
    .bind(limit)
    .fetch_all(db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mod_cases WHERE guild_id = $1 AND target_id = $2")
        .bind(gid)
        .bind(uid)
        .fetch_one(db)
        .await?;
    Ok((rows, total))
}

/// Aktif uyarıları pasifleştirir (üye verilmezse sunucudaki hepsini). Temizlenen sayıyı döner.
/// Kayıtlar silinmez; `/sicil` geçmişi göstermeye devam eder.
pub async fn clear_warnings(db: &PgPool, guild_id: GuildId, target: Option<UserId>) -> anyhow::Result<u64> {
    let gid = snowflake(guild_id.get())?;
    let result = match target {
        Some(user) => {
            sqlx::query("UPDATE mod_cases SET active = FALSE WHERE guild_id = $1 AND target_id = $2 AND action = 'warn' AND active")
                .bind(gid)
                .bind(snowflake(user.get())?)
                .execute(db)
                .await?
        }
        None => {
            sqlx::query("UPDATE mod_cases SET active = FALSE WHERE guild_id = $1 AND action = 'warn' AND active")
                .bind(gid)
                .execute(db)
                .await?
        }
    };
    Ok(result.rows_affected())
}
