//! Derlenmiş regex filtreleri. Buradaki her şey senkron ve I/O içermez;
//! mesaj başına kontrol mikro saniyeler mertebesindedir.
//!
//! Kelime kalıbı sözdizimi (varsayılan liste ve `/automod kelime-ekle` için ortak):
//! - `kelime`   → sadece tam kelime
//! - `kelime*`  → kelimeyle başlayan her şey (ekler ve bitişik yazımlar: "sikkafalı")
//! - `*kelime*` → kelimenin içinde geçtiği her şey ("motherfucker")
//!
//! Harf tekrarları her zaman yakalanır ("siiiiktir"), `i`/`ı` birbirinin yerine geçer.

use std::collections::HashSet;

use regex::Regex;

use super::config::{AutoModConfig, AutoModSettings};

/// Varsayılan Türkçe küfür/hakaret listesi.
const DEFAULT_WORDS_TR: &[&str] = &[
    "sik*", "*siktir*", "*sikeyim*", "*sikerim*", "*siktiğim*", "amk*", "amq", "aq",
    "amına*", "amcık*", "*orospu*", "oç", "yarrak*", "yarak*", "dalyarak*", "dallama*",
    "pezevenk*", "kahpe*", "piç*", "ibne*", "gavat*", "yavşak*", "şerefsiz*", "puşt*",
    "kaltak*", "sürtük*", "göt*", "gerizekalı*",
];

/// Varsayılan İngilizce küfür/hakaret listesi.
const DEFAULT_WORDS_EN: &[&str] = &[
    "*fuck*", "fck*", "*shit*", "*bitch*", "*cunt*", "*nigger*", "*nigga*", "*faggot*",
    "fag", "fags", "*whore*", "*slut*", "*asshole*", "ass", "dick", "dicks", "*dickhead*",
    "pussy*", "cock", "cocks", "*cocksucker*", "bastard*", "wanker*", "twat*",
];

/// Kalıplara takılan ama masum olan kelimeler. `*` ile biten giriş önek, diğerleri tam kelime.
/// Noktasız "sık-" ile başlayan kelimeler Türkçe'de masumdur (sık, sıkıldım, sıkıntı);
/// ASCII klavyeyle yazılmış karşılıkları da ayrıca listelenir.
const ALLOWED_WORDS: &[&str] = &[
    "sık*", "siki", "sikil*", "sikint*", "sikici*", "sikist*", "sikisik*", "sikca", "sikma*",
    "siklik*", "sikke*", "sikkin*", "götür*", "scunthorpe", "shiitake*", "shitake*",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    Invite,
    Link,
    Profanity,
    /// `first` yalnızca limitin aşıldığı ilk mesajda `true`; uyarı bir kez gönderilir.
    Spam { first: bool },
    Repeated,
    Caps,
    Emojis,
    Spoilers,
    Mentions,
    Zalgo,
}

impl Violation {
    /// Loglarda kullanılan kısa sebep.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Invite => "davet linki",
            Self::Link => "link",
            Self::Profanity => "küfür/hakaret",
            Self::Spam { .. } => "spam",
            Self::Repeated => "tekrarlanan yazı",
            Self::Caps => "aşırı büyük harf",
            Self::Emojis => "aşırı emoji",
            Self::Spoilers => "aşırı spoiler",
            Self::Mentions => "aşırı bahsetme",
            Self::Zalgo => "zalgo",
        }
    }

    /// Kullanıcıya kanalda gösterilen uyarı.
    pub fn notice(self) -> &'static str {
        match self {
            Self::Invite => "bu sunucuda başka sunuculara davet linki paylaşmak yasak.",
            Self::Link => "bu sunucuda link paylaşmak yasak.",
            Self::Profanity => "lütfen küfür ve hakaret içermeyen bir dil kullan.",
            Self::Spam { .. } => "çok hızlı mesaj gönderiyorsun, biraz yavaşla.",
            Self::Repeated => "aynı mesajı tekrar tekrar gönderme.",
            Self::Caps => "lütfen mesajın tamamını büyük harfle yazma.",
            Self::Emojis => "tek mesajda bu kadar çok emoji kullanma.",
            Self::Spoilers => "tek mesajda bu kadar çok spoiler kullanma.",
            Self::Mentions => "tek mesajda bu kadar çok kişiyi etiketleme.",
            Self::Zalgo => "lütfen bozuk (zalgo) yazı kullanma.",
        }
    }

    pub fn should_notify(self) -> bool {
        !matches!(self, Self::Spam { first: false })
    }
}

