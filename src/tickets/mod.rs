//! Buton tabanlı destek bileti sistemi.
//!
//! Butonların `custom_id`'leri sabittir; bot yeniden başlasa da eski paneller çalışmaya devam eder.
//! - Panel butonu (`ticket:open`) → kullanıcıya özel kanal açılır.
//! - Kapat butonu (`ticket:close`) → konuşma kaydı (transcript) log kanalına ve açan kişiye
//!   gönderilir, kanal silinir.
//! - Üstlen butonu (`ticket:claim`) / `/bilet üstlen` → bileti bir yetkili sahiplenir.
//! - `/bilet ekle` / `/bilet çıkar` → bilete başka üye eklenir veya çıkarılır.

mod transcript;

use std::time::Duration;

use anyhow::Context as _;
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ChannelType, ComponentInteraction, ComponentInteractionDataKind,
    CreateActionRow, CreateAttachment, CreateButton, CreateChannel, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateSelectMenu,
    CreateSelectMenuKind, CreateSelectMenuOption, EditChannel, EditInteractionResponse, GuildId, Mentionable,
    PermissionOverwrite, PermissionOverwriteType, Permissions, RoleId, UserId,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{debug, info, warn};

use crate::data::Data;
use crate::db::snowflake;
use crate::modlog::user_line;

pub const OPEN_BUTTON: &str = "ticket:open";
pub const CLOSE_BUTTON: &str = "ticket:close";
pub const CLAIM_BUTTON: &str = "ticket:claim";
pub const TOPIC_SELECT: &str = "ticket:topic";
pub const REOPEN_BUTTON: &str = "ticket:reopen";
pub const DELETE_BUTTON: &str = "ticket:delete";
/// Memnuniyet butonları: `ticket:rate:<sunucu>:<numara>:<puan>`.
pub const RATE_PREFIX: &str = "ticket:rate:";
pub const MAX_AUTO_CLOSE_HOURS: u32 = 720;
/// Otomatik kapatma döngüsü bu aralıkla bakar.
const AUTO_CLOSE_EVERY: Duration = Duration::from_secs(10 * 60);
pub const MAX_TOPICS: usize = 10;
pub const MAX_TOPIC_LEN: usize = 50;

/// Bilet eklentisinin ek ayarları (kategori ve destek rolü `ticket_settings` tablosundadır).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TicketExtras {
    /// Doluysa panelde buton yerine konu seçilen açılır menü gösterilir.
    pub topics: Vec<String>,
    /// Bilet açılınca gösterilen karşılama metni; boşsa hazır metin.
    pub intro: Option<String>,
    /// Bir üyenin aynı anda açık tutabileceği bilet sayısı.
    pub max_open: u8,
    /// Kapatılan bilet silinmez; saklanır, yeniden açılabilir.
    pub keep_closed: bool,
    /// Saklanan biletlerin taşınacağı kategori.
    pub closed_category: Option<u64>,
    /// Sık sorulan sorular: doluysa yapay zekâ üyenin ilk mesajına buradan cevap vermeyi dener.
    pub faq: Option<String>,
    /// Bu kadar saat kimse yazmazsa bilet kendiliğinden kapanır (0 = kapalı). Bir saat önce uyarılır.
    pub auto_close_hours: u32,
    /// Kapatılınca açana 1-5 yıldızlı memnuniyet sorusu gönderilir.
    pub rating: bool,
}

impl Default for TicketExtras {
    fn default() -> Self {
        Self {
            topics: Vec::new(),
            intro: None,
            max_open: 1,
            keep_closed: false,
            closed_category: None,
            faq: None,
            auto_close_hours: 0,
            rating: false,
        }
    }
}

impl crate::settings::PluginConfig for TicketExtras {
    const KEY: &'static str = "tickets";
}

async fn extras(data: &Data, guild_id: GuildId) -> std::sync::Arc<TicketExtras> {
    match data.settings.get::<TicketExtras>(&data.db, guild_id).await {
        Ok(extras) => extras,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Bilet ayarları okunamadı, varsayılan kullanılıyor");
            std::sync::Arc::new(TicketExtras::default())
        }
    }
}

/// Konudan kanal adı öneki: küçük harf, harf/rakam ve tire; boş kalırsa `ticket`.
pub fn topic_slug(topic: Option<&str>) -> String {
    let slug: String = topic
        .unwrap_or_default()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(20)
        .collect();
    if slug.is_empty() {
        "ticket".to_owned()
    } else {
        slug
    }
}

/// Çift tıklamada iki bilet açılmasını engelleyen Redis kilidinin süresi.
const OPEN_LOCK_SECS: u64 = 15;
/// Kapatma mesajı görüldükten sonra kanalın silinmesine kadar beklenen süre.
const CLOSE_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct TicketSettings {
    pub category_id: Option<ChannelId>,
    pub support_role_id: Option<RoleId>,
}

static SETTINGS_CACHE: std::sync::LazyLock<dashmap::DashMap<GuildId, Option<TicketSettings>>> =
    std::sync::LazyLock::new(dashmap::DashMap::new);

/// Bilet ayar önbelleğini temizler (ayar doğrudan SQL ile güncellendiğinde çağrılabilir).
#[allow(dead_code)]
pub fn invalidate_settings_cache(guild_id: GuildId) {
    SETTINGS_CACHE.remove(&guild_id);
}

pub async fn save_settings(db: &PgPool, guild_id: GuildId, settings: &TicketSettings) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO ticket_settings (guild_id, category_id, support_role_id) VALUES ($1, $2, $3)
         ON CONFLICT (guild_id) DO UPDATE SET category_id = EXCLUDED.category_id,
                                              support_role_id = EXCLUDED.support_role_id,
                                              updated_at = now()",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(settings.category_id.map(|c| snowflake(c.get())).transpose()?)
    .bind(settings.support_role_id.map(|r| snowflake(r.get())).transpose()?)
    .execute(db)
    .await?;
    SETTINGS_CACHE.insert(guild_id, Some(settings.clone()));
    Ok(())
}

pub async fn load_settings(db: &PgPool, guild_id: GuildId) -> anyhow::Result<Option<TicketSettings>> {
    if let Some(cached) = SETTINGS_CACHE.get(&guild_id) {
        return Ok(cached.value().clone());
    }
    let row: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT category_id, support_role_id FROM ticket_settings WHERE guild_id = $1",
    )
    .bind(snowflake(guild_id.get())?)
    .fetch_optional(db)
    .await?;

    let settings = row.map(|(category, role)| TicketSettings {
        category_id: to_id(category).map(ChannelId::new),
        support_role_id: to_id(role).map(RoleId::new),
    });
    SETTINGS_CACHE.insert(guild_id, settings.clone());
    Ok(settings)
}

