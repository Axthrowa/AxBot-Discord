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

/// Aktif uyarılara (zaman, puan) göre tetiklenen kural. Eşik uyarı PUANI toplamıdır; son uyarının (`latest` puan)
/// toplamı eşiğin altından eşiğe ya da üstüne taşıdığı kurallardan en ağırı seçilir (eşik bir kez tetiklenir).
/// Bütün uyarılar 1 puanken bu, "uyarı sayısı tam eşikte" demektir.
pub fn due_rule(rules: Vec<PunishmentRule>, warnings: &[(i64, i32)], now: i64, latest: i32) -> Option<(PunishmentRule, i32)> {
    rules
        .into_iter()
        .filter_map(|rule| {
            let since = rule.window.map_or(i64::MIN, |window| now - i64::try_from(window.as_secs()).unwrap_or(i64::MAX));
            let total: i32 = warnings.iter().filter(|(t, _)| *t >= since).map(|(_, p)| *p).sum();
            (total >= rule.warn_count && total - latest < rule.warn_count).then_some((rule, total))
        })
        .max_by_key(|(rule, _)| rule.warn_count)
}

/// Üyenin aktif uyarılarına göre şu an uygulanması gereken otomatik ceza.
pub async fn due_punishment(data: &Data, guild_id: GuildId, user: &User) -> anyhow::Result<Option<(PunishmentRule, i32)>> {
    let rules = super::punishments(&data.db, guild_id).await?;
    if rules.is_empty() {
        return Ok(None);
    }
    // En yeni uyarı (listenin başı) az önce verilendir; eşiği onun geçirip geçirmediğine bakılır.
    let warnings = super::active_warnings(&data.db, guild_id, user.id).await?;
    let latest = warnings.first().map_or(0, |w| w.points);
    let rows: Vec<(i64, i32)> = warnings.iter().map(|w| (w.created_unix, w.points)).collect();
    Ok(due_rule(rules, &rows, serenity::Timestamp::now().unix_timestamp(), latest))
}

pub fn punishment_reason(count: i32) -> String {
    format!("Otomatik ceza: uyarı puanı {count} oldu")
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

    /// Her uyarı 1 puan.
    fn ones(times: &[i64]) -> Vec<(i64, i32)> {
        times.iter().map(|t| (*t, 1)).collect()
    }

    #[test]
    fn rule_fires_exactly_at_its_threshold() {
        let now = 10_000;
        let times = ones(&[9_990, 9_000, 100]);
        let due = due_rule(vec![rule(3, Punishment::Kick, None), rule(5, Punishment::Ban, None)], &times, now, 1);
        assert!(matches!(due, Some((r, 3)) if r.punishment == Punishment::Kick));
        // Dördüncü uyarıda 3'lük kural yeniden tetiklenmez.
        assert!(due_rule(vec![rule(3, Punishment::Kick, None)], &ones(&[1, 2, 3, 4]), now, 1).is_none());
    }

    #[test]
    fn window_counts_only_recent_warnings() {
        let now = 10_000;
        let times = ones(&[9_990, 9_500, 100]);
        // Son 1 saatte 2 uyarı var: pencereli 2'lik kural tetiklenir, pencereli 3'lük tetiklenmez.
        assert!(due_rule(vec![rule(2, Punishment::Timeout, Some(3600))], &times, now, 1).is_some());
        assert!(due_rule(vec![rule(3, Punishment::Timeout, Some(3600))], &times, now, 1).is_none());
        // Aynı anda iki kural tutarsa ağır olan (eşiği yüksek) uygulanır.
        let both = due_rule(vec![rule(2, Punishment::Timeout, Some(3600)), rule(3, Punishment::Ban, None)], &times, now, 1);
        assert!(matches!(both, Some((r, 3)) if r.punishment == Punishment::Ban));
    }

    #[test]
    fn heavy_warning_can_jump_past_a_threshold() {
        let now = 10_000;
        let rules = || vec![rule(3, Punishment::Timeout, None), rule(5, Punishment::Kick, None)];
        // 2 puan vardı, 3 puanlık uyarı geldi (toplam 5): iki eşik birden geçildi, ağır olan seçilir.
        let warnings = [(9_990, 3), (9_000, 1), (8_000, 1)];
        assert!(matches!(due_rule(rules(), &warnings, now, 3), Some((r, 5)) if r.punishment == Punishment::Kick));
        // Sonraki 1 puanlık uyarı (toplam 6) geçilmiş eşikleri yeniden tetiklemez.
        let later = [(9_995, 1), (9_990, 3), (9_000, 1), (8_000, 1)];
        assert!(due_rule(rules(), &later, now, 1).is_none());
    }
}
