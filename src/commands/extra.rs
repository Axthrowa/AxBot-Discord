//! `/hayal-et` (yapay zekâ ile resim üretimi) ve `/kartım` (kişisel rütbe kartı).

use std::time::Duration;

use base64::Engine as _;
use poise::serenity_prelude::{self as serenity, CreateAllowedMentions, CreateAttachment};
use poise::CreateReply;
use tokio::sync::Semaphore;

use super::moderation::reply_private;
use super::profile::{profil, rep};
use crate::data::Context;
use crate::error::Error;
use crate::rankcard::{self, Personal};

const IMAGE_TIMEOUT: Duration = Duration::from_secs(90);
/// Aynı anda en fazla bu kadar resim üretilir (API kotasını korur).
static IMAGE_SLOTS: Semaphore = Semaphore::const_new(2);

#[derive(Debug, PartialEq, Eq)]
enum ImageError {
    /// Model isteği uygunsuz bulup reddetti.
    Filtered,
    Failed,
}

/// NVIDIA NIM resim üretim cevabından resmi çıkarır.
fn image_from_response(json: &serde_json::Value) -> Result<Vec<u8>, ImageError> {
    let artifact = json.pointer("/artifacts/0").ok_or(ImageError::Failed)?;
    let reason = artifact.get("finishReason").and_then(serde_json::Value::as_str).unwrap_or("SUCCESS");
    if reason != "SUCCESS" {
        return Err(ImageError::Filtered);
    }
    let encoded = artifact.get("base64").and_then(serde_json::Value::as_str).ok_or(ImageError::Failed)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| ImageError::Failed)?;
    // Boş ya da resim olmayan cevap gönderilmez.
    let is_image = bytes.starts_with(&[0xFF, 0xD8]) || bytes.starts_with(&[0x89, b'P', b'N', b'G']);
    if is_image { Ok(bytes) } else { Err(ImageError::Failed) }
}

async fn generate(client: &reqwest::Client, key: &str, model: &str, prompt: &str) -> Result<Vec<u8>, ImageError> {
    // "schnell" modelleri en fazla 4 adım kabul eder.
    let steps = if model.contains("schnell") { 4 } else { 30 };
    let body = serde_json::json!({ "prompt": prompt, "width": 1024, "height": 1024, "seed": 0, "steps": steps });
    let response = client
        .post(format!("https://ai.api.nvidia.com/v1/genai/{model}"))
        .bearer_auth(key)
        .header("Accept", "application/json")
        .json(&body)
        .timeout(IMAGE_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "Resim üretim servisine ulaşılamadı");
            ImageError::Failed
        })?;
    let status = response.status();
    if !status.is_success() {
        tracing::warn!(%status, model, "Resim üretim servisi hata döndü");
        // 4xx çoğunlukla içerik süzgecidir (422) ya da kotadır (429).
        return Err(if status == reqwest::StatusCode::UNPROCESSABLE_ENTITY { ImageError::Filtered } else { ImageError::Failed });
    }
    let json: serde_json::Value = response.json().await.map_err(|_| ImageError::Failed)?;
    image_from_response(&json)
}

