//! Üye giriş/çıkışı: hesap yaşı filtresi, baskın (raid) algılama, otomatik rol, karşılama/veda.
//!
//! Sıra önemlidir: önce koruma (şüpheli hesap atılırsa karşılanmaz), sonra rol ve mesaj.

use poise::serenity_prelude::{
    self as serenity, CreateAllowedMentions, CreateAttachment, CreateEmbed, CreateMessage, GuildId, Member,
    Mentionable, User,
};
use redis::aio::ConnectionManager;
use tracing::{info, warn};

use crate::cards::{self, WelcomeSettings};
use crate::data::Data;
use crate::eventlog;
use crate::greetings::{self, Greeting};

/// Baskın sayacının penceresi.
const RAID_WINDOW_SECS: u64 = 10;
/// Baskın modu bu kadar sürer (her yeni baskın girişi süreyi uzatmaz).
pub const RAID_MODE_SECS: u64 = 10 * 60;
/// Baskın modunda bu yaştan genç hesaplar otomatik atılır.
const RAID_MIN_ACCOUNT_AGE_DAYS: i64 = 7;

/// Karşılama embed'i: metin, üyenin avatarı ve (hazır temalarda) başlık + GIF.
pub fn welcome_embed(custom: Option<&str>, themes: &[String], user: &User, guild: &str, members: u64) -> CreateEmbed {
    greeting_embed(greetings::pick_welcome(custom, themes), user, guild, members)
}

/// Veda embed'i; karşılamayla aynı düzen.
pub fn goodbye_embed(custom: Option<&str>, themes: &[String], user: &User, guild: &str, members: u64) -> CreateEmbed {
    greeting_embed(greetings::pick_goodbye(custom, themes), user, guild, members)
}

fn greeting_embed(greeting: Greeting, user: &User, guild: &str, members: u64) -> CreateEmbed {
    let (r, g, b) = greeting.colour;
    let mut embed = CreateEmbed::new()
        .description(render(&greeting.template, user, guild, members))
        .thumbnail(user.face())
        .colour(serenity::Colour::from_rgb(r, g, b));
    if let Some(title) = greeting.title {
        embed = embed.title(title);
    }
    if let Some(gif) = greeting.gif {
        embed = embed.image(gif);
    }
    embed
}

