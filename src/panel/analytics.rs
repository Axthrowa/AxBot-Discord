//! Panel: Statbot analitikleri, aktivite trendleri, 7x24 ısı haritası ve sunucu istatistikleri sayfası.

use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use dashmap::DashMap;
use poise::serenity_prelude::GuildId;
use serde::Deserialize;
use tracing::warn;

use super::auth;
use super::can_manage;
use super::pages::{forbidden, layout, GuildNav, Nav, Page};
use crate::db::snowflake;
use crate::web::{escape, WebState};

/// Sunucuya ait bot kayıt sayıları. Sorgular sabittir; yalnızca sunucu kimliği bağlanır.
pub const COUNTS: &[(&str, &str)] = &[
    ("Moderasyon vakası", "SELECT COUNT(*) FROM mod_cases WHERE guild_id = $1"),
    ("Açılan bilet", "SELECT COUNT(*) FROM tickets WHERE guild_id = $1"),
    ("XP kazanan üye", "SELECT COUNT(*) FROM member_xp WHERE guild_id = $1"),
    ("Kazanılan başarım", "SELECT COUNT(*) FROM member_achievements WHERE guild_id = $1"),
    ("Çekiliş", "SELECT COUNT(*) FROM giveaways WHERE guild_id = $1"),
    ("Öneri", "SELECT COUNT(*) FROM suggestions WHERE guild_id = $1"),
    ("Starboard gönderisi", "SELECT COUNT(*) FROM starboard_posts WHERE guild_id = $1"),
    ("Davetle katılım", "SELECT COUNT(*) FROM invite_joins WHERE guild_id = $1"),
    ("Ekonomi hesabı", "SELECT COUNT(*) FROM economy_accounts WHERE guild_id = $1"),
    ("Kayıtlı doğum günü", "SELECT COUNT(*) FROM birthdays WHERE guild_id = $1"),
    ("Etiket", "SELECT COUNT(*) FROM tags WHERE guild_id = $1"),
    ("Yakalanan hayvan", "SELECT COUNT(*) FROM animal_catches WHERE guild_id = $1"),
];

#[derive(Deserialize, Default)]
pub struct AnalyticsQuery {
    pub days: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ChannelStat {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone)]
pub struct MemberStat {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone)]
pub struct DailyPoint {
    pub date_str: String,
    pub full_date: String,
    pub messages: u64,
    pub voice_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct AnalyticsData {
    pub total_messages: i64,
    pub total_voice_seconds: i64,
    pub members_joined: i64,
    pub members_left: i64,
    pub retention_stayed: u64,
    pub retention_joined: u64,
    pub daily_points: Vec<DailyPoint>,
    pub max_daily_messages: u64,
    pub max_daily_voice_seconds: u64,
    pub top_text_channels: Vec<ChannelStat>,
    pub top_voice_channels: Vec<ChannelStat>,
    pub top_message_members: Vec<MemberStat>,
    pub top_voice_members: Vec<MemberStat>,
    pub heatmap_grid: [[u64; 24]; 7],
    pub bot_records: Vec<(String, String)>,
}

/// Analitik sorgu sonuçları önbelleği (sunucu + gün sayısı bazlı 60 saniyelik TTL).
pub struct AnalyticsCache {
    entries: DashMap<(GuildId, u32), (Instant, Arc<AnalyticsData>)>,
    ttl: Duration,
}

impl AnalyticsCache {
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: DashMap::new(),
            ttl,
        }
    }

    #[must_use]
    pub fn get(&self, guild_id: GuildId, days: u32, now: Instant) -> Option<Arc<AnalyticsData>> {
        let entry = self.entries.get(&(guild_id, days))?;
        if now.duration_since(entry.0) < self.ttl {
            Some(Arc::clone(&entry.1))
        } else {
            None
        }
    }

    pub fn insert(&self, guild_id: GuildId, days: u32, data: Arc<AnalyticsData>, now: Instant) {
        self.entries.insert((guild_id, days), (now, data));
    }

    #[cfg(test)]
    pub fn clear(&self) {
        self.entries.clear();
    }
}

static CACHE: OnceLock<AnalyticsCache> = OnceLock::new();

pub fn cache() -> &'static AnalyticsCache {
    CACHE.get_or_init(|| AnalyticsCache::new(Duration::from_secs(60)))
}

