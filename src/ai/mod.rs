//! Belirli kanallarda yapay zekâ ile sohbet.
//!
//! - Hangi kanalların açık olduğu bellekte önbelleklenir (mesaj başına veritabanı sorgusu yok).
//! - Kanal başına kısa sohbet geçmişi Redis'te tutulur (son 20 mesaj, 1 saat).
//! - Kullanıcı başına hız sınırı (Redis) ve eşzamanlı istek sınırı (semaphore) API kotasını korur.
//! - Model çıktısındaki @everyone / rol / kullanıcı etiketleri devre dışıdır.
//! - Modele her soruda güncel tarih/saat verilir; arama açıksa güncel konularda web'de arar
//!   (NVIDIA: Tavily aracı, Gemini: yerleşik Google araması).

pub mod assist;
pub mod characters;
pub mod provider;
pub mod search;

use std::collections::HashSet;
use std::sync::Arc;

use dashmap::DashMap;
use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateAllowedMentions, CreateMessage, EditMessage, GuildId, Message,
    MessageFlags, MessageId,
};
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use self::provider::{ChatMessage, Role};
use crate::config::{AiConfig, AiProvider};
use crate::data::Data;
use crate::db::snowflake;

const HISTORY_LEN: isize = 20;
const HISTORY_TTL_SECS: u64 = 60 * 60;
const USER_LIMIT: i64 = 5;
const USER_WINDOW_SECS: u64 = 60;
const MAX_CONCURRENT_REQUESTS: usize = 4;
const MAX_INPUT_CHARS: usize = 2000;
/// Discord mesaj sınırı 2000; biraz pay bırakılır.
const DISCORD_CHUNK: usize = 1900;
/// Cevap yazılırken mesajın düzenlenme aralığı.
const LIVE_EDIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1100);

pub struct Ai {
    config: Option<AiConfig>,
    client: reqwest::Client,
    limiter: Semaphore,
    channels: DashMap<GuildId, Arc<HashSet<ChannelId>>>,
}

impl Ai {
    pub fn new(config: Option<AiConfig>, client: reqwest::Client) -> Self {
        Self {
            config,
            client,
            limiter: Semaphore::new(MAX_CONCURRENT_REQUESTS),
            channels: DashMap::new(),
        }
    }

    /// Etkinse "Sağlayıcı (model) · web araması" bilgisi.
    pub fn describe(&self) -> Option<String> {
        self.config.as_ref().map(|c| {
            let search = match (c.provider, &c.search) {
                (AiProvider::Gemini, _) => "✅ Google araması".to_owned(),
                (AiProvider::Nim, Some(backend)) => format!("✅ {}", backend.name()),
                (AiProvider::Nim, None) => "❌ kapalı (`SEARXNG_URL` yok)".to_owned(),
            };
            format!("{} (`{}`) · Web araması: {search}", c.provider.name(), c.model)
        })
    }

    pub async fn channels(&self, db: &PgPool, guild_id: GuildId) -> anyhow::Result<Arc<HashSet<ChannelId>>> {
        if let Some(entry) = self.channels.get(&guild_id) {
            return Ok(Arc::clone(entry.value()));
        }
        let ids: Vec<i64> = sqlx::query_scalar("SELECT channel_id FROM ai_channels WHERE guild_id = $1")
            .bind(snowflake(guild_id.get())?)
            .fetch_all(db)
            .await?;
        let set: HashSet<ChannelId> = ids
            .into_iter()
            .filter_map(|id| u64::try_from(id).ok().filter(|id| *id != 0))
            .map(ChannelId::new)
            .collect();
        let set = Arc::new(set);
        self.channels.insert(guild_id, Arc::clone(&set));
        Ok(set)
    }

    /// Eklendiyse `true`, zaten varsa `false`.
    pub async fn add_channel(&self, db: &PgPool, guild_id: GuildId, channel: ChannelId) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "INSERT INTO ai_channels (guild_id, channel_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(channel.get())?)
        .execute(db)
        .await?;
        self.channels.remove(&guild_id);
        Ok(result.rows_affected() > 0)
    }

    /// Silindiyse `true`.
    pub async fn remove_channel(&self, db: &PgPool, guild_id: GuildId, channel: ChannelId) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM ai_channels WHERE guild_id = $1 AND channel_id = $2")
            .bind(snowflake(guild_id.get())?)
            .bind(snowflake(channel.get())?)
            .execute(db)
            .await?;
        self.channels.remove(&guild_id);
        Ok(result.rows_affected() > 0)
    }
}

