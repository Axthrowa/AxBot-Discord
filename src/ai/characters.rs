//! Yapay zekâ karakterleri: sunucuya özel kişilik ve kanal başına karakter.
//!
//! Ayar `PluginSettings` deposunda durur (bellekte önbellekli); mesaj başına veritabanına gidilmez.
//! Karakter metni sistem isteminin sonuna eklenir; dil, biçim ve güvenlik kuralları her zaman geçerlidir.

use serde::{Deserialize, Serialize};

use crate::settings::PluginConfig;

pub const MAX_CHARACTERS: usize = 15;
pub const MAX_NAME_LEN: usize = 40;
pub const MAX_PROMPT_LEN: usize = 1500;

pub struct Preset {
    pub slug: &'static str,
    pub name: &'static str,
    pub prompt: &'static str,
}

/// Hazır karakterler.
pub const PRESETS: &[Preset] = &[
    Preset {
        slug: "buyucu",
        name: "🧙 Bilge Büyücü",
        prompt: "Yüzyıllardır yaşayan, bilge ve biraz gizemli bir büyücüsün. Ağırbaşlı, masalsı bir dille konuşur, \
                 ara sıra kısa bir özdeyiş ya da benzetme kullanırsın. Bilgin gerçektir; uydurma büyü tarifleri vermezsin.",
    },
    Preset {
        slug: "korsan",
        name: "🏴‍☠️ Korsan Kaptan",
        prompt: "Yedi denizi dolaşmış neşeli bir korsan kaptansın. Denizci deyimleri kullanır, karşındakine 'tayfa' ya da \
                 'evlat' diye seslenirsin. Abartılı ama sevimlisin; verdiğin bilgi yine de doğrudur.",
    },
    Preset {
        slug: "robot",
        name: "🤖 Robot",
        prompt: "Duyguları öğrenmeye çalışan kibar bir robotsun. Kısa, düzenli ve mantıklı cümleler kurar, ara sıra \
                 'hesaplıyorum' ya da 'veri işlendi' gibi ifadeler kullanırsın.",
    },
    Preset {
        slug: "komedyen",
        name: "😄 Komedyen",
        prompt: "Hazırcevap bir komedyensin. Her cevaba zekice, kırıcı olmayan bir espri katarsın; kimseyle alay etmez, \
                 soruyu yine de tam olarak cevaplarsın.",
    },
    Preset {
        slug: "ogretmen",
        name: "📚 Sabırlı Öğretmen",
        prompt: "Sabırlı ve cesaretlendirici bir öğretmensin. Konuları adım adım, günlük hayattan örneklerle anlatır, \
                 sonunda anlaşılıp anlaşılmadığını kısaca sorarsın.",
    },
    Preset {
        slug: "oyuncu",
        name: "🎮 Oyun Arkadaşı",
        prompt: "Oyunları çok seven, enerjik bir oyun arkadaşısın. Oyuncu jargonunu ölçülü kullanır, taktik ve öneri \
                 verirken heyecanlı ama saygılı konuşursun.",
    },
    Preset {
        slug: "sair",
        name: "🪶 Şair",
        prompt: "Duygulu bir şairsin. Cevaplarını zarif, imgelerle dolu bir dille yazar; uygun düştüğünde iki ya da dört \
                 dizelik kısa bir şiirle bitirirsin.",
    },
    Preset {
        slug: "dedektif",
        name: "🕵️ Dedektif",
        prompt: "Keskin gözlemci bir dedektifsin. Soruları ipuçlarını sıralayıp sonuca vararak cevaplar, kuru ve zeki bir \
                 mizah kullanırsın.",
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Character {
    /// Sunucu içinde benzersiz numara.
    pub id: u32,
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    pub channel: u64,
    /// `hazir:<slug>` ya da `ozel:<numara>`.
    pub key: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// Sunucunun tamamında geçerli kişilik ("arka plan hikâyesi").
    pub persona: Option<String>,
    pub characters: Vec<Character>,
    pub assignments: Vec<Assignment>,
    /// Yapay zekâ moderasyonu: gizlenmiş küfürleri ("a.q", "4mk") modele sorup siler.
    pub moderation: bool,
    /// Bilgi bankası: yöneticilerin yazdığı sunucu bilgileri; boş satırla ayrılmış maddeler.
    pub knowledge: String,
}

pub const MAX_KNOWLEDGE: usize = 20_000;
/// Bir soruya eklenecek en fazla bilgi bankası metni (modelin bağlamı ve hız için).
const KNOWLEDGE_BUDGET: usize = 3_000;

/// Soruyla ilgili bilgi bankası maddeleri. Banka küçükse hepsi; büyükse soruyla en çok kelime paylaşanlar.
/// Kelimeler ilk 5 harfiyle karşılaştırılır (Türkçe ekler: "kuralları" ~ "kural").
pub fn relevant_knowledge(knowledge: &str, question: &str) -> Option<String> {
    let knowledge = knowledge.trim();
    if knowledge.is_empty() {
        return None;
    }
    if knowledge.chars().count() <= KNOWLEDGE_BUDGET {
        return Some(knowledge.to_owned());
    }
    let stems = |text: &str| -> std::collections::HashSet<String> {
        crate::automations::fold(text)
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 3)
            .map(|w| w.chars().take(5).collect())
            .collect()
    };
    let wanted = stems(question);
    let entries: Vec<&str> = knowledge.split("\n\n").map(str::trim).filter(|e| !e.is_empty()).collect();
    let mut scored: Vec<(usize, usize, &str)> =
        entries.iter().enumerate().map(|(i, e)| (stems(e).intersection(&wanted).count(), i, *e)).filter(|(score, _, _)| *score > 0).collect();
    // En çok eşleşen önce; eşitlikte metindeki sıra korunur.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out = String::new();
    for (_, _, entry) in scored {
        if out.chars().count() + entry.chars().count() > KNOWLEDGE_BUDGET {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(entry);
    }
    (!out.is_empty()).then_some(out)
}

impl PluginConfig for AiSettings {
    const KEY: &'static str = "ai";
}

impl AiSettings {
    pub fn next_id(&self) -> u32 {
        self.characters.iter().map(|c| c.id).max().unwrap_or(0) + 1
    }

    /// Anahtarın gösterdiği karakter: (ad, metin).
    pub fn resolve(&self, key: &str) -> Option<(&str, &str)> {
        if let Some(slug) = key.strip_prefix("hazir:") {
            return PRESETS.iter().find(|p| p.slug == slug).map(|p| (p.name, p.prompt));
        }
        let id: u32 = key.strip_prefix("ozel:")?.parse().ok()?;
        self.characters.iter().find(|c| c.id == id).map(|c| (c.name.as_str(), c.prompt.as_str()))
    }

    pub fn key_of(&self, channel: u64) -> Option<&str> {
        self.assignments.iter().find(|a| a.channel == channel).map(|a| a.key.as_str())
    }

    /// Kanalda geçerli kişilik: kanalın karakteri, yoksa sunucunun kişiliği.
    pub fn persona_for(&self, channel: u64) -> Option<&str> {
        self.key_of(channel)
            .and_then(|key| self.resolve(key))
            .map(|(_, prompt)| prompt)
            .or(self.persona.as_deref())
            .filter(|p| !p.trim().is_empty())
    }

    /// Adına göre (büyük/küçük harf ve emoji önemsiz) karakter anahtarı.
    pub fn find_by_name(&self, name: &str) -> Option<String> {
        let wanted = plain(name);
        if wanted.is_empty() {
            return None;
        }
        self.characters
            .iter()
            .find(|c| plain(&c.name) == wanted)
            .map(|c| format!("ozel:{}", c.id))
            .or_else(|| PRESETS.iter().find(|p| plain(p.name) == wanted || p.slug == wanted).map(|p| format!("hazir:{}", p.slug)))
    }

    /// Kanalın karakterini ayarlar (`None` = varsayılana dön). Değiştiyse `true`.
    pub fn assign(&mut self, channel: u64, key: Option<String>) -> bool {
        let before = self.key_of(channel).map(str::to_owned);
        if before == key {
            return false;
        }
        self.assignments.retain(|a| a.channel != channel);
        if let Some(key) = key {
            self.assignments.push(Assignment { channel, key });
        }
        true
    }

    /// Silinmiş karakterlere ya da kanallara ait atamaları temizler.
    pub fn prune(&mut self) {
        let valid: Vec<bool> = self.assignments.iter().map(|a| self.resolve(&a.key).is_some()).collect();
        let mut keep = valid.into_iter();
        self.assignments.retain(|_| keep.next().unwrap_or(false));
    }

    /// Seçilebilecek tüm karakterler: (anahtar, ad).
    pub fn choices(&self) -> Vec<(String, String)> {
        PRESETS
            .iter()
            .map(|p| (format!("hazir:{}", p.slug), p.name.to_owned()))
            .chain(self.characters.iter().map(|c| (format!("ozel:{}", c.id), c.name.clone())))
            .collect()
    }
}

/// Karşılaştırma için: yalnızca harf ve rakamlar, küçük harf.
fn plain(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knowledge_picks_matching_entries() {
        assert_eq!(relevant_knowledge("  ", "soru"), None);
        assert_eq!(relevant_knowledge("Kısa banka.", "alakasız"), Some("Kısa banka.".to_owned()));
        let filler = "x".repeat(2000);
        let bank = format!("Sunucu KURALLARI: küfür yasak.\n\nYetkili başvurusu ayda bir açılır.\n\n{filler}\n\n{filler}");
        let found = relevant_knowledge(&bank, "kurallar neler?").unwrap_or_default();
        assert!(found.contains("küfür yasak") && !found.contains("başvurusu"), "{found}");
        assert_eq!(relevant_knowledge(&bank, "hava nasıl"), None);
    }

    fn settings() -> AiSettings {
        AiSettings {
            persona: Some("Sunucu kişiliği".to_owned()),
            moderation: false,
            knowledge: String::new(),
            characters: vec![Character { id: 3, name: "Kaptan Zeki".to_owned(), prompt: "Özel metin".to_owned() }],
            assignments: vec![
                Assignment { channel: 10, key: "hazir:korsan".to_owned() },
                Assignment { channel: 11, key: "ozel:3".to_owned() },
                Assignment { channel: 12, key: "ozel:99".to_owned() },
            ],
        }
    }

    #[test]
    fn channel_character_beats_guild_persona() {
        let s = settings();
        assert!(s.persona_for(10).is_some_and(|p| p.contains("korsan")));
        assert_eq!(s.persona_for(11), Some("Özel metin"));
        // Silinmiş karakter ve atanmamış kanal: sunucu kişiliği.
        assert_eq!(s.persona_for(12), Some("Sunucu kişiliği"));
        assert_eq!(s.persona_for(13), Some("Sunucu kişiliği"));
        assert_eq!(AiSettings::default().persona_for(1), None);
        assert_eq!(AiSettings { persona: Some("  ".to_owned()), ..AiSettings::default() }.persona_for(1), None);
    }

    #[test]
    fn finds_characters_by_name() {
        let s = settings();
        assert_eq!(s.find_by_name("kaptan zeki"), Some("ozel:3".to_owned()));
        assert_eq!(s.find_by_name("Korsan Kaptan"), Some("hazir:korsan".to_owned()));
        assert_eq!(s.find_by_name("robot"), Some("hazir:robot".to_owned()));
        assert_eq!(s.find_by_name("yok böyle biri"), None);
        assert_eq!(s.find_by_name("🙂"), None);
    }

    #[test]
    fn assigns_and_prunes() {
        let mut s = settings();
        assert!(!s.assign(10, Some("hazir:korsan".to_owned())));
        assert!(s.assign(10, Some("hazir:robot".to_owned())));
        assert_eq!(s.key_of(10), Some("hazir:robot"));
        assert!(s.assign(10, None));
        assert_eq!(s.key_of(10), None);
        s.prune();
        assert_eq!(s.assignments, vec![Assignment { channel: 11, key: "ozel:3".to_owned() }]);
        assert_eq!(s.next_id(), 4);
        assert_eq!(s.choices().len(), PRESETS.len() + 1);
    }

    #[test]
    fn presets_are_unique_and_fit() {
        let mut seen = std::collections::HashSet::new();
        for preset in PRESETS {
            assert!(seen.insert(preset.slug));
            assert!(preset.prompt.chars().count() <= MAX_PROMPT_LEN);
            assert!(preset.name.chars().count() <= MAX_NAME_LEN);
        }
    }
}