/// Panel kanalına gönderilecek mesaj: konu tanımlıysa açılır menü, değilse tek buton.
pub fn panel_message(extras: &TicketExtras) -> CreateMessage {
    let embed = CreateEmbed::new()
        .title("🎫 Destek Talebi")
        .description(if extras.topics.is_empty() {
            "Yardıma mı ihtiyacın var? Aşağıdaki butona tıkla; sadece senin ve destek ekibinin \
             görebileceği özel bir kanal açılsın."
        } else {
            "Yardıma mı ihtiyacın var? Aşağıdaki menüden konunu seç; sadece senin ve destek ekibinin \
             görebileceği özel bir kanal açılsın."
        })
        .colour(serenity::Colour::BLURPLE);
    let row = if extras.topics.is_empty() {
        let button = CreateButton::new(OPEN_BUTTON)
            .label("Destek Talebi Aç")
            .emoji('🎫')
            .style(ButtonStyle::Primary);
        CreateActionRow::Buttons(vec![button])
    } else {
        let options = extras
            .topics
            .iter()
            .take(MAX_TOPICS)
            .map(|topic| CreateSelectMenuOption::new(topic.chars().take(100).collect::<String>(), topic.chars().take(100).collect::<String>()))
            .collect();
        CreateActionRow::SelectMenu(CreateSelectMenu::new(TOPIC_SELECT, CreateSelectMenuKind::String { options }).placeholder("Konu seç"))
    };
    CreateMessage::new().embed(embed).components(vec![row])
}

/// Bilet butonlarından gelen etkileşimleri yönlendirir.
pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
) -> anyhow::Result<()> {
    if let Some(rest) = interaction.data.custom_id.strip_prefix(RATE_PREFIX) {
        return rate(ctx, data, interaction, rest).await;
    }
    match interaction.data.custom_id.as_str() {
        OPEN_BUTTON => open(ctx, data, interaction, None).await,
        TOPIC_SELECT => {
            let topic = match &interaction.data.kind {
                ComponentInteractionDataKind::StringSelect { values } => values.first().cloned(),
                _ => None,
            };
            open(ctx, data, interaction, topic).await
        }
        CLOSE_BUTTON => close(ctx, data, interaction).await,
        CLAIM_BUTTON => claim_button(ctx, data, interaction).await,
        REOPEN_BUTTON => reopen_button(ctx, data, interaction).await,
        DELETE_BUTTON => delete_button(ctx, data, interaction).await,
        _ => Ok(()),
    }
}

async fn open(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction, topic: Option<String>) -> anyhow::Result<()> {
    let Some(guild_id) = interaction.guild_id else {
        return Ok(());
    };
    let user = &interaction.user;

    // Kanal oluşturma 3 saniyeyi geçebilir; Discord'a hemen "düşünüyorum" yanıtı ver.
    interaction.defer_ephemeral(ctx).await?;
    let respond = |text: String| async move {
        interaction
            .edit_response(ctx, EditInteractionResponse::new().content(text))
            .await
    };

    // Çift tıklama koruması.
    let lock_key = format!("ticket:lock:{guild_id}:{}", user.id);
    let mut conn = data.redis.clone();
    let locked: Option<String> = redis::cmd("SET")
        .arg(&lock_key)
        .arg(1)
        .arg("NX")
        .arg("EX")
        .arg(OPEN_LOCK_SECS)
        .query_async(&mut conn)
        .await?;
    if locked.is_none() {
        respond("⏳ Biletin zaten oluşturuluyor, lütfen bekle.".to_owned()).await?;
        return Ok(());
    }

    let result = open_inner(ctx, data, guild_id, user, topic.as_deref()).await;
    let _: redis::RedisResult<()> = redis::cmd("DEL").arg(&lock_key).query_async(&mut conn).await;

    let text = match result {
        Ok(OpenOutcome::Created(channel)) => format!("✅ Biletin açıldı: {}", channel.mention()),
        Ok(OpenOutcome::AlreadyOpen(channel)) => {
            format!("ℹ️ Zaten açık bir biletin var: {}", channel.mention())
        }
        Ok(OpenOutcome::NotConfigured) => {
            "❌ Bilet sistemi henüz kurulmamış. Bir yöneticinin `/bilet kur` çalıştırması gerekiyor.".to_owned()
        }
        Err(e) => {
            warn!(guild = %guild_id, user = %user.id, error = ?e, "Bilet açılamadı");
            "❌ Bilet açılamadı. Botun 'Kanalları Yönet' ve 'Rolleri Yönet' iznini kontrol edin.".to_owned()
        }
    };
    respond(text).await?;
    Ok(())
}

enum OpenOutcome {
    Created(ChannelId),
    AlreadyOpen(ChannelId),
    NotConfigured,
}

