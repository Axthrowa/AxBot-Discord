//! `/doğrulama` ve `/başvuru` (formlar + ban itirazı kanalı).

use poise::serenity_prelude::{self as serenity, Mentionable};

use super::community::cannot_send;
use super::moderation::{guild_id, reply_private};
use crate::applications;
use crate::data::Context;
use crate::error::Error;
use crate::guild_config::Setting;
use crate::{modals, rolepanel, verification};

/// Butonlu doğrulama sistemi.
#[poise::command(
    slash_command,
    guild_only,
    rename = "doğrulama",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("dogrulama_kur", "dogrulama_kapat"),
    subcommand_required,
    category = "Güvenlik"
)]
pub async fn dogrulama(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Doğrulama panelini gönderir: butona basan üyeye rol verilir.
#[poise::command(slash_command, guild_only, rename = "kur", required_bot_permissions = "MANAGE_ROLES")]
async fn dogrulama_kur(
    ctx: Context<'_>,
    #[description = "Panelin gönderileceği kanal (yeni üyelerin görebildiği)"]
    #[channel_types("Text")]
    kanal: serenity::GuildChannel,
    #[description = "Doğrulanınca verilecek rol (diğer kanalları bu rol görmeli)"] rol: serenity::Role,
    #[rename = "işlem_sorusu"]
    #[description = "Butondan sonra basit bir toplama sorusu sorulsun mu? (bot hesaplarına karşı)"]
    math: Option<bool>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    if let Err(reason) = rolepanel::check_role(ctx.serenity_context(), guild_id, &rol) {
        return reply_private(ctx, format!("❌ {reason}")).await;
    }
    if let Some(problem) = cannot_send(ctx, &kanal) {
        return reply_private(ctx, problem).await;
    }
    let math = math.unwrap_or(false);
    // Panel gönderme + ayar kaydı 3 sn'yi aşabilir.
    ctx.defer_ephemeral().await?;
    kanal.id.send_message(ctx, verification::panel_message(math)).await?;
    let data = ctx.data();
    data.guild_configs.set(&data.db, guild_id, Setting::Verify(Some(rol.id), math)).await?;
    reply_private(
        ctx,
        format!(
            "✅ Doğrulama paneli {} kanalına gönderildi. Butona basanlara {} rolü verilecek.\n\
             **Önemli:** Kanalların @everyone için gizli, {} rolü için görünür olmalı; yoksa doğrulama bir işe yaramaz.",
            kanal.id.mention(),
            rol.id.mention(),
            rol.id.mention()
        ),
    )
    .await
}

/// Doğrulamayı kapatır (panel butonları çalışmayı bırakır).
#[poise::command(slash_command, guild_only, rename = "kapat")]
async fn dogrulama_kapat(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    data.guild_configs.set(&data.db, guild_id, Setting::Verify(None, false)).await?;
    reply_private(ctx, "Doğrulama kapatıldı. Panel mesajını silebilirsin.").await
}

/// Başvuru formları ve ban itirazları.
#[poise::command(
    slash_command,
    guild_only,
    rename = "başvuru",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("form_ekle", "form_panel", "form_sil", "form_liste", "itiraz_kanali"),
    subcommand_required,
    category = "Güvenlik"
)]
pub async fn basvuru(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

async fn autocomplete_form(ctx: Context<'_>, partial: &str) -> Vec<String> {
    let Some(guild_id) = ctx.guild_id() else {
        return Vec::new();
    };
    let partial = partial.to_lowercase();
    applications::forms(&ctx.data().db, guild_id)
        .await
        .map(|forms| forms.into_iter().map(|f| f.name).filter(|n| n.to_lowercase().contains(&partial)).take(25).collect())
        .unwrap_or_default()
}

/// Başvuru formu oluşturur veya günceller (en fazla 5 soru).
#[poise::command(slash_command, guild_only, rename = "form-ekle")]
#[allow(clippy::too_many_arguments)]
async fn form_ekle(
    ctx: Context<'_>,
    #[rename = "ad"]
    #[description = "Form adı (örn. Yetkili Başvurusu)"]
    #[max_length = 45]
    name: String,
    #[rename = "yetkili_kanalı"]
    #[description = "Başvuruların düşeceği (yetkililerin gördüğü) kanal"]
    #[channel_types("Text")]
    review_channel: serenity::GuildChannel,
    #[description = "1. soru (en fazla 45 karakter)"]
    #[max_length = 45]
    soru1: String,
    #[description = "2. soru"]
    #[max_length = 45]
    soru2: Option<String>,
    #[description = "3. soru"]
    #[max_length = 45]
    soru3: Option<String>,
    #[description = "4. soru"]
    #[max_length = 45]
    soru4: Option<String>,
    #[description = "5. soru"]
    #[max_length = 45]
    soru5: Option<String>,
    #[description = "Kabul edilene verilecek rol (opsiyonel)"] rol: Option<serenity::Role>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    if let Some(role) = &rol {
        if let Err(reason) = rolepanel::check_role(ctx.serenity_context(), guild_id, role) {
            return reply_private(ctx, format!("❌ {reason}\n_(Yetkili rolleri güvenlik için otomatik verilmez; kabul edince elle verebilirsin.)_")).await;
        }
    }
    if let Some(problem) = cannot_send(ctx, &review_channel) {
        return reply_private(ctx, problem).await;
    }
    let questions: Vec<String> = [Some(soru1), soru2, soru3, soru4, soru5]
        .into_iter()
        .flatten()
        .map(|q| q.trim().to_owned())
        .filter(|q| !q.is_empty())
        .take(modals::MAX_FIELDS)
        .collect();
    if let Err(e) = applications::create_form(&ctx.data().db, guild_id, &name, &questions, review_channel.id, rol.as_ref().map(|r| r.id)).await {
        return reply_private(ctx, format!("❌ {e}")).await;
    }
    reply_private(
        ctx,
        format!(
            "✅ **{}** formu kaydedildi ({} soru). Başvurular {} kanalına düşecek.\nŞimdi `/başvuru panel` ile başvuru butonunu bir kanala gönder.",
            name.trim(),
            questions.len(),
            review_channel.id.mention()
        ),
    )
    .await
}

