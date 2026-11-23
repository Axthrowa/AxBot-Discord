//! YZ komutlarının cevabı yazıldıkça gösterilir: ilk cümleler 1-2 sn'de görünür, son hâli aynı mesaja yazılır.

use poise::{CreateReply, ReplyHandle};

use crate::data::Context;
use crate::error::Error;

/// Tek seferlik YZ isteğini akışlı gösterir. `render` ara ve son metinden mesajı kurar (gizlilik, başlık,
/// uzunluk sınırı ondadır). Hata ya da boş cevapta `failure` yazılır. Çağıran önce `defer` etmiş olmalı.
pub(crate) async fn ai_reply(
    ctx: Context<'_>,
    guild_name: &str,
    persona: Option<&str>,
    request: &str,
    failure: &str,
    render: impl Fn(&str) -> CreateReply,
) -> Result<(), Error> {
    let (progress, mut rx) = tokio::sync::watch::channel(String::new());
    let work = crate::ai::write_once(ctx.data(), guild_name, persona, request, Some(&progress));
    tokio::pin!(work);
    let mut handle = None;
    let result = 'wait: loop {
        tokio::select! {
            biased;
            result = &mut work => break result,
            changed = rx.changed() => {
                if changed.is_err() {
                    break (&mut work).await;
                }
            }
        }
        let text = crate::ai::live_text(&rx.borrow_and_update());
        if text.is_empty() {
            continue;
        }
        show(ctx, &mut handle, render(&text)).await?;
        // Discord bir mesaja 5 sn'de 5 düzenlemeye izin verir; beklerken cevap biterse hemen yazılır.
        tokio::select! {
            biased;
            result = &mut work => break 'wait result,
            () = tokio::time::sleep(crate::ai::LIVE_EDIT_INTERVAL) => {}
        }
    };
    let reply = match result {
        None => CreateReply::default().content("Bu iş için yapay zekâ bu botta açık olmalı."),
        Some(Ok(answer)) if !answer.trim().is_empty() => render(answer.trim()),
        Some(Ok(_)) => CreateReply::default().content(failure),
        Some(Err(e)) => {
            tracing::warn!(command = %ctx.command().qualified_name, error = ?e, "YZ cevabı alınamadı");
            CreateReply::default().content(failure)
        }
    };
    show(ctx, &mut handle, reply).await
}

/// İlk gösterim cevabı gönderir (ertelenmiş cevabın yerine geçer), sonrakiler aynı mesajı düzenler.
async fn show<'a>(ctx: Context<'a>, handle: &mut Option<ReplyHandle<'a>>, reply: CreateReply) -> Result<(), Error> {
    match handle {
        Some(sent) => sent.edit(ctx, reply).await?,
        None => *handle = Some(ctx.send(reply).await?),
    }
    Ok(())
}
