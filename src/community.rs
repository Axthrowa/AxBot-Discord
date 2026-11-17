//! Topluluk özellikleri: AFK, sabit (yapışkan) mesajlar, sayma oyunu, itiraflar, kalıcı roller,
//! ses XP'si ve günlük mesaj etkinliği.
//!
//! Ayarlar `PluginSettings` deposunda (bellekte); mesaj başına veritabanına gidilmez. Sayma oyununun
//! durumu ve etkinlik sayaçları Redis'te, AFK ve sabit mesaj durumu bellekte tutulur.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use dashmap::{DashMap, DashSet};
use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateAllowedMentions, CreateEmbed, CreateMessage, GuildId, Member, Mentionable, Message, MessageId, ReactionType, RoleId, UserId,
};
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::data::Data;
use crate::settings::PluginConfig;

pub const MAX_STICKY: usize = 10;
pub const MAX_STICKY_LEN: usize = 1500;
pub const MAX_VOICE_XP: u32 = 50;
pub const MAX_AFK_REASON: usize = 150;
/// Sabit mesaj en az bu kadar yeni mesajdan ve süreden sonra yeniden gönderilir.
const STICKY_EVERY: u32 = 4;
const STICKY_COOLDOWN: Duration = Duration::from_secs(12);
/// Tek seferde en fazla bu kadar dakika ses XP'si verilir (kanalda unutulan hesaplar için).
const VOICE_XP_MAX_MINUTES: u64 = 600;
const ROLE_MEMORY_SECS: u64 = 30 * 24 * 3600;
const ACTIVITY_DAYS: u64 = 45;
pub const MAX_BOOST_MESSAGE: usize = 1000;
const DEFAULT_BOOST_MESSAGE: &str = "💖 {kullanıcı} sunucumuza takviye yaptı, çok teşekkürler! **{sunucu}** artık {sayı} takviyeye sahip. 🚀";

/// Takviye teşekkür şablonunu doldurur.
pub fn boost_text(template: &str, mention: &str, name: &str, guild: &str, count: u64) -> String {
    let template = if template.trim().is_empty() { DEFAULT_BOOST_MESSAGE } else { template };
    template
        .replace("{kullanıcı}", mention)
        .replace("{isim}", name)
        .replace("{sunucu}", guild)
        .replace("{sayı}", &count.to_string())
}

