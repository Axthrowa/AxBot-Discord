//! Topluluk ayarları: `/karşılama`, `/rol otomatik`, `/koruma`, `/rol panel`.

use poise::serenity_prelude::{self as serenity, Mentionable, Role};

use super::moderation::{discord_error, guild_id, reply_private};
use crate::data::Context;
use crate::error::Error;
use crate::greetings;
use crate::guild_config::Setting;
use crate::members::{self, RAID_MODE_SECS};
use crate::reaction_roles;

const TEMPLATE_HELP: &str = concat!(
    "Değişkenler: `{kullanıcı}` (etiket), `{isim}`, `{sunucu}`, `{üye}` (üye sayısı). ",
    "Mesajı boş bırakırsan her seferinde seçili temalardan (Star Wars, Game of Thrones, Breaking Bad, Marvel… `/karşılama tema`) ",
    "GIF'li hazır mesajlardan biri seçilir; ",
    "kendi metinlerini `|` ile ayırarak birden fazla yazabilirsin.",
);

/// Yeni üyeleri karşılama ve ayrılanlara veda mesajları.
#[poise::command(
    slash_command,
    guild_only,
    rename = "karşılama",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("karsilama_ayarla", "veda_ayarla", "karsilama_tema", "karsilama_kapat", "karsilama_test"),
    subcommand_required,
    category = "Topluluk"
)]
pub async fn karsilama(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Hoş geldin mesajının kanalını ve metnini ayarlar.
#[poise::command(slash_command, guild_only, rename = "hoşgeldin")]
async fn karsilama_ayarla(
    ctx: Context<'_>,
    #[description = "Mesajın gönderileceği kanal"]
    #[channel_types("Text")]
    kanal: serenity::GuildChannel,
    #[description = "Boşsa her seferinde farklı metin; birden fazlası için | ile ayır. {kullanıcı} {isim} {sunucu} {üye}"]
    #[max_length = 1500]
    mesaj: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    if let Some(problem) = cannot_send(ctx, &kanal) {
        return reply_private(ctx, problem).await;
    }
    let data = ctx.data();
    data.guild_configs
        .set(&data.db, guild_id, Setting::Welcome(Some(kanal.id), mesaj.clone()))
        .await?;
    let preview = members::render(&greetings::pick_welcome(mesaj.as_deref(), &[]).template, ctx.author(), "Sunucu", 42);
    reply_private(
        ctx,
        format!(
            "✅ Hoş geldin mesajları {} kanalına gönderilecek.\n**Önizleme:** {preview}\n_{TEMPLATE_HELP}_{}",
            kanal.id.mention(),
            members_warning(ctx)
        ),
    )
    .await
}

/// Ayrılan üyeler için veda mesajını ayarlar.
#[poise::command(slash_command, guild_only, rename = "veda")]
async fn veda_ayarla(
    ctx: Context<'_>,
    #[description = "Mesajın gönderileceği kanal"]
    #[channel_types("Text")]
    kanal: serenity::GuildChannel,
    #[description = "Boşsa her seferinde farklı metin; birden fazlası için | ile ayır. {isim} {sunucu} {üye}"]
    #[max_length = 1500]
    mesaj: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    if let Some(problem) = cannot_send(ctx, &kanal) {
        return reply_private(ctx, problem).await;
    }
    let data = ctx.data();
    data.guild_configs
        .set(&data.db, guild_id, Setting::Goodbye(Some(kanal.id), mesaj.clone()))
        .await?;
    let preview = members::render(&greetings::pick_goodbye(mesaj.as_deref(), &[]).template, ctx.author(), "Sunucu", 41);
    reply_private(
        ctx,
        format!(
            "✅ Veda mesajları {} kanalına gönderilecek.\n**Önizleme:** {preview}\n_{TEMPLATE_HELP}_{}",
            kanal.id.mention(),
            members_warning(ctx)
        ),
    )
    .await
}

/// Hazır mesaj temaları (`greetings::THEMES` ile aynı sıra).
#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum ThemeChoice {
    #[name = "Hepsi (karışık)"]
    All,
    #[name = "Star Wars"]
    StarWars,
    #[name = "Yüzüklerin Efendisi"]
    Lotr,
    #[name = "Harry Potter"]
    HarryPotter,
    #[name = "Game of Thrones"]
    Got,
    #[name = "Breaking Bad"]
    BreakingBad,
    #[name = "Marvel"]
    Marvel,
    #[name = "Stranger Things"]
    Stranger,
    #[name = "The Witcher"]
    Witcher,
    #[name = "Super Mario"]
    Mario,
}

