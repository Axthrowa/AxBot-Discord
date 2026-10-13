//! Topluluk etkileşimi: `/starboard`, `/etiket`, `/öneri`, `/öneri-kanalı`, `/öneri-yanıtla`.

use poise::serenity_prelude::{self as serenity, CreateEmbed, Mentionable};
use poise::CreateReply;

use super::community::cannot_send;
use super::moderation::{guild_id, reply_private};
use crate::data::Context;
use crate::error::Error;
use crate::guild_config::Setting;
use crate::suggestions::{self, ReviewOutcome, Status};
use crate::tags;

const DEFAULT_STAR_THRESHOLD: u32 = 3;

/// Starboard'u ayarlar: ⭐ tepkisi eşiği geçen mesajlar seçilen kanala kopyalanır.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    category = "Topluluk"
)]
pub async fn starboard(
    ctx: Context<'_>,
    #[description = "Starboard kanalı (boş bırakırsan starboard kapanır)"]
    #[channel_types("Text")]
    kanal: Option<serenity::GuildChannel>,
    #[rename = "eşik"]
    #[description = "Kaç ⭐ gereksin (varsayılan 3)"]
    #[min = 1]
    #[max = 50]
    threshold: Option<u32>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let threshold = threshold.unwrap_or(DEFAULT_STAR_THRESHOLD);
    let Some(kanal) = kanal else {
        data.guild_configs
            .set(&data.db, guild_id, Setting::Starboard(None, threshold))
            .await?;
        return reply_private(ctx, "⭐ Starboard kapatıldı.").await;
    };
    if let Some(problem) = cannot_send(ctx, &kanal) {
        return reply_private(ctx, problem).await;
    }
    data.guild_configs
        .set(&data.db, guild_id, Setting::Starboard(Some(kanal.id), threshold))
        .await?;
    reply_private(
        ctx,
        format!(
            "⭐ Starboard {} kanalına ayarlandı. **{threshold}** ⭐ alan mesajlar oraya kopyalanacak.\n\
             _Mesaj sahibinin ve botların yıldızları sayılmaz; yıldızlar eşiğin altına düşerse kopya silinir._",
            kanal.id.mention()
        ),
    )
    .await
}

/// Etiketler: sohbette `!ad` yazınca kaydedilmiş metni gönderir.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("etiket_goster", "etiket_ekle", "etiket_sil", "etiket_liste"),
    subcommand_required,
    category = "Topluluk"
)]
pub async fn etiket(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

async fn autocomplete_tag(ctx: Context<'_>, partial: &str) -> Vec<String> {
    let Some(guild_id) = ctx.guild_id() else {
        return Vec::new();
    };
    let data = ctx.data();
    let Ok(all) = data.tags.all(&data.db, guild_id).await else {
        return Vec::new();
    };
    let partial = partial.to_lowercase();
    all.keys()
        .filter(|name| name.contains(&partial))
        .take(25)
        .cloned()
        .collect()
}

/// Bir etiketi gönderir.
#[poise::command(slash_command, guild_only, rename = "göster")]
async fn etiket_goster(
    ctx: Context<'_>,
    #[rename = "ad"]
    #[description = "Etiket adı"]
    #[autocomplete = "autocomplete_tag"]
    name: String,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let Ok(name) = tags::normalize_name(&name) else {
        return reply_private(ctx, "Böyle bir etiket yok.").await;
    };
    match data.tags.all(&data.db, guild_id).await?.get(&name) {
        Some(content) => {
            ctx.say(content.as_str()).await?;
            Ok(())
        }
        None => reply_private(ctx, format!("`{name}` adında bir etiket yok. `/etiket liste` ile bakabilirsin.")).await,
    }
}

/// Etiket ekler veya günceller (Mesajları Yönet izni gerekir).
#[poise::command(slash_command, guild_only, rename = "ekle", required_permissions = "MANAGE_MESSAGES")]
async fn etiket_ekle(
    ctx: Context<'_>,
    #[rename = "ad"]
    #[description = "Etiket adı (boşluksuz, örn. kurallar). Sohbette !ad ile çağrılır."]
    name: String,
    #[rename = "içerik"]
    #[description = "Gönderilecek metin. Satır atlamak için \\n yaz."]
    #[max_length = 2000]
    content: String,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let name = match tags::normalize_name(&name) {
        Ok(name) => name,
        Err(reason) => return reply_private(ctx, format!("❌ {reason}")).await,
    };
    let content = content.replace("\\n", "\n");
    if content.trim().is_empty() {
        return reply_private(ctx, "❌ İçerik boş olamaz.").await;
    }
    let existing = data.tags.all(&data.db, guild_id).await?;
    if !existing.contains_key(&name) && existing.len() >= tags::MAX_TAGS {
        return reply_private(ctx, format!("❌ Bir sunucuda en fazla {} etiket olabilir.", tags::MAX_TAGS)).await;
    }
    let created = data.tags.set(&data.db, guild_id, &name, &content, ctx.author().id).await?;
    let verb = if created { "eklendi" } else { "güncellendi" };
    reply_private(ctx, format!("✅ `{name}` etiketi {verb}. Sohbette `!{name}` yazarak kullanabilirsin.")).await
}

/// Bir etiketi siler (Mesajları Yönet izni gerekir).
#[poise::command(slash_command, guild_only, rename = "sil", required_permissions = "MANAGE_MESSAGES")]
async fn etiket_sil(
    ctx: Context<'_>,
    #[rename = "ad"]
    #[description = "Silinecek etiket"]
    #[autocomplete = "autocomplete_tag"]
    name: String,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let name = tags::normalize_name(&name).unwrap_or(name);
    let text = if data.tags.remove(&data.db, guild_id, &name).await? {
        format!("🗑️ `{name}` etiketi silindi.")
    } else {
        format!("`{name}` adında bir etiket yok.")
    };
    reply_private(ctx, text).await
}

/// Sunucudaki etiketleri listeler.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn etiket_liste(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let all = data.tags.all(&data.db, guild_id).await?;
    if all.is_empty() {
        return reply_private(ctx, "Henüz etiket yok. `/etiket ekle` ile ekleyebilirsin.").await;
    }
    let mut list = String::new();
    for name in all.keys() {
        let item = format!("`!{name}` ");
        if list.len() + item.len() > 3900 {
            list.push('…');
            break;
        }
        list.push_str(&item);
    }
    let embed = CreateEmbed::new()
        .title(format!("🏷️ Etiketler ({})", all.len()))
        .description(list)
        .colour(serenity::Colour::BLURPLE);
    ctx.send(CreateReply::default().embed(embed).ephemeral(true)).await?;
    Ok(())
}