/// Tek seferlik metin üretimi (`/ai yaz`): sohbet geçmişi kullanılmaz ve kaydedilmez.
/// AI kapalıysa `None`.
pub async fn write_once(data: &Data, guild_name: &str, persona: Option<&str>, request: &str) -> Option<anyhow::Result<String>> {
    write_once_ai(&data.ai, guild_name, persona, request).await
}

pub async fn write_once_ai(ai: &Ai, guild_name: &str, persona: Option<&str>, request: &str) -> Option<anyhow::Result<String>> {
    let config = ai.config.as_ref()?;
    let Ok(_permit) = ai.limiter.acquire().await else {
        return None;
    };
    let system = format!(
        "{}\n\nBu tek seferlik bir yazı isteğidir: istenen metni doğrudan yaz, soru sorma, açıklama ekleme. \
         En fazla 1500 karakter.",
        system_prompt(config, guild_name, persona)
    );
    let request: String = request.trim().chars().take(MAX_INPUT_CHARS).collect();
    let message = ChatMessage { role: Role::User, content: request };
    Some(provider::complete(&ai.client, config, &system, &[message], None, false).await)
}

/// Arka plan işleri için tek seferlik istek: verilen sistem metniyle (sunucu kişiliği yok).
/// Yapay zekâ kapalıysa `None`.
pub async fn complete_once(ai: &Ai, system: &str, request: &str) -> Option<anyhow::Result<String>> {
    let config = ai.config.as_ref()?;
    let Ok(_permit) = ai.limiter.acquire().await else {
        return None;
    };
    let system = format!("{system}\n\nCevabı doğrudan yaz; soru sorma, açıklama ekleme. Her zaman doğal, akıcı Türkçe yaz.");
    let request: String = request.trim().chars().take(MAX_INPUT_CHARS).collect();
    let message = ChatMessage { role: Role::User, content: request };
    Some(provider::complete(&ai.client, config, &system, &[message], None, false).await)
}

pub async fn clear_history(redis: &ConnectionManager, channel: ChannelId) -> anyhow::Result<()> {
    let mut conn = redis.clone();
    let _: i64 = redis::cmd("DEL")
        .arg(history_key(channel))
        .query_async(&mut conn)
        .await?;
    Ok(())
}