/// Çubuk yüksekliği yüzdesi (0 bölme koruması, 0-100 aralığı, pozitif değerler için en az %4 görünürlük).
#[must_use]
pub fn scale_bar(value: u64, max: u64) -> u32 {
    if max == 0 || value == 0 {
        return 0;
    }
    let pct = (value as f64 / max as f64 * 100.0).round() as u32;
    pct.clamp(4, 100)
}

/// Saniye cinsinden ses süresini okunabilir metne dönüştürür.
#[must_use]
pub fn format_voice_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let mins = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}s {mins}dk")
    } else {
        format!("{mins}dk")
    }
}

fn stat_card(label: &str, value: &str) -> String {
    format!(
        "<div class=\"pcard\"><div class=\"note\">{}</div><div class=\"ptitle\" style=\"font-size:22px\">{}</div></div>",
        escape(label),
        escape(value)
    )
}

async fn load_analytics(state: &WebState, guild_id: GuildId, days: u32) -> anyhow::Result<AnalyticsData> {
    let db_id = snowflake(guild_id.get())?;
    let days_i32 = days as i32;

    // 1) Özet sayaçlar
    let (total_messages, total_voice_seconds, members_joined, members_left): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(messages_count), 0)::BIGINT,
                COALESCE(SUM(voice_seconds), 0)::BIGINT,
                COALESCE(SUM(members_joined), 0)::BIGINT,
                COALESCE(SUM(members_left), 0)::BIGINT
         FROM server_daily_stats
         WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)",
    )
    .bind(db_id)
    .bind(days_i32)
    .fetch_one(&state.db)
    .await?;

    // 2) Günlük döküm
    let daily_rows: Vec<(chrono::NaiveDate, i64, i64)> = sqlx::query_as(
        "SELECT stat_date,
                COALESCE(messages_count, 0)::BIGINT,
                COALESCE(voice_seconds, 0)::BIGINT
         FROM server_daily_stats
         WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)
         ORDER BY stat_date ASC",
    )
    .bind(db_id)
    .bind(days_i32)
    .fetch_all(&state.db)
    .await?;

    let today = chrono::Utc::now().date_naive();
    let mut daily_points = Vec::with_capacity(days as usize);
    let mut max_daily_messages = 0_u64;
    let mut max_daily_voice_seconds = 0_u64;

    for i in (0..days).rev() {
        let date = today - chrono::Duration::days(i64::from(i));
        let (msg, voice) = daily_rows
            .iter()
            .find(|(d, _, _)| *d == date)
            .map(|(_, m, v)| ((*m).max(0) as u64, (*v).max(0) as u64))
            .unwrap_or((0, 0));

        if msg > max_daily_messages {
            max_daily_messages = msg;
        }
        if voice > max_daily_voice_seconds {
            max_daily_voice_seconds = voice;
        }

        daily_points.push(DailyPoint {
            date_str: date.format("%d.%m").to_string(),
            full_date: date.format("%d.%m.%Y").to_string(),
            messages: msg,
            voice_seconds: voice,
        });
    }

    // 3) Kalıcılık (retention) ve 7x24 ısı haritası
    let (retention_stayed, retention_joined) = crate::community::retention(&state.redis, guild_id).await;
    let heatmap_grid = crate::community::heatmap(&state.redis, guild_id).await;

    // 4) Kanal ve Üye adları için önbellek
    let (channel_names, member_names) = state
        .cache
        .guild(guild_id)
        .map(|g| {
            let ch: std::collections::HashMap<u64, String> = g.channels.iter().map(|(&id, c)| (id.get(), c.name.clone())).collect();
            let me: std::collections::HashMap<u64, String> = g.members.iter().map(|(&id, m)| (id.get(), m.display_name().to_owned())).collect();
            (ch, me)
        })
        .unwrap_or_default();

    // 5) En aktif kanallar
    let text_channels_raw: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT channel_id, COALESCE(SUM(messages_count), 0)::BIGINT AS total
         FROM channel_daily_stats
         WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int) AND NOT is_voice
         GROUP BY channel_id
         ORDER BY total DESC
         LIMIT 10",
    )
    .bind(db_id)
    .bind(days_i32)
    .fetch_all(&state.db)
    .await?;

    let top_text_channels = text_channels_raw
        .into_iter()
        .map(|(cid, count)| {
            let ucid = cid as u64;
            let name = channel_names.get(&ucid).map(|n| format!("#{n}")).unwrap_or_else(|| format!("Kanal {ucid}"));
            ChannelStat { name, count }
        })
        .collect();

    let voice_channels_raw: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT channel_id, COALESCE(SUM(voice_seconds), 0)::BIGINT AS total
         FROM channel_daily_stats
         WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int) AND is_voice
         GROUP BY channel_id
         ORDER BY total DESC
         LIMIT 10",
    )
    .bind(db_id)
    .bind(days_i32)
    .fetch_all(&state.db)
    .await?;

    let top_voice_channels = voice_channels_raw
        .into_iter()
        .map(|(cid, count)| {
            let ucid = cid as u64;
            let name = channel_names.get(&ucid).map(|n| format!("🔊 {n}")).unwrap_or_else(|| format!("Ses Kanalı {ucid}"));
            ChannelStat { name, count }
        })
        .collect();

    // 6) En aktif üyeler
    let msg_members_raw: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT user_id, COALESCE(SUM(messages_count), 0)::BIGINT AS total
         FROM member_daily_stats
         WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)
         GROUP BY user_id
         ORDER BY total DESC
         LIMIT 10",
    )
    .bind(db_id)
    .bind(days_i32)
    .fetch_all(&state.db)
    .await?;

    let top_message_members = msg_members_raw
        .into_iter()
        .map(|(uid, count)| {
            let uuid = uid as u64;
            let name = member_names.get(&uuid).cloned().unwrap_or_else(|| format!("Kullanıcı {uuid}"));
            MemberStat { name, count }
        })
        .collect();

    let voice_members_raw: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT user_id, COALESCE(SUM(voice_seconds), 0)::BIGINT AS total
         FROM member_daily_stats
         WHERE guild_id = $1 AND stat_date >= CURRENT_DATE - make_interval(days => $2::int)
         GROUP BY user_id
         ORDER BY total DESC
         LIMIT 10",
    )
    .bind(db_id)
    .bind(days_i32)
    .fetch_all(&state.db)
    .await?;

    let top_voice_members = voice_members_raw
        .into_iter()
        .map(|(uid, count)| {
            let uuid = uid as u64;
            let name = member_names.get(&uuid).cloned().unwrap_or_else(|| format!("Kullanıcı {uuid}"));
            MemberStat { name, count }
        })
        .collect();

    // 7) Bot kayıtları
    let mut bot_records = Vec::with_capacity(COUNTS.len());
    for (label, sql) in COUNTS {
        let val_str = match sqlx::query_scalar::<_, i64>(*sql).bind(db_id).fetch_one(&state.db).await {
            Ok(count) => count.to_string(),
            Err(e) => {
                warn!(guild = %guild_id, error = %e, "Panel: istatistik okunamadı");
                "—".to_owned()
            }
        };
        bot_records.push(((*label).to_owned(), val_str));
    }

    Ok(AnalyticsData {
        total_messages,
        total_voice_seconds,
        members_joined,
        members_left,
        retention_stayed,
        retention_joined,
        daily_points,
        max_daily_messages,
        max_daily_voice_seconds,
        top_text_channels,
        top_voice_channels,
        top_message_members,
        top_voice_members,
        heatmap_grid,
        bot_records,
    })
}

