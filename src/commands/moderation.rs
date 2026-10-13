//! Moderasyon komutları: ban, unban, kick, timeout, untimeout, warn, uyarılar, uyarı-sil, log-kanalı.
//!
//! Ortak akış: hiyerarşi kontrolü → üyeye DM → Discord işlemi → vaka kaydı → log kanalı → onay.

use std::time::Duration;

use anyhow::anyhow;
use poise::serenity_prelude::{
    self as serenity, CreateEmbed, CreateEmbedFooter, CreateMessage, EditMember, GuildId, Member,
    Mentionable, User,
};
use poise::CreateReply;
use tracing::debug;

use crate::data::Context;
use crate::error::Error;
use crate::guild_config::Setting;
use crate::moderation::{self, clean_reason, duration, hierarchy, ModAction};
use crate::modlog::CaseLog;
use crate::scheduler::Task;

const DEFAULT_REASON: &str = "Sebep belirtilmedi";

/// Bir kullanıcıyı sunucudan yasaklar (sunucuda olmasa bile).
#[poise::command(
    slash_command,
    rename = "yasakla",
    guild_only,
    default_member_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS",
    category = "Moderasyon"
)]
pub async fn ban(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Yasaklanacak kullanıcı (ID de yapıştırabilirsin)"]
    user: User,
    #[rename = "sebep"]
    #[description = "Yasaklama sebebi"]
    reason: Option<String>,
    #[rename = "mesaj_sil"]
    #[description = "Son kaç günün mesajları silinsin (0-7, varsayılan 0)"]
    #[min = 0]
    #[max = 7]
    delete_days: Option<u8>,
    #[rename = "süre"]
    #[description = "Süreli yasak (örn. 1g, 1hf); boşsa kalıcı"]
    length: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let reason = clean_reason(reason);
    let length = match length.as_deref().map(duration::parse) {
        None => None,
        Some(Ok(d)) => Some(d),
        Some(Err(e)) => return reply_private(ctx, format!("Geçersiz süre: {e}")).await,
    };

    // Sunucudaysa hiyerarşi kontrol edilir ve yasaklanmadan önce DM atılır
    // (yasaktan sonra ortak sunucu kalmadığı için DM gönderilemez).
    let in_guild = match guild_id.member(ctx.serenity_context(), user.id).await {
        Ok(member) => {
            if let Some(problem) = hierarchy_problem(ctx, guild_id, &member).await {
                return reply_private(ctx, problem).await;
            }
            true
        }
        Err(_) => false,
    };
    // Buradan sonra birkaç Discord isteği yapılır (DM, işlem, log); 3 sn sınırını aşmamak için erteleriz.
    ctx.defer().await?;
    if in_guild {
        notify(ctx, &user, ModAction::Ban, reason.as_deref(), length).await;
    }

    let result = guild_id
        .ban_with_reason(
            ctx.http(),
            user.id,
            delete_days.unwrap_or(0),
            reason.as_deref().unwrap_or(DEFAULT_REASON),
        )
        .await;
    if let Err(e) = result {
        return reply_private(ctx, discord_error(&e)).await;
    }
    if let Some(d) = length {
        schedule_unban(ctx, guild_id, user.id, d).await?;
    }

    finish(ctx, guild_id, ModAction::Ban, &user, reason, length).await
}

async fn schedule_unban(ctx: Context<'_>, guild_id: GuildId, user: serenity::UserId, length: Duration) -> Result<(), Error> {
    let at = serenity::Timestamp::now().unix_timestamp() + i64::try_from(length.as_secs())?;
    ctx.data()
        .scheduler
        .schedule(at, &Task::Unban { guild_id: guild_id.get(), user_id: user.get() })
        .await?;
    Ok(())
}

/// Bir kullanıcının yasağını kaldırır.
#[poise::command(
    slash_command,
    rename = "yasağı-kaldır",
    guild_only,
    default_member_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS",
    category = "Moderasyon"
)]
pub async fn unban(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Yasağı kaldırılacak kullanıcı (ID yapıştırabilirsin)"]
    user: User,
    #[rename = "sebep"]
    #[description = "Yasak kaldırma sebebi"]
    reason: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let reason = clean_reason(reason);

    // Buradan sonra birkaç Discord isteği yapılır (DM, işlem, log); 3 sn sınırını aşmamak için erteleriz.
    ctx.defer().await?;
    let result = ctx
        .http()
        .remove_ban(
            guild_id,
            user.id,
            Some(reason.as_deref().unwrap_or(DEFAULT_REASON)),
        )
        .await;
    if let Err(e) = result {
        return reply_private(ctx, discord_error(&e)).await;
    }

    finish(ctx, guild_id, ModAction::Unban, &user, reason, None).await
}