/// AI kanalındaki bir mesaja yanıt verir. AI kapalıysa veya kanal AI kanalı değilse hiçbir şey yapmaz.
pub async fn handle_message(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    msg: &Message,
) -> anyhow::Result<()> {
    let ai = &data.ai;
    let Some(config) = &ai.config else {
        return Ok(());
    };
    if !ai.channels(&data.db, guild_id).await?.contains(&msg.channel_id) {
        return Ok(());
    }

    let bot_id = ctx.cache.current_user().id;
    let content = msg
        .content
        .replace(&format!("<@{bot_id}>"), "")
        .replace(&format!("<@!{bot_id}>"), "");
    let content: String = content.trim().chars().take(MAX_INPUT_CHARS).collect();
    if content.is_empty() {
        return Ok(());
    }

    // "Yazıyor…" hemen görünsün; hız sınırı ve geçmiş aynı anda okunur.
    let typing = msg.channel_id.start_typing(&ctx.http);
    let (limit, mut history) = tokio::join!(
        rate_limit(&data.redis, guild_id, msg),
        load_history(&data.redis, msg.channel_id),
    );
    match limit {
        Ok(RateLimit::Allowed) => {}
        Ok(RateLimit::Exceeded { first }) => {
            typing.stop();
            if first {
                reply(ctx, msg, "⏳ Çok hızlı soru soruyorsun, bir dakika sonra tekrar dene.").await;
            }
            return Ok(());
        }
        Err(e) => warn!(error = %e, "AI hız sınırı kontrol edilemedi"),
    }

    // Aynı anda en fazla N istek; fazlası sırada bekler.
    let _permit = ai.limiter.acquire().await?;

    let author = msg
        .member
        .as_ref()
        .and_then(|m| m.nick.clone())
        .or_else(|| msg.author.global_name.clone())
        .unwrap_or_else(|| msg.author.name.clone());
    let user_message = ChatMessage {
        role: Role::User,
        content: format!("{author}: {content}"),
    };

    history.push(user_message.clone());

    let guild_name = ctx
        .cache
        .guild(guild_id)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "bu".to_owned());
    // Karakter ayarı bellekteki önbellekten okunur.
    let characters = data.settings.get::<characters::AiSettings>(&data.db, guild_id).await.ok();
    let persona = characters.as_deref().and_then(|c| c.persona_for(msg.channel_id.get()));
    let mut system = system_prompt(config, &guild_name, persona);
    if let Some(facts) = characters.as_deref().and_then(|c| characters::relevant_knowledge(&c.knowledge, &content)) {
        system.push_str(&format!(
            "\n\nSUNUCU BİLGİ BANKASI (yöneticilerin yazdığı, güvenilir bilgi). Bu sunucuyla ilgili sorularda önce buna dayan; \
             burada olmayan sunucu bilgisini (kural, rol, etkinlik, tarih) uydurma, bilmediğini söyle:\n\"\"\"\n{facts}\n\"\"\""
        ));
    }

    // Cevap yazıldıkça aynı mesaj düzenlenerek gösterilir; kullanıcı cevabın bitmesini beklemez.
    let (progress, progress_rx) = tokio::sync::watch::channel(String::new());
    let (done, done_rx) = tokio::sync::oneshot::channel::<()>();
    let live = tokio::spawn(live_reply(ctx.clone(), msg.clone(), progress_rx, done_rx));

    let result = provider::complete(&ai.client, config, &system, &history, Some(&progress), true).await;
    drop(done);
    // Yarım kalan gönderim varsa bitmesi beklenir; böylece aynı cevap iki kez gönderilmez.
    let live_message = live.await.unwrap_or_default();
    typing.stop();

    let answer = match result {
        Ok(answer) => answer,
        Err(e) => {
            warn!(guild = %guild_id, provider = config.provider.name(), error = ?e, "AI yanıtı alınamadı");
            show(ctx, msg, live_message, "⚠️ Şu an yanıt veremiyorum, biraz sonra tekrar dene.").await;
            return Ok(());
        }
    };

    let mut live_message = live_message;
    for chunk in split_message(&answer, DISCORD_CHUNK) {
        // İlk parça canlı mesajın yerine yazılır, kalanı yeni mesaj olur.
        show(ctx, msg, live_message.take(), &chunk).await;
    }

    let assistant_message = ChatMessage {
        role: Role::Assistant,
        content: answer,
    };
    if let Err(e) = save_history(&data.redis, msg.channel_id, &user_message, &assistant_message).await {
        debug!(error = %e, "AI geçmişi kaydedilemedi");
    }
    Ok(())
}

fn system_prompt(config: &AiConfig, guild_name: &str, persona: Option<&str>) -> String {
    let base = config.system_prompt.clone().unwrap_or_else(|| {
        format!(
            "Sen \"{guild_name}\" Discord sunucusunda yardımsever, samimi bir asistansın.\n\n\
             Dil kuralları (çok önemli):\n\
             - Varsayılan dilin Türkçe. Kullanıcı açıkça başka bir dilde yazarsa o dilde cevap ver.\n\
             - Ana dili Türkçe olan biri gibi doğal, akıcı ve dil bilgisi açısından doğru Türkçe yaz. \
             Cümlelerin anlamlı ve mantıklı olsun; süslü ama anlamsız cümleler kurma.\n\
             - Türkçe karşılığı olan kelimeler için İngilizce kelime kullanma (ör. \"torch\" değil \
             \"meşale\", \"settings\" değil \"ayarlar\"). Oyun ve programlardaki öğeleri Türkçe arayüzdeki \
             adlarıyla söyle; emin değilsen Türkçe adın yanına parantez içinde orijinal adı ekle.\n\
             - Emin olmadığın bir kelimenin Türkçesini uydurma; yanlış çeviri yerine bilinen terimi kullan.\n\
             - Başka dillerden kelime veya karakter (Çince, Rusça vb.) karıştırma.\n\
             - Bilmediğin bilgiyi uydurma.\n\n\
             Biçim: Kısa ve net ol, gerektiğinde Discord markdown kullan. Kullanıcı mesajları \
             \"İsim: mesaj\" biçiminde gelir; cevabına asla \"İsim:\" ile başlama, doğrudan cevabı yaz. \
             Küfür, nefret söylemi ve zararlı içerik üretme."
        )
    });
    // Karakter, temel kuralların üstüne eklenir: üslup değişir; dil, biçim ve güvenlik kuralları kalır.
    let base = match persona {
        Some(persona) => format!(
            "{base}\n\nCANLANDIRDIĞIN KARAKTER (yukarıdaki genel asistan tanımının yerine geçer; diğer bütün kurallar \
             aynen geçerlidir). Her cevabını bu karakterin üslubuyla yaz, karakterden çıkma, ama doğru bilgi ver:\n{persona}"
        ),
        None => base,
    };
    // Özel kişilik verilse bile zaman ve arama talimatları her zaman eklenir.
    format!("{base}\n\n{}", time_and_search_rules(config))
}

