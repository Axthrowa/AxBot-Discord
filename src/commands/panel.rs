//! `/panel`: web yönetim paneline tek kullanımlık giriş bağlantısı (sadece komutu yazan görür).

use poise::serenity_prelude::{CreateActionRow, CreateButton};
use poise::CreateReply;

use crate::data::Context;
use crate::error::Error;

/// Web paneline giriş bağlantısı verir (sadece sen görürsün, 10 dk geçerli, tek kullanımlık).
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", ephemeral)]
pub async fn panel(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();
    let Some(base) = data.public_url.get() else {
        ctx.say("⏳ Panel adresi henüz hazır değil (tünel bağlanıyor). Birkaç saniye sonra tekrar dene.").await?;
        return Ok(());
    };
    let token = crate::panel::create_login(&data.redis, ctx.author().id).await?;
    let link = format!("{base}/panel/giris/{token}");

    let reply = if base.starts_with("https://") {
        CreateReply::default()
            .content("🖥️ **AxBot paneli** — bağlantı sadece sana özel, **tek kullanımlık** ve **10 dakika** geçerli. Kimseyle paylaşma.")
            .components(vec![CreateActionRow::Buttons(vec![CreateButton::new_link(link).label("Paneli aç").emoji('🖥')])])
    } else {
        // Yerel adres (bot bu bilgisayarda): Discord http bağlantılı butonları her zaman kabul etmez.
        CreateReply::default().content(format!(
            "🖥️ **AxBot paneli** (sadece botun çalıştığı bilgisayarda açılır):\n{link}\n\
             Bağlantı tek kullanımlık ve 10 dakika geçerli."
        ))
    };
    ctx.send(reply.ephemeral(true)).await?;
    Ok(())
}
