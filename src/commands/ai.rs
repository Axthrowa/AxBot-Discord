//! `/ai` — yapay zekâ sohbet kanallarının yönetimi.

use anyhow::anyhow;
use poise::serenity_prelude::{self as serenity, GuildId, Mentionable};
use poise::CreateReply;

use crate::ai;
use crate::data::Context;
use crate::error::Error;

/// Yapay zekâ sohbet ayarları.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    subcommands("kanal_ekle", "kanal_sil", "karakter", "durum", "sifirla"),
    subcommand_required,
    category = "Yapay Zekâ"
)]
pub async fn ai(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Botun her mesaja yapay zekâ ile yanıt vereceği bir kanal ekler.
#[poise::command(slash_command, guild_only, rename = "kanal-ekle")]
async fn kanal_ekle(
    ctx: Context<'_>,
    #[description = "Yapay zekâ kanalı"]
    #[channel_types("Text")]
    kanal: serenity::GuildChannel,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();

    let added = data.ai.add_channel(&data.db, guild_id, kanal.id).await?;
    let mut text = if added {
        format!("✅ {} artık yapay zekâ kanalı. Oraya yazılan her mesaja yanıt vereceğim.", kanal.id.mention())
    } else {
        format!("ℹ️ {} zaten yapay zekâ kanalı.", kanal.id.mention())
    };
    if data.ai.describe().is_none() {
        text.push_str(
            "\n\n⚠️ Ancak yapay zekâ anahtarı ayarlı değil: `.env` dosyasına `NVIDIA_API_KEY` \
             veya `GEMINI_API_KEY` eklenip bot yeniden başlatılmalı.",
        );
    }
    reply(ctx, text).await
}

/// Bir kanalda yapay zekâ yanıtlarını kapatır.
#[poise::command(slash_command, guild_only, rename = "kanal-sil")]
async fn kanal_sil(
    ctx: Context<'_>,
    #[description = "Yapay zekâsı kapatılacak kanal"]
    #[channel_types("Text")]
    kanal: serenity::GuildChannel,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let text = if data.ai.remove_channel(&data.db, guild_id, kanal.id).await? {
        ai::clear_history(&data.redis, kanal.id).await?;
        format!("✅ {} artık yapay zekâ kanalı değil.", kanal.id.mention())
    } else {
        format!("ℹ️ {} zaten yapay zekâ kanalı değil.", kanal.id.mention())
    };
    reply(ctx, text).await
}

/// Bu kanalda botun canlandıracağı karakteri seçer.
#[poise::command(slash_command, guild_only)]
async fn karakter(
    ctx: Context<'_>,
    #[description = "Karakterin adı; varsayılana dönmek için boş bırak"]
    #[max_length = 40]
    #[autocomplete = "autocomplete_character"]
    ad: Option<String>,
) -> Result<(), Error> {
    use crate::ai::characters::AiSettings;
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let channel = ctx.channel_id();
    let mut settings = (*data.settings.get::<AiSettings>(&data.db, guild_id).await?).clone();
    let (key, text) = match ad.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        None => (None, "✅ Bu kanal varsayılan kişiliğe döndü.".to_owned()),
        Some(name) => match settings.find_by_name(name) {
            Some(key) => {
                let shown = settings.resolve(&key).map_or_else(String::new, |(name, _)| name.to_owned());
                (Some(key), format!("✅ Bu kanalda artık **{shown}** konuşuyor."))
            }
            None => {
                let names: Vec<String> = settings.choices().into_iter().map(|(_, name)| name).collect();
                return reply(ctx, format!("Böyle bir karakter yok. Seçenekler: {}", names.join(", "))).await;
            }
        },
    };
    if settings.assign(channel.get(), key) {
        data.settings.set(&data.db, guild_id, settings).await?;
        ai::clear_history(&data.redis, channel).await?;
    }
    let mut text = text;
    if !data.ai.channels(&data.db, guild_id).await?.contains(&channel) {
        text.push_str("\nℹ️ Bu kanal henüz yapay zekâ kanalı değil; `/ai kanal-ekle` ile açabilirsin.");
    }
    reply(ctx, text).await
}