async fn open_inner(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    user: &serenity::User,
    topic: Option<&str>,
) -> anyhow::Result<OpenOutcome> {
    let db = &data.db;
    let extras = extras(data, guild_id).await;
    // Menüden gelen konu yalnızca tanımlı konulardan biriyse kabul edilir.
    let topic = topic.filter(|t| extras.topics.iter().any(|known| known == t));

    let mut alive = Vec::new();
    for existing in open_tickets_of(db, guild_id, user.id).await? {
        // Kanal elle silinmiş olabilir; o zaman kaydı kapatılır.
        let still_exists = ctx
            .cache
            .guild(guild_id)
            .is_some_and(|g| g.channels.contains_key(&existing));
        if still_exists {
            alive.push(existing);
        } else {
            mark_closed(db, existing, None).await?;
        }
    }
    if let (true, Some(first)) = (alive.len() >= usize::from(extras.max_open.max(1)), alive.first().copied()) {
        return Ok(OpenOutcome::AlreadyOpen(first));
    }

    let Some(settings) = load_settings(db, guild_id).await? else {
        return Ok(OpenOutcome::NotConfigured);
    };

    let number: i32 = sqlx::query_scalar(
        "UPDATE ticket_settings SET counter = counter + 1 WHERE guild_id = $1 RETURNING counter",
    )
    .bind(snowflake(guild_id.get())?)
    .fetch_one(db)
    .await?;

    let bot_id = ctx.cache.current_user().id;
    let member_perms = member_permissions();

    let mut overwrites = vec![
        PermissionOverwrite {
            allow: Permissions::empty(),
            deny: Permissions::VIEW_CHANNEL,
            kind: PermissionOverwriteType::Role(guild_id.everyone_role()),
        },
        PermissionOverwrite {
            allow: member_perms,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(user.id),
        },
        PermissionOverwrite {
            allow: member_perms | Permissions::MANAGE_CHANNELS,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(bot_id),
        },
    ];
    if let Some(role) = settings.support_role_id {
        overwrites.push(PermissionOverwrite {
            allow: member_perms | Permissions::MANAGE_MESSAGES,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Role(role),
        });
    }

    let mut builder = CreateChannel::new(format!("{}-{number:04}", topic_slug(topic)))
        .kind(ChannelType::Text)
        .topic(format!("Destek bileti #{number:04} — {} ({})", user.tag(), user.id))
        .permissions(overwrites)
        .audit_log_reason("Destek bileti açıldı");
    if let Some(category) = settings.category_id {
        builder = builder.category(category);
    }
    let channel = guild_id
        .create_channel(ctx, builder)
        .await
        .context("bilet kanalı oluşturulamadı")?;

    sqlx::query("INSERT INTO tickets (guild_id, number, channel_id, opener_id) VALUES ($1, $2, $3, $4)")
        .bind(snowflake(guild_id.get())?)
        .bind(number)
        .bind(snowflake(channel.id.get())?)
        .bind(snowflake(user.id.get())?)
        .execute(db)
        .await?;

    AWAITING_STAFF.insert(channel.id, (guild_id, user.id));

    let mut ping = user.id.mention().to_string();
    if let Some(role) = settings.support_role_id {
        ping.push_str(&format!(" {}", role.mention()));
    }
    let intro = extras.intro.as_deref().filter(|i| !i.trim().is_empty()).unwrap_or(
        "Destek ekibi en kısa sürede seninle ilgilenecek. Sorununu ayrıntılı şekilde yaz.\n\n\
         İşin bitince aşağıdaki **Bileti Kapat** butonunu kullanabilirsin.",
    );
    let mut welcome = CreateEmbed::new()
        .title(format!("🎫 Bilet #{number:04}"))
        .description(intro)
        .colour(serenity::Colour::BLURPLE)
        .footer(CreateEmbedFooter::new(format!("Açan: {}", user.tag())));
    if let Some(topic) = topic {
        welcome = welcome.field("Konu", topic, false);
    }
    let close_button = CreateButton::new(CLOSE_BUTTON)
        .label("Bileti Kapat")
        .emoji('🔒')
        .style(ButtonStyle::Danger);
    let claim_button = CreateButton::new(CLAIM_BUTTON)
        .label("Üstlen")
        .emoji('🙋')
        .style(ButtonStyle::Secondary);
    channel
        .id
        .send_message(
            ctx,
            CreateMessage::new()
                .content(ping)
                .embed(welcome)
                .components(vec![CreateActionRow::Buttons(vec![claim_button, close_button])]),
        )
        .await?;

    if extras.faq.as_deref().is_some_and(|f| !f.trim().is_empty()) {
        crate::ai::assist::expect_question(channel.id, user.id);
    }
    info!(guild = %guild_id, user = %user.id, ticket = number, "Destek bileti açıldı");
    Ok(OpenOutcome::Created(channel.id))
}

/// Kapatılacak biletin bilgileri (kapatma iki adımdır: önce işaretle ve haber ver, sonra kaydı çıkar).
pub struct Closing {
    number: i32,
    opener: Option<UserId>,
    opened_unix: i64,
    claimed_by: Option<i64>,
}

/// Bileti kapatılmış olarak işaretler. Kapatılamıyorsa kullanıcıya gösterilecek sebep döner.
pub async fn begin_close(
    db: &PgPool,
    guild_id: GuildId,
    channel_id: ChannelId,
    user: UserId,
    member: Option<&serenity::Member>,
) -> anyhow::Result<Result<Closing, &'static str>> {
    let row: Option<(i32, i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT number, opener_id, EXTRACT(EPOCH FROM created_at)::BIGINT, claimed_by
         FROM tickets WHERE channel_id = $1 AND status = 'open'",
    )
    .bind(snowflake(channel_id.get())?)
    .fetch_optional(db)
    .await?;
    let Some((number, opener_id, opened_unix, claimed_by)) = row else {
        return Ok(Err("Bu kanal açık bir destek bileti değil."));
    };
    let opener = to_id(Some(opener_id)).map(UserId::new);
    // Açan kişi, destek rolündekiler veya kanalı yönetebilenler kapatabilir.
    let allowed = opener == Some(user)
        || match member {
            Some(member) => is_staff(db, guild_id, member).await?,
            None => false,
        };
    if !allowed {
        return Ok(Err("Bu bileti sadece açan kişi veya destek ekibi kapatabilir."));
    }
    // Aynı anda iki kişi kapatırsa sadece biri devam eder.
    if !mark_closed(db, channel_id, Some(user)).await? {
        return Ok(Err("Bu bilet zaten kapatılıyor."));
    }
    Ok(Ok(Closing { number, opener, opened_unix, claimed_by }))
}

/// Kapatılırken kanalda gösterilen duyuru.
pub fn closing_notice(closer: UserId, keep: bool) -> String {
    if keep {
        format!("🔒 Bilet {} tarafından kapatıldı. Kayıt oluşturuluyor…", closer.mention())
    } else {
        format!(
            "🔒 Bilet {} tarafından kapatıldı. Kayıt oluşturuluyor, kanal {} saniye içinde silinecek…",
            closer.mention(),
            CLOSE_DELAY.as_secs()
        )
    }
}