pub async fn on_join(ctx: &serenity::Context, data: &Data, member: &Member) -> anyhow::Result<()> {
    let guild_id = member.guild_id;
    if member.user.bot {
        return Ok(());
    }
    // Karantinadan kaçmak için çıkıp giren üye karşılanmaz, rolü geri verilir.
    match crate::quarantine::on_rejoin(&ctx.http, data, member).await {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(e) => warn!(guild = %guild_id, error = %e, "Karantina rolü yeniden verilemedi"),
    }
    let cfg = data.guild_configs.get(&data.db, guild_id).await?;
    let age_days = (serenity::Timestamp::now().unix_timestamp() - member.user.created_at().unix_timestamp()) / 86_400;
    // Davet, koruma üyeyi atsa bile kaydedilir (kimin sahte hesap getirdiği görülsün).
    let invite = data.invites.on_join(&ctx.http, &data.db, member, age_days).await;

    // 1) Hesap yaşı filtresi.
    let min_age = i64::from(cfg.min_account_age_days);
    if min_age > 0 && age_days < min_age {
        let reason = format!("Hesap çok yeni ({age_days} gün; en az {min_age} gün gerekli)");
        kick(ctx, member, &reason).await;
        eventlog::alert(
            ctx,
            data,
            guild_id,
            "🛡️ Giriş filtresi",
            format!("{} atıldı: {reason}.", member.user.id.mention()),
        )
        .await;
        return Ok(());
    }

    // 2) Baskın algılama.
    if cfg.raid_join_threshold > 0 {
        match raid_state(&data.redis, guild_id, cfg.raid_join_threshold).await {
            Ok(RaidState::Started) => {
                warn!(guild = %guild_id, "Baskın algılandı, baskın modu açıldı");
                eventlog::alert(
                    ctx,
                    data,
                    guild_id,
                    "🚨 Baskın algılandı!",
                    format!(
                        "{RAID_WINDOW_SECS} saniyede **{}+** kişi katıldı. **{} dakika** boyunca {RAID_MIN_ACCOUNT_AGE_DAYS} \
                         günden yeni hesaplar otomatik atılacak.\nErken bitirmek için: `/koruma baskın-bitir`",
                        cfg.raid_join_threshold,
                        RAID_MODE_SECS / 60
                    ),
                )
                .await;
            }
            Ok(RaidState::Active | RaidState::Calm) => {}
            Err(e) => warn!(error = %e, "Baskın sayacı güncellenemedi"),
        }
        if is_raid_mode(&data.redis, guild_id).await && age_days < RAID_MIN_ACCOUNT_AGE_DAYS {
            kick(ctx, member, "Baskın modu: yeni hesaplar geçici olarak kabul edilmiyor").await;
            return Ok(());
        }
    }

    let welcome = welcome_settings(data, guild_id).await;

    // 3) Otomatik rol ve ek roller.
    let extra = welcome.extra_roles.iter().filter(|id| **id != 0).map(|id| serenity::RoleId::new(*id));
    let wanted: Vec<serenity::RoleId> = cfg.autorole.into_iter().chain(extra.filter(|role| Some(*role) != cfg.autorole)).collect();
    // Sunucuda artık olmayan roller verilmeye çalışılmaz; ayardan da temizlenir (kilit `await`'ten önce bırakılır).
    let gone: Vec<serenity::RoleId> = match ctx.cache.guild(guild_id) {
        Some(guild) => wanted.iter().copied().filter(|role| !guild.roles.contains_key(role)).collect(),
        None => Vec::new(),
    };
    for role in &gone {
        crate::cleanup::role_deleted(ctx, data, guild_id, *role).await;
    }
    let roles: Vec<serenity::RoleId> = wanted.into_iter().filter(|role| !gone.contains(role)).collect();
    let delay = std::time::Duration::from_secs(u64::from(welcome.role_delay_mins.min(MAX_ROLE_DELAY_MINS)) * 60);
    if delay.is_zero() {
        give_roles(&ctx.http, guild_id, member.user.id, &roles).await;
    } else if !roles.is_empty() {
        // Gecikmeli otorol: bekleme bellekte tutulur (bot bu sürede yeniden başlarsa rol verilmez).
        let (http, user) = (std::sync::Arc::clone(&ctx.http), member.user.id);
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            // Üye bu arada ayrıldıysa rol verilmez.
            if guild_id.member(&http, user).await.is_ok() {
                give_roles(&http, guild_id, user, &roles).await;
            }
        });
    }

    // 4) Karşılama mesajı.
    let (guild, count) = (guild_name(ctx, guild_id), member_count(ctx, guild_id));
    if let Some(channel) = cfg.welcome_channel {
        let message = welcome_message(data, guild_id, cfg.welcome_message.as_deref(), &member.user, &guild, count, true).await;
        if let Err(e) = channel.send_message(&ctx.http, message).await {
            warn!(channel = %channel, error = %e, "Karşılama mesajı gönderilemedi");
        }
    }

    // 5) Özelden karşılama (üyenin DM'leri kapalıysa sessizce geçilir).
    if welcome.dm {
        let template = welcome.dm_message.as_deref().filter(|m| !m.trim().is_empty()).unwrap_or(DEFAULT_DM);
        let text = render(template, &member.user, &guild, count);
        if let Err(e) = member.user.direct_message(ctx, CreateMessage::new().content(text)).await {
            tracing::debug!(user = %member.user.id, error = %e, "Karşılama DM'i gönderilemedi");
        }
    }


    eventlog::member_joined(ctx, data, member, age_days, invite.as_ref()).await;
    // İtirazı kabul edilip geri dönen üye artık itiraz sunucusunda kalmasın.
    crate::appeals::leave_appeal_server(ctx, data, guild_id, member.user.id).await;
    Ok(())
}

/// Özel metin yazılmamışsa özelden gönderilen karşılama.
pub const DEFAULT_DM: &str = "👋 Selam {isim}! **{sunucu}** sunucusuna hoş geldin. Kurallara göz atmayı unutma.";
const CARD_FILE: &str = "hosgeldin.jpg";

pub async fn welcome_settings(data: &Data, guild_id: GuildId) -> std::sync::Arc<WelcomeSettings> {
    match data.settings.get::<WelcomeSettings>(&data.db, guild_id).await {
        Ok(settings) => settings,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Karşılama ayarları okunamadı, varsayılan kullanılıyor");
            std::sync::Arc::new(WelcomeSettings::default())
        }
    }
}

