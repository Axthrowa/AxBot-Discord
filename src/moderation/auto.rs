//! Bot adına yapılan moderasyon: otomatik moderasyonun verdiği uyarılar ve uyarı sayısı bir
//! eşiğe ulaşınca uygulanan otomatik cezalar. Hem `/mod uyar` komutu hem otomatik moderasyon kullanır.

use std::time::Duration;

use anyhow::anyhow;
use poise::serenity_prelude::{self as serenity, CreateEmbed, CreateMessage, EditMember, GuildId, User};
use tracing::debug;

use super::{duration, record_case, ModAction, Punishment, PunishmentRule};
use crate::data::Data;
use crate::modlog::CaseLog;
use crate::scheduler::Task;

/// Süresi verilmemiş otomatik susturmanın uzunluğu.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3600);

/// Verilen uyarı zamanlarına göre tetiklenen kural: penceresindeki uyarı sayısı tam olarak
/// eşiğe eşit olan kurallardan en ağırı (eşik bir kez tetiklenir).
pub fn due_rule(rules: Vec<PunishmentRule>, warning_times: &[i64], now: i64) -> Option<(PunishmentRule, i32)> {
    rules
        .into_iter()
        .filter_map(|rule| {
            let count = match rule.window {
                Some(window) => {
                    let since = now - i64::try_from(window.as_secs()).unwrap_or(i64::MAX);
                    warning_times.iter().filter(|t| **t >= since).count()
                }
                None => warning_times.len(),
            };
            let count = i32::try_from(count).ok()?;
            (count == rule.warn_count).then_some((rule, count))
        })
        .max_by_key(|(rule, _)| rule.warn_count)
}

/// Üyenin aktif uyarılarına göre şu an uygulanması gereken otomatik ceza.
pub async fn due_punishment(data: &Data, guild_id: GuildId, user: &User) -> anyhow::Result<Option<(PunishmentRule, i32)>> {
    let rules = super::punishments(&data.db, guild_id).await?;
    if rules.is_empty() {
        return Ok(None);
    }
    let times: Vec<i64> = super::active_warnings(&data.db, guild_id, user.id).await?.iter().map(|w| w.created_unix).collect();
    Ok(due_rule(rules, &times, serenity::Timestamp::now().unix_timestamp()))
}

pub fn punishment_reason(count: i32) -> String {
    format!("Otomatik ceza: {count} uyarıya ulaştı")
}

/// Cezayı Discord'da uygular, vakayı kaydeder ve log kanalına yazar.
/// Başarılıysa herkese gösterilebilecek özet metni, değilse kullanıcıya gösterilecek hata döner.
pub async fn apply_punishment(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    user: &User,
    rule: &PunishmentRule,
    count: i32,
) -> anyhow::Result<Result<String, String>> {
    let reason = punishment_reason(count);
    let action = rule.punishment.action();
    let result = match rule.punishment {
        Punishment::Timeout => {
            let length = rule.duration.unwrap_or(DEFAULT_TIMEOUT).min(duration::MAX_TIMEOUT);
            let until = serenity::Timestamp::from_unix_timestamp(
                serenity::Timestamp::now().unix_timestamp() + i64::try_from(length.as_secs())?,
            )
            .map_err(|e| anyhow!("geçersiz zaman: {e}"))?;
            let edit = EditMember::new().disable_communication_until_datetime(until).audit_log_reason(&reason);
            guild_id.edit_member(ctx, user.id, edit).await.map(|_| ())
        }
        Punishment::Kick => guild_id.kick_with_reason(&ctx.http, user.id, &reason).await,
        Punishment::Ban => guild_id.ban_with_reason(&ctx.http, user.id, 0, &reason).await,
    };
    if let Err(e) = result {
        return Ok(Err(format!("⚠️ Otomatik ceza uygulanamadı: {e}")));
    }
    if let (Punishment::Ban, Some(length)) = (rule.punishment, rule.duration) {
        let at = serenity::Timestamp::now().unix_timestamp() + i64::try_from(length.as_secs())?;
        data.scheduler.schedule(at, &Task::Unban { guild_id: guild_id.get(), user_id: user.id.get() }).await?;
    }

    let bot_id = ctx.cache.current_user().id;
    let case = record_case(&data.db, guild_id, action, user.id, bot_id, Some(&reason), rule.duration).await?;
    data.modlog
        .case(
            &ctx.http,
            &data.db,
            guild_id,
            CaseLog { action, case_number: case.number, target: user, moderator: bot_id, reason: Some(&reason), duration: rule.duration },
        )
        .await;

    let mut text = format!("⚖️ **{}** {} ({reason}) | Vaka #{}", user.tag(), action.past_tense(), case.number);
    if let Some(d) = rule.duration {
        text.push_str(&format!(" | {}", duration::format(d)));
    }
    Ok(Ok(text))
}