/// Konuşma kaydını çıkarır, log kanalına ve açan kişiye gönderir; ayara göre kanalı siler ya da kilitler.
#[allow(clippy::too_many_arguments)]
pub async fn finish_close(
    ctx: &serenity::Context,
    db: &PgPool,
    modlog: &crate::modlog::ModLog,
    guild_id: GuildId,
    channel_id: ChannelId,
    closing: Closing,
    closer: &serenity::User,
    extras: &TicketExtras,
) {
    let Closing { number, opener, opened_unix, claimed_by } = closing;
    let guild_name = ctx
        .cache
        .guild(guild_id)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| guild_id.to_string());
    let opener_user = match opener {
        Some(id) => id.to_user(ctx).await.ok(),
        None => None,
    };

    let header = transcript::Header {
        guild_name: &guild_name,
        number,
        opener: opener_user.as_ref(),
        closed_by: closer,
        opened_unix,
    };
    let text = match transcript::build(ctx, channel_id, &header).await {
        Ok(text) => text,
        Err(e) => {
            warn!(channel = %channel_id, error = ?e, "Konuşma kaydı oluşturulamadı");
            format!("Konuşma kaydı oluşturulamadı: {e}")
        }
    };
    let filename = format!("ticket-{number:04}.txt");

    let mut summary = CreateEmbed::new()
        .title(format!("🎫 Bilet #{number:04} kapatıldı"))
        .colour(serenity::Colour::DARK_GREY)
        .field("Kapatan", user_line(closer), true)
        .field("Açılış", format!("<t:{opened_unix}:f>"), true)
        .timestamp(serenity::Timestamp::now());
    if let Some(opener) = &opener_user {
        summary = summary.field("Açan", user_line(opener), true);
    }
    if let Some(staff) = to_id(claimed_by) {
        summary = summary.field("Üstlenen", format!("<@{staff}>"), true);
    }

    modlog
        .ticket_closed(
            &ctx.http,
            db,
            guild_id,
            summary,
            CreateAttachment::bytes(text.clone().into_bytes(), filename.clone()),
        )
        .await;

    if let Some(opener) = &opener_user {
        let dm = CreateMessage::new()
            .content(format!(
                "🎫 **{guild_name}** sunucusundaki **#{number:04}** numaralı destek biletin kapatıldı. \
                 Konuşma kaydı ektedir.{}",
                if extras.rating { "\n\n**Destekten ne kadar memnun kaldın?**" } else { "" }
            ))
            .add_file(CreateAttachment::bytes(text.into_bytes(), filename));
        let dm = if extras.rating { dm.components(vec![rating_row(guild_id, number)]) } else { dm };
        if let Err(e) = opener.direct_message(ctx, dm).await {
            debug!(user = %opener.id, error = %e, "Bilet kaydı DM ile gönderilemedi");
        }
    }

    if extras.keep_closed {
        archive(ctx, channel_id, number, opener, extras).await;
    } else {
        tokio::time::sleep(CLOSE_DELAY).await;
        if let Err(e) = channel_id.delete(ctx).await {
            warn!(channel = %channel_id, error = %e, "Bilet kanalı silinemedi");
        }
    }
    info!(guild = %guild_id, ticket = number, by = %closer.id, kept = extras.keep_closed, "Destek bileti kapatıldı");
}

/// Kapalı bileti saklar: açan kişi yazamaz, kanal yeniden adlandırılır ve (ayarlıysa) kapalı
/// biletler kategorisine taşınır; "Yeniden Aç" ve "Sil" butonları gönderilir.
async fn archive(ctx: &serenity::Context, channel_id: ChannelId, number: i32, opener: Option<UserId>, extras: &TicketExtras) {
    if let Some(opener) = opener {
        let read_only = PermissionOverwrite {
            allow: Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY,
            deny: Permissions::SEND_MESSAGES,
            kind: PermissionOverwriteType::Member(opener),
        };
        if let Err(e) = channel_id.create_permission(&ctx.http, read_only).await {
            warn!(channel = %channel_id, error = %e, "Kapalı bilette yazma izni kaldırılamadı");
        }
    }
    let mut edit = EditChannel::new().name(format!("kapalı-{number:04}"));
    if let Some(category) = extras.closed_category.filter(|id| *id != 0) {
        edit = edit.category(Some(ChannelId::new(category)));
    }
    if let Err(e) = channel_id.edit(ctx, edit).await {
        debug!(channel = %channel_id, error = %e, "Kapalı bilet kanalı yeniden adlandırılamadı");
    }
    let buttons = vec![
        CreateButton::new(REOPEN_BUTTON).label("Yeniden Aç").emoji('🔓').style(ButtonStyle::Success),
        CreateButton::new(DELETE_BUTTON).label("Sil").emoji('🗑').style(ButtonStyle::Danger),
    ];
    let message = CreateMessage::new()
        .content("📁 Bilet kapatıldı ve saklandı. Destek ekibi yeniden açabilir ya da kalıcı olarak silebilir.")
        .components(vec![CreateActionRow::Buttons(buttons)]);
    if let Err(e) = channel_id.send_message(&ctx.http, message).await {
        warn!(channel = %channel_id, error = %e, "Kapalı bilet mesajı gönderilemedi");
    }
}

async fn close(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let Some(guild_id) = interaction.guild_id else {
        return Ok(());
    };
    let channel_id = interaction.channel_id;
    let closing = match begin_close(&data.db, guild_id, channel_id, interaction.user.id, interaction.member.as_ref()).await? {
        Ok(closing) => closing,
        Err(reason) => return ephemeral(ctx, interaction, reason).await,
    };
    let extras = extras(data, guild_id).await;
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new().content(closing_notice(interaction.user.id, extras.keep_closed)),
            ),
        )
        .await?;
    finish_close(ctx, &data.db, &data.modlog, guild_id, channel_id, closing, &interaction.user, &extras).await;
    Ok(())
}

fn rating_row(guild_id: GuildId, number: i32) -> CreateActionRow {
    let buttons = (1..=5)
        .map(|n| {
            CreateButton::new(format!("{RATE_PREFIX}{guild_id}:{number}:{n}"))
                .label(format!("{n} ⭐"))
                .style(if n >= 4 { ButtonStyle::Success } else { ButtonStyle::Secondary })
        })
        .collect();
    CreateActionRow::Buttons(buttons)
}

/// `<sunucu>:<numara>:<puan>` → değerler (puan 1-5).
fn parse_rating(rest: &str) -> Option<(GuildId, i32, u8)> {
    let mut parts = rest.split(':');
    let guild = parts.next()?.parse::<u64>().ok().filter(|g| *g != 0).map(GuildId::new)?;
    let number = parts.next()?.parse::<i32>().ok()?;
    let score = parts.next()?.parse::<u8>().ok().filter(|s| (1..=5).contains(s))?;
    parts.next().is_none().then_some((guild, number, score))
}

fn rating_key(guild_id: GuildId) -> String {
    format!("bilet:puan:{guild_id}")
}