impl ThemeChoice {
    fn key(self) -> Option<&'static str> {
        Some(match self {
            Self::All => return None,
            Self::StarWars => "starwars",
            Self::Lotr => "lotr",
            Self::HarryPotter => "hp",
            Self::Got => "got",
            Self::BreakingBad => "bb",
            Self::Marvel => "marvel",
            Self::Stranger => "stranger",
            Self::Witcher => "witcher",
            Self::Mario => "mario",
        })
    }
}

/// Hazır karşılama ve veda mesajlarının temasını seçer (birden fazlası için web paneli).
#[poise::command(slash_command, guild_only, rename = "tema")]
async fn karsilama_tema(
    ctx: Context<'_>,
    #[description = "Hangi tema kullanılsın"] tema: ThemeChoice,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let mut settings = (*members::welcome_settings(data, guild_id).await).clone();
    settings.themes = tema.key().map(|k| vec![k.to_owned()]).unwrap_or_default();
    data.settings.set(&data.db, guild_id, settings).await?;
    let text = match tema.key() {
        Some(_) => format!("🎬 Hazır karşılama ve veda mesajları artık **{}** temalı. Denemek için `/karşılama test`.", poise::ChoiceParameter::name(&tema)),
        None => "🎬 Hazır mesajlar artık bütün temalardan karışık seçilecek. Birden fazla tema seçmek için web paneli (`/panel`).".to_owned(),
    };
    reply_private(ctx, text).await
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum GreetingKind {
    #[name = "Hoş geldin"]
    Welcome,
    #[name = "Veda"]
    Goodbye,
    #[name = "İkisi de"]
    Both,
}

/// Karşılama ve/veya veda mesajlarını kapatır.
#[poise::command(slash_command, guild_only, rename = "kapat")]
async fn karsilama_kapat(ctx: Context<'_>, #[rename = "tür"] #[description = "Hangisi"] kind: GreetingKind) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    if matches!(kind, GreetingKind::Welcome | GreetingKind::Both) {
        data.guild_configs.set(&data.db, guild_id, Setting::Welcome(None, None)).await?;
    }
    if matches!(kind, GreetingKind::Goodbye | GreetingKind::Both) {
        data.guild_configs.set(&data.db, guild_id, Setting::Goodbye(None, None)).await?;
    }
    reply_private(ctx, "✅ Kapatıldı.").await
}

/// Hoş geldin (veya veda) mesajını sana göndererek dener.
#[poise::command(slash_command, guild_only, rename = "test")]
async fn karsilama_test(
    ctx: Context<'_>,
    #[rename = "tür"]
    #[description = "Hangisi denensin (boşsa hoş geldin)"]
    kind: Option<GreetingKind>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let cfg = ctx.data().guild_configs.get(&ctx.data().db, guild_id).await?;
    let kind = kind.unwrap_or(GreetingKind::Welcome);
    let guild_name = ctx.guild().map(|g| g.name.clone()).unwrap_or_default();
    let count = ctx.guild().map_or(0, |g| g.member_count);
    let mut sends = Vec::new();
    if matches!(kind, GreetingKind::Welcome | GreetingKind::Both) {
        let Some(channel) = cfg.welcome_channel else {
            return reply_private(ctx, "Önce `/karşılama hoşgeldin` ile bir kanal ayarla.").await;
        };
        let message =
            members::welcome_message(ctx.data(), guild_id, cfg.welcome_message.as_deref(), ctx.author(), &guild_name, count, false).await;
        sends.push((channel, message.content("🧪 _Bu bir test mesajıdır._")));
    }
    if matches!(kind, GreetingKind::Goodbye | GreetingKind::Both) {
        let Some(channel) = cfg.goodbye_channel else {
            return reply_private(ctx, "Önce `/karşılama veda` ile bir kanal ayarla.").await;
        };
        let themes = members::welcome_settings(ctx.data(), guild_id).await.themes.clone();
        let embed = members::goodbye_embed(cfg.goodbye_message.as_deref(), &themes, ctx.author(), &guild_name, count)
            .footer(serenity::CreateEmbedFooter::new("Bu bir test mesajıdır"));
        sends.push((channel, serenity::CreateMessage::new().embed(embed)));
    }
    ctx.defer_ephemeral().await?;
    let mut channels = Vec::new();
    for (channel, message) in sends {
        if let Err(e) = channel.send_message(ctx, message).await {
            return reply_private(
                ctx,
                format!("❌ {} kanalına yazamadım ({}). {SEND_PERMISSIONS_HELP}", channel.mention(), discord_error(&e)),
            )
            .await;
        }
        channels.push(channel.mention().to_string());
    }
    reply_private(ctx, format!("✅ Test mesajı gönderildi: {}", channels.join(", "))).await
}

