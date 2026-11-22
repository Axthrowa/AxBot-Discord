//! Discord koyu temalı tek parça bağımsız HTML bilet transkript motoru.

use poise::serenity_prelude as serenity;

use super::transcript::Header;

/// HTML özel karakterlerini kaçırır (XSS koruması).
pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// UTC zaman damgasını okunur metne dönüştürür.
fn format_time(ts: serenity::Timestamp) -> String {
    let iso = ts.to_string();
    iso.replacen('T', " ", 1).trim_end_matches('Z').chars().take(19).collect()
}

fn format_unix(unix: i64) -> String {
    serenity::Timestamp::from_unix_timestamp(unix)
        .map(format_time)
        .unwrap_or_else(|_| unix.to_string())
}

/// Bilet konuşma kaydını Discord koyu temalı bağımsız HTML belgesi olarak üretir.
pub fn render_html(
    messages: &[serenity::Message],
    header: &Header<'_>,
    topic: Option<&str>,
    rating: Option<i16>,
) -> String {
    let mut body = String::with_capacity(messages.len() * 300 + 4096);

    let guild_esc = escape_html(header.guild_name);
    let topic_esc = topic.map(escape_html).unwrap_or_else(|| "Genel".to_owned());
    let closer_tag_esc = escape_html(&header.closed_by.tag());
    let opener_tag_esc = header
        .opener
        .map(|u| escape_html(&u.tag()))
        .unwrap_or_else(|| "Bilinmeyen".to_owned());
    let opened_time = format_unix(header.opened_unix);
    let closed_time = format_time(serenity::Timestamp::now());

    let rating_html = match rating {
        Some(r) if (1..=5).contains(&r) => {
            format!(
                "<span class=\"pill rating\">{} / 5 ⭐</span>",
                r
            )
        }
        _ => String::new(),
    };

    let reason_html = match header.close_reason {
        Some(r) => format!(
            "<div class=\"meta-item\"><span class=\"meta-label\">Kapanış Gerekçesi:</span> <span class=\"meta-val reason\">{}</span></div>",
            escape_html(r)
        ),
        None => String::new(),
    };

    body.push_str(r#"<!DOCTYPE html>
<html lang="tr">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
"#);
    body.push_str(&format!(
        "<title>Destek Bileti #{:04} — {}</title>\n",
        header.number, guild_esc
    ));
    body.push_str(r#"<style>
:root {
  --bg-primary: #313338;
  --bg-secondary: #2b2d31;
  --bg-tertiary: #1e1f22;
  --text-primary: #f2f3f5;
  --text-secondary: #dbdee1;
  --text-muted: #949ba4;
  --accent: #5865f2;
  --accent-hover: #4752c4;
  --border: #3f4147;
  --badge-bot: #5865f2;
  --status-closed: #ed4245;
  --success: #57f287;
  --rating: #f1c40f;
}
* { box-sizing: border-box; margin: 0; padding: 0; }
body {
  background-color: var(--bg-primary);
  color: var(--text-secondary);
  font-family: "gg sans", "Noto Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  font-size: 14px;
  line-height: 1.4;
  padding: 24px;
}
.container {
  max-width: 960px;
  margin: 0 auto;
}
.header-card {
  background-color: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 20px;
  margin-bottom: 24px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
}
.header-top {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 12px;
  margin-bottom: 16px;
  border-bottom: 1px solid var(--border);
  padding-bottom: 12px;
}
.ticket-title {
  font-size: 20px;
  font-weight: 700;
  color: var(--text-primary);
  display: flex;
  align-items: center;
  gap: 10px;
}
.pills {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  align-items: center;
}
.pill {
  font-size: 12px;
  font-weight: 600;
  padding: 4px 10px;
  border-radius: 12px;
  background-color: var(--bg-tertiary);
  color: var(--text-secondary);
  text-transform: uppercase;
}
.pill.status { background-color: rgba(237, 66, 69, 0.2); color: var(--status-closed); }
.pill.topic { background-color: rgba(88, 101, 242, 0.2); color: var(--accent); }
.pill.rating { background-color: rgba(241, 196, 15, 0.2); color: var(--rating); }
.meta-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 12px;
}
.meta-item { font-size: 13px; }
.meta-label { color: var(--text-muted); font-weight: 500; }
.meta-val { color: var(--text-primary); font-weight: 600; }
.meta-val.reason { color: #f9a8d4; }
.messages-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.message-row {
  display: flex;
  gap: 16px;
  padding: 8px 12px;
  border-radius: 4px;
  transition: background-color 0.1s ease;
}
.message-row:hover {
  background-color: var(--bg-secondary);
}
.avatar {
  width: 40px;
  height: 40px;
  border-radius: 50%;
  background-color: var(--accent);
  color: #fff;
  display: flex;
  align-items: center;
  justify-content: center;
  font-weight: 700;
  font-size: 16px;
  flex-shrink: 0;
  overflow: hidden;
}
.avatar img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.message-body {
  flex: 1;
  min-width: 0;
}
.message-header {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 4px;
}
.author-name {
  font-weight: 600;
  color: var(--text-primary);
  font-size: 15px;
}
.bot-badge {
  background-color: var(--badge-bot);
  color: #fff;
  font-size: 10px;
  font-weight: 700;
  padding: 1px 4px;
  border-radius: 3px;
  text-transform: uppercase;
}
.timestamp {
  color: var(--text-muted);
  font-size: 11px;
}
.message-text {
  color: var(--text-secondary);
  word-break: break-word;
  white-space: pre-wrap;
}
.embed {
  margin-top: 8px;
  background-color: var(--bg-tertiary);
  border-left: 4px solid var(--accent);
  border-radius: 4px;
  padding: 10px 14px;
  max-width: 520px;
}
.embed-title {
  color: var(--text-primary);
  font-weight: 700;
  margin-bottom: 6px;
}
.embed-desc {
  color: var(--text-secondary);
  font-size: 13px;
  margin-bottom: 6px;
}
.embed-field {
  margin-top: 6px;
  font-size: 13px;
}
.embed-field-name {
  font-weight: 600;
  color: var(--text-primary);
}
.embed-field-val {
  color: var(--text-secondary);
}
.attachments {
  margin-top: 8px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.attachment-pill {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px;
  background-color: var(--bg-tertiary);
  border: 1px solid var(--border);
  border-radius: 6px;
  color: var(--accent);
  text-decoration: none;
  font-size: 13px;
  max-width: max-content;
}
.attachment-pill:hover {
  background-color: rgba(88, 101, 242, 0.1);
}
.footer {
  margin-top: 32px;
  padding-top: 16px;
  border-top: 1px solid var(--border);
  text-align: center;
  color: var(--text-muted);
  font-size: 12px;
}
</style>
</head>
<body>
<div class="container">
  <div class="header-card">
    <div class="header-top">
      <div class="ticket-title">
"#);

    body.push_str(&format!(
        "<span>🎫 Bilet #{:04}</span>\n<span style=\"color:var(--text-muted);font-weight:400\">{}</span>\n</div>\n",
        header.number, guild_esc
    ));

    body.push_str("<div class=\"pills\">\n");
    body.push_str("<span class=\"pill status\">KAPALI</span>\n");
    body.push_str(&format!("<span class=\"pill topic\">{}</span>\n", topic_esc));
    body.push_str(&rating_html);
    body.push_str("</div>\n</div>\n");

    body.push_str("<div class=\"meta-grid\">\n");
    body.push_str(&format!(
        "<div class=\"meta-item\"><span class=\"meta-label\">Açan:</span> <span class=\"meta-val\">{}</span></div>\n",
        opener_tag_esc
    ));
    body.push_str(&format!(
        "<div class=\"meta-item\"><span class=\"meta-label\">Kapatan:</span> <span class=\"meta-val\">{}</span></div>\n",
        closer_tag_esc
    ));
    body.push_str(&format!(
        "<div class=\"meta-item\"><span class=\"meta-label\">Açılış Tarihi:</span> <span class=\"meta-val\">{}</span></div>\n",
        opened_time
    ));
    body.push_str(&format!(
        "<div class=\"meta-item\"><span class=\"meta-label\">Kapanış Tarihi:</span> <span class=\"meta-val\">{}</span></div>\n",
        closed_time
    ));
    body.push_str(&format!(
        "<div class=\"meta-item\"><span class=\"meta-label\">Toplam Mesaj:</span> <span class=\"meta-val\">{}</span></div>\n",
        messages.len()
    ));
    body.push_str("</div>\n");
    body.push_str(&reason_html);
    body.push_str("</div>\n\n<div class=\"messages-list\">\n");

    for msg in messages {
        let author_tag_esc = escape_html(&msg.author.tag());
        let author_initial = msg
            .author
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_owned());
        let time_str = format_time(msg.timestamp);
        let content_esc = escape_html(&msg.content);

        let avatar_html = match msg.author.avatar_url() {
            Some(url) => format!("<div class=\"avatar\"><img src=\"{}\" alt=\"\"></div>", escape_html(&url)),
            None => format!("<div class=\"avatar\">{}</div>", escape_html(&author_initial)),
        };

        let bot_badge = if msg.author.bot {
            "<span class=\"bot-badge\">BOT</span>"
        } else {
            ""
        };

        body.push_str("<div class=\"message-row\">\n");
        body.push_str(&avatar_html);
        body.push_str("<div class=\"message-body\">\n");
        body.push_str("<div class=\"message-header\">\n");
        body.push_str(&format!(
            "<span class=\"author-name\">{}</span>\n{}\n<span class=\"timestamp\">{}</span>\n",
            author_tag_esc, bot_badge, time_str
        ));
        body.push_str("</div>\n");

        if !msg.content.is_empty() {
            body.push_str(&format!("<div class=\"message-text\">{}</div>\n", content_esc));
        }

        for embed in &msg.embeds {
            body.push_str("<div class=\"embed\">\n");
            if let Some(title) = &embed.title {
                body.push_str(&format!("<div class=\"embed-title\">{}</div>\n", escape_html(title)));
            }
            if let Some(desc) = &embed.description {
                body.push_str(&format!("<div class=\"embed-desc\">{}</div>\n", escape_html(desc)));
            }
            for field in &embed.fields {
                body.push_str(&format!(
                    "<div class=\"embed-field\"><div class=\"embed-field-name\">{}</div><div class=\"embed-field-val\">{}</div></div>\n",
                    escape_html(&field.name),
                    escape_html(&field.value)
                ));
            }
            body.push_str("</div>\n");
        }

        if !msg.attachments.is_empty() {
            body.push_str("<div class=\"attachments\">\n");
            for att in &msg.attachments {
                let filename_esc = escape_html(&att.filename);
                let url_esc = escape_html(&att.url);
                body.push_str(&format!(
                    "<a class=\"attachment-pill\" href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">📎 {}</a>\n",
                    url_esc, filename_esc
                ));
            }
            body.push_str("</div>\n");
        }

        body.push_str("</div>\n</div>\n");
    }

    body.push_str("</div>\n");
    body.push_str("<div class=\"footer\">AxBot Destek Sistemi — Otomatik oluşturulan konuşma kaydı</div>\n");
    body.push_str("</div>\n</body>\n</html>");

    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_html_sanitizes_dangerous_inputs() {
        let dangerous = "<script>alert('xss')</script> & \"hello\"";
        let escaped = escape_html(dangerous);
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('>'));
        assert!(!escaped.contains('"'));
        assert_eq!(
            escaped,
            "&lt;script&gt;alert(&#39;xss&#39;)&lt;/script&gt; &amp; &quot;hello&quot;"
        );
    }
}