/// Tüm sunucularda ortak, açılışta bir kez derlenen kalıplar.
pub struct BasePatterns {
    invite: Regex,
    #[cfg(test)]
    link: Regex,
    allowed: AllowList,
}

impl BasePatterns {
    pub fn new() -> Result<Self, regex::Error> {
        Ok(Self {
            invite: Regex::new(
                r"(?i)(?:discord(?:app)?\.com/invite|discord\.gg|dsc\.gg|discord\.me)/[a-z0-9-]+",
            )?,
            #[cfg(test)]
            link: Regex::new(r"(?i)\b(?:https?://|www\.)\S+")?,
            allowed: AllowList::new(ALLOWED_WORDS.iter().copied()),
        })
    }

    pub fn has_invite(&self, content: &str) -> bool {
        if self.invite.is_match(content) {
            return true;
        }
        // "discord . gg / abc" gibi boşluklarla gizlenmiş davetler.
        if content.contains(char::is_whitespace) {
            let compact: String = content.chars().filter(|c| !c.is_whitespace()).collect();
            return self.invite.is_match(&compact);
        }
        false
    }
}

struct AllowList {
    exact: HashSet<String>,
    prefixes: Vec<String>,
}

impl AllowList {
    fn new<'a>(entries: impl Iterator<Item = &'a str>) -> Self {
        let mut exact = HashSet::new();
        let mut prefixes = Vec::new();
        for entry in entries {
            match entry.strip_suffix('*') {
                Some(prefix) => prefixes.push(normalize(prefix)),
                None => {
                    exact.insert(normalize(entry));
                }
            }
        }
        Self { exact, prefixes }
    }

    fn contains(&self, word: &str) -> bool {
        self.exact.contains(word) || self.prefixes.iter().any(|p| word.starts_with(p.as_str()))
    }
}

/// Bir sunucunun ayarlarından derlenmiş filtre. L1 önbellekte `Arc` ile paylaşılır.
pub struct GuildFilter {
    pub settings: AutoModSettings,
    words: Option<Regex>,
    /// Sunucunun kendi eklediği istisnalar (varsayılan istisnalar `BasePatterns`'te).
    allowed: AllowList,
}

impl GuildFilter {
    pub fn compile(config: AutoModConfig) -> Result<Self, regex::Error> {
        let words = if config.settings.filter_profanity {
            let all = DEFAULT_WORDS_TR
                .iter()
                .chain(DEFAULT_WORDS_EN)
                .copied()
                .chain(config.custom_words.iter().map(String::as_str));
            build_word_regex(all)?
        } else {
            None
        };

        Ok(Self {
            settings: config.settings,
            words,
            allowed: AllowList::new(config.allowed_words.iter().map(String::as_str)),
        })
    }

    /// Eski üç kuralı (davet, link, küfür) tek seferde dener; yalnızca testler kullanır.
    #[cfg(test)]
    pub fn check(&self, base: &BasePatterns, content: &str) -> Option<Violation> {
        if self.settings.block_invites && base.has_invite(content) {
            return Some(Violation::Invite);
        }
        if self.settings.block_links && base.link.is_match(content) {
            return Some(Violation::Link);
        }
        self.has_profanity(base, content).then_some(Violation::Profanity)
    }

    /// Yasaklı kelime var mı? (Filtre kapalıysa kalıp derlenmemiştir ve hep `false` döner.)
    pub fn has_profanity(&self, base: &BasePatterns, content: &str) -> bool {
        let Some(words) = &self.words else {
            return false;
        };
        let text = normalize(content);
        // Her eşleşme bütün bir kelimedir; istisna listesindeyse atlanır.
        words
            .find_iter(&text)
            .any(|m| !base.allowed.contains(m.as_str()) && !self.allowed.contains(m.as_str()))
    }
}

/// Tüm kalıpları tek bir alternation regex'ine birleştirir; tek geçişte eşleşir.
fn build_word_regex<'a>(words: impl Iterator<Item = &'a str>) -> Result<Option<Regex>, regex::Error> {
    let alternatives: Vec<String> = words.filter_map(word_to_pattern).collect();
    if alternatives.is_empty() {
        return Ok(None);
    }
    // `\b` regex crate'inde Unicode farkındadır (ç, ş, ğ, ı, ö, ü kelime karakteri sayılır).
    // Her alternatif iki ucundan `\b` ile sınırlı olduğu için eşleşmeler hep bütün kelimelerdir.
    Regex::new(&format!(r"\b(?:{})\b", alternatives.join("|"))).map(Some)
}