const SEND_PERMISSIONS_HELP: &str =
    "Kanal ayarlarından botun rolüne 'Kanalı Görüntüle', 'Mesaj Gönder' ve 'Bağlantı Yerleştir' izni ver.";

/// Bot kanala embed'li mesaj gönderemiyorsa kullanıcıya gösterilecek açıklama (önbellekten bakılır).
pub(super) fn cannot_send(ctx: Context<'_>, channel: &serenity::GuildChannel) -> Option<String> {
    let bot_id = ctx.serenity_context().cache.current_user().id;
    let guild = ctx.guild()?;
    // Bot üyeliği önbellekte yoksa kontrol edilemez; gönderim sırasında hata loglanır.
    let me = guild.members.get(&bot_id)?;
    let perms = guild.user_permissions_in(channel, me);
    let needed = serenity::Permissions::VIEW_CHANNEL | serenity::Permissions::SEND_MESSAGES | serenity::Permissions::EMBED_LINKS;
    (!perms.contains(needed)).then(|| format!("❌ {} kanalına yazma iznim yok. {SEND_PERMISSIONS_HELP}", channel.id.mention()))
}

/// Yeni üyelere otomatik verilecek rol.
#[poise::command(
    slash_command,
    guild_only,
    rename = "otomatik",
    required_permissions = "MANAGE_ROLES",
    required_bot_permissions = "MANAGE_ROLES"
)]
pub async fn otorol(
    ctx: Context<'_>,
    #[description = "Verilecek rol (boşsa otomatik rol kapanır)"] rol: Option<Role>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let Some(role) = rol else {
        data.guild_configs.set(&data.db, guild_id, Setting::Autorole(None)).await?;
        return reply_private(ctx, "✅ Otomatik rol kapatıldı.").await;
    };
    if let Err(reason) = reaction_roles::check_role(ctx.serenity_context(), guild_id, &role) {
        return reply_private(ctx, format!("❌ {reason}")).await;
    }
    data.guild_configs.set(&data.db, guild_id, Setting::Autorole(Some(role.id))).await?;
    reply_private(
        ctx,
        format!("✅ Yeni üyelere {} rolü verilecek.{}", role.id.mention(), members_warning(ctx)),
    )
    .await
}

