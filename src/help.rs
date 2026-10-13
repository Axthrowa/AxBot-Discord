//! `/yardım` rehberinin sayfaları. Üyeler kullanıcı rehberini, 'Sunucuyu Yönet' izni olanlar ayrıca
//! yönetici rehberini görür. Komut adı değişirse buradaki metinleri de güncelle
//! (`cargo test print_command_tree -- --ignored --nocapture` tüm komutları listeler).

pub struct Page {
    pub key: &'static str,
    pub label: &'static str,
    pub emoji: &'static str,
    /// Yalnızca yöneticilere gösterilir.
    pub admin: bool,
    pub body: &'static str,
}

pub const USER_INTRO: &str = "AxBot; müzik, seviye, ekonomi, oyunlar, yapay zekâ ve daha fazlasını sunan çok amaçlı bir bot.\n\n\
**Nasıl kullanılır?** Sohbete `/` yazınca botun komutları listelenir; bir komut seçince ne yaptığı ve seçenekleri görünür.\n\
Aşağıdaki menüden bir konu seç, o konunun komutlarını örnekleriyle anlatayım.\n\n\
💡 Mesajlara ve üyelere **sağ tıklayıp → Uygulamalar** menüsünde de birkaç kısayol var (çevir, özetle, bildir).";

pub const ADMIN_INTRO: &str = "AxBot; müzik, seviye, ekonomi, oyunlar, yapay zekâ ve daha fazlasını sunan çok amaçlı bir bot.\n\n\
**Sunucuyu yönetme yetkin olduğu için yönetici rehberini de görüyorsun** (🛠️ ile başlayan konular).\n\
• İlk iş: `/kurulum` — eksik izinleri ve hangi özelliğin açık olduğunu gösterir.\n\
• Ayarların çoğunu komut yazmadan **web panelinden** yapabilirsin: `/panel`.\n\n\
Aşağıdaki menüden bir konu seç.";