/// `sik*` → `s+[iı]+k+\w*` gibi: her harf tekrarlanabilir, `*` kelime devamıdır.
fn word_to_pattern(word: &str) -> Option<String> {
    let normalized = normalize(word.trim());
    if normalized.chars().all(|c| c == '*' || c.is_whitespace()) {
        return None;
    }

    let mut pattern = String::new();
    let mut in_space = false;
    for c in normalized.chars() {
        if c.is_whitespace() {
            if !in_space {
                pattern.push_str(r"\s+");
            }
            in_space = true;
            continue;
        }
        in_space = false;

        match c {
            '*' => pattern.push_str(r"\w*"),
            'i' | 'ı' => pattern.push_str("[iı]+"),
            c => {
                pattern.push_str(&regex::escape(c.encode_utf8(&mut [0; 4])));
                pattern.push('+');
            }
        }
    }
    Some(pattern)
}

/// Filtre atlatma denemelerini etkisizleştirir; mesaja, kalıplara ve istisna listesine
/// aynı şekilde uygulanır:
/// - Türkçe büyük/küçük harf: `I` → `ı`, `İ` → `i`. Ancak tamamen büyük ASCII harflerle
///   yazılmış kelimelerde (Türkçe klavye kullanılmamış: "SIKKAFALI") `I` → `i`.
/// - Leetspeak: `0→o 1→i 3→e 4→a @→a $→s`
/// - Görünmez karakterler silinir
pub fn normalize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut word = String::new();

    for c in input.chars() {
        if matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}') {
            continue;
        }
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            push_word(&mut out, &word);
            word.clear();
            push_char(&mut out, c, false);
        }
    }
    push_word(&mut out, &word);

    out
}

fn push_word(out: &mut String, word: &str) {
    let ascii_caps = word.chars().any(|c| c.is_ascii_uppercase())
        && word.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    for c in word.chars() {
        push_char(out, c, ascii_caps);
    }
}