/// Sunucunun ortalama memnuniyet puanı ve oy sayısı.
pub async fn rating_summary(redis: &redis::aio::ConnectionManager, guild_id: GuildId) -> Option<(f64, u64)> {
    let mut conn = redis.clone();
    let (sum, count): (Option<u64>, Option<u64>) =
        redis::cmd("HMGET").arg(rating_key(guild_id)).arg("toplam").arg("adet").query_async(&mut conn).await.ok()?;
    let (sum, count) = (sum?, count.filter(|c| *c > 0)?);
    Some((sum as f64 / count as f64, count))
}

/// Açanın DM'deki yıldız butonu.
async fn rate(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction, rest: &str) -> anyhow::Result<()> {
    let Some((guild_id, number, score)) = parse_rating(rest) else {
        return Ok(());
    };
    let mut conn = data.redis.clone();
    // Her bilet bir kez puanlanır (DM'deki butonlar yalnızca açana görünür).
    let first: Option<String> = redis::cmd("SET")
        .arg(format!("bilet:puanlandi:{guild_id}:{number}"))
        .arg(1)
        .arg("NX")
        .arg("EX")
        .arg(90 * 86_400)
        .query_async(&mut conn)
        .await?;
    let text = if first.is_some() {
        let _: () = redis::pipe()
            .cmd("HINCRBY").arg(rating_key(guild_id)).arg("toplam").arg(score)
            .cmd("HINCRBY").arg(rating_key(guild_id)).arg("adet").arg(1)
            .query_async(&mut conn)
            .await?;
        let embed = CreateEmbed::new()
            .title(format!("⭐ Bilet #{number:04} puanlandı: {score}/5"))
            .description(format!("{} {}", "⭐".repeat(usize::from(score)), user_line(&interaction.user)))
            .colour(if score >= 4 { serenity::Colour::DARK_GREEN } else if score == 3 { serenity::Colour::GOLD } else { serenity::Colour::RED })
            .timestamp(serenity::Timestamp::now());
        data.modlog.ticket_rated(&ctx.http, &data.db, guild_id, embed).await;
        format!("Teşekkürler! Bileti **{score}/5** ⭐ ile puanladın.")
    } else {
        "Bu bileti zaten puanlamıştın, teşekkürler!".to_owned()
    };
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::UpdateMessage(CreateInteractionResponseMessage::new().content(text).components(Vec::new())),
        )
        .await?;
    Ok(())
}

/// Hareketsiz biletleri kapatır (her sunucunun `auto_close_hours` ayarına göre). Kapanmadan bir saat önce uyarır.
pub fn spawn_auto_close(
    ctx: serenity::Context,
    db: PgPool,
    redis: redis::aio::ConnectionManager,
    settings: crate::settings::PluginSettings,
    modlog: std::sync::Arc<crate::modlog::ModLog>,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(AUTO_CLOSE_EVERY).await;
            crate::supervise::round("bilet otomatik kapatma", Duration::from_secs(600), async {
                if let Err(e) = auto_close_round(&ctx, &db, &redis, &settings, &modlog).await {
                    warn!(error = ?e, "Hareketsiz biletler kontrol edilemedi");
                }
            })
            .await;
        }
    });
}

/// Hareketsizlik durumuna göre ne yapılmalı?
#[derive(Debug, PartialEq, Eq)]
enum Idle {
    Fine,
    Warn,
    Close,
}

fn idle_state(idle_secs: i64, hours: u32) -> Idle {
    let limit = i64::from(hours) * 3600;
    if hours == 0 {
        Idle::Fine
    } else if idle_secs >= limit {
        Idle::Close
    } else if hours >= 2 && idle_secs >= limit - 3600 {
        Idle::Warn
    } else {
        Idle::Fine
    }
}

async fn auto_close_round(
    ctx: &serenity::Context,
    db: &PgPool,
    redis: &redis::aio::ConnectionManager,
    settings: &crate::settings::PluginSettings,
    modlog: &crate::modlog::ModLog,
) -> anyhow::Result<()> {
    let open: Vec<(i64, i64, i64)> =
        sqlx::query_as("SELECT guild_id, channel_id, EXTRACT(EPOCH FROM created_at)::BIGINT FROM tickets WHERE status = 'open'")
            .fetch_all(db)
            .await?;
    let now = serenity::Timestamp::now().unix_timestamp();
    for (guild, channel, created) in open {
        let (Some(guild_id), Some(channel_id)) = (to_id(Some(guild)).map(GuildId::new), to_id(Some(channel)).map(ChannelId::new)) else {
            continue;
        };
        let extras = settings.get::<TicketExtras>(db, guild_id).await?;
        if extras.auto_close_hours == 0 {
            continue;
        }
        // Son mesajın zamanı önbellekten (kanal kimliği gibi mesaj kimliği de zaman taşır).
        let Some(last) = ctx.cache.guild(guild_id).and_then(|g| g.channels.get(&channel_id).map(|c| c.last_message_id)) else {
            continue; // Kanal yok (silinmiş); kanal silme olayı kaydı kapatır.
        };
        let last_unix = last.map_or(created, |m| m.created_at().unix_timestamp()).max(created);
        match idle_state(now - last_unix, extras.auto_close_hours) {
            Idle::Fine => {}
            Idle::Warn => {
                let mut conn = redis.clone();
                let fresh: Option<String> =
                    redis::cmd("SET").arg(format!("bilet:uyari:{channel_id}:{last_unix}")).arg(1).arg("NX").arg("EX").arg(7200).query_async(&mut conn).await?;
                if fresh.is_some() {
                    let text = "⏰ Bu bilette bir süredir yazışma yok. **1 saat** içinde kimse yazmazsa bilet otomatik kapatılacak.";
                    let _ = channel_id.send_message(&ctx.http, CreateMessage::new().content(text)).await;
                }
            }
            Idle::Close => {
                let row: Option<(i32, i64, Option<i64>)> = sqlx::query_as(
                    "SELECT number, opener_id, claimed_by FROM tickets WHERE channel_id = $1 AND status = 'open'",
                )
                .bind(channel)
                .fetch_optional(db)
                .await?;
                let Some((number, opener_id, claimed_by)) = row else { continue };
                let bot = ctx.cache.current_user().id;
                if !mark_closed(db, channel_id, Some(bot)).await? {
                    continue;
                }
                let closing = Closing { number, opener: to_id(Some(opener_id)).map(UserId::new), opened_unix: created, claimed_by };
                let notice = format!(
                    "⏰ Bu bilette {} saattir kimse yazmadığı için otomatik kapatılıyor. Kayıt oluşturuluyor…",
                    extras.auto_close_hours
                );
                let _ = channel_id.send_message(&ctx.http, CreateMessage::new().content(notice)).await;
                let bot_user: serenity::User = (**ctx.cache.current_user()).clone();
                info!(guild = %guild_id, ticket = number, "Hareketsiz bilet otomatik kapatılıyor");
                finish_close(ctx, db, modlog, guild_id, channel_id, closing, &bot_user, &extras).await;
            }
        }
    }
    Ok(())
}

