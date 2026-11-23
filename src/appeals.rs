//! Ban itirazı: yasaklanan üyeye giden DM'de "İtiraz et" butonu olur (itiraz kanalı ayarlıysa).
//! Üye formu doldurur; itiraz yetkili kanalına Kabul/Ret butonlarıyla düşer. Kabulde yasak kalkar.
//!
//! DM'deki butonda sunucu bilgisi olmadığı için sunucu ID'si buton kimliğindedir
//! (`appeal:open:<sunucu>`). Bekleyen itirazı olan veya son 7 günde reddedilen tekrar itiraz edemez.

use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow, CreateButton,
    CreateEmbed, CreateEmbedAuthor, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreateMessage, CreateModal, EditMessage, GuildId, Mentionable, MessageId, ModalInteraction,
    Permissions, UserId,
};
use sqlx::PgPool;
use tracing::{debug, info, warn};

use crate::applications::review_buttons;
use crate::data::Data;
use crate::db::snowflake;
use crate::guild_config::to_id;
use crate::modals;
use crate::verification::modal_reply;

pub const PREFIX: &str = "appeal:";

/// Web formu açıksa yasak DM'ine eklenecek bağlantı butonu.
pub fn dm_web_button(link: &str) -> CreateActionRow {
    CreateActionRow::Buttons(vec![CreateButton::new_link(link).label("Web'den itiraz et").emoji('🌐')])
}

/// Web formu açıkken yasak DM'i açıklaması.
pub const DM_WEB_TEXT: &str = "📨 **Yasağa itiraz etmek mi istiyorsun?** Aşağıdaki **Web'den itiraz et** butonuna \
     bas ve formu doldur. _(Bağlantı sadece sana özel ve 30 gün geçerli; kimseyle paylaşma.)_";

/// Yasak DM'ine eklenecek butonlar. Davet varsa önce itiraz sunucusuna katılma butonu gelir:
/// yasaklının botla ortak sunucusu kalmadığından Discord itiraz butonunu ancak o zaman iletir.
pub fn dm_buttons(guild_id: GuildId, invite: Option<&str>) -> CreateActionRow {
    let mut buttons = Vec::new();
    if let Some(invite) = invite {
        buttons.push(CreateButton::new_link(invite).label("1. İtiraz sunucusuna katıl").emoji('🚪'));
    }
    let label = if invite.is_some() { "2. Yasağa itiraz et" } else { "Yasağa itiraz et" };
    buttons.push(
        CreateButton::new(format!("{PREFIX}open:{guild_id}"))
            .label(label)
            .emoji('📨')
            .style(ButtonStyle::Primary),
    );
    CreateActionRow::Buttons(buttons)
}

/// Yasak DM'inde butonların üstündeki açıklama.
pub fn dm_text(invite: Option<&str>) -> &'static str {
    if invite.is_some() {
        "📨 **Yasağa itiraz etmek mi istiyorsun?** Önce **İtiraz sunucusuna katıl** butonuyla küçük itiraz \
         sunucumuza gir, sonra **Yasağa itiraz et** butonuna bas. _(Yasaklıyken Discord butonu ancak böyle iletir.)_"
    } else {
        "📨 Yasağa itiraz etmek istersen aşağıdaki butonu kullanabilirsin."
    }
}

/// discord.gg/kod veya discord.com/invite/kod → kod
pub fn invite_code(link: &str) -> Option<String> {
    let link = link.trim().trim_end_matches('/');
    let code = link
        .rsplit_once("discord.gg/")
        .or_else(|| link.rsplit_once("discord.com/invite/"))
        .map_or(link, |(_, code)| code);
    let valid = !code.is_empty() && code.len() <= 32 && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    valid.then(|| code.to_owned())
}

fn parse(custom_id: &str) -> Option<(&str, i64)> {
    let rest = custom_id.strip_prefix(PREFIX)?;
    let (kind, id) = rest.split_once(':')?;
    Some((kind, id.parse().ok()?))
}

fn can_review(interaction: &ComponentInteraction) -> bool {
    interaction
        .member
        .as_ref()
        .and_then(|m| m.permissions)
        .is_some_and(|p| p.contains(Permissions::BAN_MEMBERS) || p.contains(Permissions::ADMINISTRATOR))
}

