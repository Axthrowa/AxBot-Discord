//! Modül aç/kapa: panelde kapatılan modülün komutları o sunucuya kaydedilmez (üye görmez) ve
//! eski istemciden gelse de çalışmaz. Varsayılan hepsi açık; çekirdek komutlar (ping, yardım, mod…) kapanmaz.
//! Yalnızca komutlar kapanır; modülün arka plan işleri (log, XP vb.) kendi ayarına bağlıdır.

use serde::{Deserialize, Serialize};

use crate::settings::PluginConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleSettings {
    /// Kapatılan modüllerin panel adları (`plugins::PLUGINS` slug'ları).
    pub disabled: Vec<String>,
}

impl PluginConfig for ModuleSettings {
    const KEY: &'static str = "modules";
}

/// Üst düzey komut (ya da sağ tık komutunun fonksiyon adı) → modülün panel adı.
const COMMAND_MODULES: &[(&str, &str)] = &[
    ("automod", "moderasyon"),
    ("bilet", "biletler"),
    ("ai", "yz"),
    ("yaz", "yz"),
    ("hayal-et", "yz"),
    ("çevir", "yz"),
    ("cevir_mesaj", "yz"),
    ("ozetle", "yz"),
    ("cevap_taslagi", "yz"),
    ("doğrulama", "dogrulama"),
    ("başvuru", "basvurular"),
    ("hayvan", "hayvanlar"),
    ("otocevap", "otocevap"),
    ("karşılama", "karsilama"),
    ("koruma", "koruma"),
    ("çekiliş", "cekilis"),
    ("starboard", "starboard"),
    ("starboard_ekle", "starboard"),
    ("etiket", "etiketler"),
    ("davetler", "davetler"),
    ("sayaç", "sayaclar"),
    ("doğum-günü", "dogum-gunu"),
    ("geçici-oda", "gecici-odalar"),
    ("oda", "gecici-odalar"),
    ("youtube", "yayinlar"),
    ("yayın", "yayinlar"),
    ("akış", "sosyal"),
    ("ekonomi", "ekonomi"),
    ("mağaza-yönet", "ekonomi"),
    ("ekonomi-yönet", "ekonomi"),
    ("seviye", "seviye"),
    ("öneri", "oneriler"),
    ("çal", "muzik"),
    ("müzik", "muzik"),
];

/// Komutun bağlı olduğu modül (çekirdek komutsa `None`).
pub fn module_of(command: &str) -> Option<&'static str> {
    COMMAND_MODULES.iter().find(|(name, _)| *name == command).map(|(_, slug)| *slug)
}

/// Bu modülün kapatılabilir komutu var mı (panelde düğme yalnızca bunlarda gösterilir)?
pub fn has_commands(slug: &str) -> bool {
    COMMAND_MODULES.iter().any(|(_, s)| *s == slug)
}

impl ModuleSettings {
    pub fn is_disabled(&self, slug: &str) -> bool {
        self.disabled.iter().any(|d| d == slug)
    }

    /// Üst düzey komut bu sunucuda açık mı?
    pub fn allows(&self, command: &str) -> bool {
        module_of(command).is_none_or(|slug| !self.is_disabled(slug))
    }

    pub fn set(&mut self, slug: &str, enabled: bool) {
        self.disabled.retain(|d| d != slug);
        if !enabled {
            self.disabled.push(slug.to_owned());
            self.disabled.sort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_module_hides_only_its_commands() {
        let mut modules = ModuleSettings::default();
        assert!(modules.allows("ekonomi") && modules.allows("ping"));
        modules.set("ekonomi", false);
        assert!(!modules.allows("ekonomi") && !modules.allows("mağaza-yönet"));
        assert!(modules.allows("ping") && modules.allows("seviye"));
        modules.set("ekonomi", false);
        assert_eq!(modules.disabled, vec!["ekonomi".to_owned()]);
        modules.set("ekonomi", true);
        assert!(modules.allows("ekonomi"));
    }

    /// Eşlemedeki her komut gerçekten var olmalı, her modül de panelde bulunmalı (ad değişirse test kırılır).
    #[test]
    fn mapping_matches_real_commands_and_plugins() {
        let names: Vec<String> = crate::commands::all().into_iter().map(|c| c.name).collect();
        for (command, slug) in COMMAND_MODULES {
            assert!(names.iter().any(|n| n == command), "komut yok: {command}");
            assert!(crate::panel::plugin_exists(slug), "eklenti yok: {slug}");
        }
    }
}