/// Başvuru butonunu bir kanala gönderir.
#[poise::command(slash_command, guild_only, rename = "panel")]
async fn form_panel(
    ctx: Context<'_>,
    #[rename = "form"]
    #[description = "Form"]
    #[autocomplete = "autocomplete_form"]
    name: String,
    #[description = "Butonun gönderileceği kanal"]
    #[channel_types("Text")]
    kanal: serenity::GuildChannel,
    #[rename = "açıklama"]
    #[description = "Panelde yazacak açıklama"]
    #[max_length = 1000]
    description: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let Some(form) = applications::find_form(&ctx.data().db, guild_id, &name).await? else {
        return reply_private(ctx, format!("**{name}** adında form yok.")).await;
    };
    if let Some(problem) = cannot_send(ctx, &kanal) {
        return reply_private(ctx, problem).await;
    }
    ctx.defer_ephemeral().await?;
    kanal
        .id
        .send_message(ctx, applications::panel_message(&form, description.as_deref()))
        .await?;
    reply_private(ctx, format!("✅ **{}** başvuru paneli {} kanalına gönderildi.", form.name, kanal.id.mention())).await
}

/// Bir formu (ve başvurularını) siler.
#[poise::command(slash_command, guild_only, rename = "form-sil")]
async fn form_sil(
    ctx: Context<'_>,
    #[rename = "form"]
    #[description = "Form"]
    #[autocomplete = "autocomplete_form"]
    name: String,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let text = if applications::delete_form(&ctx.data().db, guild_id, &name).await? {
        format!("🗑️ **{name}** formu silindi (eski panel butonları artık çalışmaz).")
    } else {
        format!("**{name}** adında form yok.")
    };
    reply_private(ctx, text).await
}