fn time_and_search_rules(config: &AiConfig) -> String {
    let now = turkey_now();
    let mut rules = format!(
        "Şu anki tarih ve saat: {}. Eğitim verilerin bu tarihten eskidir; o tarihten sonra olanları \
         bilmiyor olabilirsin, bu yüzden kendi bilgini güncelmiş gibi sunma.",
        now.text
    );
    let can_search = config.provider == AiProvider::Gemini || config.search.is_some();
    if can_search {
        rules.push_str(&format!(
            " Haberler, spor sonuçları, maçlar, fiyatlar/kurlar, hava durumu, yeni çıkan ürün ve sürümler, \
             güncel olaylar ya da emin olmadığın her gerçek bilgi için önce web araması yap ve cevabını \
             arama sonuçlarına dayandır. Arama sorgularını kullanıcının dilinde, kısa ve doğal yaz \
             (örn. \"dolar kuru\", \"Galatasaray son maç skoru\", \"İstanbul hava durumu\"); sorguya tarih \
             ekleme, arama motoru zaten en güncel sonuçları getirir. Yıl gerçekten gerekiyorsa {} kullan. \
             Bugünün tarihini zaten biliyorsun, onun için arama yapma. Bilgiyi aldığın kaynağın \
             bağlantısını cevabın sonunda kısaca belirt.",
            now.year
        ));
    } else {
        rules.push_str(
            " İnternete erişimin yok. Güncel bilgi gerektiren sorularda (haber, skor, fiyat, hava durumu vb.) \
             bunu açıkça söyle ve tahmin yürütme.",
        );
    }
    rules
}

struct Now {
    text: String,
    year: i32,
}

/// Türkiye saati (UTC+3, 2016'dan beri yaz saati uygulaması yok).
fn turkey_now() -> Now {
    use chrono::{Datelike, FixedOffset, Timelike, Utc};

    const MONTHS: [&str; 12] = [
        "Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim",
        "Kasım", "Aralık",
    ];
    const DAYS: [&str; 7] = ["Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi", "Pazar"];

    let now = match FixedOffset::east_opt(3 * 3600) {
        Some(tr) => Utc::now().with_timezone(&tr),
        None => Utc::now().fixed_offset(),
    };
    let month = MONTHS.get(now.month0() as usize).copied().unwrap_or_default();
    let day = DAYS
        .get(now.weekday().num_days_from_monday() as usize)
        .copied()
        .unwrap_or_default();

    Now {
        text: format!(
            "{} {month} {} {day}, saat {:02}:{:02} (Türkiye saati)",
            now.day(),
            now.year(),
            now.hour(),
            now.minute()
        ),
        year: now.year(),
    }
}

enum RateLimit {
    Allowed,
    Exceeded { first: bool },
}

async fn rate_limit(redis: &ConnectionManager, guild_id: GuildId, msg: &Message) -> redis::RedisResult<RateLimit> {
    let key = format!("ai:rl:{guild_id}:{}", msg.author.id);
    let mut conn = redis.clone();
    let (count,): (i64,) = redis::pipe()
        .atomic()
        .cmd("SET")
        .arg(&key)
        .arg(0)
        .arg("EX")
        .arg(USER_WINDOW_SECS)
        .arg("NX")
        .ignore()
        .cmd("INCR")
        .arg(&key)
        .query_async(&mut conn)
        .await?;

    Ok(if count > USER_LIMIT {
        RateLimit::Exceeded {
            first: count == USER_LIMIT + 1,
        }
    } else {
        RateLimit::Allowed
    })
}

