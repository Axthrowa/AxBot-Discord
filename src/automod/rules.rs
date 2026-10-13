//! Otomatik moderasyon kuralları: kural başına eylem, eşikler, muaf roller, kısıtlı kanallar ve
//! denetim kaydı seçenekleri. Ayarlar `PluginSettings` deposunda tek kayıt olarak durur.
//!
//! Buradaki denetimler senkron ve I/O'suzdur (tekrarlanan yazı sayacı hariç; o Redis'te).

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::config::AutoModSettings;
use crate::settings::PluginConfig;

/// Bir kural tetiklenince yapılacak iş.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    #[default]
    Off,
    Delete,
    Warn,
    DeleteWarn,
}

impl RuleAction {
    pub const ALL: [(Self, &'static str, &'static str); 4] = [
        (Self::Off, "off", "Devre dışı"),
        (Self::Delete, "delete", "Mesajı sil"),
        (Self::Warn, "warn", "Üyeyi uyar"),
        (Self::DeleteWarn, "delete_warn", "Mesajı sil + üyeyi uyar"),
    ];

    pub fn code(self) -> &'static str {
        Self::ALL.iter().find(|(a, _, _)| *a == self).map_or("off", |(_, code, _)| code)
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, c, _)| *c == code).map(|(a, _, _)| *a)
    }

    pub fn deletes(self) -> bool {
        matches!(self, Self::Delete | Self::DeleteWarn)
    }

    pub fn warns(self) -> bool {
        matches!(self, Self::Warn | Self::DeleteWarn)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    BadWords,
    Repeated,
    Invites,
    Links,
    Caps,
    Emojis,
    Spoilers,
    Mentions,
    Zalgo,
    Spam,
}