/// Formları listeler.
#[poise::command(slash_command, guild_only, rename = "form-liste")]
async fn form_liste(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let forms = applications::forms(&ctx.data().db, guild_id).await?;
    let text = if forms.is_empty() {
        "Henüz form yok. `/başvuru form-ekle` ile oluşturabilirsin.".to_owned()
    } else {
        forms
            .iter()
            .map(|f| {
                let channel = f.review_channel().map_or_else(|| "?".to_owned(), |c| c.mention().to_string());
                let role = f.role().map_or_else(String::new, |r| format!(" · rol {}", r.mention()));
                format!("• **{}** — {} soru → {channel}{role}", f.name, f.questions.len())
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    reply_private(ctx, text).await
}

/// Ban itirazlarının düşeceği kanal (boşsa itiraz kapanır).
#[poise::command(slash_command, guild_only, rename = "itiraz-kanalı")]
async fn itiraz_kanali(
    ctx: Context<'_>,
    #[description = "Yetkililerin gördüğü kanal"]
    #[channel_types("Text")]
    kanal: Option<serenity::GuildChannel>,
    #[rename = "itiraz_sunucusu_daveti"]
    #[description = "Botun da olduğu ayrı itiraz sunucusunun süresiz davet linki (yasaklılar için gerekli)"]
    #[max_length = 100]
    invite: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let Some(kanal) = kanal else {
        data.guild_configs.set(&data.db, guild_id, Setting::AppealChannel(None, None, None)).await?;
        return reply_private(ctx, "Ban itirazları kapatıldı.").await;
    };
    if let Some(problem) = cannot_send(ctx, &kanal) {
        return reply_private(ctx, problem).await;
    }
    ctx.defer_ephemeral().await?;

    // Davet: geçerli mi, başka bir sunucuya mı ait, bot orada mı?
    let invite = match invite.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
        None => None::<(String, serenity::GuildId)>,
        Some(raw) => {
            let Some(code) = crate::appeals::invite_code(raw) else {
                return reply_private(ctx, "❌ Bu bir Discord davet linki değil (örn. https://discord.gg/abc123).").await;
            };
            let info = match ctx.http().get_invite(&code, false, true, None).await {
                Ok(info) => info,
                Err(_) => return reply_private(ctx, "❌ Davet linki geçersiz veya süresi dolmuş.").await,
            };
            let Some(target) = info.guild.as_ref().map(|g| g.id) else {
                return reply_private(ctx, "❌ Bu davet bir sunucuya ait değil.").await;
            };
            if target == guild_id {
                return reply_private(ctx, "❌ Bu davet bu sunucunun kendisine ait; yasaklılar buraya giremez. Ayrı bir itiraz sunucusu gerekli.").await;
            }
            if ctx.serenity_context().cache.guild(target).is_none() {
                return reply_private(ctx, "❌ Ben o sunucuda değilim. Önce beni itiraz sunucusuna ekle (ana sunucuya eklediğin davet linkiyle).").await;
            }
            if info.expires_at.is_some() {
                return reply_private(ctx, "❌ Bu davetin süresi dolacak; itiraz sunucusu için **süresiz** bir davet oluştur.").await;
            }
            Some((format!("https://discord.gg/{code}"), target))
        }
    };
    let (invite, appeal_guild) = match invite {
        Some((link, guild)) => (Some(link), Some(guild)),
        None => (None, None),
    };

    data.guild_configs
        .set(&data.db, guild_id, Setting::AppealChannel(Some(kanal.id), invite.clone(), appeal_guild))
        .await?;
    let note = if invite.is_some() {
        "Yasaklılar önce itiraz sunucusuna katılıp sonra butonla itiraz edebilecek. İtirazı kabul edilip \
         buraya geri dönenler itiraz sunucusundan otomatik atılır _(bunun için itiraz sunucusunda bana \
         **Üyeleri At** izni ver)_."
    } else {
        "⚠️ **İtiraz sunucusu daveti vermedin.** Yasaklıyken botla ortak sunucusu kalmayan kişinin \
         butonu çalışmaz; ayrı bir itiraz sunucusu açıp `itiraz_sunucusu_daveti` seçeneğiyle tekrar kur."
    };
    reply_private(
        ctx,
        format!(
            "✅ Yasaklananlara giden DM'de **İtiraz et** butonu olacak; itirazlar {} kanalına düşecek. \
             Kabul edilirse yasak otomatik kalkar.\n{note}",
            kanal.id.mention()
        ),
    )
    .await
}