async fn autocomplete_character(ctx: Context<'_>, partial: &str) -> Vec<String> {
    use crate::ai::characters::AiSettings;
    let Some(guild_id) = ctx.guild_id() else {
        return Vec::new();
    };
    let data = ctx.data();
    let Ok(settings) = data.settings.get::<AiSettings>(&data.db, guild_id).await else {
        return Vec::new();
    };
    let wanted = partial.to_lowercase();
    settings.choices().into_iter().map(|(_, name)| name).filter(|name| name.to_lowercase().contains(&wanted)).take(25).collect()
}

/// Yapay zekâya kısa bir metin yazdırır (duyuru, şiir, açıklama…).
#[poise::command(slash_command, guild_only, rename = "yaz", user_cooldown = 20, category = "Yapay Zekâ")]
pub async fn yaz(
    ctx: Context<'_>,
    #[description = "Ne yazılsın? (ör. sunucu kuralları için kısa bir duyuru)"]
    #[max_length = 600]
    istek: String,
) -> Result<(), Error> {
    use crate::ai::characters::AiSettings;
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    if data.ai.describe().is_none() {
        return reply(ctx, "Yapay zekâ bu botta açık değil.".to_owned()).await;
    }
    ctx.defer().await?;
    let guild_name = ctx.guild().map_or_else(|| "bu".to_owned(), |g| g.name.clone());
    let settings = data.settings.get::<AiSettings>(&data.db, guild_id).await.ok();
    let persona = settings.as_deref().and_then(|s| s.persona_for(ctx.channel_id().get()));
    let answer = match ai::write_once(data, &guild_name, persona, &istek).await {
        Some(Ok(answer)) if !answer.trim().is_empty() => answer,
        Some(Err(e)) => {
            tracing::warn!(guild = %guild_id, error = ?e, "AI metni üretilemedi");
            return reply(ctx, "⚠️ Şu an yazamıyorum, biraz sonra tekrar dene.".to_owned()).await;
        }
        _ => return reply(ctx, "⚠️ Şu an yazamıyorum, biraz sonra tekrar dene.".to_owned()).await,
    };
    let text: String = answer.chars().take(1900).collect();
    // Model çıktısındaki etiketler kimseyi bildirmez.
    ctx.send(CreateReply::default().content(text).allowed_mentions(serenity::CreateAllowedMentions::new())).await?;
    Ok(())
}

/// Yapay zekâ sağlayıcısını ve açık kanalları gösterir.
#[poise::command(slash_command, guild_only)]
async fn durum(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let channels = data.ai.channels(&data.db, guild_id).await?;

    let provider = data
        .ai
        .describe()
        .unwrap_or_else(|| "❌ Kapalı (`NVIDIA_API_KEY` veya `GEMINI_API_KEY` yok)".to_owned());
    let list = if channels.is_empty() {
        "_Yok — `/ai kanal-ekle` ile ekleyebilirsin._".to_owned()
    } else {
        channels
            .iter()
            .map(|c| c.mention().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    reply(ctx, format!("🤖 **Sağlayıcı:** {provider}\n**Kanallar:** {list}")).await
}

/// Bu kanalın yapay zekâ sohbet geçmişini siler (bot önceki konuşmayı unutur).
#[poise::command(slash_command, guild_only, rename = "sıfırla")]
async fn sifirla(ctx: Context<'_>) -> Result<(), Error> {
    ai::clear_history(&ctx.data().redis, ctx.channel_id()).await?;
    reply(ctx, "🧹 Bu kanalın sohbet geçmişi silindi.".to_owned()).await
}

fn guild_id(ctx: Context<'_>) -> Result<GuildId, Error> {
    ctx.guild_id()
        .ok_or_else(|| anyhow!("guild_only komut sunucu dışında çalıştı"))
}

async fn reply(ctx: Context<'_>, content: String) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(content).ephemeral(true))
        .await?;
    Ok(())
}