/// Saklanmış (kapalı) bileti yeniden açar. Yalnızca destek ekibi. Sonuç kanalda gösterilecek metindir.
pub async fn reopen_ticket(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    channel_id: ChannelId,
    member: &serenity::Member,
) -> anyhow::Result<Result<String, &'static str>> {
    let db = &data.db;
    if !is_staff(db, guild_id, member).await? {
        return Ok(Err("Biletleri sadece destek ekibi yeniden açabilir."));
    }
    let row: Option<(i32, i64)> = sqlx::query_as("SELECT number, opener_id FROM tickets WHERE channel_id = $1 AND status = 'closed'")
        .bind(snowflake(channel_id.get())?)
        .fetch_optional(db)
        .await?;
    let Some((number, opener_id)) = row else {
        return Ok(Err("Bu kanal kapatılmış bir destek bileti değil."));
    };
    let extras = extras(data, guild_id).await;
    let opener = to_id(Some(opener_id)).map(UserId::new);
    if let Some(opener) = opener {
        if open_count(db, guild_id, opener).await? >= i64::from(extras.max_open.max(1)) {
            return Ok(Err("Bu üyenin zaten açık bileti var; önce o kapatılmalı."));
        }
    }
    let reopened = sqlx::query("UPDATE tickets SET status = 'open', closed_at = NULL, closed_by = NULL WHERE channel_id = $1 AND status = 'closed'")
        .bind(snowflake(channel_id.get())?)
        .execute(db)
        .await?
        .rows_affected();
    if reopened == 0 {
        return Ok(Err("Bu bilet az önce başka biri tarafından değiştirildi."));
    }
    if let Some(opener) = opener {
        add_member(&ctx.http, channel_id, opener).await?;
    }
    let mut edit = EditChannel::new().name(format!("ticket-{number:04}"));
    if let Some(settings) = load_settings(db, guild_id).await? {
        if let Some(category) = settings.category_id {
            edit = edit.category(Some(category));
        }
    }
    if let Err(e) = channel_id.edit(ctx, edit).await {
        debug!(channel = %channel_id, error = %e, "Yeniden açılan bilet kanalı düzenlenemedi");
    }
    info!(guild = %guild_id, ticket = number, by = %member.user.id, "Destek bileti yeniden açıldı");
    Ok(Ok(format!("🔓 Bilet #{number:04} {} tarafından yeniden açıldı.", member.user.id.mention())))
}

/// Saklanmış (kapalı) bileti kalıcı olarak siler. Yalnızca destek ekibi.
pub async fn delete_ticket(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    channel_id: ChannelId,
    member: &serenity::Member,
) -> anyhow::Result<Result<(), &'static str>> {
    let db = &data.db;
    if !is_staff(db, guild_id, member).await? {
        return Ok(Err("Biletleri sadece destek ekibi silebilir."));
    }
    let number: Option<i32> = sqlx::query_scalar("SELECT number FROM tickets WHERE channel_id = $1 AND status = 'closed'")
        .bind(snowflake(channel_id.get())?)
        .fetch_optional(db)
        .await?;
    let Some(number) = number else {
        return Ok(Err("Bu kanal kapatılmış bir destek bileti değil (açık biletler önce kapatılır)."));
    };
    channel_id.delete(ctx).await?;
    info!(guild = %guild_id, ticket = number, by = %member.user.id, "Kapalı destek bileti silindi");
    Ok(Ok(()))
}

async fn reopen_button(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let (Some(guild_id), Some(member)) = (interaction.guild_id, interaction.member.as_ref()) else {
        return Ok(());
    };
    match reopen_ticket(ctx, data, guild_id, interaction.channel_id, member).await? {
        Ok(text) => {
            // Butonlar tekrar kullanılmasın diye eski mesajdan kaldırılır.
            let response = CreateInteractionResponseMessage::new().content(text).components(Vec::new());
            interaction.create_response(ctx, CreateInteractionResponse::UpdateMessage(response)).await?;
            Ok(())
        }
        Err(reason) => ephemeral(ctx, interaction, reason).await,
    }
}

async fn delete_button(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let (Some(guild_id), Some(member)) = (interaction.guild_id, interaction.member.as_ref()) else {
        return Ok(());
    };
    match delete_ticket(ctx, data, guild_id, interaction.channel_id, member).await? {
        Ok(()) => Ok(()),
        Err(reason) => ephemeral(ctx, interaction, reason).await,
    }
}

/// Bilet kanalındaki üyeye verilen izinler.
fn member_permissions() -> Permissions {
    Permissions::VIEW_CHANNEL
        | Permissions::SEND_MESSAGES
        | Permissions::READ_MESSAGE_HISTORY
        | Permissions::ATTACH_FILES
        | Permissions::EMBED_LINKS
}

/// Destek rolündekiler ve kanalları yönetebilenler yetkilidir.
pub async fn is_staff(db: &PgPool, guild_id: GuildId, member: &serenity::Member) -> anyhow::Result<bool> {
    if member
        .permissions
        .is_some_and(|p| p.contains(Permissions::MANAGE_CHANNELS) || p.contains(Permissions::ADMINISTRATOR))
    {
        return Ok(true);
    }
    let support_role = load_settings(db, guild_id)
        .await?
        .and_then(|s| s.support_role_id);
    Ok(support_role.is_some_and(|role| member.roles.contains(&role)))
}

/// Kanaldaki açık bilet: (numara, açan, üstlenen).
pub struct OpenTicket {
    pub number: i32,
    pub opener: Option<UserId>,
    pub claimed_by: Option<UserId>,
}

pub async fn open_ticket_in(db: &PgPool, channel_id: ChannelId) -> anyhow::Result<Option<OpenTicket>> {
    let row: Option<(i32, i64, Option<i64>)> = sqlx::query_as(
        "SELECT number, opener_id, claimed_by FROM tickets WHERE channel_id = $1 AND status = 'open'",
    )
    .bind(snowflake(channel_id.get())?)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(number, opener, claimed)| OpenTicket {
        number,
        opener: to_id(Some(opener)).map(UserId::new),
        claimed_by: to_id(claimed).map(UserId::new),
    }))
}