/// Yapay zekâya yazdığın şeyin resmini çizdirir.
#[poise::command(
    slash_command,
    rename = "hayal-et",
    user_cooldown = 30,
    category = "Yapay Zekâ",
    install_context = "Guild|User",
    interaction_context = "Guild|BotDm|PrivateChannel"
)]
pub async fn hayal_et(
    ctx: Context<'_>,
    #[description = "Ne çizilsin? (İngilizce yazarsan sonuç daha iyi olur)"]
    #[min_length = 3]
    #[max_length = 500]
    istek: String,
) -> Result<(), Error> {
    let settings = crate::integrations::get();
    let Some(key) = settings.nvidia_api_key.as_deref() else {
        return reply_private(ctx, "Resim üretimi için `.env` dosyasında `NVIDIA_API_KEY` tanımlı olmalı.").await;
    };
    // Sunucunun yasaklı kelimeleri resim isteğinde de geçerlidir.
    if let Some(guild_id) = ctx.guild_id() {
        if ctx.data().automod.has_profanity(&ctx.data().db, &ctx.data().redis, guild_id, &istek).await {
            return reply_private(ctx, "Bu istek sunucunun yasaklı kelimelerini içeriyor.").await;
        }
    }
    ctx.defer().await?;
    let Ok(_slot) = IMAGE_SLOTS.acquire().await else {
        return reply_private(ctx, "⚠️ Şu an çizemiyorum, biraz sonra tekrar dene.").await;
    };
    match generate(&ctx.data().http_client, key, &settings.image_model, istek.trim()).await {
        Ok(bytes) => {
            let name = if bytes.starts_with(&[0xFF, 0xD8]) { "resim.jpg" } else { "resim.png" };
            let caption: String = format!("🎨 **{}**", istek.trim().replace("**", "")).chars().take(600).collect();
            ctx.send(
                CreateReply::default()
                    .content(caption)
                    .attachment(CreateAttachment::bytes(bytes, name))
                    .allowed_mentions(CreateAllowedMentions::new()),
            )
            .await?;
        }
        Err(ImageError::Filtered) => reply_private(ctx, "🚫 Bu istek güvenlik süzgecine takıldı; başka bir şey dene.").await?,
        Err(ImageError::Failed) => reply_private(ctx, "⚠️ Resim şu an üretilemedi, biraz sonra tekrar dene.").await?,
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum FontChoice {
    #[name = "Noto Sans (varsayılan)"]
    Noto,
    #[name = "Poppins"]
    Poppins,
    #[name = "PT Serif"]
    PtSerif,
    #[name = "Pacifico (el yazısı)"]
    Pacifico,
}

impl FontChoice {
    fn key(self) -> &'static str {
        match self {
            Self::Noto => "noto",
            Self::Poppins => "poppins",
            Self::PtSerif => "ptserif",
            Self::Pacifico => "pacifico",
        }
    }
}

/// Rütbe kartını özelleştir; profilini gör, itibar ver.
#[poise::command(slash_command, guild_only, rename = "kartım", subcommands("kart_ayarla", "kart_sifirla", "profil", "rep"), subcommand_required, category = "Seviye")]
pub async fn kartim(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Rütbe kartının rengini, arka plan resmini ya da yazı tipini değiştirir.
#[poise::command(slash_command, guild_only, rename = "ayarla", user_cooldown = 10)]
async fn kart_ayarla(
    ctx: Context<'_>,
    #[description = "Vurgu rengi (#RRGGBB, ör. #FF8800)"]
    #[max_length = 7]
    renk: Option<String>,
    #[description = "Arka plan resmi (https bağlantısı; PNG, JPEG ya da WebP)"]
    #[rename = "arka-plan"]
    #[max_length = 400]
    background: Option<String>,
    #[description = "Yazı tipi"]
    #[rename = "yazı-tipi"]
    font: Option<FontChoice>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    if renk.is_none() && background.is_none() && font.is_none() {
        return reply_private(ctx, "En az bir şey seç: `renk`, `arka-plan` ya da `yazı-tipi`.").await;
    }
    let data = ctx.data();
    let mut card: Personal = rankcard::get(&data.db, guild_id, ctx.author().id).await?;
    if let Some(raw) = renk {
        let Some([r, g, b]) = crate::cards::parse_hex(&raw) else {
            return reply_private(ctx, "Renk `#RRGGBB` biçiminde olmalı (ör. `#FF8800`).").await;
        };
        card.accent = Some(format!("#{r:02X}{g:02X}{b:02X}"));
    }
    if let Some(font) = font {
        card.font = Some(font.key().to_owned());
    }
    if let Some(url) = background {
        let url = url.trim().to_owned();
        ctx.defer_ephemeral().await?;
        // Kabul etmeden önce gerçekten indirilebilen bir resim mi diye bakılır (https + herkese açık adres).
        if url.chars().count() > rankcard::MAX_URL_LEN || crate::cards::fetch_image(&url).await.is_none() {
            return reply_private(ctx, "❌ Resim alınamadı. `https://` ile başlayan, herkese açık bir resim bağlantısı ver (en fazla 8 MB).").await;
        }
        card.background = Some(url);
    }
    rankcard::set(&data.db, guild_id, ctx.author().id, &card).await?;
    reply_private(ctx, "✅ Rütbe kartın güncellendi. `/seviye` ile görebilirsin.").await
}

/// Rütbe kartını varsayılana döndürür (yöneticiler başkasınınkini de sıfırlayabilir).
#[poise::command(slash_command, guild_only, rename = "sıfırla")]
async fn kart_sifirla(
    ctx: Context<'_>,
    #[description = "Kartı sıfırlanacak üye (yalnızca yöneticiler; boşsa sen)"]
    #[rename = "kullanıcı"]
    user: Option<serenity::User>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let target = user.map_or(ctx.author().id, |u| u.id);
    if target != ctx.author().id {
        let allowed = match ctx.author_member().await {
            Some(member) => ctx.guild().is_some_and(|g| {
                g.owner_id == member.user.id || g.member_permissions(&member).intersects(serenity::Permissions::ADMINISTRATOR | serenity::Permissions::MANAGE_GUILD)
            }),
            None => false,
        };
        if !allowed {
            return reply_private(ctx, "Başkasının kartını sıfırlamak için 'Sunucuyu Yönet' iznin olmalı.").await;
        }
    }
    let text = if rankcard::reset(&ctx.data().db, guild_id, target).await? {
        "🧹 Rütbe kartı varsayılana döndü."
    } else {
        "Bu üyenin özel kart ayarı zaten yok."
    };
    reply_private(ctx, text).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_image_responses() {
        // "/9j/" = JPEG başlangıcı (FF D8 FF).
        let ok = serde_json::json!({ "artifacts": [{ "base64": "/9j/4AAQ", "finishReason": "SUCCESS", "seed": 1 }] });
        assert!(image_from_response(&ok).is_ok_and(|bytes| bytes.starts_with(&[0xFF, 0xD8])));
        let filtered = serde_json::json!({ "artifacts": [{ "base64": "/9j/4AAQ", "finishReason": "CONTENT_FILTERED" }] });
        assert_eq!(image_from_response(&filtered), Err(ImageError::Filtered));
        let not_image = serde_json::json!({ "artifacts": [{ "base64": "aGVsbG8=", "finishReason": "SUCCESS" }] });
        assert_eq!(image_from_response(&not_image), Err(ImageError::Failed));
        assert_eq!(image_from_response(&serde_json::json!({ "artifacts": [] })), Err(ImageError::Failed));
        assert_eq!(image_from_response(&serde_json::json!({ "artifacts": [{ "base64": "!!!" }] })), Err(ImageError::Failed));
    }

    #[test]
    fn font_keys_exist() {
        for font in [FontChoice::Noto, FontChoice::Poppins, FontChoice::PtSerif, FontChoice::Pacifico] {
            assert!(crate::cards::font_set(font.key()).is_some(), "{}", font.key());
        }
    }
}