/// Bir üyeyi sunucudan atar.
#[poise::command(
    slash_command,
    rename = "at",
    guild_only,
    default_member_permissions = "KICK_MEMBERS",
    required_bot_permissions = "KICK_MEMBERS",
    category = "Moderasyon"
)]
pub async fn kick(
    ctx: Context<'_>,
    #[rename = "üye"]
    #[description = "Atılacak üye"]
    member: Member,
    #[rename = "sebep"]
    #[description = "Atma sebebi"]
    reason: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let reason = clean_reason(reason);

    if let Some(problem) = hierarchy_problem(ctx, guild_id, &member).await {
        return reply_private(ctx, problem).await;
    }
    // Buradan sonra birkaç Discord isteği yapılır (DM, işlem, log); 3 sn sınırını aşmamak için erteleriz.
    ctx.defer().await?;
    notify(ctx, &member.user, ModAction::Kick, reason.as_deref(), None).await;

    let result = guild_id
        .kick_with_reason(
            ctx.http(),
            member.user.id,
            reason.as_deref().unwrap_or(DEFAULT_REASON),
        )
        .await;
    if let Err(e) = result {
        return reply_private(ctx, discord_error(&e)).await;
    }

    finish(ctx, guild_id, ModAction::Kick, &member.user, reason, None).await
}

/// Bir üyeyi belirli bir süre susturur (Discord timeout, en fazla 28 gün).
#[poise::command(
    slash_command,
    rename = "sustur",
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS",
    category = "Moderasyon"
)]
pub async fn timeout(
    ctx: Context<'_>,
    #[rename = "üye"]
    #[description = "Susturulacak üye"]
    member: Member,
    #[rename = "süre"]
    #[description = "Örn. 10dk, 1sa30dk, 2g, 1hf (en fazla 28g)"]
    length: String,
    #[rename = "sebep"]
    #[description = "Susturma sebebi"]
    reason: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let reason = clean_reason(reason);

    let length = match duration::parse(&length) {
        Ok(d) if d > duration::MAX_TIMEOUT => {
            return reply_private(ctx, "Discord en fazla 28 günlük susturmaya izin veriyor.").await;
        }
        Ok(d) => d,
        Err(e) => return reply_private(ctx, format!("Geçersiz süre: {e}")).await,
    };

    if let Some(problem) = hierarchy_problem(ctx, guild_id, &member).await {
        return reply_private(ctx, problem).await;
    }
    // Buradan sonra birkaç Discord isteği yapılır (DM, işlem, log); 3 sn sınırını aşmamak için erteleriz.
    ctx.defer().await?;

    let until_unix = serenity::Timestamp::now().unix_timestamp()
        + i64::try_from(length.as_secs()).map_err(|_| anyhow!("süre çok uzun"))?;
    let until = serenity::Timestamp::from_unix_timestamp(until_unix)
        .map_err(|e| anyhow!("geçersiz zaman damgası: {e}"))?;

    let edit = EditMember::new()
        .disable_communication_until_datetime(until)
        .audit_log_reason(reason.as_deref().unwrap_or(DEFAULT_REASON));
    if let Err(e) = guild_id
        .edit_member(ctx.serenity_context(), member.user.id, edit)
        .await
    {
        return reply_private(ctx, discord_error(&e)).await;
    }

    notify(ctx, &member.user, ModAction::Timeout, reason.as_deref(), Some(length)).await;
    finish(ctx, guild_id, ModAction::Timeout, &member.user, reason, Some(length)).await
}