fn push_char(out: &mut String, c: char, ascii_caps: bool) {
    let mapped = match c {
        'I' if ascii_caps => 'i',
        'I' => 'ı',
        'İ' => 'i',
        '0' => 'o',
        '1' => 'i',
        '3' => 'e',
        '4' => 'a',
        '@' => 'a',
        '$' => 's',
        c => c,
    };
    out.extend(mapped.to_lowercase());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(settings: AutoModSettings, custom: &[&str]) -> Result<GuildFilter, regex::Error> {
        filter_with_allowed(settings, custom, &[])
    }

    fn filter_with_allowed(
        settings: AutoModSettings,
        custom: &[&str],
        allowed: &[&str],
    ) -> Result<GuildFilter, regex::Error> {
        let owned = |words: &[&str]| words.iter().map(|w| (*w).to_owned()).collect();
        GuildFilter::compile(AutoModConfig {
            settings,
            custom_words: owned(custom),
            allowed_words: owned(allowed),
        })
    }

    fn assert_blocked(f: &GuildFilter, base: &BasePatterns, samples: &[&str]) {
        for s in samples {
            assert_eq!(f.check(base, s), Some(Violation::Profanity), "engellenmeliydi: {s}");
        }
    }

    fn assert_allowed(f: &GuildFilter, base: &BasePatterns, samples: &[&str]) {
        for s in samples {
            assert_eq!(f.check(base, s), None, "serbest olmalıydı: {s}");
        }
    }

    #[test]
    fn normalize_handles_turkish_case_and_leet() {
        assert_eq!(normalize("SİKTİR"), "siktir");
        assert_eq!(normalize("S1KT1R"), "siktir");
        assert_eq!(normalize("AMINA"), "amina");
        assert_eq!(normalize("Işık ŞIK"), "ışık şık");
        assert_eq!(normalize("SIKILDIM"), "sikildim");
        assert_eq!(normalize("a$$"), "ass");
        assert_eq!(normalize("a\u{200B}m\u{200B}k"), "amk");
    }

    #[test]
    fn turkish_profanity_including_compounds_and_repeats() -> Result<(), regex::Error> {
        let base = BasePatterns::new()?;
        let f = filter(AutoModSettings::default(), &[])?;
        assert_blocked(&f, &base, &[
            "amk", "sen tam bir P1Ç", "siktir git", "orospuçocuğu", "GÖT", "sikkafalı",
            "SİKKAFALI", "SIKKAFALI", "siiiiiktirrr", "ananısikeyim", "piçkurusu", "götveren",
            "amınakoyim", "yarrrrak", "sik",
        ]);
        Ok(())
    }

    #[test]
    fn turkish_innocent_words_are_not_blocked() -> Result<(), regex::Error> {
        let base = BasePatterns::new()?;
        let f = filter(AutoModSettings::default(), &[])?;
        assert_allowed(&f, &base, &[
            "sık sık gelirim", "SIKILDIM", "ÇOK SIKICI", "çok sıkıldım", "sikildim ya", "bu iş çok sıkıcı",
            "sikici bir film", "canım sıkkın", "sikke koleksiyonu", "sıkça sorulan sorular",
            "kutuyu götürmek lazım", "aquapark", "bugün hava güzel", "amaç", "ışık şık",
        ]);
        Ok(())
    }

    #[test]
    fn english_profanity() -> Result<(), regex::Error> {
        let base = BasePatterns::new()?;
        let f = filter(AutoModSettings::default(), &[])?;
        assert_blocked(&f, &base, &[
            "fuck", "FUCKING hell", "motherfucker", "sh1t", "SHIT", "bullshit", "you are an ass",
            "a$$", "biiitch", "dickhead", "what a cunt",
        ]);
        assert_allowed(&f, &base, &[
            "as soon as possible", "class assignment", "I got it", "Scunthorpe", "cocktail",
            "Dickens", "assassin", "shiitake mushrooms", "passed the test",
        ]);
        Ok(())
    }

    #[test]
    fn custom_words_and_wildcards() -> Result<(), regex::Error> {
        let base = BasePatterns::new()?;
        let f = filter(AutoModSettings::default(), &["kötükelime", "spam*", "*yasak*"])?;

        assert_blocked(&f, &base, &["bu bir KÖTÜKELİME", "spamcı", "çokyasaklı"]);
        assert_allowed(&f, &base, &["kötükelimeler"]);
        Ok(())
    }

    #[test]
    fn guild_allowed_words_override_defaults() -> Result<(), regex::Error> {
        let base = BasePatterns::new()?;
        let strict = filter(AutoModSettings::default(), &[])?;
        let relaxed =
            filter_with_allowed(AutoModSettings::default(), &[], &["piçikato", "sikko*"])?;

        assert_blocked(&strict, &base, &["piçikato", "sikkolar"]);
        assert_allowed(&relaxed, &base, &["piçikato", "SİKKOLAR"]);
        // İstisna sadece o kelimeyi açar, kökün diğer türevleri hâlâ yasak.
        assert_blocked(&relaxed, &base, &["piçkurusu", "sikkafalı"]);
        Ok(())
    }

    #[test]
    fn invites_including_obfuscated() -> Result<(), regex::Error> {
        let base = BasePatterns::new()?;
        let f = filter(AutoModSettings::default(), &[])?;

        for bad in ["gel discord.gg/abc123", "https://discord.com/invite/xyz", "discord . gg / abc"] {
            assert_eq!(f.check(&base, bad), Some(Violation::Invite), "{bad}");
        }
        assert_eq!(f.check(&base, "discord güzel bir uygulama"), None);
        Ok(())
    }

    #[test]
    fn links_only_when_enabled() -> Result<(), regex::Error> {
        let base = BasePatterns::new()?;
        let off = filter(AutoModSettings::default(), &[])?;
        let on = filter(
            AutoModSettings {
                block_links: true,
                ..AutoModSettings::default()
            },
            &[],
        )?;

        assert_eq!(off.check(&base, "https://example.com"), None);
        assert_eq!(on.check(&base, "bak: https://example.com"), Some(Violation::Link));
        assert_eq!(on.check(&base, "www.example.com"), Some(Violation::Link));
        Ok(())
    }

    #[test]
    fn disabled_profanity_filter_compiles_no_regex() -> Result<(), regex::Error> {
        let base = BasePatterns::new()?;
        let f = filter(
            AutoModSettings {
                filter_profanity: false,
                ..AutoModSettings::default()
            },
            &[],
        )?;
        assert_eq!(f.check(&base, "amk"), None);
        Ok(())
    }
}
