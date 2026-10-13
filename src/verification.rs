//! Butonlu doğrulama: yeni üyeler panel mesajındaki butona basarak "doğrulanmış" rolünü alır.
//! İstenirse basit bir işlem sorusu da sorulur (botların otomatik tıklamasına karşı).
//!
//! İşlem sorusunun cevabı Redis'te 5 dakika tutulur; form kimliğinde açıkça yer almaz.

use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteraction, CreateActionRow, CreateButton,
    CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
    CreateModal, GuildId, Mentionable, ModalInteraction,
};
use tracing::{info, warn};

use crate::data::Data;
use crate::{modals, rand_util, rolepanel};

pub const PREFIX: &str = "verify:";
const BUTTON: &str = "verify:start";
const MODAL: &str = "verify:answer";
const CODE_BUTTON: &str = "verify:code";

fn answers_match(expected: &str, given: &str) -> bool {
    let clean = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
    clean(expected) == clean(given)
}
const ANSWER_TTL_SECS: u64 = 300;

pub fn panel_message(math: bool) -> CreateMessage {
    let extra = if math { "\nKısa bir işlem sorusu cevaplaman istenecek." } else { "" };
    let embed = CreateEmbed::new()
        .title("✅ Doğrulama")
        .description(format!(
            "Sunucunun tüm kanallarını görmek için aşağıdaki **Doğrula** butonuna bas.{extra}"
        ))
        .colour(serenity::Colour::from_rgb(87, 242, 135));
    let button = CreateButton::new(BUTTON).label("Doğrula").emoji('✅').style(ButtonStyle::Success);
    CreateMessage::new().embed(embed).components(vec![CreateActionRow::Buttons(vec![button])])
}

fn answer_key(guild: GuildId, user: serenity::UserId) -> String {
    format!("verify:answer:{guild}:{user}")
}

pub async fn handle_button(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let (Some(guild_id), Some(member)) = (interaction.guild_id, interaction.member.as_ref()) else {
        return Ok(());
    };
    let cfg = data.guild_configs.get(&data.db, guild_id).await?;
    let Some(role) = cfg.verify_role else {
        return reply(ctx, interaction, "Doğrulama şu an kapalı.").await;
    };
    if member.roles.contains(&role) {
        return reply(ctx, interaction, "Zaten doğrulanmışsın. ✅").await;
    }

    // Captcha resmindeki "Kodu gir" butonu: kod zaten Redis'te, sadece giriş penceresi açılır.
    if interaction.data.custom_id == CODE_BUTTON {
        let modal = CreateModal::new(MODAL, "Doğrulama").components(vec![modals::text_field(
            "answer",
            "Resimdeki kodu yaz",
            false,
            true,
            8,
        )]);
        interaction.create_response(ctx, CreateInteractionResponse::Modal(modal)).await?;
        return Ok(());
    }

    let captcha = data
        .settings
        .get::<crate::cards::WelcomeSettings>(&data.db, guild_id)
        .await
        .is_ok_and(|settings| settings.captcha);
    if captcha {
        let code = crate::cards::captcha_code();
        let mut conn = data.redis.clone();
        let _: () = redis::cmd("SET")
            .arg(answer_key(guild_id, interaction.user.id))
            .arg(&code)
            .arg("EX")
            .arg(ANSWER_TTL_SECS)
            .query_async(&mut conn)
            .await?;
        let image = tokio::task::spawn_blocking(move || crate::cards::render_captcha(&code)).await??;
        let button = CreateButton::new(CODE_BUTTON).label("Kodu gir").emoji('⌨').style(ButtonStyle::Primary);
        let response = CreateInteractionResponseMessage::new()
            .content("🔒 Resimdeki kodu oku, sonra **Kodu gir** butonuna bas. (Büyük/küçük harf fark etmez; 5 dakika geçerli.)")
            .add_file(serenity::CreateAttachment::bytes(image, "dogrulama.jpg"))
            .components(vec![CreateActionRow::Buttons(vec![button])])
            .ephemeral(true);
        interaction.create_response(ctx, CreateInteractionResponse::Message(response)).await?;
        return Ok(());
    }

    if cfg.verify_math {
        let (a, b) = (2 + rand_util::below(9), 2 + rand_util::below(9));
        let mut conn = data.redis.clone();
        let _: () = redis::cmd("SET")
            .arg(answer_key(guild_id, interaction.user.id))
            .arg(a + b)
            .arg("EX")
            .arg(ANSWER_TTL_SECS)
            .query_async(&mut conn)
            .await?;
        let modal = CreateModal::new(MODAL, "Doğrulama").components(vec![modals::text_field(
            "answer",
            &format!("{a} + {b} kaç eder?"),
            false,
            true,
            4,
        )]);
        interaction.create_response(ctx, CreateInteractionResponse::Modal(modal)).await?;
        return Ok(());
    }
    let text = grant(ctx, guild_id, interaction.user.id, role).await;
    reply(ctx, interaction, &text).await
}