/// Bir üyenin susturmasını kaldırır.
#[poise::command(
    slash_command,
    rename = "susturmayı-kaldır",
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS",
    category = "Moderasyon"
)]
pub async fn untimeout(
    ctx: Context<'_>,
    #[rename = "üye"]
    #[description = "Susturması kaldırılacak üye"]
    member: Member,
    #[rename = "sebep"]
    #[description = "Kaldırma sebebi"]
    reason: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let reason = clean_reason(reason);

    let muted = member
        .communication_disabled_until
        .is_some_and(|until| until.unix_timestamp() > serenity::Timestamp::now().unix_timestamp());
    if !muted {
        return reply_private(ctx, "Bu üye şu anda susturulmuş değil.").await;
    }
    // Buradan sonra birkaç Discord isteği yapılır (DM, işlem, log); 3 sn sınırını aşmamak için erteleriz.
    ctx.defer().await?;

    let edit = EditMember::new()
        .enable_communication()
        .audit_log_reason(reason.as_deref().unwrap_or(DEFAULT_REASON));
    if let Err(e) = guild_id
        .edit_member(ctx.serenity_context(), member.user.id, edit)
        .await
    {
        return reply_private(ctx, discord_error(&e)).await;
    }

    finish(ctx, guild_id, ModAction::Untimeout, &member.user, reason, None).await
}

/// Bir üyeyi uyarır; uyarı kaydedilir ve üyeye DM ile bildirilir.
#[poise::command(
    slash_command,
    rename = "uyar",
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    category = "Moderasyon"
)]
pub async fn warn(
    ctx: Context<'_>,
    #[rename = "üye"]
    #[description = "Uyarılacak üye"]
    member: Member,
    #[rename = "sebep"]
    #[description = "Uyarı sebebi"]
    reason: String,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let Some(reason) = clean_reason(Some(reason)) else {
        return reply_private(ctx, "Uyarı için bir sebep yazmalısın.").await;
    };

    if member.user.bot {
        return reply_private(ctx, "Botlar uyarılamaz.").await;
    }
    if let Some(problem) = hierarchy_problem(ctx, guild_id, &member).await {
        return reply_private(ctx, problem).await;
    }
    // Buradan sonra birkaç Discord isteği yapılır (DM, işlem, log); 3 sn sınırını aşmamak için erteleriz.
    ctx.defer().await?;

    notify(ctx, &member.user, ModAction::Warn, Some(&reason), None).await;
    finish(ctx, guild_id, ModAction::Warn, &member.user, Some(reason), None).await?;
    auto_punish(ctx, guild_id, &member).await
}

/// Uyarı sayısı tanımlı bir eşiğe ulaştıysa cezayı bot adına uygular.
async fn auto_punish(ctx: Context<'_>, guild_id: GuildId, member: &Member) -> Result<(), Error> {
    let data = ctx.data();
    let Some((rule, count)) = moderation::auto::due_punishment(data, guild_id, &member.user).await? else {
        return Ok(());
    };
    let reason = moderation::auto::punishment_reason(count);
    notify(ctx, &member.user, rule.punishment.action(), Some(&reason), rule.duration).await;
    let outcome = moderation::auto::apply_punishment(ctx.serenity_context(), data, guild_id, &member.user, &rule, count).await?;
    ctx.say(match outcome {
        Ok(text) | Err(text) => text,
    })
    .await?;
    Ok(())
}

