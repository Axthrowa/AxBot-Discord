//! Mesaj araçları: `/duyuru` (embed oluşturucu) ve `/otocevap` (otomatik cevaplar).

use std::time::Duration;

use poise::serenity_prelude::{self as serenity, CreateAllowedMentions, CreateEmbed, CreateEmbedFooter, CreateMessage, EditMessage, Mentionable};
use poise::Modal as _;
use regex::Regex;

use super::community::cannot_send;
use super::moderation::{guild_id, reply_private};
use crate::autoresponder::{self, MatchMode};
use crate::data::{Context, Data};
use crate::error::Error;

type AppContext<'a> = poise::ApplicationContext<'a, Data, Error>;

const MODAL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Default, poise::Modal)]
#[name = "Duyuru"]
struct AnnouncementModal {
    #[name = "Başlık"]
    #[max_length = 256]
    title: Option<String>,
    #[name = "Metin"]
    #[paragraph]
    #[max_length = 4000]
    description: String,
    #[name = "Alt yazı (isteğe bağlı)"]
    #[max_length = 2048]
    footer: Option<String>,
    #[name = "Büyük görsel bağlantısı (isteğe bağlı)"]
    #[max_length = 500]
    image: Option<String>,
    #[name = "Küçük görsel bağlantısı (isteğe bağlı)"]
    #[max_length = 500]
    thumbnail: Option<String>,
}

/// "kırmızı", "#ff8800", "ff8800" → renk.
pub fn parse_colour(input: &str) -> Option<serenity::Colour> {
    let text = crate::games::hangman::tr_lower(input.trim());
    let named = match text.as_str() {
        "kırmızı" => Some((237, 66, 69)),
        "mavi" => Some((52, 152, 219)),
        "yeşil" => Some((87, 242, 135)),
        "sarı" => Some((254, 231, 92)),
        "turuncu" => Some((230, 126, 34)),
        "mor" => Some((155, 89, 182)),
        "pembe" => Some((255, 115, 250)),
        "siyah" => Some((35, 39, 42)),
        "beyaz" => Some((255, 255, 255)),
        "gri" => Some((149, 165, 166)),
        "altın" | "altin" => Some((241, 196, 15)),
        "discord" => Some((88, 101, 242)),
        _ => None,
    };
    if let Some((r, g, b)) = named {
        return Some(serenity::Colour::from_rgb(r, g, b));
    }
    let hex = text.trim_start_matches('#');
    (hex.len() == 6).then(|| u32::from_str_radix(hex, 16).ok()).flatten().map(serenity::Colour::new)
}

fn url_or_none(value: Option<&String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| v.starts_with("https://") || v.starts_with("http://"))
}

fn build_embed(form: &AnnouncementModal, colour: serenity::Colour) -> CreateEmbed {
    let mut embed = CreateEmbed::new().description(&form.description).colour(colour);
    if let Some(title) = form.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        embed = embed.title(title);
    }
    if let Some(footer) = form.footer.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        embed = embed.footer(CreateEmbedFooter::new(footer));
    }
    if let Some(image) = url_or_none(form.image.as_ref()) {
        embed = embed.image(image);
    }
    if let Some(thumb) = url_or_none(form.thumbnail.as_ref()) {
        embed = embed.thumbnail(thumb);
    }
    embed
}