/// Gecikmeli otorol en fazla bu kadar bekletilebilir.
pub const MAX_ROLE_DELAY_MINS: u32 = 24 * 60;

async fn give_roles(http: &serenity::Http, guild_id: GuildId, user: serenity::UserId, roles: &[serenity::RoleId]) {
    for role in roles {
        if let Err(e) = http.add_member_role(guild_id, user, *role, Some("Otomatik rol")).await {
            warn!(guild = %guild_id, role = %role, error = %e, "Otomatik rol verilemedi (botun rolü bu rolün üstünde mi?)");
        }
    }
}

/// Üyenin avatarını kart için indirir (hareketli avatarın sabit hâli; yoksa varsayılan avatar).
pub async fn avatar_image(user: &User) -> Option<image::RgbaImage> {
    let url = user.static_avatar_url().unwrap_or_else(|| user.default_avatar_url());
    let url = url.split('?').next().unwrap_or_default().to_owned() + "?size=256";
    cards::fetch_image(&url).await
}

/// Karşılama kartını çizer; çizilemezse `None` (mesaj kartsız gönderilir).
pub async fn welcome_card(settings: &WelcomeSettings, user: &User, guild: &str, members: u64) -> Option<Vec<u8>> {
    let name = user.global_name.clone().unwrap_or_else(|| user.name.clone());
    let card = cards::WelcomeCard {
        title: cards::card_text(settings.title(), &name, guild, members),
        subtitle: cards::card_text(settings.subtitle(), &name, guild, members),
        text: cards::parse_hex(&settings.text_color).unwrap_or([255, 255, 255]),
        background: cards::parse_hex(&settings.bg_color).unwrap_or([35, 39, 42]),
        overlay: settings.overlay,
        font: settings.font.clone(),
    };
    let backdrop = async {
        match settings.background_url.as_deref() {
            Some(url) => cards::fetch_image(url).await,
            None => None,
        }
    };
    let (avatar, backdrop) = tokio::join!(avatar_image(user), backdrop);
    let drawn = tokio::task::spawn_blocking(move || cards::render_welcome(&card, avatar.as_ref(), backdrop.as_ref())).await;
    match drawn {
        Ok(Ok(bytes)) => Some(bytes),
        Ok(Err(e)) => {
            warn!(error = ?e, "Karşılama kartı çizilemedi");
            None
        }
        Err(e) => {
            warn!(error = %e, "Karşılama kartı görevi çöktü");
            None
        }
    }
}

/// Kanala gönderilecek karşılama mesajı: embed ve (ayar açıksa) resimli kart.
/// `mention`: üye etiketlensin mi (deneme mesajında etiketlenmez).
pub async fn welcome_message(
    data: &Data,
    guild_id: GuildId,
    custom: Option<&str>,
    user: &User,
    guild: &str,
    members: u64,
    mention: bool,
) -> CreateMessage {
    let settings = welcome_settings(data, guild_id).await;
    let mut embed = welcome_embed(custom, &settings.themes, user, guild, members);
    let mut message = CreateMessage::new();
    if mention {
        message = message
            .content(user.id.mention().to_string())
            .allowed_mentions(CreateAllowedMentions::new().users(vec![user.id]));
    }
    if settings.card {
        if let Some(bytes) = welcome_card(&settings, user, guild, members).await {
            // Kart, temalı GIF'in yerine geçer.
            embed = embed.image(format!("attachment://{CARD_FILE}"));
            message = message.add_file(CreateAttachment::bytes(bytes, CARD_FILE));
        }
    }
    message.embed(embed)
}

pub async fn on_leave(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    user: &User,
    member: Option<&Member>,
) -> anyhow::Result<()> {
    if user.bot {
        return Ok(());
    }
    let cfg = data.guild_configs.get(&data.db, guild_id).await?;
    if let Some(channel) = cfg.goodbye_channel {
        let themes = welcome_settings(data, guild_id).await.themes.clone();
        let embed = goodbye_embed(
            cfg.goodbye_message.as_deref(),
            &themes,
            user,
            &guild_name(ctx, guild_id),
            member_count(ctx, guild_id),
        );
        if let Err(e) = channel.send_message(&ctx.http, CreateMessage::new().embed(embed)).await {
            warn!(channel = %channel, error = %e, "Veda mesajı gönderilemedi");
        }
    }
    eventlog::member_left(ctx, data, guild_id, user, member).await;
    Ok(())
}