/// Bir kullanıcının aktif uyarılarını listeler.
#[poise::command(
    slash_command,
    guild_only,
    rename = "uyarılar",
    default_member_permissions = "MODERATE_MEMBERS",
    category = "Moderasyon"
)]
pub async fn uyarilar(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Uyarıları gösterilecek kullanıcı"]
    user: User,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let warnings = moderation::active_warnings(&ctx.data().db, guild_id, user.id).await?;

    let description = if warnings.is_empty() {
        "Aktif uyarı yok. ✨".to_owned()
    } else {
        let mut lines: Vec<String> = warnings
            .iter()
            .take(15)
            .map(|w| {
                format!(
                    "**#{}** • <t:{}:R> • <@{}>\n{}",
                    w.case_number,
                    w.created_unix,
                    w.moderator_id,
                    w.reason.as_deref().unwrap_or(DEFAULT_REASON)
                )
            })
            .collect();
        if warnings.len() > 15 {
            lines.push(format!("_…ve {} uyarı daha._", warnings.len() - 15));
        }
        lines.join("\n\n")
    };

    let embed = CreateEmbed::new()
        .title(format!("⚠️ {} — {} aktif uyarı", user.tag(), warnings.len()))
        .colour(ModAction::Warn.colour())
        .description(description)
        .footer(CreateEmbedFooter::new(format!("Kullanıcı ID: {}", user.id)));

    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

/// Bir uyarıyı vaka numarasıyla kaldırır (kayıt geçmişte kalır).
#[poise::command(
    slash_command,
    guild_only,
    rename = "uyarı-sil",
    default_member_permissions = "MODERATE_MEMBERS",
    category = "Moderasyon"
)]
pub async fn uyari_sil(
    ctx: Context<'_>,
    #[description = "Kaldırılacak uyarının vaka numarası (/uyarılar ile görebilirsin)"]
    #[min = 1]
    vaka: i32,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;

    match moderation::deactivate_warning(&ctx.data().db, guild_id, vaka).await? {
        Some(target) => {
            ctx.say(format!("✅ {} kullanıcısının **#{vaka}** numaralı uyarısı kaldırıldı.", target.mention()))
                .await?;
            Ok(())
        }
        None => reply_private(ctx, format!("#{vaka} numaralı aktif bir uyarı bulunamadı.")).await,
    }
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum LogKind {
    #[name = "Moderasyon (ceza, auto-mod, bilet)"]
    Moderation,
    #[name = "Sunucu olayları (mesaj, üye, ses)"]
    Events,
}

/// Log kanalını ayarlar: moderasyon kayıtları veya sunucu olayları (boş bırakırsan kapatır).
#[poise::command(
    slash_command,
    guild_only,
    rename = "log-kanalı",
    default_member_permissions = "MANAGE_GUILD",
    category = "Moderasyon"
)]
pub async fn log_kanali(
    ctx: Context<'_>,
    #[description = "Log kanalı (boş bırakırsan bu log türü kapanır)"]
    #[channel_types("Text")]
    kanal: Option<serenity::GuildChannel>,
    #[rename = "tür"]
    #[description = "Hangi loglar (varsayılan: Moderasyon)"]
    kind: Option<LogKind>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let kind = kind.unwrap_or(LogKind::Moderation);

    let Some(channel) = kanal else {
        match kind {
            LogKind::Moderation => data.modlog.set_channel(&data.db, guild_id, None).await?,
            LogKind::Events => data.guild_configs.set(&data.db, guild_id, Setting::EventLog(None)).await?,
        }
        return reply_private(ctx, "Bu log türü kapatıldı.").await;
    };

    // Ayarı kaydetmeden önce kanala yazabildiğimizi doğrula.
    let description = match kind {
        LogKind::Moderation => "Moderasyon işlemleri, auto-mod ve bilet kayıtları bu kanala gönderilecek.",
        LogKind::Events => {
            "Silinen/düzenlenen mesajlar, üye giriş/çıkışları, rol ve ses kanalı değişiklikleri bu kanala gönderilecek."
        }
    };
    let test = CreateMessage::new().embed(
        CreateEmbed::new()
            .title("📋 Log kanalı ayarlandı")
            .description(description)
            .colour(serenity::Colour::BLURPLE),
    );
    if let Err(e) = channel.id.send_message(ctx.http(), test).await {
        return reply_private(
            ctx,
            format!(
                "{} kanalına yazamıyorum ({}). Kanalda 'Kanalı Görüntüle', 'Mesaj Gönder' ve \
                 'Bağlantı Yerleştir' iznim olduğundan emin ol.",
                channel.id.mention(),
                discord_error(&e)
            ),
        )
        .await;
    }

    match kind {
        LogKind::Moderation => data.modlog.set_channel(&data.db, guild_id, Some(channel.id)).await?,
        LogKind::Events => {
            data.guild_configs
                .set(&data.db, guild_id, Setting::EventLog(Some(channel.id)))
                .await?;
        }
    }
    let mut text = format!("✅ Log kanalı {} olarak ayarlandı.", channel.id.mention());
    if matches!(kind, LogKind::Events) && !data.members_intent {
        text.push_str(
            "\n⚠️ Üye giriş/çıkış ve rol logları için Developer Portal → Bot → **Server Members Intent** açılmalı.",
        );
    }
    reply_private(ctx, text).await
}

