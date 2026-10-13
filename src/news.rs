//! Haber eklentisi: teknoloji, oyun, sinema, dizi haberleri ve ücretsiz oyunlar.
//!
//! Diğer haber botlarından (News.gg, Postman, Gaming News, Steam News, rss.app) örnek alınanlar:
//! - Kategoriler ve 5 dilde kaynak (Türkçe, İngilizce, Almanca, İspanyolca, Fransızca). Sunucu hangi dillerdeki
//!   kaynakların geleceğini seçer; istenirse haber (başlık + özet) yapay zekâyla seçilen dile çevrilir.
//! - Anahtar kelime süzgeci (yalnızca şunlar / şunlar hariç), aynı haberin başka sitelerden tekrarının süzülmesi.
//! - Tam (görselli, özetli) ya da kompakt görünüm; anlık ya da günlük toplu gönderim; kategoriye özel etiket rolü.
//! - Ücretsiz oyunlar (GamerPower: Epic, Steam, GOG… dağıtımları).
//! - Haftalık özet: seçilen gün ve saatte kategorinin öne çıkan gelişmeleri.
//!
//! Tek döngü bütün sunucular için kaynakları okur (kaynak başına tek istek). Görülen haberler Redis'te; kaynak
//! ilk kez okunduğunda eski haberler duyurulmaz. Bir sunucuda haberler ilk açıldığında, çalıştığı görülsün diye
//! her kategoriden en yeni bir haber gönderilir. Çeviri ve özetler haber + dil başına bir kez üretilip saklanır.

use std::collections::BTreeMap;
use std::time::Duration;

use poise::serenity_prelude::{self as serenity, ChannelId, CreateAllowedMentions, CreateEmbed, CreateMessage, GuildId, RoleId};
use redis::aio::ConnectionManager;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{debug, info, warn};

use crate::settings::{PluginConfig, PluginSettings};

const POLL_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Kaynak başına bir turda en fazla bu kadar yeni haber işlenir.
const MAX_NEW_PER_SOURCE: usize = 3;
/// Bundan eski haberler (yayın tarihine göre) duyurulmaz.
const MAX_AGE_SECS: i64 = 24 * 60 * 60;
const MAX_SEEN: usize = 300;
/// Haftalık özet için tutulan başlık sayısı (kategori başına).
const WEEK_ITEMS: isize = 400;
const EXCERPT_CHARS: usize = 350;
pub const MAX_PER_HOUR: u32 = 20;
pub const MAX_KEYWORDS: usize = 30;
pub const MAX_KEYWORD_LEN: usize = 40;
/// Aynı haber sayılması için başlık kelimelerinin benzerliği (Jaccard).
const DUPLICATE_SIMILARITY: f32 = 0.6;
/// Günlük toplu gönderimde kategori başına en fazla başlık.
const DIGEST_LINES: usize = 15;

pub struct Language {
    pub code: &'static str,
    pub name: &'static str,
    pub flag: &'static str,
}

pub const LANGUAGES: &[Language] = &[
    Language { code: "tr", name: "Türkçe", flag: "🇹🇷" },
    Language { code: "en", name: "İngilizce", flag: "🇬🇧" },
    Language { code: "de", name: "Almanca", flag: "🇩🇪" },
    Language { code: "es", name: "İspanyolca", flag: "🇪🇸" },
    Language { code: "fr", name: "Fransızca", flag: "🇫🇷" },
];

pub fn language(code: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.code == code)
}

/// Yapay zekâya verilecek dil adı (kendi dilinde).
fn native_name(code: &str) -> &'static str {
    match code {
        "tr" => "Türkçe",
        "en" => "English",
        "de" => "Deutsch",
        "es" => "Español",
        "fr" => "Français",
        _ => "Türkçe",
    }
}

pub enum Feed {
    Rss(&'static str),
    /// GamerPower ücretsiz oyun listesi (JSON).
    GamerPower,
}

pub struct Source {
    pub name: &'static str,
    pub lang: &'static str,
    pub feed: Feed,
}

const fn rss(name: &'static str, lang: &'static str, url: &'static str) -> Source {
    Source { name, lang, feed: Feed::Rss(url) }
}

const GAMERPOWER_URL: &str = "https://www.gamerpower.com/api/giveaways?type=game&sort-by=date";

impl Source {
    fn url(&self) -> &'static str {
        match self.feed {
            Feed::Rss(url) => url,
            Feed::GamerPower => GAMERPOWER_URL,
        }
    }
}

pub struct Category {
    pub key: &'static str,
    pub name: &'static str,
    pub emoji: &'static str,
    pub colour: (u8, u8, u8),
    pub sources: &'static [Source],
}

/// 2026-10-12'de hepsi denendi (HTTP 200, haber içeriyor). Türkçe kaynaklar her kategoride başta.
pub const CATEGORIES: &[Category] = &[
    Category {
        key: "teknoloji",
        name: "Teknoloji",
        emoji: "💻",
        colour: (59, 130, 246),
        sources: &[
            rss("Webtekno", "tr", "https://www.webtekno.com/rss.xml"),
            rss("Chip Online", "tr", "https://www.chip.com.tr/rss"),
            rss("LOG", "tr", "https://www.log.com.tr/feed/"),
            rss("ShiftDelete.Net", "tr", "https://shiftdelete.net/feed"),
            rss("The Verge", "en", "https://www.theverge.com/rss/index.xml"),
            rss("Ars Technica", "en", "https://feeds.arstechnica.com/arstechnica/index"),
            rss("Engadget", "en", "https://www.engadget.com/rss.xml"),
            rss("heise online", "de", "https://www.heise.de/rss/heise-atom.xml"),
            rss("Golem", "de", "https://rss.golem.de/rss.php?feed=RSS2.0"),
            rss("Xataka", "es", "https://www.xataka.com/feedburner.xml"),
            rss("Numerama", "fr", "https://www.numerama.com/feed/"),
            rss("01net", "fr", "https://www.01net.com/feed/"),
        ],
    },
    Category {
        key: "oyun",
        name: "Oyun",
        emoji: "🎮",
        colour: (168, 85, 247),
        sources: &[
            rss("IGN Türkiye", "tr", "https://tr.ign.com/feed.xml"),
            rss("Merlin'in Kazanı", "tr", "https://www.merlininkazani.com/rss"),
            rss("GameSpot", "en", "https://www.gamespot.com/feeds/news/"),
            rss("PC Gamer", "en", "https://www.pcgamer.com/rss/"),
            rss("IGN", "en", "https://feeds.feedburner.com/ign/games-all"),
            rss("PC Games", "de", "https://www.pcgames.de/feed.cfm?menu_alias=home"),
            rss("3DJuegos", "es", "https://www.3djuegos.com/feedburner.xml"),
            rss("Jeuxvideo.com", "fr", "https://www.jeuxvideo.com/rss/rss.xml"),
            rss("Gamekult", "fr", "https://www.gamekult.com/feed.xml"),
        ],
    },
    Category {
        key: "sinema",
        name: "Sinema",
        emoji: "🎬",
        colour: (234, 179, 8),
        sources: &[
            rss("Beyazperde", "tr", "https://www.beyazperde.com/rss/haberler.xml"),
            rss("Variety", "en", "https://variety.com/v/film/feed/"),
            rss("Collider", "en", "https://collider.com/feed/category/movie-news/"),
            rss("Filmstarts", "de", "https://www.filmstarts.de/rss/nachrichten.xml"),
            rss("SensaCine", "es", "https://www.sensacine.com/rss/noticias.xml"),
            rss("AlloCiné", "fr", "https://www.allocine.fr/rss/news.xml"),
        ],
    },
    Category {
        key: "dizi",
        name: "Dizi",
        emoji: "📺",
        colour: (239, 68, 68),
        sources: &[
            rss("TVLine", "en", "https://tvline.com/feed/"),
            rss("Variety TV", "en", "https://variety.com/v/tv/feed/"),
            rss("Screen Rant", "en", "https://screenrant.com/feed/tv/"),
            rss("Serienjunkies", "de", "https://www.serienjunkies.de/rss/news.xml"),
            rss("Espinof", "es", "https://www.espinof.com/feedburner.xml"),
            rss("AlloCiné Séries", "fr", "https://www.allocine.fr/rss/news-series.xml"),
        ],
    },
    Category {
        key: "ucretsiz-oyun",
        name: "Ücretsiz oyunlar",
        emoji: "🎁",
        colour: (34, 197, 94),
        sources: &[Source { name: "GamerPower", lang: "en", feed: Feed::GamerPower }],
    },
];