/// Öneri kanalını ayarlar (boş bırakırsan öneriler kapanır).
#[poise::command(
    slash_command,
    guild_only,
    rename = "öneri-kanalı",
    default_member_permissions = "MANAGE_GUILD",
    category = "Topluluk"
)]
pub async fn oneri_kanali(
    ctx: Context<'_>,
    #[description = "Önerilerin gönderileceği kanal"]
    #[channel_types("Text")]
    kanal: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let Some(kanal) = kanal else {
        data.guild_configs
            .set(&data.db, guild_id, Setting::SuggestionChannel(None))
            .await?;
        return reply_private(ctx, "💡 Öneri sistemi kapatıldı.").await;
    };
    if let Some(problem) = cannot_send(ctx, &kanal) {
        return reply_private(ctx, problem).await;
    }
    data.guild_configs
        .set(&data.db, guild_id, Setting::SuggestionChannel(Some(kanal.id)))
        .await?;
    reply_private(
        ctx,
        format!(
            "💡 Öneriler {} kanalına gönderilecek. Üyeler `/öneri` ile öneri yazabilir, \
             yetkililer `/öneri-yanıtla` ile sonuçlandırır.\n\
             _Her öneriye tartışma için alt başlık açılır; bunun için 'Herkese Açık Alt Başlık Oluştur' iznim olmalı._",
            kanal.id.mention()
        ),
    )
    .await
}

/// Sunucu için bir öneri gönderir; diğer üyeler oylar.
#[poise::command(slash_command, guild_only, rename = "öneri", user_cooldown = 60, category = "Topluluk")]
pub async fn oneri(
    ctx: Context<'_>,
    #[rename = "öneri"]
    #[description = "Önerin"]
    #[min_length = 10]
    #[max_length = 1500]
    content: String,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let cfg = data.guild_configs.get(&data.db, guild_id).await?;
    let Some(channel) = cfg.suggestion_channel else {
        return reply_private(ctx, "Bu sunucuda öneri sistemi kapalı. Bir yöneticinin `/öneri-kanalı` ayarlaması gerekiyor.").await;
    };
    ctx.defer_ephemeral().await?;
    let number = suggestions::create(ctx.http(), &data.db, guild_id, channel, ctx.author(), content.trim()).await?;
    reply_private(ctx, format!("✅ **#{number}** numaralı önerin {} kanalına gönderildi.", channel.mention())).await
}

/// Bir öneriyi sonuçlandırır; öneren kişiye DM ile bildirilir.
#[poise::command(
    slash_command,
    guild_only,
    rename = "öneri-yanıtla",
    default_member_permissions = "MANAGE_GUILD",
    category = "Topluluk"
)]
pub async fn oneri_yanitla(
    ctx: Context<'_>,
    #[rename = "numara"]
    #[description = "Öneri numarası"]
    #[min = 1]
    number: u32,
    #[rename = "durum"]
    #[description = "Sonuç"]
    status: Status,
    #[rename = "not"]
    #[description = "Açıklama (öneren kişiye de gider)"]
    #[max_length = 1000]
    note: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    ctx.defer_ephemeral().await?;
    let outcome = suggestions::review(
        ctx.http(),
        &data.db,
        guild_id,
        i32::try_from(number)?,
        status,
        ctx.author().id,
        note.as_deref().map(str::trim).filter(|n| !n.is_empty()),
    )
    .await?;
    let text = match outcome {
        ReviewOutcome::NotFound => format!("**#{number}** numaralı bir öneri yok."),
        ReviewOutcome::Done { notified } => {
            let who = notified.map_or_else(
                || "Öneren kişiye DM gönderilemedi (DM'leri kapalı olabilir).".to_owned(),
                |id| format!("{} DM ile bilgilendirildi.", id.mention()),
            );
            format!("✅ Öneri **#{number}** güncellendi. {who}")
        }
    };
    reply_private(ctx, text).await
}
