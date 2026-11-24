//! Modül sözleşmesi: her özellik `MODULES` içinde tek bir tanım verir (ad, alan, komutlar, ayar şeması).
//!
//! Modül aç/kapa: panelde kapatılan modülün komutları o sunucuya kaydedilmez (üye görmez) ve
//! eski istemciden gelse de çalışmaz. Varsayılan hepsi açık; çekirdek komutlar (ping, yardım, mod…) kapanmaz.
//! Yalnızca komutlar kapanır; modülün arka plan işleri (log, XP vb.) kendi ayarına bağlıdır.

use serde::{Deserialize, Serialize};

use crate::settings::PluginConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleSettings {
    /// Kapatılan modüllerin adları (`MODULES` slug'ları).
    pub disabled: Vec<String>,
}

impl PluginConfig for ModuleSettings {
    const KEY: &'static str = "modules";
}

pub struct Module {
    pub slug: &'static str,
    pub icon: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub group: &'static str,
    /// Bu modülün üst düzey komutları (sağ tık komutlarında fonksiyon adı); modül kapanınca kaydedilmez.
    pub commands: &'static [&'static str],
    /// Ayarları şemadan üretilen modüllerde form, doğrulama ve kayıt tanımı.
    pub schema: Option<&'static crate::panel::schema::Schema>,
}