pub async fn handle_button(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let Some((kind, id)) = parse(&interaction.data.custom_id) else {
        return Ok(());
    };
    match kind {
        "open" => open(ctx, data, interaction, id).await,
        "accept" | "deny" if !can_review(interaction) => {
            ephemeral(ctx, interaction, "İtirazları sadece 'Üyeleri Yasakla' yetkisi olanlar değerlendirebilir.").await
        }
        "accept" => {
            interaction.defer_ephemeral(ctx).await?;
            let text = decide(ctx, &data.db, id, true, None, interaction.user.id).await?;
            interaction.edit_response(ctx, serenity::EditInteractionResponse::new().content(text)).await?;
            Ok(())
        }
        "deny" => {
            let modal = CreateModal::new(format!("{PREFIX}reason:{id}"), "Ret sebebi")
                .components(vec![modals::text_field("reason", "Sebep (itiraz edene iletilir, boş olabilir)", true, false, 500)]);
            interaction.create_response(ctx, CreateInteractionResponse::Modal(modal)).await?;
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn open(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction, guild: i64) -> anyhow::Result<()> {
    let Some(guild_id) = to_id(Some(guild)).map(GuildId::new) else {
        return Ok(());
    };
    info!(guild = %guild_id, user = %interaction.user.id, "İtiraz butonuna basıldı");
    if data.guild_configs.get(&data.db, guild_id).await?.appeal_channel.is_none() {
        return ephemeral(ctx, interaction, "Bu sunucu şu an itiraz kabul etmiyor.").await;
    }
    if let Some(problem) = blocked(&data.db, guild_id, interaction.user.id).await? {
        return ephemeral(ctx, interaction, &problem).await;
    }
    let modal = CreateModal::new(format!("{PREFIX}submit:{guild_id}"), "Yasak itirazı").components(vec![
        modals::text_field("why", "Neden yasaklandığını düşünüyorsun?", true, true, 1000),
        modals::text_field("plea", "Yasağın neden kaldırılmalı?", true, true, 1000),
    ]);
    interaction.create_response(ctx, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}

/// İtiraz edemiyorsa sebebi.
pub(crate) async fn blocked(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<Option<String>> {
    let (pending, recent_denied): (bool, bool) = sqlx::query_as(
        "SELECT COALESCE(bool_or(status = 'pending'), FALSE),
                COALESCE(bool_or(status = 'denied' AND created_at > now() - interval '7 days'), FALSE)
         FROM ban_appeals WHERE guild_id = $1 AND user_id = $2",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(user.get())?)
    .fetch_one(db)
    .await?;
    Ok(if pending {
        Some("⏳ Zaten değerlendirilmeyi bekleyen bir itirazın var.".to_owned())
    } else if recent_denied {
        Some("Son itirazın reddedildi; 7 gün sonra tekrar itiraz edebilirsin.".to_owned())
    } else {
        None
    })
}

pub async fn handle_modal(ctx: &serenity::Context, data: &Data, modal: &ModalInteraction) -> anyhow::Result<()> {
    let Some((kind, id)) = parse(&modal.data.custom_id) else {
        return Ok(());
    };
    match kind {
        "submit" => submit(ctx, data, modal, id).await,
        "reason" => {
            modal.defer_ephemeral(ctx).await?;
            let reason = modals::values(modal).into_iter().next().filter(|r| !r.trim().is_empty());
            let text = decide(ctx, &data.db, id, false, reason, modal.user.id).await?;
            modal.edit_response(ctx, serenity::EditInteractionResponse::new().content(text)).await?;
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn submit(ctx: &serenity::Context, data: &Data, modal: &ModalInteraction, guild: i64) -> anyhow::Result<()> {
    let Some(guild_id) = to_id(Some(guild)).map(GuildId::new) else {
        return Ok(());
    };
    let channel = data.guild_configs.get(&data.db, guild_id).await?.appeal_channel;
    let answers = modals::values(modal);
    let why = answers.first().cloned().unwrap_or_default();
    let plea = answers.get(1).cloned().unwrap_or_default();
    let text = match file_appeal(&ctx.http, &data.db, guild_id, channel, &modal.user, &why, &plea).await? {
        Filed::Sent => "✅ İtirazın yetkililere iletildi. Sonuç sana DM ile bildirilecek.".to_owned(),
        other => other.message(),
    };
    modal_reply(ctx, modal, &text).await
}

/// İtiraz kaydının sonucu (Discord formu ve web formu ortak kullanır).
pub enum Filed {
    Sent,
    Closed,
    Blocked(String),
    NotBanned,
    Failed,
}

impl Filed {
    pub fn message(&self) -> String {
        match self {
            Self::Sent => "✅ İtirazın yetkililere iletildi. Sonuç sana Discord DM'i ile bildirilecek.".to_owned(),
            Self::Closed => "Bu sunucu şu an itiraz kabul etmiyor.".to_owned(),
            Self::Blocked(reason) => reason.clone(),
            Self::NotBanned => "Bu sunucuda yasaklı görünmüyorsun; itiraza gerek yok.".to_owned(),
            Self::Failed => "❌ İtirazın iletilemedi, daha sonra tekrar dene.".to_owned(),
        }
    }
}

/// İtirazı kaydeder ve yetkili kanalına Kabul/Ret butonlarıyla gönderir.
pub async fn file_appeal(
    http: &serenity::Http,
    db: &PgPool,
    guild_id: GuildId,
    channel: Option<ChannelId>,
    user: &serenity::User,
    why: &str,
    plea: &str,
) -> anyhow::Result<Filed> {
    let Some(channel) = channel else {
        return Ok(Filed::Closed);
    };
    if let Some(problem) = blocked(db, guild_id, user.id).await? {
        return Ok(Filed::Blocked(problem));
    }
    // Hâlâ yasaklı mı? (Yasak kalkmışsa itiraza gerek yok.)
    if http.get_ban(guild_id, user.id).await.ok().flatten().is_none() {
        return Ok(Filed::NotBanned);
    }

    let content = format!("Neden yasaklandı: {why}\nNeden kaldırılmalı: {plea}");
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO ban_appeals (guild_id, user_id, content, channel_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(user.id.get())?)
    .bind(&content)
    .bind(snowflake(channel.get())?)
    .fetch_one(db)
    .await?;

    let embed = CreateEmbed::new()
        .author(CreateEmbedAuthor::new(user.tag()).icon_url(user.face()))
        .title(format!("📨 Yasak itirazı #{id}"))
        .colour(serenity::Colour::ORANGE)
        .field("Kullanıcı", format!("{} (`{}`)", user.id.mention(), user.id), false)
        .field("Neden yasaklandığını düşünüyor", truncate(why), false)
        .field("Yasak neden kaldırılmalı", truncate(plea), false)
        .timestamp(serenity::Timestamp::now());
    let buttons = review_buttons(&format!("{PREFIX}accept:{id}"), &format!("{PREFIX}deny:{id}"), false);
    match channel.send_message(http, CreateMessage::new().embed(embed).components(buttons)).await {
        Ok(message) => {
            sqlx::query("UPDATE ban_appeals SET message_id = $2 WHERE id = $1")
                .bind(id)
                .bind(snowflake(message.id.get())?)
                .execute(db)
                .await?;
        }
        Err(e) => {
            sqlx::query("DELETE FROM ban_appeals WHERE id = $1").bind(id).execute(db).await?;
            warn!(guild = %guild_id, error = %e, "İtiraz kanala gönderilemedi");
            return Ok(Filed::Failed);
        }
    }
    info!(guild = %guild_id, user = %user.id, appeal = id, "Ban itirazı alındı");
    Ok(Filed::Sent)
}

// ===================== Web formu anahtarları =====================

/// Web itiraz bağlantısının geçerlilik süresi.
pub const TOKEN_DAYS: i64 = 30;

/// Kişiye ve yasağa özel, tahmin edilemez anahtar (2 × UUIDv4 = 244 bit rastgelelik).
pub async fn create_token(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<String> {
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    sqlx::query("INSERT INTO appeal_tokens (token, guild_id, user_id) VALUES ($1, $2, $3)")
        .bind(&token)
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .execute(db)
        .await?;
    Ok(token)
}

pub enum TokenState {
    Valid { guild_id: GuildId, user: UserId },
    Used,
    Expired,
    Unknown,
}

pub async fn token_state(db: &PgPool, token: &str) -> anyhow::Result<TokenState> {
    if token.len() != 64 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(TokenState::Unknown);
    }
    let row: Option<(i64, i64, bool, bool)> = sqlx::query_as(
        "SELECT guild_id, user_id, used, created_at < now() - make_interval(days => $2::int)
         FROM appeal_tokens WHERE token = $1",
    )
    .bind(token)
    .bind(i32::try_from(TOKEN_DAYS)?)
    .fetch_optional(db)
    .await?;
    Ok(match row {
        None => TokenState::Unknown,
        Some((_, _, true, _)) => TokenState::Used,
        Some((_, _, _, true)) => TokenState::Expired,
        Some((g, u, false, false)) => match (to_id(Some(g)), to_id(Some(u))) {
            (Some(g), Some(u)) => TokenState::Valid { guild_id: GuildId::new(g), user: UserId::new(u) },
            _ => TokenState::Unknown,
        },
    })
}

/// Bağlantıyı tek seferde "kullanıldı" yapar; aynı anda gelen ikinci gönderim `false` alır.
pub async fn claim_token(db: &PgPool, token: &str) -> anyhow::Result<bool> {
    let claimed = sqlx::query("UPDATE appeal_tokens SET used = TRUE WHERE token = $1 AND NOT used")
        .bind(token)
        .execute(db)
        .await?
        .rows_affected();
    Ok(claimed == 1)
}

/// Gönderim tamamlanamadıysa bağlantı yeniden kullanılabilir olur.
pub async fn release_token(db: &PgPool, token: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE appeal_tokens SET used = FALSE WHERE token = $1").bind(token).execute(db).await?;
    Ok(())
}

/// Web formu için itiraz kanalı (önbellek olmadan, doğrudan veritabanından).
pub async fn appeal_channel(db: &PgPool, guild_id: GuildId) -> anyhow::Result<Option<ChannelId>> {
    let id: Option<Option<i64>> = sqlx::query_scalar("SELECT appeal_channel_id FROM guild_settings WHERE guild_id = $1")
        .bind(snowflake(guild_id.get())?)
        .fetch_optional(db)
        .await?;
    Ok(to_id(id.flatten()).map(ChannelId::new))
}

#[derive(sqlx::FromRow)]
struct AppealRow {
    guild_id: i64,
    user_id: i64,
    channel_id: Option<i64>,
    message_id: Option<i64>,
}

async fn decide(ctx: &serenity::Context, db: &PgPool, id: i64, accept: bool, reason: Option<String>, reviewer: UserId) -> anyhow::Result<String> {
    let status = if accept { "accepted" } else { "denied" };
    let row: Option<AppealRow> = sqlx::query_as(
        "UPDATE ban_appeals SET status = $2, reviewer_id = $3, reason = $4
         WHERE id = $1 AND status = 'pending' RETURNING guild_id, user_id, channel_id, message_id",
    )
    .bind(id)
    .bind(status)
    .bind(snowflake(reviewer.get())?)
    .bind(reason.as_deref())
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return Ok("Bu itiraz zaten sonuçlandırılmış.".to_owned());
    };
    let (Some(guild), Some(user)) = (to_id(Some(row.guild_id)), to_id(Some(row.user_id))) else {
        return Ok("İtiraz kaydı bozuk.".to_owned());
    };
    let (guild_id, user_id) = (GuildId::new(guild), UserId::new(user));

    let mut notes = Vec::new();
    if accept {
        let audit = format!("İtiraz #{id} kabul edildi");
        if let Err(e) = ctx.http.remove_ban(guild_id, user_id, Some(&audit)).await {
            warn!(error = %e, "İtiraz sonrası yasak kaldırılamadı");
            notes.push("⚠️ Yasak kaldırılamadı (zaten kalkmış veya yetkim yok).".to_owned());
        }
    }

    if let (Some(channel), Some(message)) = (to_id(row.channel_id), to_id(row.message_id)) {
        let (channel, message) = (ChannelId::new(channel), MessageId::new(message));
        if let Ok(original) = channel.message(ctx, message).await {
            let mut embed = original.embeds.first().cloned().map(CreateEmbed::from).unwrap_or_default();
            let verdict = if accept { "✅ Kabul edildi, yasak kaldırıldı" } else { "❌ Reddedildi" };
            let mut value = format!("{verdict} — {}", reviewer.mention());
            if let Some(reason) = &reason {
                value.push_str(&format!("\n> {reason}"));
            }
            embed = embed
                .field("Sonuç", value, false)
                .colour(if accept { serenity::Colour::from_rgb(87, 242, 135) } else { serenity::Colour::from_rgb(237, 66, 69) });
            let buttons = review_buttons(&format!("{PREFIX}accept:{id}"), &format!("{PREFIX}deny:{id}"), true);
            if let Err(e) = channel.edit_message(ctx, message, EditMessage::new().embed(embed).components(buttons)).await {
                debug!(error = %e, "İtiraz mesajı güncellenemedi");
            }
        }
    }

    let guild_name = ctx.cache.guild(guild_id).map(|g| g.name.clone()).unwrap_or_else(|| "sunucu".to_owned());
    // Kabulde kişiye özel davet: tek kullanımlık, 7 gün geçerli.
    let invite = if accept { rejoin_invite(ctx, guild_id).await } else { None };
    let mut dm = match (accept, &invite) {
        (true, Some(link)) => format!(
            "🎉 **{guild_name}** sunucusundaki yasağına yaptığın itiraz **kabul edildi**; yasağın kaldırıldı.\n\
             Aşağıdaki butonla sunucuya geri dönebilirsin: {link}\n_(Davet sadece sana özel: tek kullanımlık, 7 gün geçerli.)_"
        ),
        (true, None) => format!(
            "🎉 **{guild_name}** sunucusundaki yasağına yaptığın itiraz **kabul edildi**; yasağın kaldırıldı. \
             Bir davet bağlantısıyla tekrar katılabilirsin."
        ),
        (false, _) => format!("📭 **{guild_name}** sunucusundaki yasağına yaptığın itiraz reddedildi."),
    };
    if let Some(reason) = &reason {
        dm.push_str(&format!("\n> {reason}"));
    }
    let mut message = CreateMessage::new().content(dm);
    if let Some(link) = &invite {
        message = message.components(vec![CreateActionRow::Buttons(vec![
            CreateButton::new_link(link).label("Sunucuya geri dön").emoji('🏠'),
        ])]);
    }
    if accept && invite.is_none() {
        notes.push("Davet oluşturulamadı (botta 'Davet Oluştur' izni olmalı).".to_owned());
    }
    let delivered = match user_id.to_user(ctx).await {
        Ok(user) => user.direct_message(ctx, message).await.is_ok(),
        Err(_) => false,
    };
    if !delivered {
        notes.push("Kullanıcıya DM gönderilemedi.".to_owned());
    }
    info!(guild = %guild_id, appeal = id, accept, "Ban itirazı sonuçlandı");
    let verdict = if accept { "kabul edildi, yasak kaldırıldı" } else { "reddedildi" };
    Ok(format!("✅ İtiraz #{id} {verdict}. {}", notes.join(" ")))
}

/// İtirazı kabul edilip ana sunucuya geri dönen üyeyi itiraz sunucusundan atar.
pub async fn leave_appeal_server(ctx: &serenity::Context, data: &Data, guild_id: GuildId, user: UserId) {
    let cfg = match data.guild_configs.get(&data.db, guild_id).await {
        Ok(cfg) => cfg,
        Err(_) => return,
    };
    let appeal_guild = match (cfg.appeal_guild, cfg.appeal_invite.as_deref()) {
        (Some(g), _) => g,
        // Daha önce sadece davet kaydedildiyse sunucu davetten bulunup kaydedilir.
        (None, Some(link)) => {
            let Some(code) = invite_code(link) else { return };
            let Some(g) = ctx.http.get_invite(&code, false, false, None).await.ok().and_then(|i| i.guild.map(|g| g.id)) else {
                return;
            };
            let setting = crate::guild_config::Setting::AppealChannel(cfg.appeal_channel, cfg.appeal_invite.clone(), Some(g));
            if let Err(e) = data.guild_configs.set(&data.db, guild_id, setting).await {
                warn!(error = ?e, "İtiraz sunucusu kaydedilemedi");
            }
            g
        }
        (None, None) => return,
    };
    if appeal_guild == guild_id {
        return;
    }
    // Sadece itirazı kabul edilenler atılır.
    let accepted: bool = match (snowflake(guild_id.get()), snowflake(user.get())) {
        (Ok(g), Ok(u)) => sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM ban_appeals WHERE guild_id = $1 AND user_id = $2 AND status = 'accepted')",
        )
        .bind(g)
        .bind(u)
        .fetch_one(&data.db)
        .await
        .unwrap_or(false),
        _ => false,
    };
    if !accepted {
        return;
    }
    // Önbelleğe güvenilmez (üye listesi eksik olabilir): doğrudan denenir, üye orada değilse
    // Discord "Unknown Member" der ve sessizce geçilir.
    match ctx
        .http
        .kick_member(appeal_guild, user, Some("İtirazı kabul edildi ve ana sunucuya döndü"))
        .await
    {
        Ok(()) => info!(guild = %guild_id, user = %user, "İtiraz sunucusundan çıkarıldı"),
        Err(e) if e.to_string().contains("Unknown Member") => {
            debug!(user = %user, "Üye zaten itiraz sunucusunda değil");
        }
        Err(e) => warn!(
            appeal_guild = %appeal_guild,
            user = %user,
            error = %e,
            "İtiraz sunucusundan atılamadı (itiraz sunucusunda botun 'Üyeleri At' izni olmalı)"
        ),
    }
}

/// Yasağı kalkan kişi için tek kullanımlık, 7 gün geçerli davet. Sistem kanalı (yoksa ilk metin
/// kanalı) üzerinden oluşturulur; bot davet oluşturamıyorsa `None`.
async fn rejoin_invite(ctx: &serenity::Context, guild_id: GuildId) -> Option<String> {
    let candidates: Vec<ChannelId> = {
        let guild = ctx.cache.guild(guild_id)?;
        let mut text: Vec<_> = guild
            .channels
            .values()
            .filter(|c| c.kind == serenity::ChannelType::Text)
            .map(|c| (c.position, c.id))
            .collect();
        text.sort();
        guild.system_channel_id.into_iter().chain(text.into_iter().map(|(_, id)| id)).collect()
    };
    let builder = serenity::CreateInvite::new()
        .max_age(7 * 24 * 3600)
        .max_uses(1)
        .unique(true)
        .audit_log_reason("Kabul edilen yasak itirazı");
    // İlk birkaç kanalı dene (bazılarında davet izni olmayabilir).
    for channel in candidates.into_iter().take(5) {
        match channel.create_invite(ctx, builder.clone()).await {
            Ok(invite) => return Some(invite.url()),
            Err(e) => debug!(channel = %channel, error = %e, "Davet oluşturulamadı"),
        }
    }
    None
}

fn truncate(text: &str) -> String {
    let t: String = text.chars().take(1024).collect();
    if t.trim().is_empty() { "—".to_owned() } else { t }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_invite_links() {
        assert_eq!(invite_code("https://discord.gg/AbC123"), Some("AbC123".to_owned()));
        assert_eq!(invite_code("discord.com/invite/xyz-9/"), Some("xyz-9".to_owned()));
        assert_eq!(invite_code("AbC123"), Some("AbC123".to_owned()));
        assert_eq!(invite_code("https://example.com/a b"), None);
    }

    #[test]
    fn parses_appeal_ids() {
        assert_eq!(parse("appeal:open:1556440251059408966"), Some(("open", 1556440251059408966)));
        assert_eq!(parse("appeal:deny:3"), Some(("deny", 3)));
        assert_eq!(parse("app:open:3"), None);
    }
}
