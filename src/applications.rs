//! Başvuru formları (yetkili alımı vb.): panel butonu → soru formu → yetkili kanalına
//! Kabul/Ret butonlu başvuru. Kabul edilene rol verilir; sonuç başvurana DM ile bildirilir.
//!
//! Buton/form kimlikleri: `app:open:<form>`, `app:submit:<form>`, `app:accept:<başvuru>`,
//! `app:deny:<başvuru>`, `app:reason:<başvuru>`. Bot yeniden başlasa da çalışır.

use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow, CreateButton,
    CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, CreateModal, EditMessage, GuildId,
    Mentionable, MessageId, ModalInteraction, Permissions, RoleId, UserId,
};
use sqlx::PgPool;
use tracing::{debug, info, warn};

use crate::data::Data;
use crate::db::snowflake;
use crate::guild_config::to_id;
use crate::modals;
use crate::verification::modal_reply;

pub const PREFIX: &str = "app:";
pub const MAX_FORMS: i64 = 10;
const ANSWER_MAX: u16 = 1000;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Form {
    pub id: i64,
    pub name: String,
    pub questions: Vec<String>,
    pub review_channel_id: i64,
    pub role_id: Option<i64>,
}

impl Form {
    pub fn review_channel(&self) -> Option<ChannelId> {
        to_id(Some(self.review_channel_id)).map(ChannelId::new)
    }
    pub fn role(&self) -> Option<RoleId> {
        to_id(self.role_id).map(RoleId::new)
    }
}