impl Rule {
    /// `(kural, form anahtarı, ad, açıklama)` — paneldeki sırayla.
    pub const ALL: [(Self, &'static str, &'static str, &'static str); 10] = [
        (Self::BadWords, "bad_words", "Yasaklı kelimeler", "Küfür ve hakaret listesi ile sunucunun eklediği kelimeler."),
        (Self::Repeated, "repeated", "Tekrarlanan yazı", "Aynı mesajın kısa sürede art arda gönderilmesi."),
        (Self::Invites, "invites", "Sunucu davetleri", "Başka Discord sunucularına davet bağlantıları."),
        (Self::Links, "links", "Harici bağlantılar", "İzin verilen alan adları dışındaki tüm bağlantılar."),
        (Self::Caps, "caps", "Aşırı büyük harf", "Mesajın büyük bölümü BÜYÜK HARFLE yazılmışsa."),
        (Self::Emojis, "emojis", "Aşırı emoji", "Tek mesajda çok fazla emoji."),
        (Self::Spoilers, "spoilers", "Aşırı spoiler", "Tek mesajda çok fazla ||spoiler||."),
        (Self::Mentions, "mentions", "Aşırı bahsetme", "Tek mesajda çok fazla @etiket."),
        (Self::Zalgo, "zalgo", "Zalgo", "Üst üste binmiş işaretlerle bozulmuş (z̷a̷l̷g̷o̷) yazı."),
        (Self::Spam, "spam", "Spam koruması", "Kısa sürede çok fazla mesaj."),
    ];

    /// Eski ayar tablosunda (`automod_settings`) açma/kapama anahtarı olan kurallar.
    pub fn is_legacy(self) -> bool {
        matches!(self, Self::BadWords | Self::Invites | Self::Links | Self::Spam)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuleActions {
    pub bad_words: RuleAction,
    pub repeated: RuleAction,
    pub invites: RuleAction,
    pub links: RuleAction,
    pub caps: RuleAction,
    pub emojis: RuleAction,
    pub spoilers: RuleAction,
    pub mentions: RuleAction,
    pub zalgo: RuleAction,
    pub spam: RuleAction,
}

impl RuleActions {
    pub fn get(&self, rule: Rule) -> RuleAction {
        match rule {
            Rule::BadWords => self.bad_words,
            Rule::Repeated => self.repeated,
            Rule::Invites => self.invites,
            Rule::Links => self.links,
            Rule::Caps => self.caps,
            Rule::Emojis => self.emojis,
            Rule::Spoilers => self.spoilers,
            Rule::Mentions => self.mentions,
            Rule::Zalgo => self.zalgo,
            Rule::Spam => self.spam,
        }
    }

    pub fn set(&mut self, rule: Rule, action: RuleAction) {
        let slot = match rule {
            Rule::BadWords => &mut self.bad_words,
            Rule::Repeated => &mut self.repeated,
            Rule::Invites => &mut self.invites,
            Rule::Links => &mut self.links,
            Rule::Caps => &mut self.caps,
            Rule::Emojis => &mut self.emojis,
            Rule::Spoilers => &mut self.spoilers,
            Rule::Mentions => &mut self.mentions,
            Rule::Zalgo => &mut self.zalgo,
            Rule::Spam => &mut self.spam,
        };
        *slot = action;
    }
}

/// Kısıtlı kanalda izin verilen tek içerik türü.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestrictKind {
    Commands,
    Images,
    Videos,
}

impl RestrictKind {
    pub const ALL: [(Self, &'static str, &'static str); 3] = [
        (Self::Commands, "commands", "Sadece komutlar"),
        (Self::Images, "images", "Sadece resimler"),
        (Self::Videos, "videos", "Sadece videolar"),
    ];

    pub fn code(self) -> &'static str {
        Self::ALL.iter().find(|(k, _, _)| *k == self).map_or("commands", |(_, code, _)| code)
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, c, _)| *c == code).map(|(k, _, _)| *k)
    }

    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|(k, _, _)| *k == self).map_or("", |(_, _, label)| label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restricted {
    pub channel: u64,
    pub kind: RestrictKind,
}

/// Denetim kaydına yazılabilen olaylar: `(anahtar, grup, ad)`.
pub const LOG_EVENTS: [(&str, &str, &str); 21] = [
    ("mod_mute", "Moderatör olayları", "Üye susturuldu"),
    ("mod_unmute", "Moderatör olayları", "Üyenin susturması kaldırıldı"),
    ("mod_ban", "Moderatör olayları", "Moderatör yasakladı / attı / uyardı"),
    ("mod_unban", "Moderatör olayları", "Moderatör yasağı kaldırdı"),
    ("message_edit", "Mesaj olayları", "Mesaj düzenlendi"),
    ("message_delete", "Mesaj olayları", "Mesaj silindi"),
    ("invite_posted", "Mesaj olayları", "Davet bağlantısı paylaşıldı"),
    ("nickname", "Üye olayları", "Takma ad değişti"),
    ("member_ban", "Üye olayları", "Üye yasaklandı"),
    ("member_join", "Üye olayları", "Üye sunucuya katıldı"),
    ("member_leave", "Üye olayları", "Üye sunucudan ayrıldı"),
    ("member_unban", "Üye olayları", "Üyenin yasağı kaldırıldı"),
    ("member_roles", "Rol olayları", "Üyenin rolleri değişti"),
    ("role_create", "Rol olayları", "Rol oluşturuldu"),
    ("role_update", "Rol olayları", "Rol güncellendi"),
    ("role_delete", "Rol olayları", "Rol silindi"),
    ("voice", "Ses olayları", "Sesli kanala katılma / ayrılma / geçiş"),
    ("guild_update", "Sunucu olayları", "Sunucu düzenlendi"),
    ("emoji_update", "Sunucu olayları", "Emojiler güncellendi"),
    ("channel_create", "Kanal olayları", "Kanal oluşturuldu / silindi"),
    ("channel_update", "Kanal olayları", "Kanal güncellendi"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogSettings {
    /// Kapatılmış olay anahtarları (yeni eklenen olaylar varsayılan olarak açık gelsin diye "kapalılar" tutulur).
    pub disabled: Vec<String>,
    /// Bu kanallardaki mesaj düzenleme/silme olayları yazılmaz.
    pub ignored_channels: Vec<u64>,
    /// Botların yaptığı işlemler yazılmaz.
    pub skip_bots: bool,
    /// Kayıtlarda profil resmi gösterilmez.
    pub hide_avatars: bool,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self { disabled: Vec::new(), ignored_channels: Vec::new(), skip_bots: true, hide_avatars: false }
    }
}

impl LogSettings {
    pub fn logs(&self, event: &str) -> bool {
        !self.disabled.iter().any(|d| d == event)
    }

    pub fn ignores(&self, channel: u64) -> bool {
        self.ignored_channels.contains(&channel)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModSettings {
    pub actions: RuleActions,
    /// Büyük harf oranı (yüzde) ve en az harf sayısı.
    pub caps_percent: u8,
    pub caps_min_letters: u16,
    pub emoji_limit: u16,
    pub spoiler_limit: u16,
    pub mention_limit: u16,
    /// Aynı mesaj bir dakikada bu kadar kez gönderilirse "tekrarlanan yazı".
    pub repeat_limit: u16,
    /// Harici bağlantı kuralında izin verilen alan adları (alt alan adları dahil).
    pub allowed_domains: Vec<String>,
    /// Otomatik moderasyondan etkilenmeyen roller.
    pub exempt_roles: Vec<u64>,
    pub restricted: Vec<Restricted>,
    /// Kısıtlı kanalda mesaj silinince yazara özelden haber ver.
    pub restricted_dm: bool,
    /// Kısıtlı kanalda mesaj silinince kanalda kısa bir uyarı göster.
    pub restricted_reply: bool,
    pub log: LogSettings,
    /// Yasaklı kelime içeren takma adları değiştir.
    pub nick_filter: bool,
    /// Listede üste çıkmak için başa konan işaretleri (!, ., 0…) temizle.
    pub dehoist: bool,
    /// Ad tamamen temizlenirse ya da küfürlüyse verilecek takma ad.
    pub nick_replacement: String,
}

impl Default for ModSettings {
    fn default() -> Self {
        Self {
            actions: RuleActions::default(),
            caps_percent: 70,
            caps_min_letters: 10,
            emoji_limit: 6,
            spoiler_limit: 3,
            mention_limit: 5,
            repeat_limit: 3,
            allowed_domains: Vec::new(),
            exempt_roles: Vec::new(),
            restricted: Vec::new(),
            restricted_dm: false,
            restricted_reply: true,
            log: LogSettings::default(),
            nick_filter: false,
            dehoist: false,
            nick_replacement: "Üye".to_owned(),
        }
    }
}

impl PluginConfig for ModSettings {
    const KEY: &'static str = "moderation";
}

impl ModSettings {
    /// Kuralın geçerli eylemi. Eski dört kuralın açık/kapalı anahtarı `automod_settings`
    /// tablosundadır (`/automod ayarla`); açıkken eylem seçilmemişse "mesajı sil" uygulanır.
    pub fn action(&self, rule: Rule, legacy: &AutoModSettings) -> RuleAction {
        let stored = self.actions.get(rule);
        let legacy_on = match rule {
            Rule::BadWords => legacy.filter_profanity,
            Rule::Invites => legacy.block_invites,
            Rule::Links => legacy.block_links,
            Rule::Spam => legacy.anti_spam,
            _ => return stored,
        };
        match (legacy_on, stored) {
            (false, _) => RuleAction::Off,
            (true, RuleAction::Off) => RuleAction::Delete,
            (true, action) => action,
        }
    }

    pub fn restriction(&self, channel: u64) -> Option<RestrictKind> {
        self.restricted.iter().find(|r| r.channel == channel).map(|r| r.kind)
    }
}

/// Açılışta bir kez derlenen kalıplar.
pub struct RulePatterns {
    custom_emoji: Regex,
    mention: Regex,
    link_host: Regex,
    image_link: Regex,
    video_link: Regex,
}

impl RulePatterns {
    pub fn new() -> Result<Self, regex::Error> {
        Ok(Self {
            custom_emoji: Regex::new(r"<a?:\w{2,32}:\d{15,21}>")?,
            mention: Regex::new(r"<@[!&]?\d{15,21}>|@everyone|@here")?,
            link_host: Regex::new(r"(?i)\b(?:https?://|www\.)([^\s/?#<>]+)")?,
            image_link: Regex::new(r"(?i)https?://\S+\.(?:png|jpe?g|gif|webp)(?:\?\S*)?(?:\s|$)|https?://(?:tenor\.com|giphy\.com|media\.giphy\.com|i\.imgur\.com)/\S+")?,
            video_link: Regex::new(
                r"(?i)https?://\S+\.(?:mp4|mov|webm|mkv)(?:\?\S*)?(?:\s|$)|https?://(?:www\.)?(?:youtube\.com/(?:watch|shorts)|youtu\.be/|clips\.twitch\.tv/|(?:www\.)?twitch\.tv/\S+/clip/|medal\.tv/|streamable\.com/)\S*",
            )?,
        })
    }

    /// Unicode ve sunucu emojilerinin toplam sayısı.
    pub fn count_emojis(&self, content: &str) -> usize {
        let custom = self.custom_emoji.find_iter(content).count();
        let stripped = self.custom_emoji.replace_all(content, "");
        custom + stripped.chars().filter(|c| is_emoji(*c)).count()
    }

    pub fn count_mentions(&self, content: &str) -> usize {
        self.mention.find_iter(content).count()
    }

    /// İzin verilmeyen bir bağlantı varsa `true`.
    pub fn has_blocked_link(&self, content: &str, allowed_domains: &[String]) -> bool {
        self.link_host.captures_iter(content).any(|capture| {
            let host = capture.get(1).map_or("", |m| m.as_str()).to_lowercase();
            let host = host.split(':').next().unwrap_or_default().trim_start_matches("www.").trim_end_matches('.').to_owned();
            !allowed_domains.iter().any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
        })
    }

    pub fn has_image_link(&self, content: &str) -> bool {
        self.image_link.is_match(content)
    }

    pub fn has_video_link(&self, content: &str) -> bool {
        self.video_link.is_match(content)
    }
}

/// Mesajın eklerinden çıkarılan özet (kısıtlı kanal denetimi için).
#[derive(Debug, Clone, Copy, Default)]
pub struct Media {
    pub images: usize,
    pub videos: usize,
}

impl Media {
    /// Ek dosyasının türünü içerik türünden, yoksa uzantısından anlar.
    pub fn add(&mut self, content_type: Option<&str>, filename: &str) {
        let name = filename.to_lowercase();
        let ext = name.rsplit('.').next().unwrap_or_default();
        let kind = content_type.unwrap_or_default();
        if kind.starts_with("image/") || matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "webp") {
            self.images += 1;
        } else if kind.starts_with("video/") || matches!(ext, "mp4" | "mov" | "webm" | "mkv") {
            self.videos += 1;
        }
    }
}

/// Kısıtlı kanalda bu mesaja izin var mı?
pub fn allowed_in_restricted(patterns: &RulePatterns, kind: RestrictKind, content: &str, media: Media, command_prefix: char) -> bool {
    match kind {
        // Eğik çizgi komutları mesaj üretmez; burada yalnızca önekli komutlar (etiketler) kalır.
        RestrictKind::Commands => content.trim_start().starts_with(command_prefix),
        RestrictKind::Images => media.images > 0 || patterns.has_image_link(content),
        RestrictKind::Videos => media.videos > 0 || patterns.has_video_link(content),
    }
}

/// Büyük harf oranı eşiği aşıyor mu? Kısa mesajlar ("TAMAM") sayılmaz.
pub fn too_many_caps(content: &str, percent: u8, min_letters: u16) -> bool {
    let (mut letters, mut upper) = (0usize, 0usize);
    for c in content.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        if c.is_uppercase() {
            upper += 1;
        }
    }
    letters >= usize::from(min_letters) && upper * 100 >= letters * usize::from(percent)
}

pub fn count_spoilers(content: &str) -> usize {
    content.matches("||").count() / 2
}

/// Zalgo: bir harfin üstüne yığılmış 4+ birleşen işaret ya da metnin yarısından fazlası işaret.
pub fn is_zalgo(content: &str) -> bool {
    let (mut run, mut longest, mut marks, mut base) = (0usize, 0usize, 0usize, 0usize);
    for c in content.chars() {
        if is_combining(c) {
            run += 1;
            marks += 1;
            longest = longest.max(run);
        } else {
            run = 0;
            if !c.is_whitespace() {
                base += 1;
            }
        }
    }
    longest >= 4 || (marks >= 8 && marks > base)
}

fn is_combining(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}' | '\u{0483}'..='\u{0489}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}'
        | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}')
}