/// Menüde ve sunucu sayfasındaki kartlarda bu sırayla görünür.
pub const MODULES: &[Module] = &[
    Module {
        slug: "karsilama",
        icon: "👋",
        name: "Hoş geldin & Veda",
        about: "Karşılama ve veda mesajları, resimli karşılama kartı, özelden karşılama, otomatik roller.",
        group: "Temel",
        commands: &["karşılama"],
        schema: None,
    },
    Module {
        slug: "dogrulama",
        icon: "✅",
        name: "Doğrulama",
        about: "Yeni üyeler butona basıp (istersen captcha çözerek) doğrulanır ve rol alır.",
        group: "Temel",
        commands: &["doğrulama"],
        schema: None,
    },
    Module {
        slug: "seviye",
        icon: "🏆",
        name: "Seviye",
        about: "Mesaj yazan üyeler XP kazanır, seviye atlar ve ödül rolleri alır.",
        group: "Temel",
        commands: &["seviye"],
        schema: None,
    },
    Module {
        slug: "moderasyon",
        icon: "🔨",
        name: "Moderasyon",
        about: "Otomatik moderasyon kuralları, kısıtlı kanallar, muaf roller ve otomatik cezalar.",
        group: "Temel",
        commands: &["automod"],
        schema: None,
    },
    Module {
        slug: "koruma",
        icon: "🛡️",
        name: "Koruma",
        about: "Çok yeni hesapları ve toplu girişleri (baskın) durdur.",
        group: "Temel",
        commands: &["koruma"],
        schema: None,
    },
    Module {
        slug: "kayitlar",
        icon: "📋",
        name: "Olay kayıtları",
        about: "Mesaj, üye, rol, ses, kanal ve sunucu olayları ile moderasyon vakaları seçtiğin kanala yazılır.",
        group: "Temel",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "starboard",
        icon: "⭐",
        name: "Starboard",
        about: "Yeterince ⭐ alan mesajlar ayrı bir kanalda sergilenir.",
        group: "Temel",
        commands: &["starboard", "starboard_ekle"],
        schema: None,
    },
    Module {
        slug: "ek-starboard",
        icon: "🌟",
        name: "Ek starboard'lar",
        about: "⭐ dışındaki emojiler için ayrı panolar (ör. 😂 → komik anlar).",
        group: "Temel",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "basarimlar",
        icon: "🏅",
        name: "Başarımlar",
        about: "Üyeler mesaj, tepki, ses ve alt başlık sayılarına göre rozet kazanır.",
        group: "Temel",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "discord-automod",
        icon: "🧱",
        name: "Discord AutoMod",
        about: "Discord'un kendi filtreleri: mesaj gönderilmeden engellenir, bot kapalıyken de korur.",
        group: "Temel",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "yikim-korumasi",
        icon: "🚨",
        name: "Yıkım koruması",
        about: "Kısa sürede çok kanal / rol silen ya da üye yasaklayan yetkiliyi durdurur.",
        group: "Temel",
        commands: &[],
        schema: Some(&crate::panel::manage::ANTINUKE),
    },
    Module {
        slug: "biletler",
        icon: "🎫",
        name: "Destek biletleri",
        about: "Üyeler butonla ya da konu seçerek özel destek kanalı açar; kayıt tutulur.",
        group: "Sunucu yönetimi",
        commands: &["bilet"],
        schema: None,
    },
    Module {
        slug: "otomasyon",
        icon: "⚙️",
        name: "Otomasyon",
        about: "\"Şu olunca şunu yap\": mesaj, giriş, rol ve tepki olaylarına otomatik karşılık.",
        group: "Sunucu yönetimi",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "tepki-rol",
        icon: "🎭",
        name: "Rol menüleri",
        about: "Üyeler emoji tepkisi, buton ya da açılır menüyle kendi rollerini seçer.",
        group: "Sunucu yönetimi",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "ozel-komutlar",
        icon: "⌨️",
        name: "Özel komutlar",
        about: "Kendi !komutlarını oluştur: cevap yazsın, rol versin, belirli rollere özel olsun.",
        group: "Sunucu yönetimi",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "modmail",
        icon: "📬",
        name: "ModMail",
        about: "Üyeler bota özelden yazar; mesaj yetkili kanalında alt başlığa düşer.",
        group: "Sunucu yönetimi",
        commands: &[],
        schema: Some(&crate::panel::manage::MODMAIL),
    },
    Module {
        slug: "basvurular",
        icon: "📝",
        name: "Başvuru formları",
        about: "Sorulu başvuru formları; yetkili kanalında Kabul / Ret.",
        group: "Sunucu yönetimi",
        commands: &["başvuru"],
        schema: None,
    },
    Module {
        slug: "etiketler",
        icon: "🏷️",
        name: "Etiketler",
        about: "Sohbette !ad yazınca gönderilen hazır metinler.",
        group: "Sunucu yönetimi",
        commands: &["etiket"],
        schema: None,
    },
    Module {
        slug: "davetler",
        icon: "📨",
        name: "Davet takibi",
        about: "Kim kimi davet etti: davet sıralaması ve sahte davet ayıklama.",
        group: "Sunucu yönetimi",
        commands: &["davetler"],
        schema: Some(&crate::panel::community::INVITES),
    },
    Module {
        slug: "otocevap",
        icon: "💬",
        name: "Otomatik cevaplar",
        about: "Belirli kelimelere botun hazır cevap vermesi.",
        group: "Sunucu yönetimi",
        commands: &["otocevap"],
        schema: None,
    },
    Module {
        slug: "anket",
        icon: "📊",
        name: "Anket",
        about: "Seçtiğin kanala Discord anketi gönder.",
        group: "Araçlar",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "sayaclar",
        icon: "🔢",
        name: "Sayaç kanalları",
        about: "Üye, bot, kanal, rol sayısı ya da kripto fiyatını gösteren kanallar.",
        group: "Araçlar",
        commands: &["sayaç"],
        schema: None,
    },
    Module {
        slug: "gecici-odalar",
        icon: "🎙️",
        name: "Geçici odalar",
        about: "Bir kanala girene kendi ses odasını açar; boşalınca siler.",
        group: "Araçlar",
        commands: &["geçici-oda", "oda"],
        schema: None,
    },
    Module {
        slug: "zamanlayici",
        icon: "⏰",
        name: "Tekrarlı mesajlar",
        about: "Seçtiğin kanala belirli aralıklarla aynı mesajı gönderir.",
        group: "Araçlar",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "gomulu-mesaj",
        icon: "🖼️",
        name: "Gömülü mesajlar",
        about: "Kurallar ve duyurular için renkli, resimli gömülü mesajlar hazırla ve gönder.",
        group: "Araçlar",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "yayinlar",
        icon: "📺",
        name: "Yayın ve video duyuruları",
        about: "YouTube'da yeni video, Twitch ve Kick'te canlı yayın duyuruları.",
        group: "Sosyal",
        commands: &["youtube", "yayın"],
        schema: None,
    },
    Module {
        slug: "sosyal",
        icon: "📡",
        name: "Sosyal akışlar",
        about: "Reddit, Bluesky, RSS ve podcast paylaşımlarını seçtiğin kanala duyurur.",
        group: "Sosyal",
        commands: &["akış"],
        schema: None,
    },
    Module {
        slug: "haberler",
        icon: "📰",
        name: "Haberler",
        about: "Teknoloji, oyun, sinema ve dizi haberleri otomatik olarak kanala; her hafta özet.",
        group: "Sosyal",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "topluluk",
        icon: "🧰",
        name: "Topluluk araçları",
        about: "Sabit mesajlar, sayma oyunu, itiraflar, kalıcı roller ve ses XP'si.",
        group: "Topluluk",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "oneriler",
        icon: "💡",
        name: "Öneriler",
        about: "Üyeler öneri gönderir, herkes butonla oylar.",
        group: "Topluluk",
        commands: &["öneri"],
        schema: None,
    },
    Module {
        slug: "dogum-gunu",
        icon: "🎂",
        name: "Doğum günü",
        about: "Doğum günü olanlar kutlanır ve bir günlüğüne özel rol alır.",
        group: "Topluluk",
        commands: &["doğum-günü"],
        schema: None,
    },
    Module {
        slug: "muzik",
        icon: "🎵",
        name: "Müzik",
        about: "7/24 modu, otomatik çalma ve şu an çalan şarkı.",
        group: "Eğlence",
        commands: &["çal", "müzik"],
        schema: None,
    },
    Module {
        slug: "cekilis",
        icon: "🎉",
        name: "Çekiliş",
        about: "Butonla katılınan, süresi dolunca kazananı seçilen çekilişler.",
        group: "Eğlence",
        commands: &["çekiliş"],
        schema: None,
    },
    Module {
        slug: "ekonomi",
        icon: "🪙",
        name: "Ekonomi",
        about: "Sunucu parası, mağaza ürünleri ve en zenginler.",
        group: "Eğlence",
        commands: &["ekonomi", "mağaza-yönet", "ekonomi-yönet"],
        schema: None,
    },
    Module {
        slug: "hayvanlar",
        icon: "🐾",
        name: "Hayvan yakalama",
        about: "Seçilen kanalda ara sıra bir hayvan belirir; ilk basan yakalar.",
        group: "Eğlence",
        commands: &["hayvan"],
        schema: None,
    },
    Module {
        slug: "yz",
        icon: "🤖",
        name: "Yapay zekâ",
        about: "Sohbet kanalları, sunucu kişiliği ve kanal başına karakterler.",
        group: "Bot",
        commands: &["ai", "yaz", "hayal-et", "çevir", "cevir_mesaj", "ozetle", "cevap_taslagi"],
        schema: None,
    },
    Module {
        slug: "bot",
        icon: "🪪",
        name: "Bot profili",
        about: "Botun bu sunucudaki takma adı, avatarı, afişi ve hakkında yazısı.",
        group: "Bot",
        commands: &[],
        schema: None,
    },
    Module {
        slug: "ayarlar",
        icon: "⚙️",
        name: "Panel ayarları",
        about: "Paneli kullanabilen bot yöneticisi rolleri.",
        group: "Bot",
        commands: &[],
        schema: None,
    },
];

/// Komutun bağlı olduğu modül (çekirdek komutsa `None`).
pub fn module_of(command: &str) -> Option<&'static str> {
    MODULES.iter().find(|m| m.commands.contains(&command)).map(|m| m.slug)
}

/// Bu modülün kapatılabilir komutu var mı (panelde düğme yalnızca bunlarda gösterilir)?
pub fn has_commands(slug: &str) -> bool {
    MODULES.iter().any(|m| m.slug == slug && !m.commands.is_empty())
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

    /// Modüllerdeki her komut gerçekten var olmalı ve tek bir modüle ait olmalı (ad değişirse test kırılır).
    #[test]
    fn module_commands_exist_once() {
        let names: Vec<String> = crate::commands::all().into_iter().map(|c| c.name).collect();
        let mut seen = std::collections::HashSet::new();
        for command in MODULES.iter().flat_map(|m| m.commands) {
            assert!(names.iter().any(|n| n == command), "komut yok: {command}");
            assert!(seen.insert(*command), "iki modülde: {command}");
        }
    }
}