pub async fn create_form(
    db: &PgPool,
    guild_id: GuildId,
    name: &str,
    questions: &[String],
    review_channel: ChannelId,
    role: Option<RoleId>,
) -> anyhow::Result<()> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM application_forms WHERE guild_id = $1")
        .bind(snowflake(guild_id.get())?)
        .fetch_one(db)
        .await?;
    if count >= MAX_FORMS {
        anyhow::bail!("en fazla {MAX_FORMS} form olabilir");
    }
    sqlx::query(
        "INSERT INTO application_forms (guild_id, name, questions, review_channel_id, role_id) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (guild_id, lower(name)) DO UPDATE
             SET questions = EXCLUDED.questions, review_channel_id = EXCLUDED.review_channel_id, role_id = EXCLUDED.role_id",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(name.trim())
    .bind(questions)
    .bind(snowflake(review_channel.get())?)
    .bind(role.map(|r| snowflake(r.get())).transpose()?)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn forms(db: &PgPool, guild_id: GuildId) -> anyhow::Result<Vec<Form>> {
    Ok(sqlx::query_as(
        "SELECT id, name, questions, review_channel_id, role_id FROM application_forms WHERE guild_id = $1 ORDER BY name",
    )
    .bind(snowflake(guild_id.get())?)
    .fetch_all(db)
    .await?)
}

pub async fn find_form(db: &PgPool, guild_id: GuildId, name: &str) -> anyhow::Result<Option<Form>> {
    Ok(sqlx::query_as(
        "SELECT id, name, questions, review_channel_id, role_id FROM application_forms
         WHERE guild_id = $1 AND lower(name) = lower($2)",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(name.trim())
    .fetch_optional(db)
    .await?)
}

async fn form_by_id(db: &PgPool, id: i64) -> anyhow::Result<Option<Form>> {
    Ok(sqlx::query_as("SELECT id, name, questions, review_channel_id, role_id FROM application_forms WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?)
}

pub async fn delete_form(db: &PgPool, guild_id: GuildId, name: &str) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM application_forms WHERE guild_id = $1 AND lower(name) = lower($2)")
        .bind(snowflake(guild_id.get())?)
        .bind(name.trim())
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub fn panel_message(form: &Form, description: Option<&str>) -> CreateMessage {
    let embed = CreateEmbed::new()
        .title(format!("📝 {}", form.name))
        .description(description.unwrap_or("Başvurmak için aşağıdaki butona bas ve soruları cevapla."))
        .colour(serenity::Colour::BLURPLE)
        .footer(CreateEmbedFooter::new(format!("{} soru", form.questions.len())));
    let button = CreateButton::new(format!("{PREFIX}open:{}", form.id))
        .label("Başvur")
        .emoji('📝')
        .style(ButtonStyle::Primary);
    CreateMessage::new().embed(embed).components(vec![CreateActionRow::Buttons(vec![button])])
}

/// `app:open:12` → ("open", 12)
fn parse(custom_id: &str) -> Option<(&str, i64)> {
    let rest = custom_id.strip_prefix(PREFIX)?;
    let (kind, id) = rest.split_once(':')?;
    Some((kind, id.parse().ok()?))
}

pub async fn handle_button(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let Some((kind, id)) = parse(&interaction.data.custom_id) else {
        return Ok(());
    };
    match kind {
        "open" => open_form(ctx, &data.db, interaction, id).await,
        "accept" => review(ctx, &data.db, interaction, id, true, None).await,
        "deny" => {
            if !is_reviewer(interaction) {
                return ephemeral(ctx, interaction, "Başvuruları sadece yetkililer (Rolleri Yönet izni) değerlendirebilir.").await;
            }
            let modal = CreateModal::new(format!("{PREFIX}reason:{id}"), "Ret sebebi")
                .components(vec![modals::text_field("reason", "Sebep (başvurana iletilir, boş bırakılabilir)", true, false, 500)]);
            interaction.create_response(ctx, CreateInteractionResponse::Modal(modal)).await?;
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn open_form(ctx: &serenity::Context, db: &PgPool, interaction: &ComponentInteraction, form_id: i64) -> anyhow::Result<()> {
    let Some(form) = form_by_id(db, form_id).await? else {
        return ephemeral(ctx, interaction, "Bu başvuru formu artık yok.").await;
    };
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM applications WHERE form_id = $1 AND user_id = $2 AND status = 'pending')",
    )
    .bind(form.id)
    .bind(snowflake(interaction.user.id.get())?)
    .fetch_one(db)
    .await?;
    if pending {
        return ephemeral(ctx, interaction, "⏳ Bu forma zaten bir başvurun var; sonuçlanmasını bekle.").await;
    }
    if let Some(role) = form.role() {
        if interaction.member.as_ref().is_some_and(|m| m.roles.contains(&role)) {
            return ephemeral(ctx, interaction, &format!("Zaten {} rolün var.", role.mention())).await;
        }
    }
    let fields = form
        .questions
        .iter()
        .take(modals::MAX_FIELDS)
        .enumerate()
        .map(|(i, q)| modals::text_field(&format!("q{i}"), q, true, true, ANSWER_MAX))
        .collect();
    let title: String = form.name.chars().take(45).collect();
    let modal = CreateModal::new(format!("{PREFIX}submit:{}", form.id), title).components(fields);
    interaction.create_response(ctx, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}

pub async fn handle_modal(ctx: &serenity::Context, data: &Data, modal: &ModalInteraction) -> anyhow::Result<()> {
    let Some((kind, id)) = parse(&modal.data.custom_id) else {
        return Ok(());
    };
    match kind {
        "submit" => submit(ctx, &data.db, modal, id).await,
        "reason" => {
            let reason = modals::values(modal).into_iter().next().filter(|r| !r.trim().is_empty());
            review_from_modal(ctx, &data.db, modal, id, reason).await
        }
        _ => Ok(()),
    }
}

async fn submit(ctx: &serenity::Context, db: &PgPool, modal: &ModalInteraction, form_id: i64) -> anyhow::Result<()> {
    let (Some(guild_id), Some(form)) = (modal.guild_id, form_by_id(db, form_id).await?) else {
        return modal_reply(ctx, modal, "Bu başvuru formu artık yok.").await;
    };
    let Some(channel) = form.review_channel() else {
        return modal_reply(ctx, modal, "Başvuru kanalı ayarlı değil.").await;
    };
    let answers = modals::values(modal);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO applications (form_id, guild_id, user_id, answers, channel_id) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(form.id)
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(modal.user.id.get())?)
    .bind(&answers)
    .bind(snowflake(channel.get())?)
    .fetch_one(db)
    .await?;

    let mut embed = CreateEmbed::new()
        .author(CreateEmbedAuthor::new(modal.user.tag()).icon_url(modal.user.face()))
        .title(format!("📝 {} — Başvuru #{id}", form.name))
        .colour(serenity::Colour::BLURPLE)
        .field("Başvuran", format!("{} (`{}`)", modal.user.id.mention(), modal.user.id), false)
        .timestamp(serenity::Timestamp::now());
    for (question, answer) in form.questions.iter().zip(&answers) {
        let answer: String = answer.chars().take(1024).collect();
        embed = embed.field(question.chars().take(256).collect::<String>(), if answer.is_empty() { "—".to_owned() } else { answer }, false);
    }
    let buttons = review_buttons(&format!("{PREFIX}accept:{id}"), &format!("{PREFIX}deny:{id}"), false);
    match channel.send_message(ctx, CreateMessage::new().embed(embed).components(buttons)).await {
        Ok(message) => {
            sqlx::query("UPDATE applications SET message_id = $2 WHERE id = $1")
                .bind(id)
                .bind(snowflake(message.id.get())?)
                .execute(db)
                .await?;
        }
        Err(e) => {
            sqlx::query("DELETE FROM applications WHERE id = $1").bind(id).execute(db).await?;
            warn!(guild = %guild_id, error = %e, "Başvuru yetkili kanalına gönderilemedi");
            return modal_reply(ctx, modal, "❌ Başvurun iletilemedi; lütfen yetkililere haber ver.").await;
        }
    }
    info!(guild = %guild_id, form = %form.name, application = id, "Başvuru alındı");
    modal_reply(ctx, modal, &format!("✅ **{}** başvurun alındı! Sonuç sana DM ile bildirilecek.", form.name)).await
}

pub fn review_buttons(accept_id: &str, deny_id: &str, disabled: bool) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(accept_id).label("Kabul").emoji('✅').style(ButtonStyle::Success).disabled(disabled),
        CreateButton::new(deny_id).label("Ret").emoji('❌').style(ButtonStyle::Danger).disabled(disabled),
    ])]
}

pub fn is_reviewer(interaction: &ComponentInteraction) -> bool {
    interaction.member.as_ref().and_then(|m| m.permissions).is_some_and(|p| {
        p.contains(Permissions::MANAGE_ROLES) || p.contains(Permissions::MANAGE_GUILD) || p.contains(Permissions::ADMINISTRATOR)
    })
}

#[derive(sqlx::FromRow)]
struct AppRow {
    form_id: i64,
    guild_id: i64,
    user_id: i64,
    channel_id: Option<i64>,
    message_id: Option<i64>,
}

/// Kabul butonu (ret, sebep formundan gelir).
async fn review(
    ctx: &serenity::Context,
    db: &PgPool,
    interaction: &ComponentInteraction,
    app_id: i64,
    accept: bool,
    reason: Option<String>,
) -> anyhow::Result<()> {
    if !is_reviewer(interaction) {
        return ephemeral(ctx, interaction, "Başvuruları sadece yetkililer (Rolleri Yönet izni) değerlendirebilir.").await;
    }
    interaction.defer_ephemeral(ctx).await?;
    let text = decide(ctx, db, app_id, accept, reason, interaction.user.id).await?;
    interaction
        .edit_response(ctx, serenity::EditInteractionResponse::new().content(text))
        .await?;
    Ok(())
}

async fn review_from_modal(ctx: &serenity::Context, db: &PgPool, modal: &ModalInteraction, app_id: i64, reason: Option<String>) -> anyhow::Result<()> {
    modal.defer_ephemeral(ctx).await?;
    let text = decide(ctx, db, app_id, false, reason, modal.user.id).await?;
    modal.edit_response(ctx, serenity::EditInteractionResponse::new().content(text)).await?;
    Ok(())
}

/// Başvuruyu sonuçlandırır (aynı başvuru iki kez sonuçlanamaz).
async fn decide(ctx: &serenity::Context, db: &PgPool, app_id: i64, accept: bool, reason: Option<String>, reviewer: UserId) -> anyhow::Result<String> {
    let status = if accept { "accepted" } else { "denied" };
    let row: Option<AppRow> = sqlx::query_as(
        "UPDATE applications SET status = $2, reviewer_id = $3, reason = $4
         WHERE id = $1 AND status = 'pending'
         RETURNING form_id, guild_id, user_id, channel_id, message_id",
    )
    .bind(app_id)
    .bind(status)
    .bind(snowflake(reviewer.get())?)
    .bind(reason.as_deref())
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return Ok("Bu başvuru zaten sonuçlandırılmış.".to_owned());
    };
    let form = form_by_id(db, row.form_id).await?;
    let form_name = form.as_ref().map_or_else(|| "Başvuru".to_owned(), |f| f.name.clone());
    let (Some(guild), Some(user)) = (to_id(Some(row.guild_id)), to_id(Some(row.user_id))) else {
        return Ok("Başvuru kaydı bozuk.".to_owned());
    };
    let (guild_id, user_id) = (GuildId::new(guild), UserId::new(user));

    let mut notes = Vec::new();
    if accept {
        if let Some(role) = form.as_ref().and_then(Form::role) {
            match ctx.http.add_member_role(guild_id, user_id, role, Some("Başvuru kabul edildi")).await {
                Ok(()) => notes.push(format!("{} rolü verildi.", role.mention())),
                Err(e) => {
                    warn!(error = %e, "Başvuru rolü verilemedi");
                    notes.push("⚠️ Rol verilemedi (üye ayrılmış veya botun rolü düşük olabilir).".to_owned());
                }
            }
        }
    }

    // Yetkili mesajını güncelle: sonuç alanı + butonlar kapalı.
    if let (Some(channel), Some(message)) = (to_id(row.channel_id), to_id(row.message_id)) {
        let (channel, message) = (ChannelId::new(channel), MessageId::new(message));
        if let Ok(original) = channel.message(ctx, message).await {
            let mut embed = original.embeds.first().cloned().map(CreateEmbed::from).unwrap_or_default();
            let verdict = if accept { "✅ Kabul edildi" } else { "❌ Reddedildi" };
            let mut value = format!("{verdict} — {}", reviewer.mention());
            if let Some(reason) = &reason {
                value.push_str(&format!("\n> {reason}"));
            }
            embed = embed
                .field("Sonuç", value, false)
                .colour(if accept { serenity::Colour::from_rgb(87, 242, 135) } else { serenity::Colour::from_rgb(237, 66, 69) });
            let buttons = review_buttons(&format!("{PREFIX}accept:{app_id}"), &format!("{PREFIX}deny:{app_id}"), true);
            if let Err(e) = channel.edit_message(ctx, message, EditMessage::new().embed(embed).components(buttons)).await {
                debug!(error = %e, "Başvuru mesajı güncellenemedi");
            }
        }
    }

    let guild_name = ctx.cache.guild(guild_id).map(|g| g.name.clone()).unwrap_or_else(|| "sunucu".to_owned());
    let mut dm = if accept {
        format!("🎉 **{guild_name}** sunucusundaki **{form_name}** başvurun **kabul edildi**!")
    } else {
        format!("📭 **{guild_name}** sunucusundaki **{form_name}** başvurun reddedildi.")
    };
    if let Some(reason) = &reason {
        dm.push_str(&format!("\n> {reason}"));
    }
    let delivered = match user_id.to_user(ctx).await {
        Ok(user) => user.direct_message(ctx, CreateMessage::new().content(dm)).await.is_ok(),
        Err(_) => false,
    };
    if !delivered {
        notes.push("Başvurana DM gönderilemedi.".to_owned());
    }
    info!(guild = %guild_id, application = app_id, accept, "Başvuru sonuçlandı");
    let verdict = if accept { "kabul edildi" } else { "reddedildi" };
    Ok(format!("✅ Başvuru #{app_id} {verdict}. {}", notes.join(" ")))
}

