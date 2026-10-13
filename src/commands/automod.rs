//! `/automod` — sunucu yöneticileri için auto-mod ayar komutları.

use anyhow::anyhow;
use poise::serenity_prelude as serenity;
use poise::{ChoiceParameter as _, CreateReply};

use crate::automod::config::{self, WordList};
use crate::automod::AutoModSettings;
use crate::data::Context;
use crate::error::Error;

use crate::automod::config::{validate_word, MAX_CUSTOM_WORDS};

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum Feature {
    #[name = "Auto-mod (tümü)"]
    Enabled,
    #[name = "Davet linki engeli"]
    Invites,
    #[name = "Tüm linkleri engelle"]
    Links,
    #[name = "Küfür filtresi"]
    Profanity,
    #[name = "Spam koruması"]
    AntiSpam,
    #[name = "Yetkililer muaf"]
    ExemptStaff,
}

impl Feature {
    fn apply(self, s: &mut AutoModSettings, on: bool) {
        let field = match self {
            Self::Enabled => &mut s.enabled,
            Self::Invites => &mut s.block_invites,
            Self::Links => &mut s.block_links,
            Self::Profanity => &mut s.filter_profanity,
            Self::AntiSpam => &mut s.anti_spam,
            Self::ExemptStaff => &mut s.exempt_staff,
        };
        *field = on;
    }
}

/// Otomatik moderasyon (küfür, reklam, spam) ayarları.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    subcommands(
        "durum",
        "ayarla",
        "kelime_ekle",
        "kelime_sil",
        "istisna_ekle",
        "istisna_sil",
        "spam_limiti"
    ),
    subcommand_required,
    category = "Moderasyon"
)]
pub async fn automod(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Mevcut auto-mod ayarlarını gösterir.
#[poise::command(slash_command, guild_only)]
async fn durum(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let cfg = config::load_from_db(&ctx.data().db, guild_id).await?;
    let s = &cfg.settings;

    let toggles = [
        ("Auto-mod", s.enabled),
        ("Davet linki engeli", s.block_invites),
        ("Tüm linkleri engelle", s.block_links),
        ("Küfür filtresi", s.filter_profanity),
        ("Spam koruması", s.anti_spam),
        ("Yetkililer muaf", s.exempt_staff),
    ]
    .iter()
    .map(|(name, on)| format!("{} {name}", if *on { "✅" } else { "❌" }))
    .collect::<Vec<_>>()
    .join("\n");


    let embed = serenity::CreateEmbed::new()
        .title("🛡️ Auto-mod ayarları")
        .colour(if s.enabled {
            serenity::Colour::DARK_GREEN
        } else {
            serenity::Colour::DARK_GREY
        })
        .field("Özellikler", toggles, true)
        .field(
            "Spam limiti",
            format!("{} mesaj / {} sn", s.spam_max_messages, s.spam_window_secs),
            true,
        )
        .field(
            format!("Yasaklı kelimeler ({})", cfg.custom_words.len()),
            word_list_field(&cfg.custom_words, "sadece varsayılan liste kullanılıyor"),
            false,
        )
        .field(
            format!("İstisnalar ({})", cfg.allowed_words.len()),
            word_list_field(&cfg.allowed_words, "sadece varsayılan istisnalar kullanılıyor"),
            false,
        );

    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

/// Bir auto-mod özelliğini açar veya kapatır.
#[poise::command(slash_command, guild_only)]
async fn ayarla(
    ctx: Context<'_>,
    #[rename = "özellik"]
    #[description = "Değiştirilecek özellik"]
    feature: Feature,
    #[rename = "aktif"]
    #[description = "Açık mı olsun?"]
    on: bool,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();

    let mut settings = config::load_settings(&data.db, guild_id).await?;
    feature.apply(&mut settings, on);
    config::save_settings(&data.db, guild_id, &settings).await?;
    data.automod.refresh(&data.db, &data.redis, guild_id).await?;

    let state = if on { "açıldı ✅" } else { "kapatıldı ❌" };
    reply(ctx, format!("**{}** {state}", feature.name())).await
}

/// Yasaklı kelime ekler: `kelime` tam kelime, `kelime*` ile başlayan, `*kelime*` içinde geçen.
#[poise::command(slash_command, guild_only, rename = "kelime-ekle")]
async fn kelime_ekle(
    ctx: Context<'_>,
    #[description = "Yasaklanacak kelime (2-32 karakter, harf/rakam/boşluk/*)"]
    kelime: String,
) -> Result<(), Error> {
    add_to_list(ctx, WordList::Blocked, &kelime).await
}

/// Sunucuya özel bir yasaklı kelimeyi kaldırır.
#[poise::command(slash_command, guild_only, rename = "kelime-sil")]
async fn kelime_sil(
    ctx: Context<'_>,
    #[description = "Listeden kaldırılacak kelime"] kelime: String,
) -> Result<(), Error> {
    remove_from_list(ctx, WordList::Blocked, &kelime).await
}

/// Yanlışlıkla engellenen bir kelimeyi serbest bırakır: `kelime` tam kelime, `kelime*` önek.
#[poise::command(slash_command, guild_only, rename = "istisna-ekle")]
async fn istisna_ekle(
    ctx: Context<'_>,
    #[description = "Serbest bırakılacak kelime (örn. sikke veya sikke*)"] kelime: String,
) -> Result<(), Error> {
    add_to_list(ctx, WordList::Allowed, &kelime).await
}

/// Bir istisnayı kaldırır; kelime tekrar filtreye takılır.
#[poise::command(slash_command, guild_only, rename = "istisna-sil")]
async fn istisna_sil(
    ctx: Context<'_>,
    #[description = "İstisnalardan kaldırılacak kelime"] kelime: String,
) -> Result<(), Error> {
    remove_from_list(ctx, WordList::Allowed, &kelime).await
}

async fn add_to_list(ctx: Context<'_>, list: WordList, raw: &str) -> Result<(), Error> {
    let word = match validate_word(raw, list) {
        Ok(word) => word,
        Err(reason) => return reply(ctx, format!("❌ Geçersiz kelime: {reason}")).await,
    };

    let guild_id = guild_id(ctx)?;
    let data = ctx.data();

    if config::count_words(&data.db, guild_id, list).await? >= MAX_CUSTOM_WORDS {
        return reply(ctx, format!("❌ Bu listeye en fazla {MAX_CUSTOM_WORDS} kelime eklenebilir.")).await;
    }
    if !config::add_word(&data.db, guild_id, list, &word).await? {
        return reply(ctx, format!("ℹ️ ||{word}|| zaten {}.", list_name(list))).await;
    }
    data.automod.refresh(&data.db, &data.redis, guild_id).await?;
    reply(ctx, format!("✅ ||{word}|| {} eklendi.", list_dative(list))).await
}

async fn remove_from_list(ctx: Context<'_>, list: WordList, raw: &str) -> Result<(), Error> {
    let word = raw.trim().to_lowercase();
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();

    if !config::remove_word(&data.db, guild_id, list, &word).await? {
        return reply(ctx, format!("ℹ️ ||{word}|| {} yok.", list_name(list))).await;
    }
    data.automod.refresh(&data.db, &data.redis, guild_id).await?;
    reply(ctx, format!("✅ ||{word}|| {} kaldırıldı.", list_ablative(list))).await
}

/// "zaten …", "… yok" cümleleri için bulunma hâli.
fn list_name(list: WordList) -> &'static str {
    match list {
        WordList::Blocked => "yasaklı kelimelerde",
        WordList::Allowed => "istisnalarda",
    }
}

fn list_dative(list: WordList) -> &'static str {
    match list {
        WordList::Blocked => "yasaklı kelimelere",
        WordList::Allowed => "istisnalara",
    }
}