fn render_html(data: &AnalyticsData, guild_id: GuildId, guild_name: &str, days: u32) -> String {
    let mut out = String::with_capacity(8192);

    // Üst Başlık ve Dönem Seçici
    let _ = write!(
        out,
        "<p><a href=\"/panel/sunucu/{guild_id}\">← {}</a></p>\
         <h1>📈 İstatistikler & Analitik</h1>\
         <p class=\"sub\">Sunucunun mesaj, ses aktivitesi, üye büyümesi ve kanal istatistikleri</p>\
         <div class=\"toolbar\" style=\"margin-bottom:20px\">\
           <span class=\"k\" style=\"color:var(--muted);font-weight:600;font-size:0.8rem;text-transform:uppercase;margin-right:4px\">Dönem:</span>\
           <a class=\"btn{}\" href=\"?days=7\">Son 7 gün</a>\
           <a class=\"btn{}\" href=\"?days=30\">Son 30 gün</a>\
         </div>",
        escape(guild_name),
        if days == 7 { "" } else { " ghost" },
        if days == 30 { "" } else { " ghost" },
    );

    // 1) Özet Kartları
    let net_growth = data.members_joined - data.members_left;
    let net_color = if net_growth > 0 {
        "var(--ok)"
    } else if net_growth < 0 {
        "var(--err)"
    } else {
        "var(--muted)"
    };
    let retention_str = (data.retention_stayed * 100)
        .checked_div(data.retention_joined)
        .map(|pct| format!("%{pct} ({}/{})", data.retention_stayed, data.retention_joined))
        .unwrap_or_else(|| "— (Yeterli veri yok)".to_owned());

    let _ = write!(
        out,
        "<div class=\"cards\" style=\"grid-template-columns: repeat(auto-fit, minmax(210px, 1fr)); margin-bottom: 24px;\">\
           <div class=\"card\"><div class=\"k\">Toplam Mesaj</div><div class=\"v\">{}</div></div>\
           <div class=\"card\"><div class=\"k\">Toplam Ses Süresi</div><div class=\"v\">{}</div></div>\
           <div class=\"card\"><div class=\"k\">Net Üye Büyümesi</div><div class=\"v\">+{} / −{} <small style=\"font-size:0.8rem;font-weight:600;color:{net_color}\">(net {net_growth})</small></div></div>\
           <div class=\"card\"><div class=\"k\">7–14 Gün Tutundurma</div><div class=\"v\">{retention_str}</div></div>\
         </div>",
        data.total_messages,
        format_voice_duration(data.total_voice_seconds.max(0) as u64),
        data.members_joined,
        data.members_left,
    );

    // 2) Günlük Mesaj ve Ses Süresi Trend Grafikleri
    let mut msg_bars = String::from("<div class=\"chart\">");
    for point in &data.daily_points {
        let pct = scale_bar(point.messages, data.max_daily_messages);
        let _ = write!(
            msg_bars,
            "<div class=\"col\"><b>{}</b><div class=\"bar\" style=\"height:{pct}%\" title=\"{}: {} mesaj\"></div><small>{}</small></div>",
            point.messages,
            point.full_date,
            point.messages,
            point.date_str,
        );
    }
    msg_bars.push_str("</div>");

    let mut voice_bars = String::from("<div class=\"chart\">");
    for point in &data.daily_points {
        let pct = scale_bar(point.voice_seconds, data.max_daily_voice_seconds);
        let dur_full = format_voice_duration(point.voice_seconds);
        let dur_short = if point.voice_seconds >= 3600 {
            format!("{}s", point.voice_seconds / 3600)
        } else {
            format!("{}d", point.voice_seconds / 60)
        };
        let _ = write!(
            voice_bars,
            "<div class=\"col\"><b>{dur_short}</b><div class=\"bar\" style=\"height:{pct}%;background:linear-gradient(180deg,#22c55e 0%,#15803d 100%)\" title=\"{}: {dur_full} ses\"></div><small>{}</small></div>",
            point.full_date,
            point.date_str,
        );
    }
    voice_bars.push_str("</div>");

    let _ = write!(
        out,
        "<div class=\"sect\">\
           <h3>💬 Günlük Mesaj Sayısı (Son {days} Gün)</h3>\
           {msg_bars}\
         </div>\
         <div class=\"sect\">\
           <h3>🎙️ Günlük Ses Süresi (Son {days} Gün)</h3>\
           {voice_bars}\
         </div>"
    );

    // 3) 7x24 Saatlik Aktivite Yoğunluğu (Heatmap)
    let hottest = data.heatmap_grid.iter().flatten().copied().max().unwrap_or(0).max(1);
    let mut heat = String::from("<div class=\"heat\"><div></div>");
    for hour in 0..24 {
        let _ = write!(heat, "<small>{hour:02}</small>");
    }
    for (name, row) in ["Pzt", "Sal", "Çar", "Per", "Cum", "Cmt", "Paz"].iter().zip(data.heatmap_grid.iter()) {
        let _ = write!(heat, "<small>{name}</small>");
        for (hour, count) in row.iter().enumerate() {
            let opacity = 0.08 + 0.92 * (*count as f64 / hottest as f64);
            let _ = write!(
                heat,
                "<i title=\"{name} {hour:02}:00 - {:02}:00: {count} mesaj\" style=\"opacity:{opacity:.2}\"></i>",
                (hour + 1) % 24
            );
        }
    }
    heat.push_str("</div>");

    let _ = write!(
        out,
        "<div class=\"sect\">\
           <h3>⏰ 7×24 Saatlik Aktivite Yoğunluğu</h3>\
           {heat}\
           <p class=\"note\">Koyu kareler haftanın ilgili gün ve saatinde en çok mesaj yazılan zamanları gösterir (Türkiye saati).</p>\
         </div>"
    );

    // 4) En Aktif 10 Kanal ve En Aktif 10 Üye Tabloları
    let mut text_ch_table = String::from("<table><tr><th>#</th><th>Kanal</th><th>Mesaj</th></tr>");
    if data.top_text_channels.is_empty() {
        text_ch_table.push_str("<tr><td colspan=\"3\" style=\"text-align:center;color:var(--muted)\">Kayıt yok</td></tr>");
    } else {
        for (i, ch) in data.top_text_channels.iter().enumerate() {
            let _ = write!(text_ch_table, "<tr><td>{}</td><td>{}</td><td>{}</td></tr>", i + 1, escape(&ch.name), ch.count);
        }
    }
    text_ch_table.push_str("</table>");

    let mut voice_ch_table = String::from("<table><tr><th>#</th><th>Kanal</th><th>Ses Süresi</th></tr>");
    if data.top_voice_channels.is_empty() {
        voice_ch_table.push_str("<tr><td colspan=\"3\" style=\"text-align:center;color:var(--muted)\">Kayıt yok</td></tr>");
    } else {
        for (i, ch) in data.top_voice_channels.iter().enumerate() {
            let _ = write!(
                voice_ch_table,
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                i + 1,
                escape(&ch.name),
                format_voice_duration(ch.count.max(0) as u64)
            );
        }
    }
    voice_ch_table.push_str("</table>");

    let mut msg_me_table = String::from("<table><tr><th>#</th><th>Üye</th><th>Mesaj</th></tr>");
    if data.top_message_members.is_empty() {
        msg_me_table.push_str("<tr><td colspan=\"3\" style=\"text-align:center;color:var(--muted)\">Kayıt yok</td></tr>");
    } else {
        for (i, m) in data.top_message_members.iter().enumerate() {
            let _ = write!(msg_me_table, "<tr><td>{}</td><td>{}</td><td>{}</td></tr>", i + 1, escape(&m.name), m.count);
        }
    }
    msg_me_table.push_str("</table>");

    let mut voice_me_table = String::from("<table><tr><th>#</th><th>Üye</th><th>Ses Süresi</th></tr>");
    if data.top_voice_members.is_empty() {
        voice_me_table.push_str("<tr><td colspan=\"3\" style=\"text-align:center;color:var(--muted)\">Kayıt yok</td></tr>");
    } else {
        for (i, m) in data.top_voice_members.iter().enumerate() {
            let _ = write!(
                voice_me_table,
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                i + 1,
                escape(&m.name),
                format_voice_duration(m.count.max(0) as u64)
            );
        }
    }
    voice_me_table.push_str("</table>");

    let _ = write!(
        out,
        "<div style=\"display:grid;grid-template-columns:repeat(auto-fit,minmax(320px,1fr));gap:16px;margin-bottom:16px\">\
           <div class=\"sect\" style=\"margin-bottom:0\">\
             <h3>💬 En Aktif Metin Kanalları (Top 10)</h3>\
             {text_ch_table}\
           </div>\
           <div class=\"sect\" style=\"margin-bottom:0\">\
             <h3>🔊 En Aktif Ses Kanalları (Top 10)</h3>\
             {voice_ch_table}\
           </div>\
         </div>\
         <div style=\"display:grid;grid-template-columns:repeat(auto-fit,minmax(320px,1fr));gap:16px;margin-bottom:24px\">\
           <div class=\"sect\" style=\"margin-bottom:0\">\
             <h3>✉️ En Aktif Üyeler — Mesaj (Top 10)</h3>\
             {msg_me_table}\
           </div>\
           <div class=\"sect\" style=\"margin-bottom:0\">\
             <h3>🎧 En Aktif Üyeler — Ses (Top 10)</h3>\
             {voice_me_table}\
           </div>\
         </div>"
    );

    // Ekstra Bölüm: Bot Kayıtları
    let mut records_html = String::new();
    for (label, val) in &data.bot_records {
        records_html.push_str(&stat_card(label, val));
    }
    let _ = write!(
        out,
        "<h2>Bot Kayıtları</h2>\
         <div class=\"pcards\" style=\"margin-bottom:24px\">{records_html}</div>\
         <p class=\"note\">XP ve seviye sıralaması için: <a href=\"/panel/sunucu/{guild_id}/siralama\">Seviye Sıralaması →</a></p>"
    );

    out
}