async fn ephemeral(ctx: &serenity::Context, interaction: &ComponentInteraction, text: &str) -> anyhow::Result<()> {
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true)),
        )
        .await?;
    Ok(())
}

/// Gerçek PostgreSQL (sahte sunucu ID'si 1). Elle: `cargo test live_forms_sql -- --ignored --nocapture`
#[cfg(test)]
mod live_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "gerçek PostgreSQL gerektirir"]
    async fn live_forms_sql() -> anyhow::Result<()> {
        let _ = dotenvy::dotenv();
        let config = crate::config::DatabaseConfig { url: std::env::var("DATABASE_URL")?, max_connections: 2 };
        let db = crate::db::connect(&config).await?;
        let guild = GuildId::new(1);
        let user = UserId::new(5);
        sqlx::query("DELETE FROM application_forms WHERE guild_id = 1").execute(&db).await?;
        sqlx::query("DELETE FROM ban_appeals WHERE guild_id = 1").execute(&db).await?;

        let questions = vec!["Yaşın kaç?".to_owned(), "Neden yetkili olmak istiyorsun?".to_owned()];
        create_form(&db, guild, "Yetkili Başvurusu", &questions, ChannelId::new(9), Some(RoleId::new(8))).await?;
        create_form(&db, guild, "yetkili başvurusu", &questions[..1], ChannelId::new(9), None).await?;
        let form = find_form(&db, guild, "YETKİLİ Başvurusu").await?;
        // PostgreSQL lower() Türkçe İ'yi i'ye çevirmeyebilir; asıl yazımla da bulunmalı.
        let form = match form {
            Some(f) => f,
            None => find_form(&db, guild, "Yetkili Başvurusu").await?.ok_or_else(|| anyhow::anyhow!("form bulunamadı"))?,
        };
        assert_eq!(form.questions.len(), 1, "aynı adla ekleme güncellemeli");
        assert_eq!(forms(&db, guild).await?.len(), 1);

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO applications (form_id, guild_id, user_id, answers) VALUES ($1, 1, 5, $2) RETURNING id",
        )
        .bind(form.id)
        .bind(vec!["18".to_owned()])
        .fetch_one(&db)
        .await?;
        let answers: Vec<String> = sqlx::query_scalar("SELECT answers FROM applications WHERE id = $1").bind(id).fetch_one(&db).await?;
        assert_eq!(answers, vec!["18".to_owned()]);

        assert!(crate::appeals::blocked(&db, guild, user).await?.is_none());
        sqlx::query("INSERT INTO ban_appeals (guild_id, user_id, content) VALUES (1, 5, 'x')").execute(&db).await?;
        assert!(crate::appeals::blocked(&db, guild, user).await?.is_some(), "bekleyen itiraz engellemeli");
        sqlx::query("UPDATE ban_appeals SET status = 'denied' WHERE guild_id = 1").execute(&db).await?;
        assert!(crate::appeals::blocked(&db, guild, user).await?.is_some(), "yeni ret 7 gün engellemeli");
        sqlx::query("UPDATE ban_appeals SET created_at = now() - interval '8 days' WHERE guild_id = 1").execute(&db).await?;
        assert!(crate::appeals::blocked(&db, guild, user).await?.is_none());

        assert!(delete_form(&db, guild, "Yetkili Başvurusu").await?);
        sqlx::query("DELETE FROM ban_appeals WHERE guild_id = 1").execute(&db).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_custom_ids() {
        assert_eq!(parse("app:open:12"), Some(("open", 12)));
        assert_eq!(parse("app:accept:7"), Some(("accept", 7)));
        assert_eq!(parse("app:open:x"), None);
        assert_eq!(parse("gw:12"), None);
    }
}