fn history_key(channel: ChannelId) -> String {
    format!("ai:history:{channel}")
}

/// Redis'te en yeni başta tutulur; modele eskiden yeniye verilir.
async fn load_history(redis: &ConnectionManager, channel: ChannelId) -> Vec<ChatMessage> {
    let mut conn = redis.clone();
    let raw: Vec<String> = match redis::cmd("LRANGE")
        .arg(history_key(channel))
        .arg(0)
        .arg(HISTORY_LEN - 1)
        .query_async(&mut conn)
        .await
    {
        Ok(raw) => raw,
        Err(e) => {
            debug!(error = %e, "AI geçmişi okunamadı, geçmişsiz devam ediliyor");
            return Vec::new();
        }
    };

    let mut history: Vec<ChatMessage> = raw
        .iter()
        .filter_map(|entry| serde_json::from_str(entry).ok())
        .collect();
    history.reverse();
    // Geçmiş asistan mesajıyla başlamasın.
    while history.first().is_some_and(|m| m.role == Role::Assistant) {
        history.remove(0);
    }
    history
}

async fn save_history(
    redis: &ConnectionManager,
    channel: ChannelId,
    user: &ChatMessage,
    assistant: &ChatMessage,
) -> anyhow::Result<()> {
    let key = history_key(channel);
    let mut conn = redis.clone();
    let _: () = redis::pipe()
        .atomic()
        .cmd("LPUSH")
        .arg(&key)
        .arg(serde_json::to_string(user)?)
        .arg(serde_json::to_string(assistant)?)
        .ignore()
        .cmd("LTRIM")
        .arg(&key)
        .arg(0)
        .arg(HISTORY_LEN - 1)
        .ignore()
        .cmd("EXPIRE")
        .arg(&key)
        .arg(HISTORY_TTL_SECS)
        .ignore()
        .query_async(&mut conn)
        .await?;
    Ok(())
}

/// Yanıt olarak gönderir; model çıktısındaki etiketler kimseyi rahatsız etmesin diye kapalıdır.
async fn reply(ctx: &serenity::Context, msg: &Message, text: &str) -> Option<MessageId> {
    // Kaynak linkleri büyük önizlemelerle sohbeti doldurmasın.
    let message = CreateMessage::new()
        .content(text)
        .reference_message(msg)
        .flags(MessageFlags::SUPPRESS_EMBEDS)
        .allowed_mentions(CreateAllowedMentions::new().replied_user(false));
    match msg.channel_id.send_message(&ctx.http, message).await {
        Ok(sent) => Some(sent.id),
        Err(e) => {
            warn!(channel = %msg.channel_id, error = %e, "AI yanıtı gönderilemedi");
            None
        }
    }
}

/// Daha önce gönderilmiş bir mesaj varsa onu düzenler, yoksa yeni yanıt gönderir.
async fn show(ctx: &serenity::Context, msg: &Message, existing: Option<MessageId>, text: &str) -> Option<MessageId> {
    let Some(id) = existing else {
        return reply(ctx, msg, text).await;
    };
    let edit = EditMessage::new()
        .content(text)
        .allowed_mentions(CreateAllowedMentions::new().replied_user(false));
    match msg.channel_id.edit_message(&ctx.http, id, edit).await {
        Ok(_) => Some(id),
        Err(e) => {
            // Mesaj bu arada silinmiş olabilir; cevap kaybolmasın.
            debug!(channel = %msg.channel_id, error = %e, "AI yanıtı düzenlenemedi, yeniden gönderiliyor");
            reply(ctx, msg, text).await
        }
    }
}