pub const PAGES: &[Page] = &[
    // ---------------------------------------------------------------- Üyeler
    Page {
        key: "u_start",
        label: "Başlarken",
        emoji: "🚀",
        admin: false,
        body: "**Komutları bulmak:** `/` yaz → listeden AxBot'u seç. Her komutun altında kısa açıklaması yazar.\n\
**Bilgi komutları**\n\
• `/kullanıcı-bilgi`, `/sunucu-bilgi`, `/rol-bilgi`, `/avatar` — hesap, sunucu, rol ve profil resmi\n\
• `/ping` — botun gecikmesi\n\
• `/hatırlat süre:30dk mesaj:…` — belirtilen sürede sana hatırlatır\n\
• `/afk sebep:…` — biri seni etiketlerse bot AFK olduğunu söyler; ilk mesajında AFK kalkar\n\n\
**Sağ tık menüsü** (mesaja / üyeye sağ tık → Uygulamalar)\n\
• *Çevir (Türkçe)* · *Buraya kadar özetle* · *Yetkiliye bildir*",
    },
    Page {
        key: "u_music",
        label: "Müzik",
        emoji: "🎵",
        admin: false,
        body: "Önce bir **ses kanalına gir**, sonra:\n\
• `/çal şarkı:Tarkan Şımarık` — şarkı adı, YouTube / Spotify / SoundCloud bağlantısı ya da çalma listesi olabilir\n\
• `/sıra` kuyruğu gösterir · `/geç` sonraki şarkı · `/duraklat` `/devam` · `/ses` (%0-150) · `/döngü` · `/karıştır`\n\
• `/şimdi` çalan şarkı · `/durdur` müziği kapatır · `/ayrıl` kanaldan çıkar\n\n\
**Şarkı kartındaki butonlar:** ⏮ baştan başlat · ⏯ oynat/duraklat · ⏭ sonraki · ⏹ kapat · 🔀 karıştır · 🔊 ses · 🔁 döngü · \
📜 sıra · 🔍 şarkı ekle · 🧹 sırayı temizle · 🔗 şarkının bağlantısı. Açılır menüden şarkının istediğin yerine atlayabilirsin.\n\n\
**`/müzik` alt komutları**\n\
• `radyo` canlı radyo · `sözler` şarkı sözleri · `sar konum:1:30` · `otomatik` (sıra bitince benzer şarkılar)\n\
• `sil` / `taşı` kuyruk düzeni · `söyle metin:…` yazdığını seste okur\n\
• `liste-kaydet` / `liste-çal` / `listelerim` / `liste-sil` — kendi çalma listelerin (her sunucuda geçerli)",
    },
    Page {
        key: "u_level",
        label: "Seviye ve profil",
        emoji: "⭐",
        admin: false,
        body: "Sohbete yazdıkça (ve sesli kanalda vakit geçirdikçe) **XP** kazanırsın; seviye atlayınca bazı sunucularda rol ödülü verilir.\n\
• `/seviye` — seviye kartın (resimli) · `/sıralama dönem:Haftalık` — en aktifler (tüm zamanlar / haftalık / aylık)\n\
• `/başarımlar` — kazandığın başarımlar ve ilerlemen\n\n\
**`/kartım`**\n\
• `profil` — seviye, bakiye, itibar, başarımlar ve eşin tek kartta\n\
• `ayarla` — kartının rengini, arka planını, yazı tipini değiştir · `sıfırla`\n\
• `rep kullanıcı:@…` — birine itibar puanı ver (günde 1)\n\
• `evlen üye:@…` / `boşan` 💍",
    },
    Page {
        key: "u_economy",
        label: "Ekonomi",
        emoji: "💰",
        admin: false,
        body: "**Para kazan:** `/günlük` (her gün, seri bonusu artar) · `/çalış tür:` mesai (saatte bir), balık tutma veya madencilik (20 dakikada bir)\n\
**Para yönet:** `/bakiye` · `/yatır` (bankadaki para soyulamaz) · `/çek` · `/gönder üye:@… miktar:…` · `/zenginler`\n\
**Şans oyunları:** `/yazı-tura` · `/slot` · `/blackjack` (butonlarla oynanır)\n\
**Soygun:** `/soy üye:@…` biri soyulur (riskli!) · üyesiz `/soy` → sunucu çapında banka soygunu, ekip toplanır\n\
**Mağaza:** `/mağaza` · `/satın-al ürün:…` · `/envanter` (bazı ürünler rol verir)\n\
**Borsa:** `/borsa fiyatlar` · `al` · `sat` · `portföy` — fiyatlar saatte bir değişir",
    },
    Page {
        key: "u_fun",
        label: "Oyunlar ve eğlence",
        emoji: "🎮",
        admin: false,
        body: "**`/oyun`**: `xox` · `dört-bağla` (rakip seçmezsen botla) · `adam-asmaca` (kanala harf yaz) · `bilgi-yarışması` · \
`müzik-yarışması` (ses kanalında şarkıyı ilk bilen kazanır)\n\
**`/eğlence`**: `8top soru:…` · `zar` · `seç seçenekler:pizza, döner` · `doğruluk-cesaret` · `anket` · `klipler`\n\
**`/hayvan`**: ayarlı kanalda arada bir hayvan belirir, **Yakala!** butonuna ilk basan alır → `koleksiyon` · `sıralama` · `liste`\n\
**Kanal oyunları** (sunucu açtıysa): sayma oyunu (sırayla 1, 2, 3…), kelime zinciri (kelime öncekinin son harfiyle başlar)\n\
**Diğer:** `/itiraf metin:…` adın görünmeden itiraf · `/öneri` sunucuya öneri gönder, diğerleri oylar",
    },
    Page {
        key: "u_tools",
        label: "Yapay zekâ ve araçlar",
        emoji: "🤖",
        admin: false,
        body: "**Yapay zekâ**\n\
• Yapay zekâ kanalında yazman yeterli; başka kanallarda botu **etiketle** (@AxBot) ve sorunu yaz. Güncel bilgi için web'de arar.\n\
• `/yaz istek:…` kısa metin yazdırır · `/çevir metin:… dil:…` · `/hayal-et istek:…` resim çizdirir\n\n\
**Arama — `/ara`**: `youtube` · `twitch` · `anime` · `manga` · `pokemon` · `sözlük` · `imgur`\n\
**Emoji — `/emoji büyüt`** bir emojiyi büyük gösterir\n\
**Davetler:** `/davetler` kaç kişi davet ettin · `/davet-sıralama`",
    },
    Page {
        key: "u_community",
        label: "Topluluk",
        emoji: "🤝",
        admin: false,
        body: "**Destek bileti:** bilet kanalındaki **Bilet aç** butonuna bas; sadece sen ve yetkililer görürsün. Bitince `/bilet kapat`.\n\
**Doğum günü:** `/doğum-günü ayarla gün:8 ay:10` — o gün kutlanırsın · `liste` yaklaşanlar · `sil`\n\
**Geçici ses odası:** \"➕ Oda Oluştur\" kanalına gir, sana özel oda açılır. Yönet: `/oda isim` · `limit` · `kilitle` / `aç` · \
`gizle` / `göster` · `at` · `yasakla` · `devret` · `sahiplen`\n\
**Etkinlikler:** duyurudaki **Katılıyorum / Belki** butonları; başlamadan 15 dk önce hatırlatılırsın · `/etkinlik liste`\n\
**Etiketler:** `/etiket göster ad:…` ya da sohbete `!ad` yaz\n\
**Başvurular:** başvuru panelindeki butonla form doldurulur; sonuç sana DM ile gelir",
    },
    // ---------------------------------------------------------------- Yöneticiler
    Page {
        key: "a_setup",
        label: "İlk kurulum ve web paneli",
        emoji: "🛠️",
        admin: true,
        body: "1. **`/kurulum`** — eksik bot izinlerini, rol sırasını ve açık/kapalı özellikleri tek ekranda gösterir.\n\
2. Sunucu Ayarları → Roller → **AxBot rolünü en üste** taşı (yoksa üstündeki rolleri veremez, o üyelere ceza veremez).\n\
3. **`/log-kanalı`** — moderasyon kayıtları ve sunucu olayları (`tür:Sunucu olayları`) için kanal.\n\
4. **`/panel`** — sana özel, 10 dakika geçerli giriş bağlantısı. Tarayıcıda hemen her ayar formla yapılır: \
karşılama, otomod, seviye, rol menüleri, özel komutlar, otomasyonlar, istatistikler, sağlık kontrolü…\n\n\
**Botu başka sunucuya eklemek:** `/kurulum` içindeki *Davet bağlantısı*. Komutlar eklendiği anda görünür.",
    },
    Page {
        key: "a_welcome",
        label: "Karşılama, roller, doğrulama",
        emoji: "👋",
        admin: true,
        body: "**Karşılama — `/karşılama`**\n\
• `hoşgeldin kanal:#… mesaj:…` · `veda` — mesajı boş bırakırsan temalı, GIF'li hazır mesajlar kullanılır\n\
• `tema` — Star Wars, Yüzüklerin Efendisi, Harry Potter, Game of Thrones, Breaking Bad, Marvel, Stranger Things, The Witcher, \
Super Mario ya da hepsi karışık (panelde birden fazlası seçilebilir)\n\
• `test` mesajı dener · `kapat`\n\
• Değişkenler: `{kullanıcı}` `{isim}` `{sunucu}` `{üye}`; birden çok metni `|` ile ayır. Resimli karşılama kartı panelde.\n\n\
**Roller:** `/otorol rol:@…` yeni üyelere · `/rol-paneli` butonla rol alma · panelde **Rol menüleri** (tepki / buton / açılır menü)\n\
**Doğrulama:** `/doğrulama kur kanal:#… rol:@…` (işlem sorusu ya da resimli kod) · `kapat`\n\
**Giriş koruması — `/koruma`:** `hesap-yaşı` (yeni hesaplar) · `baskın kişi:…` · `baskın-bitir` · `durum`",
    },
    Page {
        key: "a_mod",
        label: "Moderasyon ve koruma",
        emoji: "🛡️",
        admin: true,
        body: "**Cezalar:** `/uyar` · `/sustur süre:10dk` · `/susturmayı-kaldır` · `/at` · `/yasakla süre:1g` (süresiz de olur) · `/yasağı-kaldır`\n\
**Kayıtlar:** `/uyarılar` · `/uyarı-sil vaka:…` · `/uyarıları-sıfırla` · `/sicil` (tüm geçmiş) · `/üye-notu ekle/liste/sil`\n\
**Otomatik ceza:** `/uyarı-cezası ekle uyarı_sayısı:3 ceza:Sustur pencere:1sa` — eşiğe ulaşan otomatik cezalandırılır\n\
**Kanal:** `/temizle miktar:50` (üye/bot/bağlantı filtresi) · `/yavaşmod` · `/kilitle` · `/kilidi-aç`\n\n\
**Otomatik moderasyon — `/automod`:** `durum` · `ayarla` (küfür, davet, bağlantı, spam) · `kelime-ekle/sil` · `istisna-ekle/sil` · \
`spam-limiti`. Panelde 10 kural (büyük harf, emoji, bahsetme, zalgo…), her biri için sil / uyar, muaf roller, kısıtlı kanallar, YZ moderasyonu.\n\
**Ban itirazı:** `/başvuru itiraz-kanalı kanal:#…` — yasaklananın DM'ine itiraz butonu gider\n\
**Yıkım koruması** (panel): toplu kanal/rol silme ve yasaklamaya karşı · **Yedek — `/yedek al/liste/yükle/sil`**",
    },
    Page {
        key: "a_systems",
        label: "Sunucu sistemleri",
        emoji: "🎫",
        admin: true,
        body: "**Bilet:** `/bilet kur kanal:#… destek_rolü:@…` → panel gönderilir. Bilette `üstlen` · `ekle` · `çıkar` · `kapat` · `yeniden-aç` · `sil`. \
Panelde konular, SSS (YZ ilk cevabı dener), üye başına sınır.\n\
**Öneriler:** `/öneri-kanalı kanal:#…` · `/öneri-yanıtla numara:1 durum:Kabul edildi`\n\
**Starboard:** `/starboard kanal:#… eşik:3` (panelde ek panolar) · **Çekiliş:** `/çekiliş başlat/bitir/yeniden-çek`\n\
**Başvuru formları:** `/başvuru form-ekle` (5 soruya kadar) · `panel` · `form-liste` · `form-sil`\n\
**Etkinlik:** `/etkinlik oluştur başlık:… zaman:GG.AA.YYYY SS:DD` · `iptal`\n\
**Sayaç kanalları:** `/sayaç ekle tür:Tüm üyeler` · `sil` · `liste` · **Geçici odalar:** `/geçici-oda` · `/oda temizle`\n\
**Doğum günü:** `/doğum-günü kanal kanal:#… rol:@…` · **Hayvan oyunu:** `/hayvan kanal kanal:#…`\n\
**Müzik:** `/müzik 7-24` (bot kanaldan ayrılmaz) · `/müzik dj-rol rol:@…`",
    },
    Page {
        key: "a_level",
        label: "Seviye ve ekonomi yönetimi",
        emoji: "📈",
        admin: true,
        body: "**Seviye**\n\
• `/seviye-ödül ekle seviye:10 rol:@…` · `sil` · `liste`\n\
• `/xp ver` / `/xp al` · `/xp-muaf` — XP kazanılmayacak kanal, kategori ya da rol\n\
• Panel → **Seviye**: duyuru yeri ve metni, XP oranı (%25-300), ödüllerin birikmesi, herkese açık sıralama sayfası; \
**Başarımlar** ve sezon (haftalık/aylık) ödülleri **Topluluk** sayfasında\n\n\
**Ekonomi**\n\
• `/mağaza-yönet ekle ad:VIP fiyat:500 rol:@VIP` · `sil`\n\
• `/ekonomi-yönet ver` · `al` · `sıfırla`",
    },
    Page {
        key: "a_ai",
        label: "YZ, otomasyon ve duyurular",
        emoji: "⚙️",
        admin: true,
        body: "**Yapay zekâ — `/ai`:** `kanal-ekle` (o kanalda her mesaja cevap) · `kanal-sil` · `karakter` (kanala kişilik) · `durum` · `sıfırla`\n\
**Otomatik cevap — `/otocevap ekle tetikleyici:sa cevap:Aleyküm selam {kullanıcı}!`** · `sil` · `liste`\n\
**Etiketler:** `/etiket ekle ad:kurallar içerik:…` → üyeler `!kurallar` yazar\n\
**Duyuru:** `/duyuru gönder kanal:#… renk:mavi` (pencereye metni yaz) · `/duyuru düzenle`\n\
**Bildirimler:** `/youtube ekle` · `/yayın ekle` (Twitch/Kick) · `/akış ekle` (Reddit, Bluesky, RSS, podcast…)\n\
**Haberler:** `/akış haberler kanal:#haberler kategori:Hepsi dil:Türkçe` — teknoloji, oyun, sinema, dizi haberleri ve ücretsiz oyunlar; 5 dilde kaynak, istenen dile çeviri, anahtar kelime süzgeci, anlık ya da günlük, haftalık özet (ayarlar panelde) · `/akış haber-özeti`\n\
**Emoji:** `/emoji ekle` · `/emoji sil`\n\n\
**Yalnızca panelden:** özel komutlar (`!ad`), otomasyonlar (\"şu olunca şunu yap\"), tekrarlı mesajlar, gömülü mesajlar, \
ModMail (DM → yetkili kanalı), sabit mesajlar, sayma / kelime zinciri kanalları, itiraf kanalı, bot profili.",
    },
];

pub fn page(key: &str) -> Option<&'static Page> {
    PAGES.iter().find(|p| p.key == key)
}

#[cfg(test)]
mod tests {
    #[test]
    fn pages_fit_discord_limits() {
        let mut keys = std::collections::HashSet::new();
        for page in super::PAGES {
            assert!(keys.insert(page.key), "yinelenen anahtar {}", page.key);
            assert!(page.body.chars().count() <= 4000, "{} çok uzun", page.key);
            assert!(page.label.chars().count() <= 100);
        }
        // Açılır menüde en fazla 25 seçenek (+ tüm komutlar).
        assert!(super::PAGES.len() < 25);
        assert!(super::PAGES.iter().any(|p| p.admin) && super::PAGES.iter().any(|p| !p.admin));
    }
}