pub enum ClaimOutcome {
    Claimed(i32),
    AlreadyClaimed(UserId),
    NotATicket,
}

/// Bileti üstlenir; başkası üstlenmişse onu döner.
pub async fn claim(db: &PgPool, channel_id: ChannelId, staff: UserId) -> anyhow::Result<ClaimOutcome> {
    let number: Option<i32> = sqlx::query_scalar(
        "UPDATE tickets SET claimed_by = $2
         WHERE channel_id = $1 AND status = 'open' AND (claimed_by IS NULL OR claimed_by = $2)
         RETURNING number",
    )
    .bind(snowflake(channel_id.get())?)
    .bind(snowflake(staff.get())?)
    .fetch_optional(db)
    .await?;
    if let Some(number) = number {
        return Ok(ClaimOutcome::Claimed(number));
    }
    Ok(match open_ticket_in(db, channel_id).await? {
        Some(OpenTicket { claimed_by: Some(other), .. }) => ClaimOutcome::AlreadyClaimed(other),
        _ => ClaimOutcome::NotATicket,
    })
}

/// Bilet kanalına üye ekler.
pub async fn add_member(http: &serenity::Http, channel_id: ChannelId, user: UserId) -> anyhow::Result<()> {
    let overwrite = PermissionOverwrite {
        allow: member_permissions(),
        deny: Permissions::empty(),
        kind: PermissionOverwriteType::Member(user),
    };
    channel_id.create_permission(http, overwrite).await?;
    Ok(())
}

/// Bilet kanalından üyeyi çıkarır.
pub async fn remove_member(http: &serenity::Http, channel_id: ChannelId, user: UserId) -> anyhow::Result<()> {
    channel_id
        .delete_permission(http, PermissionOverwriteType::Member(user))
        .await?;
    Ok(())
}

async fn claim_button(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let Some(guild_id) = interaction.guild_id else {
        return Ok(());
    };
    let staff = match &interaction.member {
        Some(member) => is_staff(&data.db, guild_id, member).await?,
        None => false,
    };
    if !staff {
        return ephemeral(ctx, interaction, "Biletleri sadece destek ekibi üstlenebilir.").await;
    }
    match claim(&data.db, interaction.channel_id, interaction.user.id).await? {
        ClaimOutcome::Claimed(number) => {
            interaction
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(format!(
                        "🙋 Bilet #{number:04} {} tarafından üstlenildi.",
                        interaction.user.id.mention()
                    ))),
                )
                .await?;
            info!(guild = %guild_id, ticket = number, by = %interaction.user.id, "Bilet üstlenildi");
            Ok(())
        }
        ClaimOutcome::AlreadyClaimed(other) => {
            ephemeral(ctx, interaction, &format!("Bu bileti zaten {} üstlendi.", other.mention())).await
        }
        ClaimOutcome::NotATicket => ephemeral(ctx, interaction, "Bu kanal açık bir destek bileti değil.").await,
    }
}

/// Kanal elle silindiğinde açık bilet kaydını kapatır.
pub async fn on_channel_deleted(db: &PgPool, channel_id: ChannelId) -> anyhow::Result<()> {
    if mark_closed(db, channel_id, None).await? {
        info!(channel = %channel_id, "Elle silinen bilet kanalı kapatıldı olarak işaretlendi");
    }
    Ok(())
}

/// Üyenin açık biletlerinin kanalları.
async fn open_tickets_of(db: &PgPool, guild_id: GuildId, user_id: UserId) -> anyhow::Result<Vec<ChannelId>> {
    let channels: Vec<i64> = sqlx::query_scalar("SELECT channel_id FROM tickets WHERE guild_id = $1 AND opener_id = $2 AND status = 'open'")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user_id.get())?)
        .fetch_all(db)
        .await?;
    Ok(channels.into_iter().filter_map(|id| to_id(Some(id))).map(ChannelId::new).collect())
}

async fn open_count(db: &PgPool, guild_id: GuildId, user_id: UserId) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM tickets WHERE guild_id = $1 AND opener_id = $2 AND status = 'open'")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user_id.get())?)
        .fetch_one(db)
        .await?)
}

/// Açık bileti kapatır; zaten kapalıysa `false`.
async fn mark_closed(db: &PgPool, channel_id: ChannelId, closed_by: Option<UserId>) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE tickets SET status = 'closed', closed_at = now(), closed_by = $2
         WHERE channel_id = $1 AND status = 'open'",
    )
    .bind(snowflake(channel_id.get())?)
    .bind(closed_by.map(|u| snowflake(u.get())).transpose()?)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

