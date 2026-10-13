//! Hata bildirimi: `ERROR` düzeyindeki loglar ve panikler bot sahibine haber verilir.
//!
//! `.env`'de `ALERT_WEBHOOK_URL` (bir Discord webhook adresi) varsa oraya, yoksa bot sahibine özel
//! mesaj olarak gider. Hata yağmurunda sahibi boğmamak için en fazla 5 dakikada bir bildirim atılır;
//! aradaki hatalar sayılıp bir sonraki bildirimde özetlenir.
//!
//! Bot (ya da sunucu) tamamen kapanırsa buradan haber gelemez; onun için dışarıdan `/saglik`
//! adresini yoklayan bir izleyici gerekir.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use poise::serenity_prelude::{CreateMessage, Http, UserId};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tracing::debug;

const MIN_GAP: Duration = Duration::from_secs(5 * 60);
const MAX_TEXT: usize = 1500;

static SENDER: OnceLock<UnboundedSender<String>> = OnceLock::new();

/// Log katmanından çağrılır (eşzamanlı, bloklamaz). Bildirim görevi başlamadıysa hiçbir şey yapmaz.
pub fn notify(target: &str, text: &str) {
    // Bildirimin kendi hataları yeniden bildirilmez (sonsuz döngü olmasın).
    if target.ends_with("::alerts") {
        return;
    }
    if let Some(sender) = SENDER.get() {
        let _ = sender.send(format!("{target}: {text}"));
    }
}

/// Paniklerde de `ERROR` logu düşsün (dolayısıyla bildirim gitsin); ardından varsayılan davranış sürer.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("isimsiz").to_owned();
        tracing::error!(panic = %info, thread = %thread, trace = %short_backtrace(), "Bot bir panikle karşılaştı");
        default(info);
    }));
}

/// Paniğin nereden geldiğini gösteren kısa iz kaydı: standart kütüphane ve panik makinesi çerçeveleri
/// atlanır, ilk 12 çerçeve ve dosya/satırları tek satırda.
fn short_backtrace() -> String {
    let full = std::backtrace::Backtrace::force_capture().to_string();
    let lines: Vec<&str> = full.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let skip = ["std::", "core::", "alloc::", "rust_begin_unwind", "__rust", "discord_bot::alerts::", "<alloc::boxed::Box"];
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() && out.len() < 12 {
        let frame = lines[i];
        let location = lines.get(i + 1).filter(|l| l.starts_with("at "));
        i += if location.is_some() { 2 } else { 1 };
        if skip.iter().any(|s| frame.contains(s)) {
            continue;
        }
        let name = frame.split_once(": ").map_or(frame, |(_, n)| n);
        match location {
            Some(at) => out.push(format!("{name} ({})", at.trim_start_matches("at "))),
            None => out.push(name.to_owned()),
        }
    }
    if out.is_empty() { "(iz kaydı yok)".to_owned() } else { out.join(" ← ") }
}

/// Geçerli bir Discord webhook adresi mi? (başka adrese hata metni gönderilmez)
pub fn valid_webhook(url: &str) -> bool {
    ["https://discord.com/api/webhooks/", "https://discordapp.com/api/webhooks/", "https://canary.discord.com/api/webhooks/"].iter().any(|prefix| url.starts_with(prefix) && url.len() > prefix.len() + 10)
}

/// Bildirim metni: ilk hata + arada bastırılanların sayısı.
pub fn compose(first: &str, suppressed: usize) -> String {
    let body: String = first.chars().take(MAX_TEXT).collect();
    let extra = if suppressed > 0 { format!("\n(+{suppressed} hata daha; ayrıntı panelde → Loglar)") } else { String::new() };
    format!("🚨 **AxBot hata verdi**\n```\n{}\n```{extra}", body.replace("```", "'''"))
}

async fn deliver(http: &Http, client: &reqwest::Client, owners: &[UserId], webhook: Option<&str>, text: &str) {
    if let Some(url) = webhook {
        let sent = client.post(url).json(&serde_json::json!({ "content": text, "allowed_mentions": { "parse": [] } })).timeout(Duration::from_secs(10)).send().await;
        match sent {
            Ok(response) if response.status().is_success() => return,
            Ok(response) => debug!(status = %response.status(), "Hata bildirimi webhook'u reddetti"),
            Err(e) => debug!(error = %e, "Hata bildirimi webhook'una ulaşılamadı"),
        }
    }
    // Webhook yoksa ya da başarısızsa: bot sahiplerine özel mesaj.
    for owner in owners {
        let Ok(dm) = owner.create_dm_channel(http).await else { continue };
        if let Err(e) = dm.id.send_message(http, CreateMessage::new().content(text)).await {
            debug!(owner = %owner, error = %e, "Hata bildirimi özelden gönderilemedi");
        }
    }
}

/// Bildirim görevini başlatır (bir kez).
pub fn spawn(http: Arc<Http>, client: reqwest::Client, owners: Vec<UserId>) {
    let (sender, mut receiver) = unbounded_channel::<String>();
    if SENDER.set(sender).is_err() {
        return;
    }
    let webhook = crate::integrations::get().alert_webhook_url.clone();
    tokio::spawn(async move {
        let mut last_sent: Option<Instant> = None;
        while let Some(first) = receiver.recv().await {
            // Son bildirimden beri yeterince zaman geçmediyse beklenir; bu sırada gelenler sayılır.
            if let Some(wait) = last_sent.and_then(|at| MIN_GAP.checked_sub(at.elapsed())) {
                tokio::time::sleep(wait).await;
            }
            let mut suppressed = 0;
            while receiver.try_recv().is_ok() {
                suppressed += 1;
            }
            deliver(&http, &client, &owners, webhook.as_deref(), &compose(&first, suppressed)).await;
            last_sent = Some(Instant::now());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_discord_webhooks_are_accepted() {
        assert!(valid_webhook("https://discord.com/api/webhooks/123456789012345678/abcDEF_token-123"));
        assert!(!valid_webhook("https://discord.com/api/webhooks/"));
        assert!(!valid_webhook("https://example.com/api/webhooks/1/abcdefghijk"));
        assert!(!valid_webhook("http://discord.com/api/webhooks/123456789012345678/abc"));
        assert!(!valid_webhook("https://discord.com.kotu.site/api/webhooks/1/abcdefghijk"));
    }

    #[test]
    fn composes_bounded_messages() {
        let text = compose("hedef: bir ``` içeren hata", 0);
        assert!(text.starts_with("🚨") && !text.contains("hata daha"));
        // Kod bloğu erken kapanmasın.
        assert_eq!(text.matches("```").count(), 2);
        assert!(compose("x", 3).contains("+3 hata daha"));
        assert!(compose(&"a".repeat(5000), 0).chars().count() < 1600);
    }
}