pub async fn handle_modal(ctx: &serenity::Context, data: &Data, modal: &ModalInteraction) -> anyhow::Result<()> {
    let Some(guild_id) = modal.guild_id else {
        return Ok(());
    };
    let cfg = data.guild_configs.get(&data.db, guild_id).await?;
    let Some(role) = cfg.verify_role else {
        return modal_reply(ctx, modal, "Doğrulama şu an kapalı.").await;
    };
    let given = modals::values(modal).into_iter().next().unwrap_or_default();
    let mut conn = data.redis.clone();
    let expected: Option<String> = redis::cmd("GETDEL")
        .arg(answer_key(guild_id, modal.user.id))
        .query_async(&mut conn)
        .await?;
    let text = match expected {
        None => "⏰ Sorunun süresi doldu; butona tekrar bas.".to_owned(),
        // Captcha kodunda büyük/küçük harf ve aradaki boşluklar önemsenmez.
        Some(expected) if answers_match(&expected, &given) => grant(ctx, guild_id, modal.user.id, role).await,
        Some(_) => "❌ Yanlış cevap. Butona tekrar basıp yeni bir soru alabilirsin.".to_owned(),
    };
    modal_reply(ctx, modal, &text).await
}

/// Rolü verir ve kullanıcıya gösterilecek sonucu döner.
async fn grant(ctx: &serenity::Context, guild_id: GuildId, user: serenity::UserId, role: serenity::RoleId) -> String {
    // Rol sonradan yetki kazandıysa güvenlik için verilmez.
    let checked = ctx
        .cache
        .guild(guild_id)
        .and_then(|g| g.roles.get(&role).cloned())
        .map(|r| rolepanel::check_role(ctx, guild_id, &r));
    match checked {
        None => return "Doğrulama rolü artık yok; yöneticilere haber ver.".to_owned(),
        Some(Err(reason)) => return format!("❌ {reason}"),
        Some(Ok(())) => {}
    }
    match ctx.http.add_member_role(guild_id, user, role, Some("Doğrulama")).await {
        Ok(()) => {
            info!(guild = %guild_id, user = %user, "Üye doğrulandı");
            format!("✅ Doğrulandın! {} rolü verildi, hoş geldin.", role.mention())
        }
        Err(e) => {
            warn!(guild = %guild_id, error = %e, "Doğrulama rolü verilemedi");
            "❌ Rol verilemedi; botun rolü doğrulama rolünün üstünde olmalı.".to_owned()
        }
    }
}

async fn reply(ctx: &serenity::Context, interaction: &ComponentInteraction, text: &str) -> anyhow::Result<()> {
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true)),
        )
        .await?;
    Ok(())
}

pub async fn modal_reply(ctx: &serenity::Context, modal: &ModalInteraction, text: &str) -> anyhow::Result<()> {
    modal
        .create_response(
            ctx,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true)),
        )
        .await?;
    Ok(())
}
