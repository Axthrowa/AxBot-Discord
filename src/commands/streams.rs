//! `/yayın` — Twitch / Kick yayın duyurusu abonelikleri.

use anyhow::anyhow;
use poise::serenity_prelude::{self as serenity, GuildId, Mentionable};
use poise::CreateReply;
use tracing::warn;

use crate::data::Context;
use crate::error::Error;
use crate::streams::{self, Platform, Subscription, MAX_SUBSCRIPTIONS_PER_GUILD};

/// Twitch / Kick yayın duyuruları.
#[poise::command(
    slash_command,
    guild_only,
    rename = "yayın",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("ekle", "sil", "liste"),
    subcommand_required,
    category = "Yayın"
)]
pub async fn yayin(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Bir yayıncı canlıya geçtiğinde duyuru yapılmasını sağlar.
#[poise::command(slash_command, guild_only)]
async fn ekle(
    ctx: Context<'_>,
    #[description = "Platform"] platform: Platform,
    #[rename = "yayıncı"]
    #[description = "Kanal adı veya bağlantısı (örn. elraenn ya da https://kick.com/elraenn)"]
    streamer: String,
    #[description = "Duyurunun yapılacağı kanal"]
    #[channel_types("Text", "News")]
    kanal: serenity::GuildChannel,
    #[description = "Duyuruda etiketlenecek rol (opsiyonel)"] rol: Option<serenity::Role>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();

    if !data.streams.is_configured(platform) {
        return reply(
            ctx,
            format!(
                "❌ {} entegrasyonu kapalı. `.env` dosyasına `{}` eklenip bot yeniden başlatılmalı.",
                platform.label(),
                platform.env_hint()
            ),
        )
        .await;
    }
    let Some(login) = streams::normalize_login(&streamer) else {
        return reply(ctx, "❌ Geçersiz kanal adı (2-25 karakter: harf, rakam, _ veya -).".to_owned()).await;
    };
    if streams::count(&data.db, guild_id).await? >= MAX_SUBSCRIPTIONS_PER_GUILD {
        return reply(ctx, format!("❌ Bir sunucu en fazla {MAX_SUBSCRIPTIONS_PER_GUILD} yayıncı takip edebilir.")).await;
    }

    ctx.defer_ephemeral().await?;
    let display = match data.streams.resolve(platform, &login).await {
        Ok(Some(name)) => name,
        Ok(None) => {
            return reply(ctx, format!("❌ {}'te **{login}** adında bir kanal bulunamadı.", platform.label())).await;
        }
        Err(e) => {
            warn!(platform = platform.label(), error = ?e, "Yayıncı doğrulanamadı");
            return reply(ctx, format!("❌ {} API'sine ulaşılamadı: {e}", platform.label())).await;
        }
    };

    let sub = Subscription {
        platform,
        login,
        channel_id: kanal.id,
        role_id: rol.as_ref().map(|r| r.id),
    };
    streams::upsert(&data.db, guild_id, &sub).await?;

    let mut text = format!(
        "✅ **{display}** ({}) canlıya geçince {} kanalında duyurulacak.",
        platform.label(),
        kanal.id.mention()
    );
    if let Some(role) = &rol {
        text.push_str(&format!(" Etiketlenecek rol: {}", role.id.mention()));
    }
    reply(ctx, text).await
}

/// Bir yayıncının duyurularını kapatır.
#[poise::command(slash_command, guild_only)]
async fn sil(
    ctx: Context<'_>,
    #[description = "Platform"] platform: Platform,
    #[rename = "yayıncı"]
    #[description = "Kanal adı"]
    streamer: String,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let Some(login) = streams::normalize_login(&streamer) else {
        return reply(ctx, "❌ Geçersiz kanal adı.".to_owned()).await;
    };
    let text = if streams::remove(&ctx.data().db, guild_id, platform, &login).await? {
        format!("✅ **{login}** ({}) artık duyurulmayacak.", platform.label())
    } else {
        format!("ℹ️ **{login}** ({}) zaten takip edilmiyor.", platform.label())
    };
    reply(ctx, text).await
}

/// Takip edilen yayıncıları listeler.
#[poise::command(slash_command, guild_only)]
async fn liste(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let subs = streams::list(&data.db, guild_id).await?;

    let text = if subs.is_empty() {
        "Henüz takip edilen yayıncı yok. `/yayın ekle` ile ekleyebilirsin.".to_owned()
    } else {
        subs.iter()
            .map(|s| {
                let role = s
                    .role_id
                    .map(|r| format!(" • {}", r.mention()))
                    .unwrap_or_default();
                let status = if data.streams.is_configured(s.platform) { "" } else { " ⚠️ entegrasyon kapalı" };
                format!("**{}** · {} → {}{role}{status}", s.platform.label(), s.login, s.channel_id.mention())
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    reply(ctx, text).await
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