/// Şablon değişkenleri: `{kullanıcı}` (etiket), `{isim}`, `{sunucu}`, `{üye}` (üye sayısı).
pub fn render(template: &str, user: &User, guild: &str, members: u64) -> String {
    let name = user.global_name.clone().unwrap_or_else(|| user.name.clone());
    template
        .replace("{kullanıcı}", &user.id.mention().to_string())
        .replace("{kullanici}", &user.id.mention().to_string())
        .replace("{isim}", &name)
        .replace("{sunucu}", guild)
        .replace("{üye}", &members.to_string())
        .replace("{uye}", &members.to_string())
}

enum RaidState {
    /// Eşik bu girişle aşıldı; baskın modu yeni açıldı.
    Started,
    Active,
    Calm,
}

async fn raid_state(redis: &ConnectionManager, guild_id: GuildId, threshold: u32) -> redis::RedisResult<RaidState> {
    let joins_key = format!("raid:joins:{guild_id}");
    let mut conn = redis.clone();
    let (count,): (i64,) = redis::pipe()
        .atomic()
        .cmd("SET")
        .arg(&joins_key)
        .arg(0)
        .arg("EX")
        .arg(RAID_WINDOW_SECS)
        .arg("NX")
        .ignore()
        .cmd("INCR")
        .arg(&joins_key)
        .query_async(&mut conn)
        .await?;

    if count < i64::from(threshold) {
        return Ok(RaidState::Calm);
    }
    let started: Option<String> = redis::cmd("SET")
        .arg(raid_mode_key(guild_id))
        .arg(1)
        .arg("EX")
        .arg(RAID_MODE_SECS)
        .arg("NX")
        .query_async(&mut conn)
        .await?;
    Ok(if started.is_some() { RaidState::Started } else { RaidState::Active })
}

pub async fn is_raid_mode(redis: &ConnectionManager, guild_id: GuildId) -> bool {
    let mut conn = redis.clone();
    redis::cmd("EXISTS")
        .arg(raid_mode_key(guild_id))
        .query_async::<i64>(&mut conn)
        .await
        .is_ok_and(|n| n > 0)
}

/// Baskın modunu kapatır; açıktıysa `true`.
pub async fn end_raid_mode(redis: &ConnectionManager, guild_id: GuildId) -> anyhow::Result<bool> {
    let mut conn = redis.clone();
    let removed: i64 = redis::cmd("DEL").arg(raid_mode_key(guild_id)).query_async(&mut conn).await?;
    Ok(removed > 0)
}

fn raid_mode_key(guild_id: GuildId) -> String {
    format!("raid:mode:{guild_id}")
}

/// Önce DM ile sebebi bildirir (atıldıktan sonra DM gönderilemez), sonra atar.
async fn kick(ctx: &serenity::Context, member: &Member, reason: &str) {
    let guild = guild_name(ctx, member.guild_id);
    let dm = CreateMessage::new().content(format!("🛡️ **{guild}** sunucusuna şu an katılamıyorsun: {reason}."));
    let _ = member.user.direct_message(ctx, dm).await;
    match member.kick_with_reason(ctx, reason).await {
        Ok(()) => info!(guild = %member.guild_id, user = %member.user.id, reason, "Koruma: üye atıldı"),
        Err(e) => warn!(guild = %member.guild_id, user = %member.user.id, error = %e, "Koruma: üye atılamadı"),
    }
}

fn guild_name(ctx: &serenity::Context, guild_id: GuildId) -> String {
    ctx.cache
        .guild(guild_id)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "sunucu".to_owned())
}

fn member_count(ctx: &serenity::Context, guild_id: GuildId) -> u64 {
    ctx.cache.guild(guild_id).map_or(0, |g| g.member_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_placeholders() {
        let mut user = User::default();
        user.name = "ahmet".to_owned();
        user.global_name = Some("Ahmet".to_owned());
        let text = render("{kullanıcı} {isim} {sunucu} {üye} {uye}", &user, "Test", 42);
        assert!(text.contains("<@"));
        assert!(text.contains("Ahmet Test 42 42"));
    }
}
