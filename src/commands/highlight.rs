//! `/vurgu`: seçilen kelimeler bir kanalda geçince özelden haber verilir.

use super::moderation::{guild_id, reply_private};
use crate::data::Context;
use crate::error::Error;
use crate::highlights;

/// Vurgu bildirimleri: seçtiğin kelime bir kanalda geçince sana özelden haber verilir.
#[poise::command(slash_command, guild_only, rename = "vurgu", subcommands("ekle", "sil", "liste"), subcommand_required, category = "Araçlar")]
pub async fn vurgu(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Bildirim alacağın bir kelime ekler (en fazla 10).
#[poise::command(slash_command, guild_only, user_cooldown = 3)]
async fn ekle(
    ctx: Context<'_>,
    #[description = "Kelime ya da kısa söz (en az 3 harf; ör. adın, oyununun adı)"]
    #[max_length = 40]
    kelime: String,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let word = match highlights::normalize(&kelime) {
        Ok(word) => word,
        Err(reason) => return reply_private(ctx, format!("❌ {reason}")).await,
    };
    if !highlights::add(&ctx.data().db, guild, ctx.author().id, &word).await? {
        return reply_private(ctx, format!("❌ En fazla {} kelime ekleyebilirsin. Önce `/vurgu sil` ile birini kaldır.", highlights::MAX_WORDS)).await;
    }
    reply_private(
        ctx,
        format!(
            "🔔 **{word}** eklendi. Bu kelime görebildiğin bir kanalda geçince sana özelden haber vereceğim \
             (o kanalda son 5 dakikada yazdıysan haber vermem). Özel mesajlarının açık olduğundan emin ol."
        ),
    )
    .await
}

/// Bir kelimeyi (ya da hepsini) kaldırır.
#[poise::command(slash_command, guild_only)]
async fn sil(
    ctx: Context<'_>,
    #[description = "Kaldırılacak kelime (boşsa hepsi)"]
    #[max_length = 40]
    kelime: Option<String>,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let word = match kelime.as_deref().map(highlights::normalize).transpose() {
        Ok(word) => word,
        Err(reason) => return reply_private(ctx, format!("❌ {reason}")).await,
    };
    let removed = highlights::remove(&ctx.data().db, guild, ctx.author().id, word.as_deref()).await?;
    let text = match (removed, word) {
        (0, _) => "Kaldırılacak kelime bulunamadı. Listen: `/vurgu liste`.".to_owned(),
        (_, Some(word)) => format!("✅ **{word}** kaldırıldı."),
        (n, None) => format!("✅ {n} kelimenin hepsi kaldırıldı."),
    };
    reply_private(ctx, text).await
}

/// Vurgu kelimelerini gösterir.
#[poise::command(slash_command, guild_only)]
async fn liste(ctx: Context<'_>) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let words = highlights::list(&ctx.data().db, guild, ctx.author().id).await?;
    if words.is_empty() {
        return reply_private(ctx, "Henüz vurgu kelimen yok. Eklemek için: `/vurgu ekle kelime:…`").await;
    }
    let lines: Vec<String> = words.iter().map(|w| format!("• {w}")).collect();
    reply_private(ctx, format!("🔔 **Vurgu kelimelerin** ({}/{})\n{}", words.len(), highlights::MAX_WORDS, lines.join("\n"))).await
}