/// Başlığında bunlar geçen haberler gönderilmez (genel sohbet kanalına uygun olmayan içerik).
const BLOCKED_WORDS: &[&str] = &["çıplak", "porno", "seks", "+18", "18+", "erotik", "nude", "nsfw", "porn", "sex tape", "onlyfans", "nackt", "desnud"];

/// Haber sohbet kanalına uygun mu? (başlıkta yetişkin içerik kelimesi yoksa)
pub fn is_suitable(item: &NewsItem) -> bool {
    !BLOCKED_WORDS.iter().any(|w| contains_word(&item.title, w))
}

/// Büyük/küçük harf ve Türkçe I/İ farkı gözetmeden karşılaştırma için.
fn fold(text: &str) -> String {
    text.replace('I', "ı").replace('İ', "i").to_lowercase()
}

/// Metinde kelime geçiyor mu? Hem Türkçe (I→ı) hem İngilizce (I→i) küçük harf kuralıyla bakılır.
fn contains_word(text: &str, word: &str) -> bool {
    fold(text).contains(&fold(word)) || text.to_lowercase().contains(&word.to_lowercase())
}

pub fn category(key: &str) -> Option<&'static Category> {
    CATEGORIES.iter().find(|c| c.key == key)
}

pub const DAYS: [&str; 7] = ["Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi", "Pazar"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// Görselli, özetli embed.
    #[default]
    Full,
    /// Tek satırlık başlık + küçük görsel.
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Haber çıktıkça.
    #[default]
    Instant,
    /// Günde bir kez toplu liste.
    Daily,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewsSettings {
    pub enabled: bool,
    /// Haber kanalı (kategoriye özel kanal yoksa).
    pub channel: Option<u64>,
    /// Açık kategoriler (`CATEGORIES` anahtarları; boşsa hepsi).
    pub categories: Vec<String>,
    /// Kategoriye özel kanallar.
    pub channels: BTreeMap<String, u64>,
    /// Hangi dillerdeki kaynaklar gelsin (`LANGUAGES` kodları; boşsa hepsi).
    pub source_languages: Vec<String>,
    /// Haberler hangi dilde yazılsın (boşsa kaynağın kendi dili); farklıysa yapay zekâyla çevrilir.
    pub output_language: String,
    /// Haber ayrıntısı yapay zekâyla özetlensin mi?
    pub ai_details: bool,
    pub style: Style,
    pub delivery: Delivery,
    /// Günlük toplu gönderim saati (Türkiye saati).
    pub digest_hour: u8,
    /// Doluysa yalnızca bu kelimelerden biri geçen haberler gelir.
    pub include_keywords: Vec<String>,
    /// Bu kelimelerden biri geçen haberler gelmez.
    pub exclude_keywords: Vec<String>,
    /// Aynı olay birden çok siteden gelirse bir kez paylaşılır.
    pub dedupe: bool,
    /// Kategori başına saatte en fazla kaç haber (anlık gönderimde).
    pub max_per_hour: u32,
    pub weekly: bool,
    /// 0 = Pazartesi … 6 = Pazar
    pub weekly_day: u8,
    /// Türkiye saati.
    pub weekly_hour: u8,
    /// Haberlerde etiketlenecek rol (kategoriye özel rol yoksa).
    pub role: Option<u64>,
    /// Kategoriye özel etiket rolleri.
    pub roles: BTreeMap<String, u64>,
}

impl Default for NewsSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            channel: None,
            categories: Vec::new(),
            channels: BTreeMap::new(),
            source_languages: vec!["tr".to_owned(), "en".to_owned()],
            output_language: "tr".to_owned(),
            ai_details: true,
            style: Style::Full,
            delivery: Delivery::Instant,
            digest_hour: 9,
            include_keywords: Vec::new(),
            exclude_keywords: Vec::new(),
            dedupe: true,
            max_per_hour: 4,
            weekly: true,
            weekly_day: 6,
            weekly_hour: 20,
            role: None,
            roles: BTreeMap::new(),
        }
    }
}

impl PluginConfig for NewsSettings {
    const KEY: &'static str = "news";
}

impl NewsSettings {
    /// Bu kategorinin haberleri hangi kanala gider? (kapalıysa `None`)
    pub fn channel_for(&self, category: &str) -> Option<ChannelId> {
        // Hiç kategori seçilmediyse hepsi açık sayılır.
        if !self.enabled || (!self.categories.is_empty() && !self.categories.iter().any(|c| c == category)) {
            return None;
        }
        self.channels.get(category).copied().or(self.channel).filter(|c| *c != 0).map(ChannelId::new)
    }

    pub fn is_active(&self) -> bool {
        self.enabled && CATEGORIES.iter().any(|c| self.channel_for(c.key).is_some())
    }

    fn role_for(&self, category: &str) -> Option<RoleId> {
        self.roles.get(category).copied().or(self.role).filter(|r| *r != 0).map(RoleId::new)
    }

    /// Haber çevrilecekse hedef dil.
    pub fn target_language(&self) -> Option<&str> {
        Some(self.output_language.as_str()).filter(|l| language(l).is_some())
    }