/// Baskın (raid) ve sahte hesap koruması.
#[poise::command(
    slash_command,
    guild_only,
    rename = "koruma",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("koruma_hesap_yasi", "koruma_baskin", "koruma_baskin_bitir", "koruma_kilit", "koruma_durum"),
    subcommand_required,
    category = "Topluluk"
)]
pub async fn koruma(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Belirli günden yeni hesapları sunucuya almaz (0 = kapalı).
#[poise::command(slash_command, guild_only, rename = "hesap-yaşı", required_bot_permissions = "KICK_MEMBERS")]
async fn koruma_hesap_yasi(
    ctx: Context<'_>,
    #[description = "En az kaç günlük hesaplar katılabilsin (0 = kapalı)"]
    #[min = 0]
    #[max = 365]
    gun: u16,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    data.guild_configs
        .set(&data.db, guild_id, Setting::MinAccountAge(u32::from(gun)))
        .await?;
    let text = if gun == 0 {
        "✅ Hesap yaşı filtresi kapatıldı.".to_owned()
    } else {
        format!("✅ **{gun} günden** yeni hesaplar katıldığında otomatik atılacak (DM ile bilgilendirilir).")
    };
    reply_private(ctx, format!("{text}{}", members_warning(ctx))).await
}

/// Kısa sürede çok sayıda giriş olunca baskın modunu açar (0 = kapalı).
#[poise::command(slash_command, guild_only, rename = "baskın", required_bot_permissions = "KICK_MEMBERS")]
async fn koruma_baskin(
    ctx: Context<'_>,
    #[rename = "kişi"]
    #[description = "10 saniyede kaç giriş baskın sayılsın (önerilen 8-15; 0 = kapalı)"]
    #[min = 0]
    #[max = 100]
    threshold: u8,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    data.guild_configs
        .set(&data.db, guild_id, Setting::RaidThreshold(u32::from(threshold)))
        .await?;
    let text = if threshold == 0 {
        "✅ Baskın algılama kapatıldı.".to_owned()
    } else {
        format!(
            "✅ 10 saniyede **{threshold}+** giriş olursa baskın modu açılır: {} dakika boyunca 7 günden yeni \
             hesaplar atılır ve olay log kanalına uyarı düşer.",
            RAID_MODE_SECS / 60
        )
    };
    reply_private(ctx, format!("{text}{}", members_warning(ctx))).await
}

/// Aktif baskın modunu erken bitirir.
#[poise::command(slash_command, guild_only, rename = "baskın-bitir")]
async fn koruma_baskin_bitir(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let text = if members::end_raid_mode(&ctx.data().redis, guild_id).await? {
        "✅ Baskın modu kapatıldı; yeni hesaplar tekrar katılabilir."
    } else {
        "ℹ️ Şu an baskın modu açık değil."
    };
    reply_private(ctx, text).await
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
enum LockChoice {
    #[name = "Kilitle (kimse yazamaz)"]
    Lock,
    #[name = "Kilidi aç (eski hâline dön)"]
    Unlock,
}

/// Baskın sırasında bütün yazı kanallarını tek seferde kilitler ya da açar.
#[poise::command(
    slash_command,
    guild_only,
    rename = "kilit",
    required_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_CHANNELS | MANAGE_ROLES"
)]
async fn koruma_kilit(
    ctx: Context<'_>,
    #[description = "Kilitle ya da aç"] durum: LockChoice,
    #[description = "Sebep (kayıtlarda görünür)"]
    #[max_length = 200]
    sebep: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let serenity_ctx = ctx.serenity_context();
    let locked = crate::lockdown::is_locked(&data.redis, guild_id).await;
    match durum {
        LockChoice::Lock if locked => reply_private(ctx, "ℹ️ Sunucu zaten kilitli. Açmak için `/koruma kilit durum:Kilidi aç`.").await,
        LockChoice::Unlock if !locked => reply_private(ctx, "ℹ️ Sunucu kilitli değil.").await,
        LockChoice::Lock => {
            ctx.defer().await?;
            let reason = sebep.unwrap_or_else(|| "Sunucu kilidi".to_owned());
            let (done, skipped, failed) = crate::lockdown::lock_all(serenity_ctx, &data.redis, guild_id, &reason).await?;
            let mut text = format!("🔒 **Sunucu kilitlendi** — {done} kanalda üyeler artık yazamıyor.
> {reason}");
            if skipped > 0 {
                text.push_str(&format!("
_{skipped} kanal zaten yazmaya kapalıydı; dokunulmadı._"));
            }
            if failed > 0 {
                text.push_str(&format!("
⚠️ {failed} kanal kilitlenemedi (botun o kanallarda 'Kanalları Yönet' / 'İzinleri Yönet' izni yok)."));
            }
            text.push_str("
Açmak için: `/koruma kilit durum:Kilidi aç`");
            ctx.say(text).await?;
            Ok(())
        }
        LockChoice::Unlock => {
            ctx.defer().await?;
            let (done, failed) = crate::lockdown::unlock_all(serenity_ctx, &data.redis, guild_id).await?;
            let mut text = format!("🔓 **Sunucu kilidi açıldı** — {done} kanal eski hâline döndü.");
            if failed > 0 {
                text.push_str(&format!("
⚠️ {failed} kanal açılamadı; tekrar dene."));
            }
            ctx.say(text).await?;
            Ok(())
        }
    }
}

/// Koruma ve topluluk ayarlarını gösterir.
#[poise::command(slash_command, guild_only, rename = "durum")]
async fn koruma_durum(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let cfg = data.guild_configs.get(&data.db, guild_id).await?;
    let raid_active = members::is_raid_mode(&data.redis, guild_id).await;

    let channel = |c: Option<serenity::ChannelId>| c.map_or_else(|| "❌ kapalı".to_owned(), |c| c.mention().to_string());
    let lines = [
        format!("**Hesap yaşı filtresi:** {}", if cfg.min_account_age_days == 0 { "❌ kapalı".to_owned() } else { format!("{} gün", cfg.min_account_age_days) }),
        format!("**Baskın algılama:** {}", if cfg.raid_join_threshold == 0 { "❌ kapalı".to_owned() } else { format!("10 sn'de {}+ giriş", cfg.raid_join_threshold) }),
        format!("**Baskın modu:** {}", if raid_active { "🚨 AKTİF" } else { "normal" }),
        format!("**Otomatik rol:** {}", cfg.autorole.map_or_else(|| "❌ kapalı".to_owned(), |r| r.mention().to_string())),
        format!("**Hoş geldin:** {}", channel(cfg.welcome_channel)),
        format!("**Veda:** {}", channel(cfg.goodbye_channel)),
        format!("**Olay logu:** {}", channel(cfg.event_log_channel)),
        format!(
            "**Server Members Intent:** {}",
            if data.members_intent { "✅ açık" } else { "❌ kapalı (giriş/çıkış özellikleri çalışmaz)" }
        ),
    ];
    reply_private(ctx, lines.join("\n")).await
}

/// Üyelerin butonla kendilerine rol alabileceği bir panel gönderir.
#[allow(clippy::too_many_arguments)]
#[poise::command(
    slash_command,
    guild_only,
    rename = "panel",
    required_permissions = "MANAGE_ROLES",
    required_bot_permissions = "MANAGE_ROLES | SEND_MESSAGES | EMBED_LINKS"
)]
pub async fn rol_paneli(
    ctx: Context<'_>,
    #[description = "Panelin gönderileceği kanal"]
    #[channel_types("Text")]
    kanal: serenity::GuildChannel,
    #[rename = "başlık"]
    #[description = "Panel başlığı (örn. Oyun rolleri)"]
    #[max_length = 200]
    title: String,
    #[description = "Rol 1"] rol1: Role,
    #[description = "Rol 2"] rol2: Option<Role>,
    #[description = "Rol 3"] rol3: Option<Role>,
    #[description = "Rol 4"] rol4: Option<Role>,
    #[description = "Rol 5"] rol5: Option<Role>,
    #[description = "Rol 6"] rol6: Option<Role>,
    #[description = "Rol 7"] rol7: Option<Role>,
    #[description = "Rol 8"] rol8: Option<Role>,
    #[description = "Rol 9"] rol9: Option<Role>,
    #[description = "Rol 10"] rol10: Option<Role>,
    #[rename = "açıklama"]
    #[description = "Panel açıklaması"]
    #[max_length = 1000]
    description: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let mut roles: Vec<Role> = Vec::with_capacity(reaction_roles::MAX_ROLES);
    for role in [Some(rol1), rol2, rol3, rol4, rol5, rol6, rol7, rol8, rol9, rol10].into_iter().flatten() {
        if roles.iter().any(|r| r.id == role.id) {
            continue;
        }
        if let Err(reason) = reaction_roles::check_role(ctx.serenity_context(), guild_id, &role) {
            return reply_private(ctx, format!("❌ {reason}")).await;
        }
        roles.push(role);
    }

    kanal
        .id
        .send_message(ctx, reaction_roles::panel_message(&title, description.as_deref(), &roles))
        .await?;
    reply_private(ctx, format!("✅ {} rollü panel {} kanalına gönderildi.", roles.len(), kanal.id.mention())).await
}

fn members_warning(ctx: Context<'_>) -> &'static str {
    if ctx.data().members_intent {
        ""
    } else {
        "\n⚠️ Bu özellik için Developer Portal → Bot → **Server Members Intent** açılıp bot yeniden başlatılmalı."
    }
}