pub async fn page(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(guild): Path<u64>,
    Query(query): Query<AnalyticsQuery>,
) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await {
        return forbidden();
    }

    let guild_name = state
        .cache
        .guild(guild_id)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| format!("Sunucu {guild_id}"));

    let days = match query.days {
        Some(30) => 30,
        _ => 7,
    };

    let now = Instant::now();
    let data = if let Some(cached) = cache().get(guild_id, days, now) {
        cached
    } else {
        match load_analytics(&state, guild_id, days).await {
            Ok(d) => {
                let arc = Arc::new(d);
                cache().insert(guild_id, days, Arc::clone(&arc), now);
                arc
            }
            Err(e) => {
                warn!(guild = %guild_id, error = ?e, "Panel: analitik verileri yüklenemedi");
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Html("<p class=\"box err\">İstatistikler yüklenirken bir hata oluştu.</p>"),
                )
                    .into_response();
            }
        }
    };

    let body = render_html(&data, guild_id, &guild_name, days);
    let nav = GuildNav {
        id: guild_id,
        name: &guild_name,
        active: "istatistik",
    };
    Html(layout(&state, &session, Nav::Home, Some(nav), Page { title: "İstatistikler", body }).await).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_queries_are_scoped_to_one_guild() {
        for (label, sql) in COUNTS {
            assert!(
                sql.starts_with("SELECT COUNT(*) FROM ") && sql.ends_with(" WHERE guild_id = $1"),
                "{label}"
            );
        }
    }

    #[test]
    fn scale_bar_handles_zero_and_bounds() {
        assert_eq!(scale_bar(0, 0), 0);
        assert_eq!(scale_bar(0, 100), 0);
        assert_eq!(scale_bar(10, 0), 0);
        assert_eq!(scale_bar(50, 100), 50);
        assert_eq!(scale_bar(100, 100), 100);
        assert_eq!(scale_bar(200, 100), 100);
        // Çok küçük pozitif değerlerin en az %4 olması
        assert_eq!(scale_bar(1, 1000), 4);
    }

    #[test]
    fn format_voice_duration_handles_minutes_and_hours() {
        assert_eq!(format_voice_duration(0), "0dk");
        assert_eq!(format_voice_duration(59), "0dk");
        assert_eq!(format_voice_duration(60), "1dk");
        assert_eq!(format_voice_duration(3599), "59dk");
        assert_eq!(format_voice_duration(3600), "1s 0dk");
        assert_eq!(format_voice_duration(3665), "1s 1dk");
        assert_eq!(format_voice_duration(7200), "2s 0dk");
    }

    #[test]
    fn analytics_cache_ttl_and_eviction() {
        let cache = AnalyticsCache::new(Duration::from_secs(60));
        let guild_id = GuildId::new(12345);
        let dummy = Arc::new(AnalyticsData {
            total_messages: 100,
            total_voice_seconds: 3600,
            members_joined: 5,
            members_left: 2,
            retention_stayed: 4,
            retention_joined: 5,
            daily_points: Vec::new(),
            max_daily_messages: 10,
            max_daily_voice_seconds: 360,
            top_text_channels: Vec::new(),
            top_voice_channels: Vec::new(),
            top_message_members: Vec::new(),
            top_voice_members: Vec::new(),
            heatmap_grid: [[0; 24]; 7],
            bot_records: Vec::new(),
        });

        let t0 = Instant::now();
        cache.insert(guild_id, 7, Arc::clone(&dummy), t0);

        // Henüz 30 saniye geçmişken önbellek geçerli olmalı
        let hit = cache.get(guild_id, 7, t0 + Duration::from_secs(30));
        assert!(hit.is_some());

        // 61 saniye sonra TTL dolmuş sayılmalı (None dönmeli)
        let miss = cache.get(guild_id, 7, t0 + Duration::from_secs(61));
        assert!(miss.is_none());

        // Farklı gün sayısı için de None dönmeli
        assert!(cache.get(guild_id, 30, t0 + Duration::from_secs(10)).is_none());

        // Temizleme testi
        cache.clear();
        assert!(cache.get(guild_id, 7, t0 + Duration::from_secs(10)).is_none());
    }
}