/// Üye sunucuya takviye yaptı (`premium_since` boştan dolu hâle geçti).
pub async fn on_boost(ctx: &serenity::Context, data: &Data, guild_id: GuildId, user: &serenity::User) {
    let Ok(settings) = data.settings.get::<CommunitySettings>(&data.db, guild_id).await else {
        return;
    };
    if let Some(role) = settings.boost_role.filter(|r| *r != 0).map(RoleId::new) {
        let safe = ctx.cache.guild(guild_id).and_then(|g| g.roles.get(&role).cloned());
        match safe.map(|r| crate::rolepanel::check_role(ctx, guild_id, &r)) {
            Some(Ok(())) => {
                if let Err(e) = ctx.http.add_member_role(guild_id, user.id, role, Some("Sunucu takviyesi")).await {
                    warn!(guild = %guild_id, error = %e, "Takviye rolü verilemedi");
                }
            }
            Some(Err(reason)) => warn!(guild = %guild_id, reason, "Takviye rolü verilmedi"),
            None => {}
        }
    }
    let Some(channel) = settings.boost_channel.filter(|c| *c != 0).map(ChannelId::new) else {
        return;
    };
    let (guild_name, count) = ctx
        .cache
        .guild(guild_id)
        .map(|g| (g.name.clone(), g.premium_subscription_count.unwrap_or(0)))
        .unwrap_or_default();
    let name = user.global_name.clone().unwrap_or_else(|| user.name.clone());
    let text = boost_text(&settings.boost_message, &user.id.mention().to_string(), &name, &guild_name, count);
    let message = CreateMessage::new()
        .content(text.chars().take(2000).collect::<String>())
        .allowed_mentions(CreateAllowedMentions::new().users(vec![user.id]));
    if let Err(e) = channel.send_message(&ctx.http, message).await {
        warn!(guild = %guild_id, channel = %channel, error = %e, "Takviye teşekkürü gönderilemedi");
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sticky {
    pub channel: u64,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommunitySettings {
    pub sticky: Vec<Sticky>,
    pub counting_channel: Option<u64>,
    /// Kelime zinciri: her kelime öncekinin son harfiyle başlar.
    pub word_chain_channel: Option<u64>,
    pub confession_channel: Option<u64>,
    /// Doluysa her itirafın yazarı bu kanala bildirilir (yalnızca yetkililerin gördüğü bir kanal olmalı).
    pub confession_log_channel: Option<u64>,
    /// Sunucudan ayrılanın rolleri hatırlanır, geri dönünce verilir.
    pub persist_roles: bool,
    /// Ses kanalında geçen her dakika için XP (0 = kapalı).
    pub voice_xp_per_min: u32,
    /// Paneli kullanabilen ek roller ("bot yöneticisi").
    pub manager_roles: Vec<u64>,
    /// Klip kanalı: buraya gönderilen videolar sayılır (AxT Capture webhook'u ya da elle yükleme).
    pub clip_channel: Option<u64>,
    /// Haftalık / aylık sıralamanın kazananlarının duyurulacağı kanal (boşsa sezon duyurusu yok).
    pub season_channel: Option<u64>,
    /// Birinciye verilecek para (ikinciye yarısı, üçüncüye çeyreği; 0 = ödül yok).
    pub season_reward: u32,
    /// Sunucuya takviye (boost) yapanlara teşekkür mesajının kanalı (boşsa kapalı).
    pub boost_channel: Option<u64>,
    /// Teşekkür şablonu: `{kullanıcı}`, `{isim}`, `{sunucu}`, `{sayı}` (boşsa hazır metin).
    pub boost_message: String,
    /// Takviye yapana ek olarak verilecek rol (Discord'un kendi takviye rolüne ek; isteğe bağlı).
    pub boost_role: Option<u64>,
}

impl PluginConfig for CommunitySettings {
    const KEY: &'static str = "community";
}

impl CommunitySettings {
    pub fn is_active(&self) -> bool {
        !self.sticky.is_empty() || self.counting_channel.is_some() || self.word_chain_channel.is_some() || self.confession_channel.is_some() || self.persist_roles || self.voice_xp_per_min > 0 || self.season_channel.is_some() || self.clip_channel.is_some() || self.boost_channel.is_some()
    }
}

// ---------------------------------------------------------------- AFK

struct Afk {
    reason: String,
    since: i64,
}

static AFK: LazyLock<DashMap<(GuildId, UserId), Afk>> = LazyLock::new(DashMap::new);
/// Redis'teki AFK kayıtları belleğe alınmış sunucular.
static AFK_LOADED: LazyLock<DashSet<GuildId>> = LazyLock::new(DashSet::new);

fn afk_key(guild_id: GuildId) -> String {
    format!("afk:{guild_id}")
}

/// Bot yeniden başladıysa sunucunun AFK kayıtlarını Redis'ten bir kez okur.
async fn load_afk(redis: &ConnectionManager, guild_id: GuildId) {
    if !AFK_LOADED.insert(guild_id) {
        return;
    }
    let mut conn = redis.clone();
    let saved: Vec<(u64, String)> = redis::cmd("HGETALL").arg(afk_key(guild_id)).query_async(&mut conn).await.unwrap_or_default();
    for (user, value) in saved {
        let Some((since, reason)) = value.split_once('|') else { continue };
        if let (Ok(since), true) = (since.parse(), user != 0) {
            AFK.insert((guild_id, UserId::new(user)), Afk { reason: reason.to_owned(), since });
        }
    }
}

pub async fn set_afk(redis: &ConnectionManager, guild_id: GuildId, user: UserId, reason: &str) {
    load_afk(redis, guild_id).await;
    let since = serenity::Timestamp::now().unix_timestamp();
    let reason: String = reason.trim().chars().take(MAX_AFK_REASON).collect();
    let mut conn = redis.clone();
    let saved: redis::RedisResult<()> = redis::cmd("HSET").arg(afk_key(guild_id)).arg(user.get()).arg(format!("{since}|{reason}")).query_async(&mut conn).await;
    if let Err(e) = saved {
        debug!(error = %e, "AFK kaydı Redis'e yazılamadı (bellekte duruyor)");
    }
    AFK.insert((guild_id, user), Afk { reason, since });
}

async fn clear_afk(redis: &ConnectionManager, guild_id: GuildId, user: UserId) -> Option<i64> {
    let (_, afk) = AFK.remove(&(guild_id, user))?;
    let mut conn = redis.clone();
    let _: redis::RedisResult<()> = redis::cmd("HDEL").arg(afk_key(guild_id)).arg(user.get()).query_async(&mut conn).await;
    Some(afk.since)
}

async fn say(ctx: &serenity::Context, channel: ChannelId, text: String) -> Option<Message> {
    // Bu modülün mesajları kimseyi etiketlemez.
    channel.send_message(&ctx.http, CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await.ok()
}

async fn handle_afk(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message) {
    load_afk(&data.redis, guild_id).await;
    if AFK.is_empty() {
        return;
    }
    if let Some(since) = clear_afk(&data.redis, guild_id, msg.author.id).await {
        say(ctx, msg.channel_id, format!("👋 Tekrar hoş geldin <@{}>! AFK durumun kaldırıldı (<t:{since}:R> ayrılmıştın).", msg.author.id)).await;
    }
    let notes: Vec<String> = msg
        .mentions
        .iter()
        .filter(|u| u.id != msg.author.id)
        .filter_map(|u| {
            let afk = AFK.get(&(guild_id, u.id))?;
            let reason = if afk.reason.is_empty() { String::new() } else { format!(": {}", afk.reason) };
            Some(format!("💤 **{}** şu an AFK{reason} (<t:{}:R>)", u.global_name.as_deref().unwrap_or(&u.name), afk.since))
        })
        .take(3)
        .collect();
    if !notes.is_empty() {
        say(ctx, msg.channel_id, notes.join("\n")).await;
    }
}

// ---------------------------------------------------------------- Sabit mesajlar

struct StickyState {
    count: u32,
    last_post: Option<Instant>,
    message: Option<MessageId>,
    /// Gönderim sürerken ikinci bir gönderim başlamasın.
    busy: bool,
}

static STICKY: LazyLock<DashMap<ChannelId, StickyState>> = LazyLock::new(DashMap::new);

/// Şimdi yeniden gönderilmeli mi? (yeterince yeni mesaj + bekleme süresi)
fn sticky_due(count: u32, since_last: Option<Duration>) -> bool {
    count >= STICKY_EVERY && since_last.is_none_or(|d| d >= STICKY_COOLDOWN)
}

async fn handle_sticky(ctx: &serenity::Context, settings: &CommunitySettings, msg: &Message) {
    let Some(sticky) = settings.sticky.iter().find(|s| s.channel == msg.channel_id.get()) else {
        return;
    };
    // Kilit `await`'ten önce bırakılır.
    let previous = {
        let mut state = STICKY.entry(msg.channel_id).or_insert(StickyState { count: STICKY_EVERY, last_post: None, message: None, busy: false });
        state.count = state.count.saturating_add(1);
        if state.busy || !sticky_due(state.count, state.last_post.map(|at| at.elapsed())) {
            return;
        }
        state.busy = true;
        state.count = 0;
        state.message.take()
    };
    if let Some(old) = previous {
        let _ = msg.channel_id.delete_message(&ctx.http, old).await;
    }
    let embed = CreateEmbed::new().description(&sticky.text).colour(serenity::Colour::from_rgb(250, 166, 26)).footer(serenity::CreateEmbedFooter::new("📌 Sabit mesaj"));
    let sent = msg.channel_id.send_message(&ctx.http, CreateMessage::new().embed(embed).allowed_mentions(CreateAllowedMentions::new())).await;
    if let Some(mut state) = STICKY.get_mut(&msg.channel_id) {
        state.busy = false;
        state.last_post = Some(Instant::now());
        state.message = sent.ok().map(|m| m.id);
    }
}

// ---------------------------------------------------------------- Sayma oyunu

/// Sayma kanalına yazılan bir sayının sonucu.
#[derive(Debug, PartialEq, Eq)]
pub enum CountResult {
    /// Doğru sayı; `record` yeni rekorsa `true`.
    Correct { record: bool },
    /// Yanlış sayı ya da aynı kişi art arda: sayım sıfırlanır.
    Broken { reached: u64, same_user: bool },
}

/// Oyunun kuralı (saf fonksiyon): `current` son doğru sayı, `last_user` onu yazan kişi.
pub fn judge_count(current: u64, last_user: Option<u64>, record: u64, author: u64, number: u64) -> CountResult {
    if last_user == Some(author) && current > 0 {
        return CountResult::Broken { reached: current, same_user: true };
    }
    if number == current + 1 {
        CountResult::Correct { record: number > record }
    } else {
        CountResult::Broken { reached: current, same_user: false }
    }
}

/// Sunucu başına kilit: iki mesaj aynı anda gelirse sayım bozulmasın.
static COUNT_LOCKS: LazyLock<DashMap<GuildId, Arc<tokio::sync::Mutex<()>>>> = LazyLock::new(DashMap::new);

async fn handle_counting(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message) {
    // Sayı olmayan mesajlar (sohbet) oyunu etkilemez.
    let Ok(number) = msg.content.trim().parse::<u64>() else {
        return;
    };
    let lock = Arc::clone(COUNT_LOCKS.entry(guild_id).or_default().value());
    let _guard = lock.lock().await;
    let key = format!("sayma:{guild_id}");
    let mut conn = data.redis.clone();
    let state: redis::RedisResult<(Option<u64>, Option<u64>, Option<u64>)> =
        redis::cmd("HMGET").arg(&key).arg("n").arg("user").arg("record").query_async(&mut conn).await;
    let Ok((current, last_user, record)) = state else {
        return;
    };
    let (current, record) = (current.unwrap_or(0), record.unwrap_or(0));
    let result = judge_count(current, last_user, record, msg.author.id.get(), number);
    let saved: redis::RedisResult<()> = match &result {
        CountResult::Correct { record: is_record } => {
            let mut cmd = redis::cmd("HSET");
            cmd.arg(&key).arg("n").arg(number).arg("user").arg(msg.author.id.get());
            if *is_record {
                cmd.arg("record").arg(number);
            }
            cmd.query_async(&mut conn).await
        }
        CountResult::Broken { .. } => redis::cmd("HSET").arg(&key).arg("n").arg(0).arg("user").arg(0).query_async(&mut conn).await,
    };
    if let Err(e) = saved {
        warn!(guild = %guild_id, error = %e, "Sayma oyunu durumu kaydedilemedi");
        return;
    }
    match result {
        CountResult::Correct { record } => {
            let emoji = if record && number > 10 { "🏆" } else { "✅" };
            let _ = msg.react(&ctx.http, ReactionType::Unicode(emoji.to_owned())).await;
        }
        CountResult::Broken { reached, same_user } => {
            let _ = msg.react(&ctx.http, ReactionType::Unicode("❌".to_owned())).await;
            let why = if same_user { "aynı kişi art arda sayamaz" } else { "yanlış sayı" };
            say(ctx, msg.channel_id, format!("💥 <@{}> sayımı bozdu ({why})! **{reached}**'e kadar gelmiştik (rekor: {}). Baştan: **1**", msg.author.id, record.max(reached))).await;
        }
    }
}

// ---------------------------------------------------------------- Kelime zinciri

/// Mesaj tek bir kelimeyse Türkçe küçük harfe çevrilmiş hâli (İ→i, I→ı); değilse `None` (sohbet sayılır).
pub fn normalize_word(raw: &str) -> Option<String> {
    let word: String = raw
        .trim()
        .chars()
        .map(|c| match c {
            'İ' => 'i',
            'I' => 'ı',
            c => c.to_lowercase().next().unwrap_or(c),
        })
        .collect();
    let valid = (2..=30).contains(&word.chars().count()) && word.chars().all(char::is_alphabetic);
    valid.then_some(word)
}

#[derive(Debug, PartialEq, Eq)]
pub enum WordResult {
    Accepted,
    /// Bu harfle başlamalıydı.
    WrongLetter(char),
    SameUser,
    AlreadyUsed,
    /// Türkçede "ğ" ile başlayan kelime olmadığından zinciri kilitler.
    EndsWithSoftG,
}

/// Kelime zincirinin kuralı (saf fonksiyon). `last`: son kabul edilen kelime, `used`: bu kelime daha önce yazıldı mı.
pub fn judge_word(last: Option<&str>, last_user: Option<u64>, author: u64, word: &str, used: bool) -> WordResult {
    if last.is_some() && last_user == Some(author) {
        return WordResult::SameUser;
    }
    if let Some(expected) = last.and_then(|l| l.chars().last()) {
        if !word.starts_with(expected) {
            return WordResult::WrongLetter(expected);
        }
    }
    if used {
        return WordResult::AlreadyUsed;
    }
    if word.ends_with('ğ') {
        return WordResult::EndsWithSoftG;
    }
    WordResult::Accepted
}

async fn handle_word_chain(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message) {
    // Tek kelime olmayan mesajlar (sohbet) oyunu etkilemez.
    let Some(word) = normalize_word(&msg.content) else {
        return;
    };
    let lock = Arc::clone(COUNT_LOCKS.entry(guild_id).or_default().value());
    let _guard = lock.lock().await;
    let (key, used_key) = (format!("kelime:{guild_id}"), format!("kelime:{guild_id}:kullanilan"));
    let mut conn = data.redis.clone();
    let state: redis::RedisResult<(Option<String>, Option<u64>, Option<u64>)> = redis::cmd("HMGET").arg(&key).arg("son").arg("user").arg("n").query_async(&mut conn).await;
    let Ok((last, last_user, count)) = state else {
        return;
    };
    let used: bool = redis::cmd("SISMEMBER").arg(&used_key).arg(&word).query_async(&mut conn).await.unwrap_or(false);
    let result = judge_word(last.as_deref(), last_user, msg.author.id.get(), &word, used);
    let react = |emoji: &'static str| msg.react(&ctx.http, ReactionType::Unicode(emoji.to_owned()));
    match result {
        WordResult::Accepted => {
            let length = count.unwrap_or(0) + 1;
            let saved: redis::RedisResult<()> = redis::pipe()
                .cmd("HSET").arg(&key).arg("son").arg(&word).arg("user").arg(msg.author.id.get()).arg("n").arg(length).ignore()
                .cmd("SADD").arg(&used_key).arg(&word).ignore()
                .query_async(&mut conn)
                .await;
            if let Err(e) = saved {
                warn!(guild = %guild_id, error = %e, "Kelime zinciri durumu kaydedilemedi");
                return;
            }
            let _ = react("✅").await;
            if length.is_multiple_of(50) {
                say(ctx, msg.channel_id, format!("🔗 Zincir **{length}** kelimeye ulaştı! Sıradaki kelime **{}** harfiyle başlamalı.", word.chars().last().unwrap_or('?'))).await;
            }
        }
        WordResult::WrongLetter(expected) => {
            let _ = react("❌").await;
            say(ctx, msg.channel_id, format!("<@{}> kelime **{expected}** harfiyle başlamalı (son kelime: **{}**).", msg.author.id, last.unwrap_or_default())).await;
        }
        WordResult::SameUser => {
            let _ = react("⏳").await;
        }
        WordResult::AlreadyUsed => {
            let _ = react("♻️").await;
        }
        WordResult::EndsWithSoftG => {
            let _ = react("🚫").await;
            say(ctx, msg.channel_id, format!("<@{}> **ğ** ile biten kelime yazılamaz (ğ ile başlayan kelime olmadığı için zincir kilitlenir).", msg.author.id)).await;
        }
    }
}

/// Kelime zincirini sıfırlar (kanal değişince).
pub async fn reset_word_chain(redis: &ConnectionManager, guild_id: GuildId) {
    let mut conn = redis.clone();
    let _: redis::RedisResult<()> = redis::cmd("DEL").arg(format!("kelime:{guild_id}")).arg(format!("kelime:{guild_id}:kullanilan")).query_async(&mut conn).await;
}

// ---------------------------------------------------------------- Etkinlik (günlük mesaj sayısı)

fn day_stamp(unix: i64) -> i64 {
    unix.div_euclid(86_400)
}

fn activity_key(guild_id: GuildId, day: i64) -> String {
    format!("etkinlik:{guild_id}:{day}")
}

async fn record_activity(redis: &ConnectionManager, guild_id: GuildId, channel: ChannelId) {
    let key = activity_key(guild_id, day_stamp(serenity::Timestamp::now().unix_timestamp()));
    let mut conn = redis.clone();
    let now = serenity::Timestamp::now().unix_timestamp();
    // Aynı istekte saatlik ısı haritası sayacı da artar (tek gidiş-dönüş).
    let counted: redis::RedisResult<(i64, i64)> = redis::pipe()
        .cmd("HINCRBY").arg(&key).arg(channel.get()).arg(1)
        .cmd("HINCRBY").arg(format!("saatler:{guild_id}")).arg(hour_slot(now)).arg(1)
        .query_async(&mut conn)
        .await;
    let count = counted.map(|(day, _)| day);
    // Günün ilk mesajında anahtara ömür verilir.
    if count.is_ok_and(|c| c == 1) {
        let _: redis::RedisResult<()> = redis::cmd("EXPIRE").arg(&key).arg(ACTIVITY_DAYS * 86_400).query_async(&mut conn).await;
    }
}

/// Türkiye saatiyle (UTC+3) haftanın günü (0 = Pazartesi) ve saat: "gün:saat".
pub fn hour_slot(unix: i64) -> String {
    let local = unix + 3 * 3600;
    // 1970-01-01 Perşembe'ydi (Pazartesi'den 3 gün sonra).
    let weekday = (local.div_euclid(86_400) + 3).rem_euclid(7);
    let hour = local.rem_euclid(86_400) / 3600;
    format!("{weekday}:{hour}")
}

/// Haftanın günü × saat mesaj sayıları (bütün zamanların toplamı).
pub async fn heatmap(redis: &ConnectionManager, guild_id: GuildId) -> [[u64; 24]; 7] {
    let mut conn = redis.clone();
    let raw: Vec<(String, u64)> = redis::cmd("HGETALL").arg(format!("saatler:{guild_id}")).query_async(&mut conn).await.unwrap_or_default();
    let mut grid = [[0_u64; 24]; 7];
    for (slot, count) in raw {
        let Some((day, hour)) = slot.split_once(':') else { continue };
        if let (Ok(day), Ok(hour)) = (day.parse::<usize>(), hour.parse::<usize>()) {
            if let Some(cell) = grid.get_mut(day).and_then(|row| row.get_mut(hour)) {
                *cell = count;
            }
        }
    }
    grid
}

fn flow_key(guild_id: GuildId, day: i64) -> String {
    format!("uyeler:{guild_id}:{day}")
}

fn retention_key(guild_id: GuildId) -> String {
    format!("katilim:{guild_id}")
}

/// Kalıcılık takibi: üye katılınca zamanıyla kaydedilir, ayrılınca kaydı silinir.
pub async fn track_member(redis: &ConnectionManager, guild_id: GuildId, user: UserId, joined: bool) {
    let now = serenity::Timestamp::now().unix_timestamp();
    let key = retention_key(guild_id);
    let mut conn = redis.clone();
    let _: redis::RedisResult<()> = if joined {
        // 60 günden eski kayıtlar ara sıra temizlenir.
        redis::pipe()
            .cmd("ZADD").arg(&key).arg(now).arg(user.get()).ignore()
            .cmd("ZREMRANGEBYSCORE").arg(&key).arg("-inf").arg(now - 60 * 86_400).ignore()
            .query_async(&mut conn)
            .await
    } else {
        redis::cmd("ZREM").arg(&key).arg(user.get()).query_async(&mut conn).await
    };
}

/// 7-14 gün önce katılanlardan hâlâ sunucuda olanların sayısı ve o dönemde katılanların toplamı.
pub async fn retention(redis: &ConnectionManager, guild_id: GuildId) -> (u64, u64) {
    let now = serenity::Timestamp::now().unix_timestamp();
    let mut conn = redis.clone();
    let stayed: u64 = redis::cmd("ZCOUNT").arg(retention_key(guild_id)).arg(now - 14 * 86_400).arg(now - 7 * 86_400).query_async(&mut conn).await.unwrap_or(0);
    // O dönemde katılanların toplamı günlük giriş sayaçlarından gelir (bugünden 7-13 gün öncesi).
    let joined: u64 = flow(redis, guild_id, 14).await.iter().take(7).map(|(_, joined, _)| joined).sum();
    (stayed.min(joined), joined)
}

/// Günlük üye giriş / çıkış sayacı.
pub async fn member_flow(redis: &ConnectionManager, guild_id: GuildId, joined: bool) {
    let key = flow_key(guild_id, day_stamp(serenity::Timestamp::now().unix_timestamp()));
    let mut conn = redis.clone();
    let _: redis::RedisResult<()> = redis::pipe()
        .cmd("HINCRBY").arg(&key).arg(if joined { "giren" } else { "cikan" }).arg(1).ignore()
        .cmd("EXPIRE").arg(&key).arg(ACTIVITY_DAYS * 86_400).ignore()
        .query_async(&mut conn)
        .await;
}

/// Son `days` günün (gün damgası, giren, çıkan) sayıları, eskiden yeniye.
pub async fn flow(redis: &ConnectionManager, guild_id: GuildId, days: i64) -> Vec<(i64, u64, u64)> {
    let today = day_stamp(serenity::Timestamp::now().unix_timestamp());
    let mut conn = redis.clone();
    let mut out = Vec::new();
    for day in (today - days + 1)..=today {
        let (joined, left): (Option<u64>, Option<u64>) = redis::cmd("HMGET").arg(flow_key(guild_id, day)).arg("giren").arg("cikan").query_async(&mut conn).await.unwrap_or((None, None));
        out.push((day, joined.unwrap_or(0), left.unwrap_or(0)));
    }
    out
}

/// Son `days` günün mesaj sayıları (eskiden yeniye): (gün damgası, toplam) ve bu aralıkta en çok yazılan kanallar.
pub async fn activity(redis: &ConnectionManager, guild_id: GuildId, days: i64) -> (Vec<(i64, u64)>, Vec<(u64, u64)>) {
    let today = day_stamp(serenity::Timestamp::now().unix_timestamp());
    let mut conn = redis.clone();
    let mut daily = Vec::new();
    let mut channels: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    for day in (today - days + 1)..=today {
        let counts: Vec<(u64, u64)> = redis::cmd("HGETALL").arg(activity_key(guild_id, day)).query_async(&mut conn).await.unwrap_or_default();
        daily.push((day, counts.iter().map(|(_, n)| n).sum()));
        for (channel, n) in counts {
            *channels.entry(channel).or_default() += n;
        }
    }
    let mut top: Vec<(u64, u64)> = channels.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    top.truncate(8);
    (daily, top)
}

// ---------------------------------------------------------------- Olay bağlantıları

/// Her sunucu mesajında çağrılır (botlar ve otomatik moderasyonun sildiği mesajlar hariç).
pub async fn on_message(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message) {
    let settings = data.settings.get::<CommunitySettings>(&data.db, guild_id).await.ok();
    let sticky = async {
        if let Some(settings) = settings.as_deref() {
            handle_sticky(ctx, settings, msg).await;
        }
    };
    let counting = async {
        if settings.as_deref().and_then(|s| s.counting_channel) == Some(msg.channel_id.get()) {
            handle_counting(ctx, data, guild_id, msg).await;
        }
    };
    let chain = async {
        if settings.as_deref().and_then(|s| s.word_chain_channel) == Some(msg.channel_id.get()) {
            handle_word_chain(ctx, data, guild_id, msg).await;
        }
    };
    tokio::join!(handle_afk(ctx, data, guild_id, msg), sticky, counting, chain, record_activity(&data.redis, guild_id, msg.channel_id));
}

fn roles_key(guild_id: GuildId, user: UserId) -> String {
    format!("kalici-roller:{guild_id}:{user}")
}

/// Ayrılan üyenin rollerini hatırlar (ayar açıksa).
pub async fn on_leave(data: &Data, guild_id: GuildId, user: UserId, roles: &[RoleId]) {
    let enabled = data.settings.get::<CommunitySettings>(&data.db, guild_id).await.is_ok_and(|s| s.persist_roles);
    if !enabled || roles.is_empty() {
        return;
    }
    let ids: Vec<String> = roles.iter().map(|r| r.get().to_string()).collect();
    let mut conn = data.redis.clone();
    let saved: redis::RedisResult<()> = redis::cmd("SET").arg(roles_key(guild_id, user)).arg(ids.join(",")).arg("EX").arg(ROLE_MEMORY_SECS).query_async(&mut conn).await;
    if let Err(e) = saved {
        debug!(error = %e, "Kalıcı roller kaydedilemedi");
    }
}

/// Geri dönen üyeye eski rollerini verir. Yetki içeren, bota ait ya da botun üstündeki roller verilmez.
pub async fn on_join(ctx: &serenity::Context, data: &Data, member: &Member) {
    let guild_id = member.guild_id;
    let enabled = data.settings.get::<CommunitySettings>(&data.db, guild_id).await.is_ok_and(|s| s.persist_roles);
    if !enabled || member.user.bot {
        return;
    }
    let mut conn = data.redis.clone();
    let saved: Option<String> = redis::cmd("GETDEL").arg(roles_key(guild_id, member.user.id)).query_async(&mut conn).await.ok().flatten();
    let Some(saved) = saved else { return };
    let wanted: Vec<RoleId> = saved.split(',').filter_map(|id| id.parse::<u64>().ok()).filter(|id| *id != 0).map(RoleId::new).collect();
    // Önbellek kilidi `await`'ten önce bırakılır.
    let known: Vec<serenity::Role> = {
        let Some(guild) = ctx.cache.guild(guild_id) else { return };
        wanted.iter().filter_map(|id| guild.roles.get(id).cloned()).collect()
    };
    // `check_role` önbelleğe kendisi bakar; yukarıdaki kilit bırakıldıktan sonra çağrılır.
    let allowed: Vec<RoleId> = known.iter().filter(|role| crate::rolepanel::check_role(ctx, guild_id, role).is_ok()).map(|role| role.id).collect();
    if allowed.is_empty() {
        return;
    }
    match member.add_roles(&ctx.http, &allowed).await {
        Ok(_) => tracing::info!(guild = %guild_id, user = %member.user.id, count = allowed.len(), "Kalıcı roller geri verildi"),
        Err(e) => warn!(guild = %guild_id, user = %member.user.id, error = %e, "Kalıcı roller verilemedi"),
    }
}

/// Seste geçen dakikalar için XP (ayar açıksa).
pub async fn voice_xp(data: &Data, guild_id: GuildId, user: UserId, minutes: u64) {
    let rate = data.settings.get::<CommunitySettings>(&data.db, guild_id).await.map_or(0, |s| s.voice_xp_per_min.min(MAX_VOICE_XP));
    let xp = u64::from(rate) * minutes.min(VOICE_XP_MAX_MINUTES);
    let Ok(delta) = i64::try_from(xp) else { return };
    if delta == 0 {
        return;
    }
    if let Err(e) = data.leveling.add_xp(&data.db, &data.redis, guild_id, user, delta).await {
        warn!(guild = %guild_id, error = ?e, "Ses XP'si eklenemedi");
    }
}

/// Klip sıralamasındaki anahtar: üyeler kimlikleriyle, webhook'lar (AxT Capture) adlarıyla sayılır.
pub fn clip_owner(user_id: u64, name: &str, is_webhook: bool) -> String {
    if is_webhook {
        format!("w:{}", name.trim().chars().take(60).collect::<String>())
    } else {
        format!("u:{user_id}")
    }
}

/// Sıralamada gösterilecek ad.
pub fn clip_label(owner: &str) -> String {
    match owner.split_once(':') {
        Some(("u", id)) => format!("<@{id}>"),
        Some(("w", name)) => format!("**{}** (AxT Capture)", name.replace(['*', '`', '@'], "")),
        _ => owner.to_owned(),
    }
}

/// Klip kanalına video geldiyse sayar. Üye mesajlarında ve webhook mesajlarında çağrılır.
pub async fn on_clip(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message) {
    let is_video = |a: &serenity::Attachment| a.content_type.as_deref().is_some_and(|t| t.starts_with("video/")) || a.filename.to_lowercase().ends_with(".mp4");
    if !msg.attachments.iter().any(is_video) {
        return;
    }
    let is_clip_channel = data.settings.get::<CommunitySettings>(&data.db, guild_id).await.is_ok_and(|s| s.clip_channel == Some(msg.channel_id.get()));
    if !is_clip_channel {
        return;
    }
    let owner = clip_owner(msg.author.id.get(), &msg.author.name, msg.webhook_id.is_some());
    let mut conn = data.redis.clone();
    let counted: redis::RedisResult<f64> = redis::cmd("ZINCRBY").arg(format!("klip:{guild_id}")).arg(1).arg(&owner).query_async(&mut conn).await;
    if counted.is_ok() {
        let _ = msg.react(&ctx.http, ReactionType::Unicode("🎬".to_owned())).await;
    }
}

/// En çok klip paylaşanlar: (sahip anahtarı, klip sayısı).
pub async fn clip_board(redis: &ConnectionManager, guild_id: GuildId, count: usize) -> Vec<(String, u64)> {
    let mut conn = redis.clone();
    let rows: Vec<(String, f64)> = redis::cmd("ZREVRANGE").arg(format!("klip:{guild_id}")).arg(0).arg(count.saturating_sub(1)).arg("WITHSCORES").query_async(&mut conn).await.unwrap_or_default();
    rows.into_iter().map(|(owner, score)| (owner, score.max(0.0) as u64)).collect()
}

pub const MAX_SEASON_REWARD: u32 = 1_000_000;
/// Dönem bittikten sonra en geç bu kadar süre içinde duyurulur (bot uzun süre kapalı kaldıysa eski dönem duyurulmaz).
const SEASON_GRACE_SECS: i64 = 3 * 86_400;

/// Biten dönemin içinden bir an: haftalık için 7 gün öncesi, aylık için geçen ayın son günü (Türkiye saati).
pub fn previous_period_instant(period: crate::leveling::Period, now: i64) -> i64 {
    use chrono::Datelike as _;
    match period {
        crate::leveling::Period::Week => now - 7 * 86_400,
        crate::leveling::Period::Month => {
            let local = now + 3 * 3600;
            let day_of_month = chrono::DateTime::from_timestamp(local, 0).map_or(1, |d| i64::from(d.day()));
            now - day_of_month * 86_400
        }
    }
}

/// Şu anki dönem ne zaman başladı? (Unix saniyesi, Türkiye saatiyle Pazartesi 00:00 / ayın 1'i 00:00)
pub fn period_start(period: crate::leveling::Period, now: i64) -> i64 {
    use chrono::Datelike as _;
    let local = now + 3 * 3600;
    let days = local.div_euclid(86_400);
    let start_day = match period {
        crate::leveling::Period::Week => days - (days + 3).rem_euclid(7),
        crate::leveling::Period::Month => days - chrono::DateTime::from_timestamp(local, 0).map_or(0, |d| i64::from(d.day()) - 1),
    };
    start_day * 86_400 - 3 * 3600
}

/// Biten haftanın / ayın kazananlarını duyurur ve ödüllerini verir (sunucu başına dönem başına bir kez).
pub fn spawn_seasons(ctx: serenity::Context, db: sqlx::PgPool, redis: ConnectionManager, settings: crate::settings::PluginSettings, leveling: Arc<crate::leveling::Leveling>) {
    use crate::leveling::Period;
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(15 * 60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            crate::supervise::round("sezon duyuruları", std::time::Duration::from_secs(300), async {
                let now = serenity::Timestamp::now().unix_timestamp();
                for guild_id in ctx.cache.guilds() {
                    let Ok(current) = settings.get::<CommunitySettings>(&db, guild_id).await else { continue };
                    let Some(channel) = current.season_channel.filter(|c| *c != 0).map(ChannelId::new) else { continue };
                    for (period, label) in [(Period::Week, "Haftanın"), (Period::Month, "Ayın")] {
                        if now - period_start(period, now) > SEASON_GRACE_SECS {
                            continue;
                        }
                        let finished = crate::leveling::period_key(guild_id, period, previous_period_instant(period, now));
                        // NX: aynı dönem iki kez duyurulmaz.
                        let mut conn = redis.clone();
                        let first: Option<String> = redis::cmd("SET").arg(format!("sezon:{finished}")).arg(1).arg("EX").arg(40 * 86_400).arg("NX").query_async(&mut conn).await.unwrap_or(None);
                        if first.is_none() {
                            continue;
                        }
                        let winners: Vec<(u64, f64)> = redis::cmd("ZREVRANGE").arg(&finished).arg(0).arg(2).arg("WITHSCORES").query_async(&mut conn).await.unwrap_or_default();
                        if winners.is_empty() {
                            continue;
                        }
                        let mut lines = Vec::new();
                        for (place, (user, xp)) in winners.iter().enumerate() {
                            let prize = i64::from(current.season_reward.min(MAX_SEASON_REWARD)) >> place;
                            let mut line = format!("{} <@{user}> — **{:.0}** XP", ["🥇", "🥈", "🥉"].get(place).copied().unwrap_or("•"), xp);
                            if prize > 0 && *user != 0 && crate::economy::add(&db, guild_id, UserId::new(*user), prize).await.is_ok() {
                                line.push_str(&format!(" · ödül {}", crate::economy::money(prize)));
                            }
                            lines.push(line);
                        }
                        let embed = CreateEmbed::new()
                            .title(format!("🏆 {label} en aktifleri"))
                            .description(format!("{}\n\nYeni dönem başladı; sıralama için `/seviye sıralama dönem:`", lines.join("\n")))
                            .colour(serenity::Colour::GOLD);
                        let users: Vec<UserId> = winners.iter().filter(|(u, _)| *u != 0).map(|(u, _)| UserId::new(*u)).collect();
                        let message = CreateMessage::new().embed(embed).allowed_mentions(CreateAllowedMentions::new().users(users));
                        match channel.send_message(&ctx.http, message).await {
                            Ok(_) => tracing::info!(guild = %guild_id, period = ?period, "Sezon kazananları duyuruldu"),
                            Err(e) => warn!(guild = %guild_id, error = %e, "Sezon duyurusu gönderilemedi"),
                        }
                    }
                }
            })
            .await;
            // `leveling` şimdilik yalnızca ömür bağı için tutulur (dönem anahtarlarını o yazar).
            let _ = &leveling;
        }
    });
}

/// Bir sonraki itiraf numarası.
pub async fn next_confession(redis: &ConnectionManager, guild_id: GuildId) -> redis::RedisResult<u64> {
    let mut conn = redis.clone();
    redis::cmd("INCR").arg(format!("itiraf:{guild_id}")).query_async(&mut conn).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_rules() {
        assert_eq!(judge_count(0, None, 0, 1, 1), CountResult::Correct { record: true });
        assert_eq!(judge_count(5, Some(2), 9, 1, 6), CountResult::Correct { record: false });
        assert_eq!(judge_count(9, Some(2), 9, 1, 10), CountResult::Correct { record: true });
        assert_eq!(judge_count(5, Some(2), 9, 1, 7), CountResult::Broken { reached: 5, same_user: false });
        assert_eq!(judge_count(5, Some(1), 9, 1, 6), CountResult::Broken { reached: 5, same_user: true });
        // Sıfırlandıktan sonra bozan kişi de 1 yazabilir.
        assert_eq!(judge_count(0, Some(1), 9, 1, 1), CountResult::Correct { record: false });
        assert_eq!(judge_count(0, None, 0, 1, 0), CountResult::Broken { reached: 0, same_user: false });
    }

    #[test]
    fn word_chain_rules() {
        assert_eq!(normalize_word("  ISIRGAN "), Some("ısırgan".to_owned()));
        assert_eq!(normalize_word("İstanbul"), Some("istanbul".to_owned()));
        for chat in ["iki kelime", "a", "kelime!", "abc123", ""] {
            assert_eq!(normalize_word(chat), None, "{chat}");
        }
        assert_eq!(judge_word(None, None, 1, "kalem", false), WordResult::Accepted);
        assert_eq!(judge_word(Some("kalem"), Some(1), 2, "masa", false), WordResult::Accepted);
        assert_eq!(judge_word(Some("kalem"), Some(1), 2, "araba", false), WordResult::WrongLetter('m'));
        assert_eq!(judge_word(Some("kalem"), Some(1), 1, "masa", false), WordResult::SameUser);
        assert_eq!(judge_word(Some("kalem"), Some(1), 2, "masa", true), WordResult::AlreadyUsed);
        assert_eq!(judge_word(Some("kalem"), Some(1), 2, "mağ", false), WordResult::EndsWithSoftG);
        // Türkçe harfler: "ağaç" → sıradaki "ç" ile başlar.
        assert_eq!(judge_word(Some("ağaç"), Some(1), 2, "çiçek", false), WordResult::Accepted);
    }

    #[test]
    fn sticky_waits_for_messages_and_cooldown() {
        assert!(sticky_due(STICKY_EVERY, None));
        assert!(!sticky_due(STICKY_EVERY - 1, None));
        assert!(!sticky_due(STICKY_EVERY, Some(Duration::from_secs(1))));
        assert!(sticky_due(STICKY_EVERY + 3, Some(STICKY_COOLDOWN)));
    }

    #[test]
    fn clip_owners_and_labels() {
        assert_eq!(clip_owner(42, "Ali", false), "u:42");
        assert_eq!(clip_owner(42, " AxThro ", true), "w:AxThro");
        assert_eq!(clip_label("u:42"), "<@42>");
        // Webhook adındaki biçimlendirme ve etiket karakterleri etkisizleştirilir.
        assert_eq!(clip_label("w:@everyone **x**"), "**everyone x** (AxT Capture)");
    }

    #[test]
    fn season_periods() {
        use crate::leveling::{period_key, Period};
        // 2026-10-12 Pazartesi 12:30 Türkiye.
        let now = 1_791_797_400;
        let guild = GuildId::new(1);
        // Dönem başlangıcı: aynı gün 00:00 Türkiye (= önceki gün 21:00 UTC); ayın başı 1 Ekim.
        assert_eq!(now - period_start(Period::Week, now), 12 * 3600 + 1800);
        assert_eq!(now - period_start(Period::Month, now), 11 * 86_400 + 12 * 3600 + 1800);
        // Biten dönemin anahtarı şimdikinden farklıdır ve bir önceki döneme aittir.
        let last_week = previous_period_instant(Period::Week, now);
        assert_ne!(period_key(guild, Period::Week, last_week), period_key(guild, Period::Week, now));
        assert_eq!(period_key(guild, Period::Week, last_week), period_key(guild, Period::Week, period_start(Period::Week, now) - 1));
        let last_month = previous_period_instant(Period::Month, now);
        assert!(period_key(guild, Period::Month, last_month).ends_with(":202609"));
        assert!(period_key(guild, Period::Month, now).ends_with(":202610"));
    }

    #[test]
    fn hour_slots_use_turkey_time() {
        // 1970-01-01 00:00 UTC = Perşembe 03:00 Türkiye.
        assert_eq!(hour_slot(0), "3:3");
        // 21:00 UTC = ertesi gün 00:00 Türkiye (Cuma).
        assert_eq!(hour_slot(21 * 3600), "4:0");
        // 2026-10-12 (Pazartesi) 09:30 UTC = Pazartesi 12:30 Türkiye.
        assert_eq!(hour_slot(1_791_797_400), "0:12");
    }

    #[test]
    fn day_stamps_and_activity_flag() {
        assert_eq!(day_stamp(0), 0);
        assert_eq!(day_stamp(86_399), 0);
        assert_eq!(day_stamp(86_400), 1);
        assert!(!CommunitySettings::default().is_active());
        assert!(CommunitySettings { voice_xp_per_min: 2, ..CommunitySettings::default() }.is_active());
        // Yalnızca yönetici rolü tanımlamak eklentiyi "açık" yapmaz.
        assert!(!CommunitySettings { manager_roles: vec![1], ..CommunitySettings::default() }.is_active());
    }
}