fn list_ablative(list: WordList) -> &'static str {
    match list {
        WordList::Blocked => "yasaklı kelimelerden",
        WordList::Allowed => "istisnalardan",
    }
}

/// Spam korumasının hassasiyetini ayarlar.
#[poise::command(slash_command, guild_only, rename = "spam-limiti")]
async fn spam_limiti(
    ctx: Context<'_>,
    #[description = "Pencere içinde izin verilen en fazla mesaj"]
    #[min = 2]
    #[max = 50]
    mesaj: i32,
    #[description = "Pencere süresi (saniye)"]
    #[min = 1]
    #[max = 120]
    saniye: i32,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();

    let mut settings = config::load_settings(&data.db, guild_id).await?;
    settings.spam_max_messages = mesaj;
    settings.spam_window_secs = saniye;
    config::save_settings(&data.db, guild_id, &settings).await?;
    data.automod.refresh(&data.db, &data.redis, guild_id).await?;

    reply(
        ctx,
        format!("✅ Spam limiti: **{saniye} saniyede en fazla {mesaj} mesaj**."),
    )
    .await
}

fn guild_id(ctx: Context<'_>) -> Result<serenity::GuildId, Error> {
    ctx.guild_id()
        .ok_or_else(|| anyhow!("guild_only komut sunucu dışında çalıştı"))
}

async fn reply(ctx: Context<'_>, content: String) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(content).ephemeral(true))
        .await?;
    Ok(())
}

fn word_list_field(words: &[String], empty: &str) -> String {
    if words.is_empty() {
        return format!("_Yok — {empty}._");
    }
    let joined = words
        .iter()
        .map(|w| format!("||{w}||"))
        .collect::<Vec<_>>()
        .join(", ");
    truncate(&joined, 1000)
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}
