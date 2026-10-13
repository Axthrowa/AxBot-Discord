//! `/kurulum`: bot yeni bir sunucuya eklendiğinde neyin eksik olduğunu tek bakışta gösterir
//! (izinler, rol sırası, ayarlanmış / ayarlanmamış özellikler). Aynı rapor web panelinde de var.

use poise::serenity_prelude::{self as serenity, CreateEmbed, Mentionable};
use poise::CreateReply;

use crate::data::Context;
use crate::error::Error;
use crate::setup_report::{self, State, Target};

/// Botun bu sunucudaki kurulum durumu: eksik izinler ve özelliklerin ayarları.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", ephemeral)]
pub async fn kurulum(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    ctx.defer_ephemeral().await?;
    let data = ctx.data();
    let sctx = ctx.serenity_context();
    let report = setup_report::build(&sctx.http, &sctx.cache, &data.db, &data.guild_configs, guild_id).await?;

    let perm_text = if report.missing.is_empty() {
        "✅ Tüm gerekli izinler var.".to_owned()
    } else {
        format!(
            "❌ Eksik: **{}**\nSunucu Ayarları → Roller → botun rolü → izinleri aç, ya da botu aşağıdaki bağlantıyla yeniden ekle.",
            crate::error::permission_names(report.missing)
        )
    };
    let role_text = if report.role_on_top {
        "✅ Botun rolü en üstte.".to_owned()
    } else {
        "⚠️ Botun rolü en üstte değil: altındaki rolleri veremez, o rollerdeki üyeleri susturamaz/atamaz.\n\
         Sunucu Ayarları → Roller → botun rolünü listenin en üstüne sürükle."
            .to_owned()
    };
    let feature_text: String = report
        .features
        .iter()
        .map(|f| {
            let state = match &f.state {
                State::On(Target::Channel(c)) => format!("✅ {}", c.mention()),
                State::On(Target::Role(r)) => format!("✅ {}", r.mention()),
                State::On(Target::Text(t)) => format!("✅ {t}"),
                State::Broken => format!("⚠️ silinmiş → `{}`", f.command),
                State::Off => format!("➖ kapalı · `{}`", f.command),
            };
            format!("**{}:** {state}\n", f.name)
        })
        .collect();

    let bot_text = format!(
        "Yapay zekâ: {}\nÜye olayları (Server Members Intent): {}",
        data.ai.describe().map_or_else(|| "➖ kapalı (bot sahibi anahtar eklemeli)".to_owned(), |d| format!("✅ {d}")),
        if data.members_intent { "✅ açık" } else { "❌ kapalı (Developer Portal → Bot)" },
    );

    let embed = CreateEmbed::new()
        .title("🛠️ AxBot kurulum durumu")
        .colour(if report.healthy() { 0x57F287 } else { 0xFEE75C })
        .field("İzinler", perm_text, false)
        .field("Rol sırası", role_text, false)
        .field("Özellikler", feature_text, false)
        .field("Bot", bot_text, false)
        .field("Botu başka sunucuya ekle", format!("[Davet bağlantısı]({})", super::invite_url(sctx.cache.current_user().id)), false)
        .footer(serenity::CreateEmbedFooter::new("Ayarları web panelinden de yapabilirsin: /panel"));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}