/// Hedefe özelden kısa bilgi verir; DM'leri kapalıysa sessizce geçilir.
async fn dm(ctx: &serenity::Context, guild_id: GuildId, user: &User, action: ModAction, reason: &str, length: Option<Duration>) {
    let guild_name = ctx.cache.guild(guild_id).map(|g| g.name.clone()).unwrap_or_else(|| "sunucu".to_owned());
    let mut embed = CreateEmbed::new()
        .title(format!("{} — {guild_name}", action.title()))
        .colour(action.colour())
        .field("Sebep", reason, false);
    if let Some(d) = length {
        embed = embed.field("Süre", duration::format(d), true);
    }
    if let Err(e) = user.direct_message(ctx, CreateMessage::new().embed(embed)).await {
        debug!(user = %user.id, error = %e, "Moderasyon DM'i gönderilemedi");
    }
}

/// Otomatik moderasyon adına uyarı verir: vaka kaydı, log, DM ve gerekiyorsa otomatik ceza.
/// Verilen ceza varsa özet metnini döner.
pub async fn warn_by_bot(ctx: &serenity::Context, data: &Data, guild_id: GuildId, user: &User, reason: &str) -> anyhow::Result<Option<String>> {
    let bot_id = ctx.cache.current_user().id;
    let case = record_case(&data.db, guild_id, ModAction::Warn, user.id, bot_id, Some(reason), None).await?;
    data.modlog
        .case(
            &ctx.http,
            &data.db,
            guild_id,
            CaseLog { action: ModAction::Warn, case_number: case.number, target: user, moderator: bot_id, reason: Some(reason), duration: None },
        )
        .await;
    dm(ctx, guild_id, user, ModAction::Warn, reason, None).await;

    let Some((rule, count)) = due_punishment(data, guild_id, user).await? else {
        return Ok(None);
    };
    dm(ctx, guild_id, user, rule.punishment.action(), &punishment_reason(count), rule.duration).await;
    Ok(apply_punishment(ctx, data, guild_id, user, &rule, count).await?.ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(warn_count: i32, punishment: Punishment, window_secs: Option<u64>) -> PunishmentRule {
        PunishmentRule { warn_count, punishment, duration: None, window: window_secs.map(Duration::from_secs) }
    }

    #[test]
    fn rule_fires_exactly_at_its_threshold() {
        let now = 10_000;
        let times = [9_990, 9_000, 100];
        let due = due_rule(vec![rule(3, Punishment::Kick, None), rule(5, Punishment::Ban, None)], &times, now);
        assert!(matches!(due, Some((r, 3)) if r.punishment == Punishment::Kick));
        // Dördüncü uyarıda 3'lük kural yeniden tetiklenmez.
        assert!(due_rule(vec![rule(3, Punishment::Kick, None)], &[1, 2, 3, 4], now).is_none());
    }

    #[test]
    fn window_counts_only_recent_warnings() {
        let now = 10_000;
        let times = [9_990, 9_500, 100];
        // Son 1 saatte 2 uyarı var: pencereli 2'lik kural tetiklenir, pencereli 3'lük tetiklenmez.
        assert!(due_rule(vec![rule(2, Punishment::Timeout, Some(3600))], &times, now).is_some());
        assert!(due_rule(vec![rule(3, Punishment::Timeout, Some(3600))], &times, now).is_none());
        // Aynı anda iki kural tutarsa ağır olan (eşiği yüksek) uygulanır.
        let both = due_rule(vec![rule(2, Punishment::Timeout, Some(3600)), rule(3, Punishment::Ban, None)], &times, now);
        assert!(matches!(both, Some((r, 3)) if r.punishment == Punishment::Ban));
    }
}