async fn ephemeral(ctx: &serenity::Context, interaction: &ComponentInteraction, text: &str) -> anyhow::Result<()> {
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(text)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

fn to_id(raw: Option<i64>) -> Option<u64> {
    raw.and_then(|id| u64::try_from(id).ok()).filter(|id| *id != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rating_buttons() {
        assert_eq!(parse_rating("12:7:5"), Some((GuildId::new(12), 7, 5)));
        assert_eq!(parse_rating("12:7:0"), None);
        assert_eq!(parse_rating("12:7:6"), None);
        assert_eq!(parse_rating("0:7:3"), None);
        assert_eq!(parse_rating("12:7:3:9"), None);
        assert_eq!(parse_rating("x:7:3"), None);
    }

    #[test]
    fn idle_tickets_are_warned_then_closed() {
        assert_eq!(idle_state(10 * 3600, 0), Idle::Fine);
        assert_eq!(idle_state(3600, 24), Idle::Fine);
        assert_eq!(idle_state(23 * 3600, 24), Idle::Warn);
        assert_eq!(idle_state(24 * 3600, 24), Idle::Close);
        // 1 saatlik ayarda uyarı yok, doğrudan kapanır.
        assert_eq!(idle_state(1800, 1), Idle::Fine);
        assert_eq!(idle_state(3600, 1), Idle::Close);
    }

    #[test]
    fn topic_slugs_are_safe_channel_names() {
        assert_eq!(topic_slug(None), "ticket");
        assert_eq!(topic_slug(Some("Satın Alma / Ödeme")), "satın-alma-ödeme");
        assert_eq!(topic_slug(Some("!!!")), "ticket");
        assert!(topic_slug(Some("çok uzun bir konu başlığı burada yazıyor")).chars().count() <= 20);
    }

    #[test]
    fn old_extras_load_with_defaults() -> anyhow::Result<()> {
        let extras: TicketExtras = serde_json::from_str("{\"topics\":[\"Genel\"]}")?;
        assert_eq!(extras.max_open, 1);
        assert!(!extras.keep_closed);
        Ok(())
    }
}

// ---------------------------------------------------------------- İstatistikler

/// Yetkilinin ilk cevabını bekleyen bilet kanalları: kanal → (sunucu, açan). Mesaj başına tek bellek bakışı.
static AWAITING_STAFF: std::sync::LazyLock<dashmap::DashMap<ChannelId, (GuildId, UserId)>> = std::sync::LazyLock::new(dashmap::DashMap::new);

/// Açılışta, henüz yetkili cevabı almamış açık biletleri belleğe alır.
pub async fn load_awaiting(db: &PgPool) -> anyhow::Result<()> {
    let rows: Vec<(i64, i64, i64)> =
        sqlx::query_as("SELECT guild_id, channel_id, opener_id FROM tickets WHERE status = 'open' AND first_response_at IS NULL").fetch_all(db).await?;
    for (guild, channel, opener) in rows {
        let ids = (u64::try_from(guild), u64::try_from(channel), u64::try_from(opener));
        if let (Ok(guild), Ok(channel), Ok(opener)) = ids {
            if guild != 0 && channel != 0 && opener != 0 {
                AWAITING_STAFF.insert(ChannelId::new(channel), (GuildId::new(guild), UserId::new(opener)));
            }
        }
    }
    Ok(())
}

/// Bilet kanalına yazan ilk yetkiliyi ve zamanı kaydeder.
pub async fn on_message(ctx: &serenity::Context, db: &PgPool, msg: &serenity::Message) {
    let Some(entry) = AWAITING_STAFF.get(&msg.channel_id).map(|e| *e) else { return };
    let (guild_id, opener) = entry;
    if msg.author.id == opener || msg.author.bot {
        return;
    }
    let roles = msg.member.as_ref().map(|m| m.roles.clone()).unwrap_or_default();
    let manages = ctx.cache.guild(guild_id).is_some_and(|g| {
        g.members.get(&msg.author.id).is_some_and(|m| {
            let perms = g.member_permissions(m);
            perms.contains(Permissions::MANAGE_CHANNELS) || perms.contains(Permissions::ADMINISTRATOR)
        })
    });
    let staff = manages || load_settings(db, guild_id).await.ok().flatten().and_then(|s| s.support_role_id).is_some_and(|r| roles.contains(&r));
    if !staff {
        return;
    }
    AWAITING_STAFF.remove(&msg.channel_id);
    let saved = async {
        sqlx::query("UPDATE tickets SET first_response_at = now(), first_responder = $2 WHERE channel_id = $1 AND first_response_at IS NULL")
            .bind(snowflake(msg.channel_id.get())?)
            .bind(snowflake(msg.author.id.get())?)
            .execute(db)
            .await?;
        anyhow::Ok(())
    };
    if let Err(e) = saved.await {
        warn!(guild = %guild_id, error = ?e, "Biletin ilk cevabı kaydedilemedi");
    }
}

/// Son `days` günün bilet özeti.
pub struct TicketStats {
    pub opened: i64,
    pub closed: i64,
    /// Ortalama ilk yetkili cevabı ve kapanış süresi (saniye).
    pub first_response: Option<f64>,
    pub resolution: Option<f64>,
    /// Yetkili başına: (kimlik, ilk cevap, kapatma, üstlenme).
    pub staff: Vec<(u64, i64, i64, i64)>,
}

pub async fn stats(db: &PgPool, guild_id: GuildId, days: i32) -> anyhow::Result<TicketStats> {
    let guild = snowflake(guild_id.get())?;
    let (opened, closed, first_response, resolution): (i64, i64, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE status <> 'open'),
                (avg(EXTRACT(EPOCH FROM first_response_at - created_at)) FILTER (WHERE first_response_at IS NOT NULL))::float8,
                (avg(EXTRACT(EPOCH FROM closed_at - created_at)) FILTER (WHERE closed_at IS NOT NULL))::float8
         FROM tickets WHERE guild_id = $1 AND created_at > now() - make_interval(days => $2)",
    )
    .bind(guild)
    .bind(days)
    .fetch_one(db)
    .await?;
    let rows: Vec<(i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT who, sum(r)::bigint, sum(c)::bigint, sum(k)::bigint FROM (
             SELECT first_responder AS who, 1 AS r, 0 AS c, 0 AS k FROM tickets
              WHERE guild_id = $1 AND created_at > now() - make_interval(days => $2) AND first_responder IS NOT NULL
             UNION ALL
             SELECT closed_by, 0, 1, 0 FROM tickets
              WHERE guild_id = $1 AND created_at > now() - make_interval(days => $2) AND closed_by IS NOT NULL AND closed_by <> opener_id
             UNION ALL
             SELECT claimed_by, 0, 0, 1 FROM tickets
              WHERE guild_id = $1 AND created_at > now() - make_interval(days => $2) AND claimed_by IS NOT NULL
         ) x GROUP BY who ORDER BY sum(r) + sum(c) + sum(k) DESC LIMIT 10",
    )
    .bind(guild)
    .bind(days)
    .fetch_all(db)
    .await?;
    let staff = rows.into_iter().filter_map(|(who, r, c, k)| u64::try_from(who).ok().map(|w| (w, r, c, k))).collect();
    Ok(TicketStats { opened, closed, first_response, resolution, staff })
}

/// Saniyeyi okunur süreye çevirir: "45 sn", "12 dk", "3 sa 5 dk", "2 g 4 sa".
pub fn human_secs(secs: f64) -> String {
    let secs = if secs.is_finite() && secs > 0.0 { secs.round() as u64 } else { 0 };
    match secs {
        0..=59 => format!("{secs} sn"),
        60..=3599 => format!("{} dk", secs / 60),
        3600..=86_399 => format!("{} sa {} dk", secs / 3600, secs % 3600 / 60),
        _ => format!("{} g {} sa", secs / 86_400, secs % 86_400 / 3600),
    }
}

#[cfg(test)]
mod stats_tests {
    use super::human_secs;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(human_secs(42.4), "42 sn");
        assert_eq!(human_secs(720.0), "12 dk");
        assert_eq!(human_secs(3600.0 * 3.0 + 300.0), "3 sa 5 dk");
        assert_eq!(human_secs(86_400.0 * 2.0 + 3600.0 * 4.0), "2 g 4 sa");
        assert_eq!(human_secs(f64::NAN), "0 sn");
    }
}
