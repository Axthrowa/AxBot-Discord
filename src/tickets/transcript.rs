//! Bilet kanalının düz metin konuşma kaydı.

use poise::serenity_prelude::{self as serenity, ChannelId, GetMessages, Message, User};

/// Çok uzun biletlerde sadece son mesajlar alınır (Discord sayfa başına 100 mesaj verir).
const MAX_MESSAGES: usize = 1000;

pub struct Header<'a> {
    pub guild_name: &'a str,
    pub number: i32,
    pub opener: Option<&'a User>,
    pub closed_by: &'a User,
    pub opened_unix: i64,
}

pub async fn build(ctx: &serenity::Context, channel_id: ChannelId, header: &Header<'_>) -> anyhow::Result<String> {
    let mut messages: Vec<Message> = Vec::new();
    let mut before = None;

    while messages.len() < MAX_MESSAGES {
        let mut request = GetMessages::new().limit(100);
        if let Some(id) = before {
            request = request.before(id);
        }
        let page = channel_id.messages(ctx, request).await?;
        let Some(oldest) = page.last() else { break };
        before = Some(oldest.id);
        let done = page.len() < 100;
        messages.extend(page);
        if done {
            break;
        }
    }
    // Discord en yeniden eskiye döndürür.
    messages.reverse();

    let mut out = String::new();
    out.push_str(&format!("Destek Bileti #{:04} — {}\n", header.number, header.guild_name));
    if let Some(opener) = header.opener {
        out.push_str(&format!("Açan     : {} ({})\n", opener.tag(), opener.id));
    }
    out.push_str(&format!("Kapatan  : {} ({})\n", header.closed_by.tag(), header.closed_by.id));
    out.push_str(&format!(
        "Açılış   : {}\nKapanış  : {}\nMesaj    : {}\n",
        format_unix(header.opened_unix),
        format_unix(serenity::Timestamp::now().unix_timestamp()),
        messages.len()
    ));
    out.push_str(&"=".repeat(60));
    out.push('\n');

    for msg in &messages {
        out.push_str(&format!(
            "[{}] {}: {}\n",
            format_unix(msg.timestamp.unix_timestamp()),
            msg.author.tag(),
            msg.content
        ));
        for embed in &msg.embeds {
            if let Some(title) = &embed.title {
                out.push_str(&format!("    [embed] {title}\n"));
            }
        }
        for attachment in &msg.attachments {
            out.push_str(&format!("    [ek] {} — {}\n", attachment.filename, attachment.url));
        }
    }

    Ok(out)
}

/// UTC "YYYY-MM-DD HH:MM:SS".
fn format_unix(unix: i64) -> String {
    serenity::Timestamp::from_unix_timestamp(unix)
        .map(|ts| {
            let iso = ts.to_string(); // 2026-10-08T06:21:35Z
            iso.replacen('T', " ", 1).trim_end_matches('Z').chars().take(19).collect()
        })
        .unwrap_or_else(|_| unix.to_string())
}