/// İşlem başarılıysa: vaka kaydı, log kanalı ve herkese açık onay mesajı.
async fn finish(
    ctx: Context<'_>,
    guild_id: GuildId,
    action: ModAction,
    target: &User,
    reason: Option<String>,
    length: Option<Duration>,
) -> Result<(), Error> {
    let data = ctx.data();
    let moderator = ctx.author().id;

    let case = moderation::record_case(
        &data.db,
        guild_id,
        action,
        target.id,
        moderator,
        reason.as_deref(),
        length,
    )
    .await?;

    data.modlog
        .case(
            ctx.http(),
            &data.db,
            guild_id,
            CaseLog {
                action,
                case_number: case.number,
                target,
                moderator,
                reason: reason.as_deref(),
                duration: length,
            },
        )
        .await;

    let mut text = format!(
        "✅ **{}** {} | Vaka #{}",
        target.tag(),
        action.past_tense(),
        case.number
    );
    if let Some(d) = length {
        text.push_str(&format!(" | {}", duration::format(d)));
    }
    if action == ModAction::Warn {
        let count = moderation::active_warnings(&data.db, guild_id, target.id)
            .await?
            .len();
        text.push_str(&format!(" | Toplam aktif uyarı: **{count}**"));
    }
    if let Some(r) = &reason {
        text.push_str(&format!("\n> {r}"));
    }

    ctx.say(text).await?;
    Ok(())
}

/// Hiyerarşi uygunsa `None`, değilse kullanıcıya gösterilecek sebep.
async fn hierarchy_problem(ctx: Context<'_>, guild_id: GuildId, target: &Member) -> Option<String> {
    let Some(moderator) = ctx.author_member().await else {
        return Some("Yetkili üyelik bilgisi alınamadı.".to_owned());
    };
    hierarchy::check(ctx.serenity_context(), guild_id, &moderator, target)
        .await
        .err()
}

/// Hedefe DM ile bilgi verir. DM'leri kapalıysa sessizce geçilir.
async fn notify(
    ctx: Context<'_>,
    user: &User,
    action: ModAction,
    reason: Option<&str>,
    length: Option<Duration>,
) {
    if user.bot {
        return;
    }
    let guild_name = ctx
        .guild()
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "sunucu".to_owned());

    let mut embed = CreateEmbed::new()
        .title(format!("{} — {guild_name}", action.title()))
        .colour(action.colour())
        .field("Sebep", reason.unwrap_or(DEFAULT_REASON), false);
    if let Some(d) = length {
        embed = embed.field("Süre", duration::format(d), true);
    }

    let mut message = CreateMessage::new().embed(embed);
    // Yasakta, itiraz kanalı ayarlıysa DM'e "İtiraz et" butonu eklenir.
    if action == ModAction::Ban {
        if let Some(guild_id) = ctx.guild_id() {
            let data = ctx.data();
            if let Ok(cfg) = data.guild_configs.get(&data.db, guild_id).await {
                if cfg.appeal_channel.is_some() {
                    // Web formu açıksa kişiye özel bağlantı (itiraz sunucusuna gerek kalmaz).
                    let web_link = match &data.public_url.public() {
                        Some(base) => match crate::appeals::create_token(&data.db, guild_id, user.id).await {
                            Ok(token) => Some(format!("{base}/itiraz/{token}")),
                            Err(e) => {
                                debug!(error = ?e, "İtiraz anahtarı oluşturulamadı");
                                None
                            }
                        },
                        None => None,
                    };
                    message = match web_link {
                        Some(link) => message
                            .content(crate::appeals::DM_WEB_TEXT)
                            .components(vec![crate::appeals::dm_web_button(&link)]),
                        None => {
                            let invite = cfg.appeal_invite.as_deref();
                            message
                                .content(crate::appeals::dm_text(invite))
                                .components(vec![crate::appeals::dm_buttons(guild_id, invite)])
                        }
                    };
                }
            }
        }
    }
    if let Err(e) = user
        .direct_message(ctx.serenity_context(), message)
        .await
    {
        debug!(user = %user.id, error = %e, "Moderasyon DM'i gönderilemedi");
    }
}

pub(super) fn guild_id(ctx: Context<'_>) -> Result<GuildId, Error> {
    ctx.guild_id()
        .ok_or_else(|| anyhow!("guild_only komut sunucu dışında çalıştı"))
}

/// Sadece komutu kullanan kişinin göreceği yanıt.
pub(super) async fn reply_private(ctx: Context<'_>, message: impl Into<String>) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(message).ephemeral(true))
        .await?;
    Ok(())
}

pub(super) fn discord_error(e: &serenity::Error) -> String {
    let text = e.to_string();
    if text.contains("Missing Permissions") {
        "Discord izin vermedi: yetkim ya da rol sıralamam yetersiz".to_owned()
    } else if text.contains("Unknown Ban") {
        "bu kullanıcı yasaklı değil".to_owned()
    } else {
        format!("Discord hatası: {text}")
    }
}
