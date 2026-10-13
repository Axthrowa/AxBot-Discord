//! Etkinlik planlama: tarihli bir etkinlik mesajı ve altında Katılıyorum / Belki / Katılmıyorum butonları.
//!
//! Etkinlikler `PluginSettings` deposunda durur. Başlamadan 15 dakika önce katılacaklar etiketlenerek
//! hatırlatılır; etkinlikten 6 saat sonra kayıt silinir (mesaj kalır).

use std::time::Duration;

use chrono::{NaiveDate, NaiveDateTime};
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow, CreateAllowedMentions, CreateButton, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateScheduledEvent, EditMessage, GuildId, MessageId,
    ScheduledEventId, ScheduledEventType, UserId,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{info, warn};

use crate::data::Data;
use crate::settings::{PluginConfig, PluginSettings};

pub const PREFIX: &str = "evt:";
pub const MAX_EVENTS: usize = 20;
/// Türkiye saati (UTC+3, yaz saati uygulaması yok).
const TURKEY_OFFSET_SECS: i64 = 3 * 3600;
const REMIND_BEFORE_SECS: i64 = 15 * 60;
const FORGET_AFTER_SECS: i64 = 6 * 3600;
const MAX_AHEAD_SECS: i64 = 366 * 86_400;
const TICK: Duration = Duration::from_secs(60);
/// Hatırlatmada en fazla bu kadar kişi etiketlenir.
const MAX_PINGS: usize = 50;
/// Discord etkinliğinin süresi (harici etkinliklerde bitiş zamanı zorunlu).
const DISCORD_EVENT_LENGTH_SECS: i64 = 2 * 3600;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Event {
    pub id: u32,
    pub channel: u64,
    pub message: u64,
    pub title: String,
    pub description: Option<String>,
    /// Başlangıç (Unix saniyesi).
    pub start: i64,
    pub host: u64,
    pub going: Vec<u64>,
    pub maybe: Vec<u64>,
    pub declined: Vec<u64>,
    pub reminded: bool,
    /// Discord'un kendi etkinlik listesinde (sunucu adının altındaki "Etkinlikler") açılan kopyası.
    pub discord_event: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventSettings {
    pub events: Vec<Event>,
}

impl PluginConfig for EventSettings {
    const KEY: &'static str = "events";
}

impl EventSettings {
    pub fn next_id(&self) -> u32 {
        self.events.iter().map(|e| e.id).max().unwrap_or(0) + 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Going,
    Maybe,
    Declined,
}

impl Answer {
    fn code(self) -> char {
        match self {
            Self::Going => 'g',
            Self::Maybe => 'm',
            Self::Declined => 'd',
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        match code {
            "g" => Some(Self::Going),
            "m" => Some(Self::Maybe),
            "d" => Some(Self::Declined),
            _ => None,
        }
    }
}

impl Event {
    /// Üyenin cevabını kaydeder (önceki cevabı silinir). Aynı cevaba yeniden basarsa cevabı kalkar; o zaman `false`.
    pub fn answer(&mut self, user: u64, answer: Answer) -> bool {
        let had_same = match answer {
            Answer::Going => self.going.contains(&user),
            Answer::Maybe => self.maybe.contains(&user),
            Answer::Declined => self.declined.contains(&user),
        };
        for list in [&mut self.going, &mut self.maybe, &mut self.declined] {
            list.retain(|u| *u != user);
        }
        if had_same {
            return false;
        }
        match answer {
            Answer::Going => self.going.push(user),
            Answer::Maybe => self.maybe.push(user),
            Answer::Declined => self.declined.push(user),
        }
        true
    }

    pub fn embed(&self) -> CreateEmbed {
        let names = |list: &[u64]| {
            if list.is_empty() {
                "—".to_owned()
            } else {
                let shown: Vec<String> = list.iter().take(20).map(|u| format!("<@{u}>")).collect();
                let more = list.len().saturating_sub(20);
                shown.join(" ") + &if more > 0 { format!(" +{more}") } else { String::new() }
            }
        };
        let mut text = format!("🗓️ <t:{0}:F> (<t:{0}:R>)", self.start);
        if let Some(description) = self.description.as_deref().filter(|d| !d.trim().is_empty()) {
            text.push_str("\n\n");
            text.push_str(description);
        }
        CreateEmbed::new()
            .title(format!("📅 {}", self.title))
            .description(text)
            .colour(serenity::Colour::from_rgb(88, 101, 242))
            .field(format!("✅ Katılıyor ({})", self.going.len()), names(&self.going), false)
            .field(format!("🤔 Belki ({})", self.maybe.len()), names(&self.maybe), true)
            .field(format!("❌ Katılmıyor ({})", self.declined.len()), names(&self.declined), true)
            .footer(CreateEmbedFooter::new(format!("Etkinlik #{} · Başlamadan 15 dk önce katılacaklara hatırlatılır", self.id)))
    }

    pub fn buttons(&self) -> Vec<CreateActionRow> {
        let button = |answer: Answer, label: &str, style: ButtonStyle| CreateButton::new(format!("{PREFIX}{}:{}", self.id, answer.code())).label(label).style(style);
        vec![CreateActionRow::Buttons(vec![
            button(Answer::Going, "Katılıyorum", ButtonStyle::Success),
            button(Answer::Maybe, "Belki", ButtonStyle::Secondary),
            button(Answer::Declined, "Katılmıyorum", ButtonStyle::Danger),
        ])]
    }
}

/// "25.12.2026 21:30" ya da "25.12 21:30" (Türkiye saati) → Unix saniyesi. `today`: yılsız yazımda kullanılacak tarih.
pub fn parse_time(raw: &str, today: NaiveDate) -> Option<i64> {
    let raw = raw.trim().replace(['/', '-'], ".");
    let local = NaiveDateTime::parse_from_str(&raw, "%d.%m.%Y %H:%M").ok().or_else(|| {
        // Yıl yazılmadıysa: bu yıl; tarih geçmişse gelecek yıl.
        use chrono::Datelike as _;
        let this_year = NaiveDateTime::parse_from_str(&format!("{raw} {}", today.year()), "%d.%m %H:%M %Y").ok()?;
        if this_year.date() >= today {
            Some(this_year)
        } else {
            NaiveDateTime::parse_from_str(&format!("{raw} {}", today.year() + 1), "%d.%m %H:%M %Y").ok()
        }
    })?;
    Some(local.and_utc().timestamp() - TURKEY_OFFSET_SECS)
}

/// Başlangıç zamanı kabul edilebilir mi? (gelecekte ve en fazla bir yıl sonra)
pub fn valid_start(start: i64, now: i64) -> bool {
    start > now + 60 && start - now <= MAX_AHEAD_SECS
}

pub fn turkey_today(now: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp(now + TURKEY_OFFSET_SECS, 0).map(|d| d.date_naive()).unwrap_or_default()
}

/// Etkinlik mesajını gönderir ve kaydeder. Hata metni kullanıcıya gösterilir.
#[allow(clippy::too_many_arguments)]
pub async fn create(
    http: &serenity::Http,
    db: &PgPool,
    settings: &PluginSettings,
    guild_id: GuildId,
    channel: ChannelId,
    host: UserId,
    title: String,
    description: Option<String>,
    start: i64,
    discord_event: bool,
) -> Result<(u32, Option<String>), &'static str> {
    let current = settings.get::<EventSettings>(db, guild_id).await.map_err(|_| "Etkinlikler okunamadı.")?;
    if current.events.len() >= MAX_EVENTS {
        return Err("Aynı anda en fazla 20 etkinlik olabilir; eskilerden iptal et.");
    }
    let mut event = Event { id: current.next_id(), channel: channel.get(), title, description, start, host: host.get(), ..Event::default() };
    // Açıklamadaki etiketler kimseyi bildirmez.
    let message = CreateMessage::new().embed(event.embed()).components(event.buttons()).allowed_mentions(CreateAllowedMentions::new());
    let sent = channel.send_message(http, message).await.map_err(|_| "Etkinlik mesajı gönderilemedi; botun o kanalda mesaj gönderme ve bağlantı yerleştirme izni olmalı.")?;
    event.message = sent.id.get();
    let id = event.id;
    // Discord'un kendi etkinlik listesine de eklenir (üyeler "İlgileniyorum" der, Discord kendisi hatırlatır).
    let mut note = None;
    if discord_event {
        match create_discord_event(http, guild_id, &event, &sent.link()).await {
            Ok(created) => event.discord_event = Some(created.get()),
            Err(e) => {
                warn!(guild = %guild_id, error = %e, "Discord etkinliği oluşturulamadı");
                note = Some("Discord etkinlik listesine eklenemedi; botun 'Etkinlikleri Yönet' izni olmalı.".to_owned());
            }
        }
    }
    // Gönderim sırasında liste değişmiş olabilir: güncel hâli alınır.
    let mut updated = (*settings.get::<EventSettings>(db, guild_id).await.map_err(|_| "Etkinlik kaydedilemedi.")?).clone();
    updated.events.push(event);
    settings.set(db, guild_id, updated).await.map_err(|_| "Etkinlik kaydedilemedi.")?;
    Ok((id, note))
}

async fn create_discord_event(http: &serenity::Http, guild_id: GuildId, event: &Event, link: &str) -> anyhow::Result<ScheduledEventId> {
    let start = serenity::Timestamp::from_unix_timestamp(event.start)?;
    let end = serenity::Timestamp::from_unix_timestamp(event.start + DISCORD_EVENT_LENGTH_SECS)?;
    let mut builder = CreateScheduledEvent::new(ScheduledEventType::External, event.title.chars().take(100).collect::<String>(), start)
        .end_time(end)
        .location(link.chars().take(100).collect::<String>())
        .audit_log_reason("/etkinlik oluştur");
    if let Some(description) = event.description.as_deref().filter(|d| !d.trim().is_empty()) {
        builder = builder.description(description.chars().take(1000).collect::<String>());
    }
    Ok(guild_id.create_scheduled_event(http, builder).await?.id)
}

/// Etkinliği iptal eder: kaydı siler, mesajı "iptal edildi" olarak günceller.
pub async fn cancel(http: &serenity::Http, db: &PgPool, settings: &PluginSettings, guild_id: GuildId, id: u32) -> anyhow::Result<bool> {
    let mut updated = (*settings.get::<EventSettings>(db, guild_id).await?).clone();
    let Some(position) = updated.events.iter().position(|e| e.id == id) else {
        return Ok(false);
    };
    let event = updated.events.remove(position);
    settings.set(db, guild_id, updated).await?;
    if let Some(discord) = event.discord_event.filter(|id| *id != 0) {
        if let Err(e) = guild_id.delete_scheduled_event(http, ScheduledEventId::new(discord)).await {
            warn!(guild = %guild_id, error = %e, "Discord etkinliği silinemedi (elle silinmiş olabilir)");
        }
    }
    let embed = CreateEmbed::new().title(format!("🚫 İptal edildi: {}", event.title)).colour(serenity::Colour::DARK_GREY);
    let edit = EditMessage::new().embed(embed).components(Vec::new());
    if let Err(e) = ChannelId::new(event.channel.max(1)).edit_message(http, MessageId::new(event.message.max(1)), edit).await {
        warn!(guild = %guild_id, error = %e, "Etkinlik mesajı güncellenemedi (silinmiş olabilir)");
    }
    Ok(true)
}

pub async fn handle_component(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let reply = |text: &'static str| CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true));
    let parsed = interaction.data.custom_id.strip_prefix(PREFIX).and_then(|rest| rest.split_once(':')).and_then(|(id, code)| Some((id.parse::<u32>().ok()?, Answer::from_code(code)?)));
    let (Some(guild_id), Some((id, answer))) = (interaction.guild_id, parsed) else {
        return Ok(());
    };
    let mut updated = (*data.settings.get::<EventSettings>(&data.db, guild_id).await?).clone();
    let Some(event) = updated.events.iter_mut().find(|e| e.id == id) else {
        interaction.create_response(&ctx.http, reply("Bu etkinlik sona ermiş ya da iptal edilmiş.")).await?;
        return Ok(());
    };
    event.answer(interaction.user.id.get(), answer);
    let (embed, buttons) = (event.embed(), event.buttons());
    data.settings.set(&data.db, guild_id, updated).await?;
    // Mesaj yerinde güncellenir; ayrı bir cevap gönderilmez.
    let update = CreateInteractionResponseMessage::new().embed(embed).components(buttons);
    interaction.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await?;
    Ok(())
}

/// Hatırlatmaları gönderir ve geçmiş etkinlikleri temizler.
pub fn spawn(ctx: serenity::Context, db: PgPool, settings: PluginSettings) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            crate::supervise::round("etkinlik hatırlatmaları", std::time::Duration::from_secs(300), async {
                let now = serenity::Timestamp::now().unix_timestamp();
                for guild_id in ctx.cache.guilds() {
                    let Ok(current) = settings.get::<EventSettings>(&db, guild_id).await else { continue };
                    let due = |e: &Event| !e.reminded && e.start > now && e.start - now <= REMIND_BEFORE_SECS;
                    let old = |e: &Event| now - e.start > FORGET_AFTER_SECS;
                    if !current.events.iter().any(|e| due(e) || old(e)) {
                        continue;
                    }
                    let mut updated = (*current).clone();
                    let reminders: Vec<Event> = updated.events.iter().filter(|e| due(e)).cloned().collect();
                    updated.events.retain(|e| !old(e));
                    for event in &mut updated.events {
                        if due(event) {
                            event.reminded = true;
                        }
                    }
                    // Önce "hatırlatıldı" yazılır: gönderim uzun sürse bile iki kez hatırlatılmaz.
                    if let Err(e) = settings.set(&db, guild_id, updated).await {
                        warn!(guild = %guild_id, error = ?e, "Etkinlik durumu kaydedilemedi");
                        continue;
                    }
                    for event in reminders {
                        let users: Vec<UserId> = event.going.iter().take(MAX_PINGS).filter(|u| **u != 0).map(|u| UserId::new(*u)).collect();
                        let mentions: Vec<String> = users.iter().map(|u| format!("<@{u}>")).collect();
                        let text = format!("⏰ **{}** <t:{}:R> başlıyor! {}", event.title, event.start, mentions.join(" "));
                        let message = CreateMessage::new().content(text.chars().take(1900).collect::<String>()).allowed_mentions(CreateAllowedMentions::new().users(users));
                        match ChannelId::new(event.channel.max(1)).send_message(&ctx.http, message).await {
                            Ok(_) => info!(guild = %guild_id, event = event.id, "Etkinlik hatırlatıldı"),
                            Err(e) => warn!(guild = %guild_id, event = event.id, error = %e, "Etkinlik hatırlatması gönderilemedi"),
                        }
                    }
                }
            })
            .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> anyhow::Result<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, d).ok_or_else(|| anyhow::anyhow!("geçersiz tarih"))
    }

    #[test]
    fn parses_turkish_times() -> anyhow::Result<()> {
        let today = day(2026, 10, 11)?;
        // 25.12.2026 21:30 Türkiye = 18:30 UTC.
        let expected = day(2026, 12, 25)?.and_hms_opt(18, 30, 0).map(|t| t.and_utc().timestamp());
        assert_eq!(parse_time("25.12.2026 21:30", today), expected);
        assert_eq!(parse_time(" 25/12/2026 21:30 ", today), expected);
        assert_eq!(parse_time("25.12 21:30", today), expected);
        // Yılsız ve tarihi geçmiş: gelecek yıl.
        let next_year = day(2027, 1, 5)?.and_hms_opt(17, 0, 0).map(|t| t.and_utc().timestamp());
        assert_eq!(parse_time("05.01 20:00", today), next_year);
        for bad in ["yarın akşam", "32.01.2027 10:00", "25.12.2026", "25.12.2026 25:00", ""] {
            assert_eq!(parse_time(bad, today), None, "{bad}");
        }
        Ok(())
    }

    #[test]
    fn start_must_be_in_the_near_future() {
        assert!(valid_start(1_000_000, 0));
        assert!(!valid_start(30, 0));
        assert!(!valid_start(-5, 0));
        assert!(!valid_start(MAX_AHEAD_SECS + 100, 0));
    }

    #[test]
    fn one_answer_per_member_and_toggle() {
        let mut event = Event::default();
        assert!(event.answer(1, Answer::Going));
        assert!(event.answer(2, Answer::Going));
        assert!(event.answer(1, Answer::Maybe));
        assert_eq!((event.going.clone(), event.maybe.clone()), (vec![2], vec![1]));
        // Aynı butona yeniden basmak cevabı kaldırır.
        assert!(!event.answer(1, Answer::Maybe));
        assert!(event.maybe.is_empty());
        assert!(event.answer(2, Answer::Declined));
        assert_eq!((event.going.len(), event.declined.clone()), (0, vec![2]));
        assert_eq!(EventSettings { events: vec![Event { id: 4, ..Event::default() }] }.next_id(), 5);
    }
}
