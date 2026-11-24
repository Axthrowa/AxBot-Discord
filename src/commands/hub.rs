//! Hub mesajları (`/ekonomi panel`, `/oyun panel`; müzikte bu iş `/müzik şimdi` kartında, `/müzik` 25 alt
//! komut sınırında): düğmeli bir kutu; her düğme aynı
//! gruptaki bir alt komutu seçeneksiz çalıştırır. Mantık tekrar yazılmaz, alt komutun kendisi çalışır;
//! cevapları komutun ek mesajları olarak gelir. Yalnızca paneli açan kullanabilir.

use std::time::Duration;

use poise::serenity_prelude as serenity;

use crate::data::Context;
use crate::error::Error;
use crate::ui;

/// Discord etkileşim anahtarı 15 dakika geçerli; panel bundan önce kapanır.
const OPEN_FOR: Duration = Duration::from_secs(12 * 60);

/// Bir düğme: emoji, yazı ve çalıştırılacak alt komutun fonksiyon adı.
pub type Item = (&'static str, &'static str, &'static str);

/// Paneli gösterir ve düğmeleri dinler. `once`: ilk seçimden sonra panel kapanır (uzun süren oyunlar için).
pub async fn run(ctx: Context<'_>, intro: ui::Message, items: &[Item], once: bool) -> Result<(), Error> {
    let poise::Context::Application(app) = ctx else {
        return ctx.say("Bu paneli `/` komutu olarak kullan.").await.map(drop).map_err(Into::into);
    };
    let Some(group) = app.parent_commands.last() else {
        return Ok(());
    };
    let prefix = format!("hub:{}:", ctx.id());
    let rows: Vec<serenity::CreateActionRow> = items
        .chunks(5)
        .map(|chunk| {
            serenity::CreateActionRow::Buttons(
                chunk
                    .iter()
                    .map(|(emoji, label, name)| {
                        serenity::CreateButton::new(format!("{prefix}{name}"))
                            .label(*label)
                            .emoji(serenity::ReactionType::Unicode((*emoji).to_owned()))
                            .style(serenity::ButtonStyle::Secondary)
                    })
                    .collect(),
            )
        })
        .collect();
    let closed = intro.clone().text("-# Panel kapandı; yeniden açmak için komutu tekrar kullan.");
    ui::reply(ctx, &intro.separator().rows(rows).ephemeral(true)).await?;

    let http = &ctx.serenity_context().http;
    while let Some(press) = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .author_id(ctx.author().id)
        .filter({
            let prefix = prefix.clone();
            move |i| i.data.custom_id.starts_with(&prefix)
        })
        .timeout(OPEN_FOR)
        .await
    {
        let name = press.data.custom_id.trim_start_matches(&prefix);
        let Some(command) = group.subcommands.iter().find(|c| c.source_code_name == name) else {
            continue;
        };
        if once {
            ui::update(http, &press, &closed).await?;
        } else {
            press.defer(http).await?;
        }
        let sub = poise::ApplicationContext { command, ..app };
        // Yetki ve modül denetimi, komut doğrudan yazılmış gibi yapılır.
        if crate::perms::check(poise::Context::Application(sub)).await? {
            if let Some(action) = command.slash_action {
                if let Err(e) = action(sub).await {
                    (app.framework.options.on_error)(e).await;
                }
            }
        }
        if once {
            return Ok(());
        }
    }
    let _ = ui::edit_original(http, &app.interaction.token, &closed).await;
    Ok(())
}