/// Model yazarken ara metni gösterir: ilk metinde yanıtı gönderir, sonra aynı mesajı
/// düzenler. `done` kapanınca döner; gönderdiği mesajın kimliğini verir.
async fn live_reply(
    ctx: serenity::Context,
    msg: Message,
    mut progress: tokio::sync::watch::Receiver<String>,
    mut done: tokio::sync::oneshot::Receiver<()>,
) -> Option<MessageId> {
    let mut sent = None;
    loop {
        tokio::select! {
            biased;
            _ = &mut done => break,
            changed = progress.changed() => {
                if changed.is_err() {
                    break;
                }
            }
        }
        let text = live_text(&progress.borrow_and_update());
        if text.is_empty() {
            continue;
        }
        sent = show(&ctx, &msg, sent, &text).await;
        // Discord kanal başına 5 sn'de 5 düzenlemeye izin verir.
        tokio::select! {
            biased;
            _ = &mut done => break,
            () = tokio::time::sleep(LIVE_EDIT_INTERVAL) => {}
        }
    }
    sent
}

/// Yazılmakta olan metin: Discord sınırına kısaltılır, sonuna imleç eklenir.
fn live_text(partial: &str) -> String {
    let partial = partial.trim();
    if partial.is_empty() {
        return String::new();
    }
    let mut text: String = partial.chars().take(DISCORD_CHUNK).collect();
    text.push_str(" ▌");
    text
}

/// Uzun yanıtları Discord sınırına göre, mümkünse satır sonlarından böler.
fn split_message(text: &str, max: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for line in text.split_inclusive('\n') {
        if current.chars().count() + line.chars().count() > max && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        if line.chars().count() > max {
            // Tek satır sınırdan uzunsa karakter bazında böl.
            let chars: Vec<char> = line.chars().collect();
            for piece in chars.chunks(max) {
                chunks.push(piece.iter().collect());
            }
        } else {
            current.push_str(line);
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_long_messages_on_lines() {
        let text = "a".repeat(10) + "\n" + &"b".repeat(10);
        let chunks = split_message(&text, 12);
        assert_eq!(chunks, vec!["a".repeat(10) + "\n", "b".repeat(10)]);
    }

    #[test]
    fn splits_single_huge_line() {
        let chunks = split_message(&"x".repeat(25), 10);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 10));
    }

    #[test]
    fn short_message_is_one_chunk() {
        assert_eq!(split_message("merhaba", 1900), vec!["merhaba".to_owned()]);
    }
}

/// Gerçek API'lerle uçtan uca test. Normal `cargo test` çalıştırmaz; elle:
/// `cargo test live_ai_answers_with_current_info -- --ignored --nocapture`
#[cfg(test)]
mod live_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "gerçek yapay zekâ ve arama servislerine istek atar"]
    async fn live_ai_answers_with_current_info() -> anyhow::Result<()> {
        dotenvy::dotenv().ok();
        // Hangi aramaların yapıldığını görmek için.
        tracing_subscriber::fmt()
            .with_env_filter("discord_bot=debug")
            .with_test_writer()
            .try_init()
            .ok();
        let config = crate::config::Config::from_env()?;
        let ai = config.ai.ok_or_else(|| anyhow::anyhow!("AI anahtarı yok"))?;
        let client = crate::net::http_client()?;

        let question = ChatMessage {
            role: Role::User,
            // Başka bir soru denemek için: AI_TEST_QUESTION="..." (model için AI_MODEL).
            content: std::env::var("AI_TEST_QUESTION")
                .unwrap_or_else(|_| "Ahmet: Bugün ayın kaçı ve şu an dolar kuru kaç TL? Kaynağını da yaz.".to_owned()),
        };
        println!("--- model: {}", ai.model);
        let started = std::time::Instant::now();
        // İlk kelimenin ne kadar sürede geldiği de ölçülür (kullanıcının beklediği süre).
        let (progress, mut progress_rx) = tokio::sync::watch::channel(String::new());
        let first = tokio::spawn(async move {
            progress_rx.changed().await.ok();
            (started.elapsed(), progress_rx.borrow().clone())
        });
        let answer = provider::complete(&client, &ai, &system_prompt(&ai, "Test", None), &[question], Some(&progress), true).await?;
        drop(progress);
        if let Ok((elapsed, text)) = first.await {
            println!("--- ilk metin {:.1} sn: {text}", elapsed.as_secs_f64());
        }
        println!("--- toplam {:.1} sn ---\n{answer}\n---", started.elapsed().as_secs_f64());
        assert!(!answer.is_empty());
        Ok(())
    }
}