fn is_emoji(c: char) -> bool {
    matches!(c, '\u{1F300}'..='\u{1FAFF}' | '\u{1F000}'..='\u{1F2FF}' | '\u{2600}'..='\u{27BF}' | '\u{2B00}'..='\u{2BFF}'
        | '\u{2300}'..='\u{23FF}' | '\u{2190}'..='\u{21FF}' | '\u{3030}' | '\u{303D}' | '\u{3297}' | '\u{3299}' | '\u{00A9}' | '\u{00AE}')
}

/// Alan adı listesini temizler: şema, yol ve `www.` atılır; geçersizler elenir.
pub fn clean_domains(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = raw
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(|entry| {
            let entry = entry.trim().to_lowercase();
            let entry = entry.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
            entry.split(['/', '?', '#']).next().unwrap_or_default().trim_matches('.').to_owned()
        })
        .filter(|d| d.len() >= 3 && d.len() <= 100 && d.contains('.') && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_needs_enough_letters() {
        assert!(too_many_caps("BU MESAJ ÇOK BAĞIRIYOR", 70, 10));
        assert!(!too_many_caps("TAMAM", 70, 10));
        assert!(!too_many_caps("Normal bir cümle yazıyorum burada", 70, 10));
        // Rakam ve noktalama harf sayılmaz.
        assert!(!too_many_caps("12345 67890 !!!! ????", 70, 10));
    }

    #[test]
    fn counts_emojis_spoilers_and_mentions() -> anyhow::Result<()> {
        let patterns = RulePatterns::new()?;
        assert_eq!(patterns.count_emojis("selam 😀😀 <:pepe:123456789012345678> <a:dans:123456789012345678>"), 4);
        assert_eq!(patterns.count_emojis("emoji yok 123"), 0);
        assert_eq!(count_spoilers("||a|| ||b|| düz ||c||"), 3);
        assert_eq!(count_spoilers("tek || çizgi"), 0);
        assert_eq!(patterns.count_mentions("<@123456789012345678> <@!123456789012345678> <@&123456789012345678> @everyone @here"), 5);
        Ok(())
    }

    #[test]
    fn zalgo_is_detected_but_accents_are_not() {
        assert!(is_zalgo("z\u{0335}\u{0321}\u{0322}\u{0327}a\u{0336}lgo"));
        assert!(!is_zalgo("çığ öğüt şükür İstanbul"));
        // Vietnamca gibi iki işaretli harfler zalgo değildir.
        assert!(!is_zalgo("Vie\u{0302}\u{0323}t Nam"));
    }

    #[test]
    fn allowed_domains_cover_subdomains_only() -> anyhow::Result<()> {
        let patterns = RulePatterns::new()?;
        let allowed = clean_domains("https://www.YouTube.com/watch, tenor.com\nbozuk giriş");
        assert_eq!(allowed, vec!["tenor.com".to_owned(), "youtube.com".to_owned()]);
        assert!(!patterns.has_blocked_link("bak https://youtube.com/watch?v=1 ve www.m.youtube.com/x", &allowed));
        assert!(patterns.has_blocked_link("https://kotusite.com/a", &allowed));
        // Sonuna izinli ad eklenmiş sahte alan adı engellenir.
        assert!(patterns.has_blocked_link("https://youtube.com.kotusite.com", &allowed));
        assert!(patterns.has_blocked_link("https://fakeyoutube.com", &allowed));
        assert!(!patterns.has_blocked_link("bağlantı yok", &allowed));
        Ok(())
    }

    #[test]
    fn legacy_switch_decides_on_off_and_stored_action_decides_how() {
        let mut settings = ModSettings::default();
        let mut legacy = AutoModSettings::default();
        // Eski anahtar açık, eylem seçilmemiş: eskisi gibi "mesajı sil".
        assert_eq!(settings.action(Rule::Invites, &legacy), RuleAction::Delete);
        settings.actions.set(Rule::Invites, RuleAction::DeleteWarn);
        assert_eq!(settings.action(Rule::Invites, &legacy), RuleAction::DeleteWarn);
        legacy.block_invites = false;
        assert_eq!(settings.action(Rule::Invites, &legacy), RuleAction::Off);
        // Yeni kurallar yalnızca kendi eylemine bakar.
        assert_eq!(settings.action(Rule::Caps, &legacy), RuleAction::Off);
        settings.actions.set(Rule::Caps, RuleAction::Warn);
        assert_eq!(settings.action(Rule::Caps, &legacy), RuleAction::Warn);
        assert!(RuleAction::DeleteWarn.deletes() && RuleAction::DeleteWarn.warns() && !RuleAction::Warn.deletes());
    }

    #[test]
    fn restricted_channels() -> anyhow::Result<()> {
        let patterns = RulePatterns::new()?;
        let mut image = Media::default();
        image.add(Some("image/png"), "a.png");
        let mut video = Media::default();
        video.add(None, "KLIP.MP4");
        assert!(allowed_in_restricted(&patterns, RestrictKind::Images, "", image, '!'));
        assert!(!allowed_in_restricted(&patterns, RestrictKind::Images, "sadece yazı", Media::default(), '!'));
        assert!(allowed_in_restricted(&patterns, RestrictKind::Images, "https://media.giphy.com/media/x/giphy.gif", Media::default(), '!'));
        assert!(allowed_in_restricted(&patterns, RestrictKind::Videos, "", video, '!'));
        assert!(allowed_in_restricted(&patterns, RestrictKind::Videos, "https://youtu.be/abc", Media::default(), '!'));
        assert!(!allowed_in_restricted(&patterns, RestrictKind::Videos, "", image, '!'));
        assert!(allowed_in_restricted(&patterns, RestrictKind::Commands, "!kurallar", Media::default(), '!'));
        assert!(!allowed_in_restricted(&patterns, RestrictKind::Commands, "sohbet", Media::default(), '!'));
        Ok(())
    }

    #[test]
    fn old_records_load_with_defaults() -> anyhow::Result<()> {
        let settings: ModSettings = serde_json::from_str("{\"emoji_limit\":9,\"actions\":{\"caps\":\"delete_warn\"}}")?;
        assert_eq!(settings.emoji_limit, 9);
        assert_eq!(settings.actions.caps, RuleAction::DeleteWarn);
        assert_eq!(settings.caps_percent, 70);
        assert!(settings.log.logs("message_delete") && settings.restricted_reply);
        assert_eq!(RuleAction::from_code(RuleAction::Warn.code()), Some(RuleAction::Warn));
        assert_eq!(RestrictKind::from_code(RestrictKind::Videos.code()), Some(RestrictKind::Videos));
        Ok(())
    }
}
