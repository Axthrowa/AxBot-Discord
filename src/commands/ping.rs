use std::time::{Duration, Instant};

use poise::serenity_prelude as serenity;
use poise::CreateReply;
use tracing::warn;

use crate::data::Context;
use crate::error::Error;
use crate::{cache, db};

/// Botun gateway, REST API, veritabanı ve önbellek gecikmelerini gösterir.
#[poise::command(slash_command, category = "Genel")]
pub async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    let gateway = ctx.ping().await;
    let data = ctx.data();

    // REST gecikmesi: ilk yanıtın Discord'a gidip onaylanma süresi.
    // PostgreSQL ve Redis aynı anda, birbirini beklemeden ölçülür.
    let rest_probe = async {
        let started = Instant::now();
        let reply = ctx.say("🏓 Ölçülüyor...").await;
        (reply, started.elapsed())
    };
    let ((reply, rest), db_latency, redis_latency) = tokio::join!(
        rest_probe,
        db::ping(&data.db),
        cache::ping(&data.redis),
    );
    let reply = reply?;

    let embed = serenity::CreateEmbed::new()
        .title("🏓 Pong!")
        .colour(latency_colour(rest))
        .field("Gateway (WebSocket)", format_gateway(gateway), true)
        .field("REST API", format_ms(rest), true)
        .field(
            "Çalışma süresi",
            format_uptime(data.started_at.elapsed()),
            true,
        )
        .field("PostgreSQL", format_backend("PostgreSQL", db_latency), true)
        .field("Redis", format_backend("Redis", redis_latency), true)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "Shard #{}",
            ctx.serenity_context().shard_id.0
        )))
        .timestamp(serenity::Timestamp::now());

    reply
        .edit(ctx, CreateReply::default().content("").embed(embed))
        .await?;

    Ok(())
}

fn latency_colour(latency: Duration) -> serenity::Colour {
    match latency.as_millis() {
        0..=150 => serenity::Colour::DARK_GREEN,
        151..=400 => serenity::Colour::GOLD,
        _ => serenity::Colour::RED,
    }
}

fn format_ms(d: Duration) -> String {
    format!("`{} ms`", d.as_millis())
}

/// Veritabanı/önbellek sorgusu milisaniyenin altında sürebildiği için ondalıklı gösterilir.
/// Hata durumunda komut düşmez; hata loglanır ve embed'de belirtilir.
fn format_backend(name: &str, result: anyhow::Result<Duration>) -> String {
    match result {
        Ok(d) => format!("`{:.2} ms`", d.as_secs_f64() * 1000.0),
        Err(e) => {
            warn!(backend = name, error = ?e, "Ping sırasında servis yanıt vermedi");
            "`❌ erişilemiyor`".to_owned()
        }
    }
}

/// İlk heartbeat'e kadar (~40 sn) shard gecikmesi bilinmez ve sıfır döner.
fn format_gateway(d: Duration) -> String {
    if d.is_zero() {
        "`ölçülüyor…`".to_owned()
    } else {
        format_ms(d)
    }
}

fn format_uptime(d: Duration) -> String {
    let secs = d.as_secs();
    let (days, hours, mins, secs) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60, secs % 60);

    if days > 0 {
        format!("`{days}g {hours}s {mins}d`")
    } else if hours > 0 {
        format!("`{hours}s {mins}d {secs}sn`")
    } else {
        format!("`{mins}d {secs}sn`")
    }
}