    /// Bu haber bu sunucunun süzgeçlerinden geçiyor mu? (kaynak dili + anahtar kelimeler)
    pub fn allows(&self, item: &NewsItem) -> bool {
        if !self.source_languages.is_empty() && !self.source_languages.contains(&item.lang) {
            return false;
        }
        let raw = format!("{} {}", item.title, item.excerpt);
        let has = |words: &[String]| words.iter().map(|w| w.trim()).filter(|w| !w.is_empty()).any(|w| contains_word(&raw, w));
        if !self.include_keywords.is_empty() && !has(&self.include_keywords) {
            return false;
        }
        !has(&self.exclude_keywords)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsItem {
    pub id: String,
    pub title: String,
    pub link: String,
    /// Temizlenmiş kısa metin (RSS açıklaması).
    pub excerpt: String,
    /// Yapay zekâ özeti için daha uzun metin.
    pub body: String,
    pub image: Option<String>,
    /// Unix saniye (bilinmiyorsa 0).
    pub published: i64,
    /// Kaynağın dili (`LANGUAGES` kodu).
    #[serde(default)]
    pub lang: String,
    /// Ek bilgi satırı (ücretsiz oyunlarda fiyat, platform, bitiş).
    #[serde(default)]
    pub extra: Option<String>,
}

/// RSS/Atom'dan haberleri okur: başlık, bağlantı, özet, görsel ve tarih.
pub fn parse(xml: &str) -> anyhow::Result<Vec<NewsItem>> {
    let item_re = Regex::new(r"(?s)<(item|entry)[\s>](.*?)</(?:item|entry)>")?;
    let tag = |name: &str| Regex::new(&format!(r"(?s)<{name}(?:\s[^>]*)?>(.*?)</{name}>"));
    let title_re = tag("title")?;
    let guid_re = tag("guid")?;
    let id_re = tag("id")?;
    let link_re = tag("link")?;
    let desc_re = tag("description")?;
    let summary_re = tag("summary")?;
    let content_re = tag("content:encoded")?;
    let atom_content_re = tag("content")?;
    let date_re = tag("pubDate")?;
    let published_re = tag("published")?;
    let updated_re = tag("updated")?;
    let href_re = Regex::new(r#"(?s)<link\s[^>]*?href\s*=\s*["']([^"']+)["'][^>]*>"#)?;
    let alternate_re = Regex::new(r#"(?s)<link\s[^>]*?rel\s*=\s*["']alternate["'][^>]*>"#)?;
    let media_re = Regex::new(r#"(?s)<(?:media:content|media:thumbnail|enclosure)\s[^>]*?url\s*=\s*["']([^"']+)["']"#)?;
    let img_re = Regex::new(r#"(?is)<img\s[^>]*?src\s*=\s*["']([^"']+)["']"#)?;
    let raw = |re: &Regex, body: &str| re.captures(body).and_then(|c| c.get(1)).map(|m| unwrap_cdata(m.as_str()));

    if !xml.contains("<rss") && !xml.contains("<feed") && !xml.contains("<rdf:RDF") {
        anyhow::bail!("bu bir RSS/Atom akışı değil");
    }
    let items = item_re
        .captures_iter(xml)
        .filter_map(|found| {
            let body = found.get(2)?.as_str();
            let link = alternate_re
                .find(body)
                .and_then(|m| raw(&href_re, m.as_str()))
                .or_else(|| raw(&link_re, body))
                .or_else(|| raw(&href_re, body))
                .map(|l| decode(&l).trim().to_owned())
                .filter(|l| l.starts_with("https://") || l.starts_with("http://"))?;
            let title = clean(&raw(&title_re, body).unwrap_or_default());
            if title.is_empty() {
                return None;
            }
            let id = raw(&guid_re, body).or_else(|| raw(&id_re, body)).map(|i| clean(&i)).filter(|i| !i.is_empty()).unwrap_or_else(|| link.clone());
            let html = raw(&content_re, body)
                .or_else(|| raw(&atom_content_re, body))
                .or_else(|| raw(&desc_re, body))
                .or_else(|| raw(&summary_re, body))
                .unwrap_or_default();
            let short_html = raw(&desc_re, body).or_else(|| raw(&summary_re, body)).unwrap_or_else(|| html.clone());
            let image = raw(&media_re, body)
                .or_else(|| img_re.captures(&decode(&html)).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned()))
                .map(|u| decode(&u))
                .filter(|u| u.starts_with("https://"));
            let published = raw(&date_re, body)
                .or_else(|| raw(&published_re, body))
                .or_else(|| raw(&updated_re, body))
                .and_then(|d| parse_date(d.trim()))
                .unwrap_or(0);
            let body_text: String = clean(&html).chars().take(4000).collect();
            let excerpt = excerpt(&clean(&short_html), &title);
            Some(NewsItem { id, title, link, excerpt, body: body_text, image, published, lang: String::new(), extra: None })
        })
        .collect();
    Ok(items)
}

fn unwrap_cdata(raw: &str) -> String {
    let raw = raw.trim();
    raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw).to_owned()
}

/// HTML kaçışları (çift kaçışlı `&amp;lt;` ve sayısal `&#34;` / `&#x27;` dahil).
fn decode(text: &str) -> String {
    let mut out = text.to_owned();
    for _ in 0..2 {
        out = decode_numeric(&out);
        out = out
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&#039;", "'")
            .replace("&apos;", "'")
            .replace("&#8217;", "’")
            .replace("&#8216;", "‘")
            .replace("&#8220;", "“")
            .replace("&#8221;", "”")
            .replace("&#8211;", "–")
            .replace("&#8230;", "…")
            .replace("&nbsp;", " ")
            .replace("&amp;", "&");
    }
    out
}

/// `&#NN;` ve `&#xHH;` biçimindeki karakter kodlarını çözer.
fn decode_numeric(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("&#") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let decoded = after.find(';').filter(|end| *end <= 8).and_then(|end| {
            let code = &after[..end];
            let value = match code.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => code.parse::<u32>().ok(),
            };
            value.and_then(char::from_u32).map(|c| (c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &after[end + 1..];
            }
            None => {
                out.push_str("&#");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Etiketleri atar, kaçışları çözer, boşlukları sadeleştirir.
fn clean(raw: &str) -> String {
    let decoded = decode(&unwrap_cdata(raw));
    let mut out = String::with_capacity(decoded.len());
    let mut in_tag = false;
    for c in decoded.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Kısa özet: başlığın tekrarını ve WordPress'in "The post … appeared first on …" eklerini atar.
fn excerpt(text: &str, title: &str) -> String {
    let mut text = text.trim().to_owned();
    if let Some(rest) = text.strip_prefix(title) {
        text = rest.trim_start_matches([' ', '-', '–', ':']).to_owned();
    }
    for marker in ["The post ", "appeared first on", "Devamını oku", "Read More", "[…]", "[...]"] {
        if let Some(i) = text.find(marker) {
            text.truncate(i);
        }
    }
    let text = text.trim();
    if text.chars().count() <= EXCERPT_CHARS {
        return text.to_owned();
    }
    let cut: String = text.chars().take(EXCERPT_CHARS).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{cut}…")
}

fn parse_date(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(raw)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(raw))
        .ok()
        .map(|d| d.timestamp())
}

/// GamerPower ücretsiz oyun listesi → haberler.
pub fn parse_gamerpower(json: &str) -> anyhow::Result<Vec<NewsItem>> {
    #[derive(Deserialize)]
    struct Giveaway {
        id: u64,
        title: String,
        #[serde(default)]
        worth: String,
        #[serde(default)]
        image: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        open_giveaway_url: String,
        #[serde(default)]
        gamerpower_url: String,
        #[serde(default)]
        published_date: String,
        #[serde(default)]
        end_date: String,
        #[serde(default)]
        platforms: String,
    }
    let list: Vec<Giveaway> = serde_json::from_str(json)?;
    Ok(list
        .into_iter()
        .filter_map(|g| {
            let link = [g.open_giveaway_url, g.gamerpower_url].into_iter().find(|u| u.starts_with("https://"))?;
            let title = g.title.trim_end_matches(" Giveaway").trim().to_owned();
            let mut extra = Vec::new();
            if !g.worth.is_empty() && g.worth != "N/A" {
                extra.push(format!("💰 Normal fiyatı: **{}**", g.worth));
            }
            if !g.platforms.is_empty() {
                extra.push(format!("🖥️ {}", g.platforms));
            }
            if !g.end_date.is_empty() && g.end_date != "N/A" {
                let end = chrono::NaiveDateTime::parse_from_str(&g.end_date, "%Y-%m-%d %H:%M:%S").ok().map(|d| d.and_utc().timestamp());
                extra.push(match end {
                    Some(ts) => format!("⏳ Son gün: <t:{ts}:R>"),
                    None => format!("⏳ Son gün: {}", g.end_date),
                });
            }
            let published = chrono::NaiveDateTime::parse_from_str(&g.published_date, "%Y-%m-%d %H:%M:%S").map(|d| d.and_utc().timestamp()).unwrap_or(0);
            Some(NewsItem {
                id: format!("gamerpower-{}", g.id),
                excerpt: excerpt(&g.description, &title),
                body: g.description,
                title,
                link,
                image: Some(g.image).filter(|u| u.starts_with("https://")),
                published,
                lang: String::new(),
                extra: (!extra.is_empty()).then(|| extra.join(" · ")),
            })
        })
        .collect())
}

/// Bir kaynağın bütün haberleri (dili işaretlenmiş).
async fn read_source(source: &Source) -> anyhow::Result<Vec<NewsItem>> {
    let text = crate::feeds::fetch_text(source.url()).await?;
    let mut items = match source.feed {
        Feed::Rss(_) => parse(&text)?,
        Feed::GamerPower => parse_gamerpower(&text)?,
    };
    for item in &mut items {
        item.lang = source.lang.to_owned();
    }
    Ok(items)
}

// ---------------------------------------------------------------- Döngü

#[derive(Clone)]
pub struct NewsContext {
    pub ctx: serenity::Context,
    pub db: PgPool,
    pub redis: ConnectionManager,
    pub settings: PluginSettings,
    pub ai: std::sync::Arc<crate::ai::Ai>,
}

impl NewsContext {
    pub fn from_data(ctx: &serenity::Context, data: &crate::data::Data) -> Self {
        Self {
            ctx: ctx.clone(),
            db: data.db.clone(),
            redis: data.redis.clone(),
            settings: data.settings.clone(),
            ai: std::sync::Arc::clone(&data.ai),
        }
    }
}

pub fn spawn(news: NewsContext) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            crate::supervise::round("haberler", Duration::from_secs(9 * 60), poll(&news)).await;
        }
    });
}

type Active = Vec<(GuildId, std::sync::Arc<NewsSettings>)>;

/// Haberi açık sunucular ve ayarları.
async fn active_guilds(news: &NewsContext) -> Active {
    let mut out = Vec::new();
    for guild_id in news.ctx.cache.guilds() {
        match news.settings.get::<NewsSettings>(&news.db, guild_id).await {
            Ok(s) if s.is_active() => out.push((guild_id, s)),
            Ok(_) => {}
            Err(e) => debug!(guild = %guild_id, error = ?e, "Haber ayarı okunamadı"),
        }
    }
    out
}

async fn poll(news: &NewsContext) {
    let guilds = active_guilds(news).await;
    if guilds.is_empty() {
        return;
    }
    for category in CATEGORIES {
        let targets: Vec<&(GuildId, std::sync::Arc<NewsSettings>)> = guilds.iter().filter(|(_, s)| s.channel_for(category.key).is_some()).collect();
        if targets.is_empty() {
            continue;
        }
        // Bu turda okunan bütün haberler (ilk açılıştaki örnek haber için).
        let mut everything: Vec<(&'static str, NewsItem)> = Vec::new();
        for source in category.sources {
            // Hiçbir sunucunun istemediği dildeki kaynak hiç okunmaz.
            if !targets.iter().any(|(_, s)| s.source_languages.is_empty() || s.source_languages.iter().any(|l| l == source.lang)) {
                continue;
            }
            let items = match read_source(source).await {
                Ok(items) => items,
                Err(e) => {
                    debug!(source = source.name, error = ?e, "Haber kaynağı okunamadı");
                    continue;
                }
            };
            let fresh = match fresh_items(news, source, &items).await {
                Ok(fresh) => fresh,
                Err(e) => {
                    debug!(source = source.name, error = ?e, "Görülen haberler okunamadı");
                    continue;
                }
            };
            for item in fresh {
                remember_for_week(news, category.key, source.name, &item).await;
                let duplicate = is_duplicate(news, &item).await;
                for (guild_id, settings) in &targets {
                    if duplicate && settings.dedupe || !settings.allows(&item) {
                        continue;
                    }
                    deliver(news, *guild_id, settings, category, source.name, &item).await;
                    crate::newsprefs::dispatch(news, *guild_id, settings, category, source.name, &item).await;
                }
            }
            everything.extend(items.into_iter().filter(is_suitable).map(|i| (source.name, i)));
        }
        // Haberleri yeni açan sunuculara her kategoriden bir örnek (en yeni, süzgeçten geçen).
        for (guild_id, settings) in &targets {
            let Some((source, item)) = everything.iter().filter(|(_, i)| settings.allows(i)).max_by_key(|(_, i)| i.published) else { continue };
            let mut conn = news.redis.clone();
            let first: Option<String> = redis::cmd("SET")
                .arg(format!("haber:ilk:{guild_id}:{}", category.key))
                .arg(1)
                .arg("EX")
                .arg(30 * 86_400)
                .arg("NX")
                .query_async(&mut conn)
                .await
                .unwrap_or(None);
            if first.is_some() {
                send_now(news, *guild_id, settings, category, source, item).await;
            }
        }
    }
    let (_, hour, today, _) = turkey_now();
    for (guild_id, settings) in &guilds {
        if hour == settings.digest_hour {
            crate::newsprefs::daily_digests(news, *guild_id, &today).await;
        }
        if settings.delivery == Delivery::Daily {
            daily_if_due(news, *guild_id, settings).await;
        }
        if settings.weekly {
            weekly_if_due(news, *guild_id, settings).await;
        }
    }
}

/// Kaynaktaki yeni haberler (eskiden yeniye). Kaynak ilk kez okunuyorsa hepsi "görüldü" sayılır.
async fn fresh_items(news: &NewsContext, source: &Source, items: &[NewsItem]) -> anyhow::Result<Vec<NewsItem>> {
    let key = format!("haber:gorulen:{:x}", fnv(source.url()));
    let mut conn = news.redis.clone();
    let stored: Option<String> = redis::cmd("GET").arg(&key).query_async(&mut conn).await?;
    let first_read = stored.is_none();
    let mut seen: Vec<String> = stored.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let now = serenity::Timestamp::now().unix_timestamp();
    let mut fresh: Vec<NewsItem> = items
        .iter()
        .filter(|i| !seen.contains(&i.id))
        .filter(|i| i.published == 0 || now - i.published <= MAX_AGE_SECS)
        .filter(|i| is_suitable(i))
        .cloned()
        .collect();
    for item in items.iter().rev() {
        if !seen.contains(&item.id) {
            seen.push(item.id.clone());
        }
    }
    let overflow = seen.len().saturating_sub(MAX_SEEN);
    seen.drain(..overflow);
    redis::cmd("SET").arg(&key).arg(serde_json::to_string(&seen)?).arg("EX").arg(30 * 86_400).query_async::<()>(&mut conn).await?;
    if first_read {
        return Ok(Vec::new());
    }
    // Akışta en yeni başta: eskiden yeniye gönder, turda en fazla birkaç tane.
    fresh.reverse();
    let skip = fresh.len().saturating_sub(MAX_NEW_PER_SOURCE);
    Ok(fresh.into_iter().skip(skip).collect())
}

/// Başlığın anlamlı kelimeleri (3 harften uzun, küçük harf, noktalama yok).
fn title_words(title: &str) -> std::collections::BTreeSet<String> {
    fold(title)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 3)
        .map(str::to_owned)
        .collect()
}

/// İki başlığın kelime benzerliği (0-1).
pub fn similarity(a: &str, b: &str) -> f32 {
    let (a, b) = (title_words(a), title_words(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let common = a.intersection(&b).count() as f32;
    common / (a.len() + b.len()) as f32 * 2.0
}

/// Aynı olay son 2 günde başka bir siteden (ya da başka kategoriden) geldi mi? (başlık benzerliği)
async fn is_duplicate(news: &NewsContext, item: &NewsItem) -> bool {
    let key = "haber:son";
    let mut conn = news.redis.clone();
    let recent: Vec<String> = redis::cmd("LRANGE").arg(key).arg(0).arg(199).query_async(&mut conn).await.unwrap_or_default();
    let duplicate = recent.iter().any(|t| similarity(t, &item.title) >= DUPLICATE_SIMILARITY);
    let _: redis::RedisResult<()> = redis::pipe()
        .cmd("LPUSH").arg(key).arg(&item.title).ignore()
        .cmd("LTRIM").arg(key).arg(0).arg(199).ignore()
        .cmd("EXPIRE").arg(key).arg(2 * 86_400).ignore()
        .query_async(&mut conn)
        .await;
    duplicate
}

#[derive(Serialize, Deserialize)]
struct WeekEntry {
    title: String,
    link: String,
    source: String,
    at: i64,
    /// Haberin dili (eski kayıtlarda yok).
    #[serde(default)]
    lang: String,
}

/// Son okunan bir haber (kişisel "Haberlerim" listesi için).
pub struct RecentEntry {
    pub title: String,
    pub link: String,
    pub source: String,
    pub at: i64,
    pub lang: String,
}

/// Kategorinin son haberleri, sunucunun dil ve kelime süzgeçlerinden geçenler (en yeni önce).
pub async fn recent(news: &NewsContext, category: &str, settings: &NewsSettings, limit: usize) -> Vec<RecentEntry> {
    let mut conn = news.redis.clone();
    let raw: Vec<String> = redis::cmd("LRANGE").arg(format!("haber:hafta:{category}")).arg(0).arg(WEEK_ITEMS - 1).query_async(&mut conn).await.unwrap_or_default();
    raw.iter()
        .filter_map(|r| serde_json::from_str::<WeekEntry>(r).ok())
        .filter(|e| {
            let item = NewsItem { title: e.title.clone(), lang: e.lang.clone(), ..NewsItem::default() };
            is_suitable(&item) && (e.lang.is_empty() && settings.source_languages.is_empty() || settings.allows(&item))
        })
        .take(limit)
        .map(|e| RecentEntry { title: e.title, link: e.link, source: e.source, at: e.at, lang: e.lang })
        .collect()
}

/// Başlığı hedef dile çevirir (gerekirse; sonuç 2 gün saklanır). Çevrilemezse olduğu gibi.
pub async fn title_in(news: &NewsContext, title: &str, lang: &str, target: Option<&str>) -> String {
    let Some(target) = target.filter(|t| *t != lang && !lang.is_empty()) else {
        return title.to_owned();
    };
    let key = format!("haber:baslik:{target}:{:x}", fnv(title));
    let mut conn = news.redis.clone();
    if let Ok(Some(cached)) = redis::cmd("GET").arg(&key).query_async::<Option<String>>(&mut conn).await {
        return cached;
    }
    match machine_translate(title, target).await {
        Some(translated) => {
            let _: redis::RedisResult<()> = redis::cmd("SET").arg(&key).arg(&translated).arg("EX").arg(2 * 86_400).query_async(&mut conn).await;
            translated
        }
        None => title.to_owned(),
    }
}

/// Bağlantının kısa özeti (kişisel gönderimde aynı haberi tanımak için).
pub fn link_hash(link: &str) -> u64 {
    fnv(link)
}

async fn remember_for_week(news: &NewsContext, category: &str, source: &str, item: &NewsItem) {
    let entry = WeekEntry { title: item.title.clone(), link: item.link.clone(), source: source.to_owned(), at: serenity::Timestamp::now().unix_timestamp(), lang: item.lang.clone() };
    let Ok(json) = serde_json::to_string(&entry) else { return };
    let key = format!("haber:hafta:{category}");
    let mut conn = news.redis.clone();
    let result: redis::RedisResult<()> = redis::pipe()
        .cmd("LPUSH").arg(&key).arg(json).ignore()
        .cmd("LTRIM").arg(&key).arg(0).arg(WEEK_ITEMS - 1).ignore()
        .cmd("EXPIRE").arg(&key).arg(14 * 86_400).ignore()
        .query_async(&mut conn)
        .await;
    if let Err(e) = result {
        debug!(error = %e, "Haftalık haber listesi güncellenemedi");
    }
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

// ---------------------------------------------------------------- Çeviri / özet

/// Sunucuya gösterilecek hâl: (gerekirse çevrilmiş) başlık ve özet.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Rendered {
    pub title: String,
    /// Yapay zekâ özeti / çevirisi (yoksa RSS özeti gösterilir).
    pub summary: Option<String>,
    /// Hangi dile çevrildi (çevrilmediyse boş).
    pub translated_to: String,
    /// "yz" (yapay zekâ) ya da "google" (Google Çeviri).
    #[serde(default)]
    pub engine: String,
}

/// Haberi sunucunun ayarına göre hazırlar: hedef dil farklıysa başlık + özet çevrilir; özet açıksa
/// ayrıntılı özet yazılır. Yapay zekâ yoksa / yetişemezse haber olduğu gibi gösterilir.
/// Sonuç haber + dil + özet türü başına bir kez üretilir (Redis'te 2 gün).
pub async fn render(news: &NewsContext, item: &NewsItem, target: Option<&str>, details: bool, limit: Duration) -> Rendered {
    let original = Rendered { title: item.title.clone(), summary: None, translated_to: String::new(), engine: String::new() };
    let translate = target.filter(|t| *t != item.lang);
    let lang = translate.or(target).unwrap_or(item.lang.as_str());
    if translate.is_none() && !details {
        return original;
    }
    let text = if item.body.chars().count() > item.excerpt.chars().count() { &item.body } else { &item.excerpt };
    if !details && text.chars().count() < 20 && translate.is_none() {
        return original;
    }
    let key = format!("haber:yz:{lang}:{}:{:x}", u8::from(details), fnv(&item.id));
    let mut conn = news.redis.clone();
    if let Ok(Some(cached)) = redis::cmd("GET").arg(&key).query_async::<Option<String>>(&mut conn).await {
        if let Ok(rendered) = serde_json::from_str::<Rendered>(&cached) {
            return rendered;
        }
    }
    // Yalnızca çeviri gerekiyorsa (özet istenmiyor) doğrudan Google Çeviri: hızlı ve yeterli.
    if !details {
        let Some(code) = translate else { return original };
        let rendered = machine_rendered(item, code).await.unwrap_or(original);
        cache_rendered(news, &key, &rendered).await;
        return rendered;
    }
    let language = native_name(lang);
    let request = format!(
        "Aşağıdaki haberi {language} dilinde yaz.\n\
         1. satıra YALNIZCA haberin başlığını yaz{}.\n\
         Sonraki satırlara {}\n\
         Yalnızca metindeki bilgileri kullan; yorum, tahmin, madde işareti ve \"Başlık:\" gibi etiketler ekleme.\n\n\
         Başlık: {}\n\nMetin: {}",
        if translate.is_some() { " (çevirerek)" } else { "" },
        if details { "haberi 3-5 cümlelik akıcı bir özetle anlat." } else { "haberi 1-2 cümleyle kısaca anlat." },
        item.title,
        text.chars().take(3000).collect::<String>()
    );
    let system = format!("Sen kısa ve tarafsız haber özetleri yazan bir haber editörüsün. Cevabı tamamen {language} dilinde yaz.");
    let answer = tokio::time::timeout(limit, crate::ai::complete_once(&news.ai, &system, &request)).await;
    let rendered = match answer {
        Ok(Some(Ok(answer))) => split_answer(&answer, translate.is_some(), lang),
        _ => None,
    };
    let rendered = match (rendered, translate) {
        (Some(r), _) => r,
        // Yapay zekâ yetişemedi: çeviri gerekiyorsa Google Çeviri; yoksa haber olduğu gibi.
        (None, Some(code)) => match machine_rendered(item, code).await {
            Some(r) => r,
            None => return original,
        },
        (None, None) => return original,
    };
    cache_rendered(news, &key, &rendered).await;
    rendered
}

async fn cache_rendered(news: &NewsContext, key: &str, rendered: &Rendered) {
    let Ok(json) = serde_json::to_string(rendered) else { return };
    let mut conn = news.redis.clone();
    let _: redis::RedisResult<()> = redis::cmd("SET").arg(key).arg(json).arg("EX").arg(2 * 86_400).query_async(&mut conn).await;
}

/// Başlık ve kısa özeti Google Çeviri ile çevirir.
async fn machine_rendered(item: &NewsItem, target: &str) -> Option<Rendered> {
    let title = machine_translate(&item.title, target).await?;
    let summary = if item.excerpt.is_empty() { None } else { machine_translate(&item.excerpt, target).await };
    Some(Rendered { title, summary, translated_to: target.to_owned(), engine: "google".to_owned() })
}

/// Google Çeviri'nin ücretsiz ucu (anahtar gerekmez; resmî değil, ileride değişebilir).
/// Botun genel istemcisi (HTTP/2) Google'a "otomatik trafik" görünüp 429 alıyor; tarayıcı gibi istemci kullanılır.
pub async fn machine_translate(text: &str, target: &str) -> Option<String> {
    let text: String = text.chars().take(1500).collect();
    let response = crate::net::plain_client()
        .ok()?
        .get("https://translate.googleapis.com/translate_a/single")
        .query(&[("client", "gtx"), ("sl", "auto"), ("tl", target), ("dt", "t"), ("q", text.as_str())])
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let value: serde_json::Value = response.json().await.ok()?;
    let translated: String = value.get(0)?.as_array()?.iter().filter_map(|part| part.get(0)?.as_str()).collect();
    let translated = translated.trim().to_owned();
    (!translated.is_empty()).then_some(translated)
}

/// Yapay zekâ cevabı: ilk satır başlık, gerisi özet.
fn split_answer(answer: &str, translated: bool, lang: &str) -> Option<Rendered> {
    let mut lines = answer.lines().map(str::trim).filter(|l| !l.is_empty());
    let title = lines.next()?;
    let title = title
        .trim_start_matches(|c: char| c == '#' || c == '*' || c.is_whitespace())
        .trim_end_matches('*');
    let title = ["Başlık:", "Title:", "Titel:", "Título:", "Titre:"].iter().fold(title.to_owned(), |t, p| t.trim_start_matches(p).trim().to_owned());
    let title = title.trim_matches(['"', '“', '”', '«', '»']).trim().to_owned();
    let summary: String = lines.collect::<Vec<_>>().join("\n").chars().take(1200).collect();
    if title.is_empty() {
        return None;
    }
    Some(Rendered {
        title: title.chars().take(250).collect(),
        summary: (!summary.is_empty()).then_some(summary),
        translated_to: if translated { lang.to_owned() } else { String::new() },
        engine: "yz".to_owned(),
    })
}

// ---------------------------------------------------------------- Gönderim

pub fn news_embed(category: &Category, source: &str, item: &NewsItem, rendered: &Rendered, style: Style) -> CreateEmbed {
    let (r, g, b) = category.colour;
    let flag = language(&item.lang).map_or("", |l| l.flag);
    let mut embed = CreateEmbed::new()
        .title(rendered.title.chars().take(250).collect::<String>())
        .url(&item.link)
        .colour(serenity::Colour::from_rgb(r, g, b));
    if item.published > 0 {
        if let Ok(ts) = serenity::Timestamp::from_unix_timestamp(item.published) {
            embed = embed.timestamp(ts);
        }
    }
    let footer = match language(&rendered.translated_to) {
        Some(to) if rendered.engine == "google" => format!("{flag} → {} Google Çeviri ile çevrildi · {source}", to.flag),
        Some(to) => format!("{flag} → {} Yapay zekâyla çevrildi · {source}", to.flag),
        None if rendered.summary.is_some() => format!("{flag} Özet yapay zekâyla hazırlandı · {source}"),
        None => format!("{flag} {source}"),
    };
    embed = embed.footer(serenity::CreateEmbedFooter::new(footer));
    match style {
        Style::Compact => {
            let mut line = format!("{} {}", category.emoji, category.name);
            if let Some(extra) = &item.extra {
                line.push_str(&format!(" · {extra}"));
            }
            embed = embed.description(line);
            if let Some(image) = &item.image {
                embed = embed.thumbnail(image);
            }
        }
        Style::Full => {
            let body = match &rendered.summary {
                Some(text) => text.clone(),
                None if item.excerpt.is_empty() => String::new(),
                None => item.excerpt.clone(),
            };
            let mut description = body;
            if let Some(extra) = &item.extra {
                description = format!("{description}\n\n{extra}");
            }
            let more = if category.key == "ucretsiz-oyun" { "Ücretsiz al →" } else { "Haberin tamamı →" };
            description = format!("{}\n\n[{more}]({})", description.trim(), item.link);
            embed = embed
                .author(serenity::CreateEmbedAuthor::new(format!("{} {} · {source}", category.emoji, category.name)))
                .description(description.trim().to_owned());
            if let Some(image) = &item.image {
                embed = embed.image(image);
            }
        }
    }
    embed
}

async fn deliver(news: &NewsContext, guild_id: GuildId, settings: &NewsSettings, category: &Category, source: &str, item: &NewsItem) {
    match settings.delivery {
        Delivery::Instant => {
            // Saat başı sınır (kanal haberle dolmasın).
            let hour = serenity::Timestamp::now().unix_timestamp() / 3600;
            let key = format!("haber:saat:{guild_id}:{}:{hour}", category.key);
            let mut conn = news.redis.clone();
            let count: i64 = redis::pipe()
                .cmd("INCR").arg(&key)
                .cmd("EXPIRE").arg(&key).arg(3700).ignore()
                .query_async::<(i64,)>(&mut conn)
                .await
                .map_or(1, |(c,)| c);
            if count > i64::from(settings.max_per_hour.clamp(1, MAX_PER_HOUR)) {
                return;
            }
            send_now(news, guild_id, settings, category, source, item).await;
        }
        Delivery::Daily => queue_for_digest(news, guild_id, settings, category, source, item).await,
    }
}

/// Haberi hemen gönderir (anlık gönderim ve ilk açılış örneği).
async fn send_now(news: &NewsContext, guild_id: GuildId, settings: &NewsSettings, category: &Category, source: &str, item: &NewsItem) {
    let Some(channel) = settings.channel_for(category.key) else { return };
    let rendered = render(news, item, settings.target_language(), settings.ai_details && settings.style == Style::Full, Duration::from_secs(15)).await;
    let embed = news_embed(category, source, item, &rendered, settings.style);
    let mut message = CreateMessage::new().embed(embed).allowed_mentions(CreateAllowedMentions::new());
    if let Some(role) = settings.role_for(category.key) {
        message = message.content(format!("<@&{role}>")).allowed_mentions(CreateAllowedMentions::new().roles(vec![role]));
    }
    match channel.send_message(&news.ctx.http, message).await {
        Ok(_) => debug!(guild = %guild_id, category = category.key, "Haber gönderildi"),
        Err(e) => warn!(guild = %guild_id, channel = %channel, error = %e, "Haber gönderilemedi (botun kanalda 'Kanalı Görüntüle', 'Mesaj Gönder' ve 'Bağlantı Yerleştir' izni olmalı)"),
    }
}

#[derive(Serialize, Deserialize)]
struct Pending {
    category: String,
    title: String,
    link: String,
    source: String,
}

async fn queue_for_digest(news: &NewsContext, guild_id: GuildId, settings: &NewsSettings, category: &Category, source: &str, item: &NewsItem) {
    // Toplu listede yalnızca başlık gerekir; çeviri istenmişse başlık (ve kısa açıklama) çevrilir.
    let rendered = render(news, item, settings.target_language(), false, Duration::from_secs(25)).await;
    let pending = Pending { category: category.key.to_owned(), title: rendered.title, link: item.link.clone(), source: source.to_owned() };
    let Ok(json) = serde_json::to_string(&pending) else { return };
    let key = format!("haber:bekleyen:{guild_id}");
    let mut conn = news.redis.clone();
    let _: redis::RedisResult<()> = redis::pipe()
        .cmd("RPUSH").arg(&key).arg(json).ignore()
        .cmd("LTRIM").arg(&key).arg(-200).arg(-1).ignore()
        .cmd("EXPIRE").arg(&key).arg(3 * 86_400).ignore()
        .query_async(&mut conn)
        .await;
}

/// Türkiye saatiyle (gün 0 = Pazartesi, saat, tarih, "yıl-hafta").
fn turkey_now() -> (u8, u8, String, String) {
    use chrono::{Datelike, Timelike};
    let now = chrono::Utc::now() + chrono::Duration::hours(3);
    let week = now.iso_week();
    (
        now.weekday().num_days_from_monday() as u8,
        now.hour() as u8,
        now.format("%Y-%m-%d").to_string(),
        format!("{}-{:02}", week.year(), week.week()),
    )
}

/// Bir kez çalışması gereken işler için Redis kilidi (`true`: ilk kez).
async fn once(news: &NewsContext, key: String, ttl_secs: u64) -> bool {
    let mut conn = news.redis.clone();
    let first: Option<String> = redis::cmd("SET").arg(key).arg(1).arg("EX").arg(ttl_secs).arg("NX").query_async(&mut conn).await.unwrap_or(None);
    first.is_some()
}

async fn daily_if_due(news: &NewsContext, guild_id: GuildId, settings: &NewsSettings) {
    let (_, hour, date, _) = turkey_now();
    if hour != settings.digest_hour || !once(news, format!("haber:gunluk:{guild_id}:{date}"), 2 * 86_400).await {
        return;
    }
    let key = format!("haber:bekleyen:{guild_id}");
    let mut conn = news.redis.clone();
    let (raw, ()): (Vec<String>, ()) = match redis::pipe().cmd("LRANGE").arg(&key).arg(0).arg(-1).cmd("DEL").arg(&key).ignore().query_async(&mut conn).await {
        Ok(result) => result,
        Err(e) => {
            debug!(error = %e, "Bekleyen haberler okunamadı");
            return;
        }
    };
    let pending: Vec<Pending> = raw.iter().filter_map(|r| serde_json::from_str(r).ok()).collect();
    for category in CATEGORIES {
        let lines: Vec<String> = pending
            .iter()
            .filter(|p| p.category == category.key)
            .rev()
            .take(DIGEST_LINES)
            .map(|p| format!("• [{}]({}) — {}", p.title.chars().take(150).collect::<String>(), p.link, p.source))
            .collect();
        let Some(channel) = settings.channel_for(category.key) else { continue };
        if lines.is_empty() {
            continue;
        }
        let (r, g, b) = category.colour;
        let embed = CreateEmbed::new()
            .title(format!("{} Günün {} haberleri", category.emoji, category.name.to_lowercase()))
            .description(lines.join("\n").chars().take(4000).collect::<String>())
            .colour(serenity::Colour::from_rgb(r, g, b))
            .footer(serenity::CreateEmbedFooter::new("AxBot haberler · günlük özet"));
        let mut message = CreateMessage::new().embed(embed).allowed_mentions(CreateAllowedMentions::new());
        if let Some(role) = settings.role_for(category.key) {
            message = message.content(format!("<@&{role}>")).allowed_mentions(CreateAllowedMentions::new().roles(vec![role]));
        }
        if let Err(e) = channel.send_message(&news.ctx.http, message).await {
            warn!(guild = %guild_id, error = %e, "Günlük haber özeti gönderilemedi");
        }
    }
}

async fn weekly_if_due(news: &NewsContext, guild_id: GuildId, settings: &NewsSettings) {
    let (day, hour, _, week) = turkey_now();
    if day != settings.weekly_day || hour != settings.weekly_hour || !once(news, format!("haber:haftalik:{guild_id}:{week}"), 8 * 86_400).await {
        return;
    }
    for category in CATEGORIES {
        let Some(channel) = settings.channel_for(category.key) else { continue };
        let Some(embed) = weekly_embed(news, category, settings).await else { continue };
        if let Err(e) = channel.send_message(&news.ctx.http, CreateMessage::new().embed(embed).allowed_mentions(CreateAllowedMentions::new())).await {
            warn!(guild = %guild_id, error = %e, "Haftalık haber özeti gönderilemedi");
        } else {
            info!(guild = %guild_id, category = category.key, "Haftalık haber özeti gönderildi");
        }
    }
}

/// Kategorinin son 7 günlük özeti (yapay zekâ özeti dil başına haftada bir üretilip saklanır).
pub async fn weekly_embed(news: &NewsContext, category: &Category, settings: &NewsSettings) -> Option<CreateEmbed> {
    let mut conn = news.redis.clone();
    let raw: Vec<String> = redis::cmd("LRANGE").arg(format!("haber:hafta:{}", category.key)).arg(0).arg(WEEK_ITEMS - 1).query_async(&mut conn).await.ok()?;
    let since = serenity::Timestamp::now().unix_timestamp() - 7 * 86_400;
    let entries: Vec<WeekEntry> = raw.iter().filter_map(|r| serde_json::from_str::<WeekEntry>(r).ok()).filter(|e| e.at >= since).collect();
    if entries.is_empty() {
        return None;
    }
    let (r, g, b) = category.colour;
    let (_, _, _, week) = turkey_now();
    let lang = settings.target_language().unwrap_or("tr");
    let mut text = None;
    if settings.ai_details {
        let key = format!("haber:haftaozet:{}:{lang}:{week}", category.key);
        if let Ok(Some(cached)) = redis::cmd("GET").arg(&key).query_async::<Option<String>>(&mut conn).await {
            text = Some(cached);
        } else {
            let list: String = entries.iter().take(120).map(|e| format!("- {} ({})\n", e.title, e.source)).collect();
            let language = native_name(lang);
            let request = format!(
                "Bu hafta \"{}\" alanında çıkan haber başlıkları aşağıda. Haftanın en önemli 5-7 gelişmesini seç ve her birini \
                 \"• **kısa başlık** — 1-2 cümle açıklama\" biçiminde {language} dilinde yaz. Başka dillerdeki başlıkları {language} diline çevir. \
                 Yalnızca listede olan bilgileri kullan, aynı olayı tekrar etme.\n\n{list}",
                category.name
            );
            let system = format!("Sen haftalık özet hazırlayan tarafsız bir haber editörüsün. Cevabı tamamen {language} dilinde yaz.");
            if let Ok(Some(Ok(summary))) = tokio::time::timeout(Duration::from_secs(60), crate::ai::complete_once(&news.ai, &system, &request)).await {
                let summary: String = summary.trim().chars().take(3500).collect();
                if !summary.is_empty() {
                    let _: redis::RedisResult<()> = redis::cmd("SET").arg(&key).arg(&summary).arg("EX").arg(8 * 86_400).query_async(&mut conn).await;
                    text = Some(summary);
                }
            }
        }
    }
    // Yapay zekâ yoksa: en yeni 10 başlık.
    let description = text.unwrap_or_else(|| {
        entries.iter().take(10).map(|e| format!("• [{}]({}) — {}", e.title.chars().take(120).collect::<String>(), e.link, e.source)).collect::<Vec<_>>().join("\n")
    });
    Some(
        CreateEmbed::new()
            .title(format!("{} Haftanın {} özeti", category.emoji, category.name.to_lowercase()))
            .description(description.chars().take(4000).collect::<String>())
            .colour(serenity::Colour::from_rgb(r, g, b))
            .footer(serenity::CreateEmbedFooter::new(format!("Son 7 günde {} haber · AxBot haberler", entries.len()))),
    )
}

/// Kategorinin, sunucunun süzgeçlerinden geçen en yeni haberi (komutla deneme için).
pub async fn latest(category: &Category, settings: &NewsSettings) -> anyhow::Result<(&'static str, NewsItem)> {
    let mut best: Option<(&'static str, NewsItem)> = None;
    for source in category.sources {
        if !settings.source_languages.is_empty() && !settings.source_languages.iter().any(|l| l == source.lang) {
            continue;
        }
        let Ok(items) = read_source(source).await else { continue };
        if let Some(item) = items.into_iter().filter(|i| is_suitable(i) && settings.allows(i)).max_by_key(|i| i.published) {
            if best.as_ref().is_none_or(|(_, b)| item.published > b.published) {
                best = Some((source.name, item));
            }
        }
    }
    best.ok_or_else(|| anyhow::anyhow!("seçilen dil ve süzgeçlerle uygun haber bulunamadı"))
}

/// Komutta kullanıcı bekliyor: yapay zekâ yavaşsa haber olduğu gibi gösterilir.
pub async fn render_for_command(news: &NewsContext, item: &NewsItem, settings: &NewsSettings) -> Rendered {
    render(news, item, settings.target_language(), settings.ai_details && settings.style == Style::Full, Duration::from_secs(10)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;

    const RSS: &str = r#"<?xml version="1.0"?><rss><channel><title>Site</title>
        <item><title><![CDATA[Yeni telefon &amp; tablet tanıtıldı]]></title><link>https://ornek.com/a?x=1&amp;y=2</link>
        <guid>a1</guid><pubDate>Mon, 12 Oct 2026 09:00:00 +0300</pubDate>
        <description><![CDATA[<p><img src="https://ornek.com/r.jpg"/>Yeni telefon &amp; tablet bugün tanıtıldı. Fiyatı belli oldu.</p><p>The post X appeared first on Site.</p>]]></description>
        </item>
        <item><title>İkinci</title><link>https://ornek.com/b</link><media:content url="https://ornek.com/m.jpg" medium="image"/></item>
        </channel></rss>"#;

    fn item(title: &str, lang: &str) -> NewsItem {
        NewsItem { id: title.to_owned(), title: title.to_owned(), link: String::new(), excerpt: String::new(), body: String::new(), image: None, published: 0, lang: lang.to_owned(), extra: None }
    }

    #[test]
    fn parses_rich_items() -> anyhow::Result<()> {
        let items = parse(RSS)?;
        assert_eq!(items.len(), 2);
        let a = &items[0];
        assert_eq!(a.title, "Yeni telefon & tablet tanıtıldı");
        assert_eq!(a.link, "https://ornek.com/a?x=1&y=2");
        assert_eq!(a.image.as_deref(), Some("https://ornek.com/r.jpg"));
        assert!(a.excerpt.starts_with("Yeni telefon & tablet bugün tanıtıldı.") && !a.excerpt.contains("appeared first"));
        assert!(a.published > 0);
        assert_eq!(items[1].image.as_deref(), Some("https://ornek.com/m.jpg"));
        Ok(())
    }

    #[test]
    fn parses_gamerpower() -> anyhow::Result<()> {
        let json = r#"[{"id":7,"title":"Cool Game (Epic Games) Giveaway","worth":"$19.99","image":"https://img/x.jpg","description":"Grab it free.","open_giveaway_url":"https://www.gamerpower.com/open/7","published_date":"2026-10-12 10:00:00","end_date":"2026-10-19 23:59:00","platforms":"PC, Epic Games Store"}]"#;
        let items = parse_gamerpower(json)?;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Cool Game (Epic Games)");
        assert!(items[0].extra.as_deref().is_some_and(|e| e.contains("$19.99") && e.contains("<t:")));
        Ok(())
    }

    #[test]
    fn decodes_numeric_entities() {
        assert_eq!(clean("&#34;Carrie&#34; &#x27;x&#x27; &amp;#8217; A &# B"), "\"Carrie\" 'x' ’ A &# B");
    }

    #[test]
    fn unsuitable_titles_are_skipped() {
        assert!(!is_suitable(&item("Bu Sefer de 4K ÇIPLAK Videosu Sızdı", "tr")));
        assert!(is_suitable(&item("Yeni ekran kartı tanıtıldı", "tr")));
    }

    #[test]
    fn channel_selection() {
        let mut s = NewsSettings { enabled: true, channel: Some(5), categories: CATEGORIES.iter().map(|c| c.key.to_owned()).collect(), ..NewsSettings::default() };
        s.channels.insert("oyun".to_owned(), 9);
        assert_eq!(s.channel_for("oyun"), Some(ChannelId::new(9)));
        assert_eq!(s.channel_for("teknoloji"), Some(ChannelId::new(5)));
        s.categories.retain(|c| c != "dizi");
        assert_eq!(s.channel_for("dizi"), None);
        // Hiç kategori seçilmemişse hepsi açık.
        s.categories.clear();
        assert_eq!(s.channel_for("dizi"), Some(ChannelId::new(5)));
        s.enabled = false;
        assert!(!s.is_active());
    }

    #[test]
    fn language_and_keyword_filters() {
        let mut s = NewsSettings { source_languages: vec!["tr".to_owned()], ..NewsSettings::default() };
        assert!(s.allows(&item("Yeni iPhone tanıtıldı", "tr")));
        assert!(!s.allows(&item("New iPhone announced", "en")));
        s.source_languages.clear();
        s.include_keywords = vec!["iphone".to_owned()];
        assert!(s.allows(&item("New IPHONE announced", "en")));
        assert!(!s.allows(&item("Yeni ekran kartı", "tr")));
        s.include_keywords.clear();
        s.exclude_keywords = vec!["sızıntı".to_owned()];
        assert!(!s.allows(&item("GTA 6 SIZINTI iddiası", "tr")));
    }

    #[test]
    fn similar_titles_are_duplicates() {
        assert!(similarity("Apple yeni iPhone 18 modelini tanıttı", "Apple, iPhone 18 modelini resmen tanıttı") >= DUPLICATE_SIMILARITY);
        assert!(similarity("Apple yeni iPhone 18 modelini tanıttı", "Samsung katlanabilir telefonunu duyurdu") < DUPLICATE_SIMILARITY);
    }

    #[test]
    fn ai_answer_is_split_into_title_and_summary() {
        let r = split_answer("**Başlık: Yeni dizi duyuruldu**\n\nNetflix yeni bir dizi duyurdu.\nÇekimler başladı.", true, "tr");
        assert!(r.as_ref().is_some_and(|r| r.title == "Yeni dizi duyuruldu" && r.translated_to == "tr"));
        assert!(r.is_some_and(|r| r.summary.is_some_and(|s| s.starts_with("Netflix"))));
        assert!(split_answer("   ", false, "tr").is_none());
    }

    #[test]
    fn long_excerpts_are_cut_on_a_word() {
        let long = "kelime ".repeat(100);
        let e = excerpt(&long, "Başlık");
        assert!(e.ends_with('…') && e.chars().count() <= EXCERPT_CHARS + 1);
    }

    #[test]
    fn every_source_language_is_known() {
        for category in CATEGORIES {
            for source in category.sources {
                assert!(language(source.lang).is_some(), "{}: bilinmeyen dil", source.name);
            }
        }
    }

    /// Google Çeviri yedeği çalışıyor mu? `cargo test live_machine_translate -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "Google Çeviri'ye istek atar"]
    async fn live_machine_translate() -> anyhow::Result<()> {
        let started = std::time::Instant::now();
        let out = machine_translate("Selma Blair Arrested After Allegedly Biting Flight Attendant", "tr").await;
        println!("{} ms: {out:?}", started.elapsed().as_millis());
        anyhow::ensure!(out.is_some_and(|t| t.contains("Tutuklandı") || t.contains("tutuklandı")), "çeviri yok");
        Ok(())
    }

    /// Gerçek kaynakların hepsi okunabiliyor mu? `cargo test live_news_sources -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "haber sitelerine istek atar"]
    async fn live_news_sources() -> anyhow::Result<()> {
        for category in CATEGORIES {
            for source in category.sources {
                let items = read_source(source).await.with_context(|| format!("{} okunamadı", source.name))?;
                let with_image = items.iter().filter(|i| i.image.is_some()).count();
                let first = items.first().map(|i| i.title.chars().take(60).collect::<String>()).unwrap_or_default();
                println!("{}/{} [{}]: {} haber, {with_image} görselli · {first}", category.key, source.name, source.lang, items.len());
                anyhow::ensure!(!items.is_empty(), "{}: haber yok", source.name);
            }
        }
        Ok(())
    }

    /// Yapay zekâ İngilizce bir dizi haberini Türkçeye çevirip özetleyebiliyor mu?
    /// `cargo test live_news_ai_summary -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "haber sitesine, Redis'e ve yapay zekâya istek atar"]
    async fn live_news_ai_summary() -> anyhow::Result<()> {
        let _ = dotenvy::dotenv();
        let config = crate::config::Config::from_env()?;
        let ai = crate::ai::Ai::new(config.ai, crate::net::http_client()?);
        let redis = crate::cache::connect(&config.redis_url).await?;
        let source = &CATEGORIES[3].sources[0];
        let item = read_source(source).await?.into_iter().next().ok_or_else(|| anyhow::anyhow!("haber yok"))?;
        println!("{}: {}\n", source.name, item.title);
        let started = std::time::Instant::now();
        let target = Some("tr");
        let lang = "tr";
        let text = if item.body.len() > item.excerpt.len() { &item.body } else { &item.excerpt };
        let request = format!("Aşağıdaki haberi {} dilinde yaz.\n1. satıra YALNIZCA haberin başlığını yaz (çevirerek).\nSonraki satırlara haberi 3-5 cümlelik akıcı bir özetle anlat.\n\nBaşlık: {}\n\nMetin: {}", native_name(lang), item.title, text.chars().take(3000).collect::<String>());
        let answer = crate::ai::complete_once(&ai, "Sen haber editörüsün. Cevabı tamamen Türkçe yaz.", &request).await.ok_or_else(|| anyhow::anyhow!("yapay zekâ kapalı"))??;
        let rendered = split_answer(&answer, target.is_some(), lang).ok_or_else(|| anyhow::anyhow!("cevap boş"))?;
        println!("({} sn)\nBAŞLIK: {}\nÖZET: {:?}", started.elapsed().as_secs(), rendered.title, rendered.summary);
        drop(redis);
        Ok(())
    }
}