/// Süslü (embed) duyurular.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_MESSAGES",
    subcommands("duyuru_gonder", "duyuru_duzenle"),
    subcommand_required,
    category = "Araçlar"
)]
pub async fn duyuru(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Embed duyuru oluşturup gönderir (metni açılan pencereye yazarsın; satır atlayabilirsin).
#[poise::command(slash_command, guild_only, rename = "gönder", required_bot_permissions = "SEND_MESSAGES | EMBED_LINKS")]
async fn duyuru_gonder(
    ctx: AppContext<'_>,
    #[description = "Duyurunun gönderileceği kanal"]
    #[channel_types("Text", "News")]
    kanal: serenity::GuildChannel,
    #[description = "Renk: kırmızı, mavi, yeşil, sarı, turuncu, mor, pembe, altın… veya #ff8800"] renk: Option<String>,
    #[rename = "etiket"]
    #[description = "Duyuruda etiketlenecek rol (isteğe bağlı)"]
    role: Option<serenity::Role>,
) -> Result<(), Error> {
    let ctx_any = Context::Application(ctx);
    if let Some(problem) = cannot_send(ctx_any, &kanal) {
        return reply_private(ctx_any, problem).await;
    }
    let colour = match renk.as_deref() {
        None => serenity::Colour::BLURPLE,
        Some(raw) => match parse_colour(raw) {
            Some(c) => c,
            None => return reply_private(ctx_any, format!("❌ `{raw}` tanınmadı. Örnek: `kırmızı`, `mavi`, `#ff8800`.")).await,
        },
    };
    let Some(form) = AnnouncementModal::execute(ctx).await? else {
        return Ok(()); // Pencere kapatıldı / süre doldu.
    };
    let mut message = CreateMessage::new().embed(build_embed(&form, colour));
    if let Some(role) = &role {
        message = message
            .content(role.id.mention().to_string())
            .allowed_mentions(CreateAllowedMentions::new().roles(vec![role.id]));
    }
    let sent = match kanal.id.send_message(ctx, message).await {
        Ok(m) => m,
        Err(e) => return reply_private(ctx_any, format!("❌ Gönderilemedi: {e}")).await,
    };
    reply_private(ctx_any, format!("✅ Duyuru gönderildi: {}\n_Düzenlemek için `/duyuru düzenle` ile bu bağlantıyı kullan._", sent.link())).await
}

/// "https://discord.com/channels/<sunucu>/<kanal>/<mesaj>" → (kanal, mesaj)
fn parse_message_link(link: &str) -> Option<(serenity::ChannelId, serenity::MessageId)> {
    let re = Regex::new(r"discord(?:app)?\.com/channels/\d+/(\d+)/(\d+)").ok()?;
    let caps = re.captures(link)?;
    let channel: u64 = caps.get(1)?.as_str().parse().ok()?;
    let message: u64 = caps.get(2)?.as_str().parse().ok()?;
    (channel != 0 && message != 0).then(|| (serenity::ChannelId::new(channel), serenity::MessageId::new(message)))
}

/// Botun gönderdiği bir duyuruyu düzenler.
#[poise::command(slash_command, guild_only, rename = "düzenle")]
async fn duyuru_duzenle(
    ctx: AppContext<'_>,
    #[rename = "mesaj_bağlantısı"]
    #[description = "Duyurunun bağlantısı (mesaja sağ tık → Mesaj Bağlantısını Kopyala)"]
    link: String,
    #[description = "Yeni renk (boşsa aynı kalır)"] renk: Option<String>,
) -> Result<(), Error> {
    let ctx_any = Context::Application(ctx);
    let Some((channel, message_id)) = parse_message_link(&link) else {
        return reply_private(ctx_any, "❌ Geçerli bir mesaj bağlantısı değil.").await;
    };
    let message = match channel.message(ctx, message_id).await {
        Ok(m) => m,
        Err(_) => return reply_private(ctx_any, "❌ Mesaj bulunamadı (silinmiş veya göremediğim bir kanalda olabilir).").await,
    };
    if message.author.id != ctx.serenity_context().cache.current_user().id {
        return reply_private(ctx_any, "❌ Sadece benim gönderdiğim duyurular düzenlenebilir.").await;
    }
    if message.guild_id != ctx.guild_id() {
        return reply_private(ctx_any, "❌ Bu mesaj bu sunucuda değil.").await;
    }
    let old = message.embeds.first();
    let defaults = AnnouncementModal {
        title: old.and_then(|e| e.title.clone()),
        description: old.and_then(|e| e.description.clone()).unwrap_or_else(|| message.content.clone()),
        footer: old.and_then(|e| e.footer.as_ref().map(|f| f.text.clone())),
        image: old.and_then(|e| e.image.as_ref().map(|i| i.url.clone())),
        thumbnail: old.and_then(|e| e.thumbnail.as_ref().map(|i| i.url.clone())),
    };
    let colour = match renk.as_deref() {
        Some(raw) => match parse_colour(raw) {
            Some(c) => c,
            None => return reply_private(ctx_any, format!("❌ `{raw}` tanınmadı.")).await,
        },
        None => old.and_then(|e| e.colour).unwrap_or(serenity::Colour::BLURPLE),
    };
    let Some(form) = poise::execute_modal(ctx, Some(defaults), Some(MODAL_TIMEOUT)).await? else {
        return Ok(());
    };
    channel
        .edit_message(ctx, message_id, EditMessage::new().embed(build_embed(&form, colour)))
        .await?;
    reply_private(ctx_any, "✅ Duyuru güncellendi.").await
}

// ===================== Otomatik cevap =====================

/// Otomatik cevaplar: bir kelime/cümle yazılınca bot cevap verir.
#[poise::command(
    slash_command,
    guild_only,
    rename = "otocevap",
    default_member_permissions = "MANAGE_MESSAGES",
    subcommands("oto_ekle", "oto_sil", "oto_liste"),
    subcommand_required,
    category = "Araçlar"
)]
pub async fn otocevap(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Otomatik cevap ekler veya günceller.
#[poise::command(slash_command, guild_only, rename = "ekle")]
async fn oto_ekle(
    ctx: Context<'_>,
    #[rename = "tetikleyici"]
    #[description = "Hangi kelime/cümle yazılınca (örn. sa, iyi geceler)"]
    #[max_length = 100]
    trigger: String,
    #[rename = "cevap"]
    #[description = "Botun cevabı. {kullanıcı} yazan kişiyi etiketler; satır için \\n"]
    #[max_length = 1500]
    response: String,
    #[rename = "eşleşme"]
    #[description = "Nasıl eşleşsin (varsayılan: kelime olarak geçerse)"]
    mode: Option<MatchMode>,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let trigger = autoresponder::normalize(&trigger);
    if trigger.is_empty() {
        return reply_private(ctx, "❌ Tetikleyici harf veya rakam içermeli.").await;
    }
    let response = response.replace("\\n", "\n");
    let data = ctx.data();
    let existing = data.autoresponder.rules(&data.db, guild).await?;
    if !existing.iter().any(|r| r.trigger == trigger) && existing.len() >= autoresponder::MAX_RULES {
        return reply_private(ctx, format!("❌ En fazla {} otomatik cevap olabilir.", autoresponder::MAX_RULES)).await;
    }
    let mode = mode.unwrap_or(MatchMode::Word);
    let created = data.autoresponder.set(&data.db, guild, &trigger, response.trim(), mode, ctx.author().id).await?;
    let verb = if created { "eklendi" } else { "güncellendi" };
    reply_private(ctx, format!("✅ `{trigger}` ({}) otomatik cevabı {verb}.", mode.label())).await
}

async fn autocomplete_trigger(ctx: Context<'_>, partial: &str) -> Vec<String> {
    let Some(guild) = ctx.guild_id() else {
        return Vec::new();
    };
    let data = ctx.data();
    let partial = autoresponder::normalize(partial);
    data.autoresponder
        .rules(&data.db, guild)
        .await
        .map(|rules| rules.iter().map(|r| r.trigger.clone()).filter(|t| t.contains(&partial)).take(25).collect())
        .unwrap_or_default()
}

/// Otomatik cevabı siler.
#[poise::command(slash_command, guild_only, rename = "sil")]
async fn oto_sil(
    ctx: Context<'_>,
    #[rename = "tetikleyici"]
    #[description = "Silinecek tetikleyici"]
    #[autocomplete = "autocomplete_trigger"]
    trigger: String,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let trigger = autoresponder::normalize(&trigger);
    let data = ctx.data();
    let text = if data.autoresponder.remove(&data.db, guild, &trigger).await? {
        format!("🗑️ `{trigger}` silindi.")
    } else {
        format!("`{trigger}` adında otomatik cevap yok.")
    };
    reply_private(ctx, text).await
}

/// Otomatik cevapları listeler.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn oto_liste(ctx: Context<'_>) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let data = ctx.data();
    let rules = data.autoresponder.rules(&data.db, guild).await?;
    let text = if rules.is_empty() {
        "Henüz otomatik cevap yok. `/otocevap ekle` ile ekleyebilirsin.".to_owned()
    } else {
        let mut lines: Vec<String> = rules
            .iter()
            .map(|r| {
                let preview: String = r.response.chars().take(60).collect::<String>().replace('\n', " ");
                format!("• `{}` _({})_ → {preview}", r.trigger, r.mode.label())
            })
            .collect();
        lines.sort();
        lines.join("\n").chars().take(1900).collect()
    };
    reply_private(ctx, text).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colours() {
        assert_eq!(parse_colour("Kırmızı"), Some(serenity::Colour::from_rgb(237, 66, 69)));
        assert_eq!(parse_colour("#FF8800"), Some(serenity::Colour::new(0xff8800)));
        assert_eq!(parse_colour("00ff00"), Some(serenity::Colour::new(0x00ff00)));
        assert_eq!(parse_colour("gökkuşağı"), None);
        assert_eq!(parse_colour("#12345"), None);
    }

    #[test]
    fn parses_message_links() {
        let link = "https://discord.com/channels/1556440251059408966/1557438450502729770/1557999999999999999";
        assert_eq!(
            parse_message_link(link),
            Some((serenity::ChannelId::new(1557438450502729770), serenity::MessageId::new(1557999999999999999)))
        );
        assert_eq!(parse_message_link("https://example.com/x"), None);
    }
}
