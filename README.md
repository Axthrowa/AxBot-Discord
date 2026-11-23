# AxTBoT — Rust Discord Botu

MEE6, Carl-bot ve Ticket Tool'un temel özelliklerini tek, hafif bir Rust botunda toplar.
`serenity` + `poise`, `tokio`, `sqlx` (PostgreSQL), `redis`. Hafif: Windows'ta ~35 MB RAM.

## Özellikler

| Modül | Komutlar | Not |
|---|---|---|
| Genel | `/ping` | Gateway, REST, PostgreSQL ve Redis gecikmeleri |
| Web paneli | `/panel` | Tek kullanımlık giriş bağlantısı → tarayıcıda panel: bot durumu, canlı loglar, yeniden başlatma (sahip); sunucu yöneticileri için eklenti kartları ve ayar formları (hoş geldin/veda (resimli kart, özelden karşılama, ek roller), doğrulama, seviye, moderasyon, koruma, olay kayıtları (olay olay seçilebilir), starboard, öneriler, doğum günü, biletler, sosyal akışlar, yapay zekâ, bot profili, hayvan yakalama, ek starboard'lar, müzik, ModMail, yıkım koruması, etiketler, ekonomi / mağaza, sayaç kanalları, geçici odalar, anket, çekiliş, başvuru formları, davet takibi, otomatik cevaplar, yayın ve video duyuruları, topluluk araçları, panel ayarları / bot yöneticisi rolleri), rol menüsü ve otomasyon için hazır şablonlar, eklenti arama kutusu, sıralama ve istatistik sayfaları (14 günlük mesaj grafiği, en hareketli kanallar, üye giriş / çıkış grafiği, 7 günlük kalıcılık oranı, en aktif saatlerin ısı haritası). Kurulum sihirbazı (`/panel/sunucu/{id}/kurulum`): izinler ve temel özellikler adım adım |
| Yardım | `/yardım` | Açılır menülü rehber: üyeler için 7, yöneticiler ('Sunucuyu Yönet') için 6 konu + tüm komut listesi |
| Haberler | `/akış haberler · haber-özeti` | Teknoloji, oyun, sinema, dizi haberleri + ücretsiz oyunlar; 5 dilde 34 kaynak, kaynak dili seçimi ya da seçilen dile YZ çevirisi, anahtar kelime süzgeci, tekrar süzgeci, tam/kompakt görünüm, anlık/günlük gönderim, haftalık özet; panelde "Haberler". **Kişisel haberler:** kanala gönderilen paneldeki ⚙️ Tercihlerim ile herkes kendi kategorilerini seçer, isterse özelden anlık / günlük özet alır (kişi başına saatte en fazla 6); 📰 Haberlerim yalnızca seçilen kategorileri sadece o üyeye gösterir |
| Kurulum | `/kurulum` | Eksik izinler, rol sırası, hangi özelliğin açık olduğu ve davet bağlantısı (yeni sunucuda ilk iş) |
| Auto-mod | `/automod durum · ayarla · kelime-ekle · kelime-sil · istisna-ekle · istisna-sil · spam-limiti` | 10 kural: yasaklı kelime (Türkçe + İngilizce), tekrarlanan yazı, davet, harici bağlantı (izinli alan adları), aşırı büyük harf / emoji / spoiler / bahsetme, zalgo, spam. Her kuralın eylemi panelden seçilir (sil / uyar / sil + uyar). Muaf roller, kısıtlı kanallar (sadece komut / resim / video); düzenlenen mesajlar da kontrol edilir |
| Moderasyon | `/mod yasakla · yasağı-kaldır · at · sustur · susturmayı-kaldır · uyar · uyarılar · uyarı-sil · uyarıları-sıfırla · karantina · karantina-kaldır · uyarı-cezası · log-kanalı` | Vaka numarası, rol hiyerarşisi kontrolü, DM bildirimi, log kanalı; uyarı sayısına göre otomatik ceza (isteğe bağlı zaman penceresi: "son 1 saatte 3 uyarı"). **Uyarı puanı** (`uyar puan:`; otomatik cezalar puan toplamına bakar), **karantina** (roller saklanır, karantina rolü verilir, geri alınınca roller iade edilir; çıkıp giren yine karantinada), **sebep kısayolları** (panelde "kısa = tam metin"), sağ tık **Hızlı ceza** (üye ya da mesaja sağ tık → yetkiye göre düğmeler) |
| Destek bileti | `/bilet kur · üstlen · ekle · çıkar · kapat · yeniden-aç · sil` | Buton ya da konu seçilen açılır menü ile özel kanal, "Üstlen" butonu, üye başına bilet sınırı, özel karşılama metni; kapatınca konuşma kaydı (.txt) log kanalına ve DM'e. İstenirse kapatılan bilet silinmez, saklanır (yeniden açılabilir). Hareketsiz bilet N saat sonra otomatik kapanır (1 saat önce uyarı); kapatınca açana 1-5 ⭐ memnuniyet sorusu (puanlar log kanalına, ortalama panelde). Panelde bilet istatistikleri (son 30 gün: ilk cevap ve çözüm süresi, yetkili başına sayılar); bilette sağ tık **YZ cevap taslağı** (yalnızca yetkiliye görünür) |
| Seviye | `/seviye göster · sıralama · başarımlar · prestij · ödül ekle/sil/liste · muaf · xp ver/al` | `/seviye göster` resimli rütbe kartı gösterir. `/seviye sıralama dönem:` ile haftalık / aylık sıralama (yeni gelenler de yarışabilir); panelden sezon duyuru kanalı ve ilk üçe para ödülü. Redis'te canlı XP, 30 sn'de bir PostgreSQL'e toplu yedek. Panelden: duyuru yeri ve mesajı, TP oranı (x0.25–x3), role göre XP çarpanı (ör. takviyecilere 2 kat), ödül türü (biriktir / sadece en yüksek), XP listeleri (izin ya da yasak), herkese açık sıralama sayfası. Ses XP süzgeci: AFK kanalı, sesi kapalı/sağır ve kanalda yalnız geçen süre XP kazandırmaz. `/seviye prestij` XP'yi sıfırlayıp prestij yıldızı verir |
| Starboard | `/starboard` · (panelden) | Eşiği geçen ⭐ mesajlar ayrı kanala kopyalanır (sahibinin ve botların yıldızı sayılmaz); panelden başka emojiler için 9 ek pano (ör. 😂 → komik anlar) |
| Etiketler | `/etiket göster · ekle · sil · liste` | Sohbette `!ad` yazınca kayıtlı metin gönderilir (bellekte önbellekli) |
| Öneriler | `/öneri gönder · kanal · yanıtla` | Butonla oylama, tartışma alt başlığı, sonuç DM'i |
| Davet takibi | `/davetler · davet-sıralama` | Kullanılan davet, kullanım sayısı karşılaştırmasıyla bulunur (botta 'Sunucuyu Yönet' gerekir); sahte (7 günden yeni) ve ayrılanlar ayrı sayılır |
| Sayaç kanalları | `/sayaç ekle · sil · liste` | Üye / insan / bot / takviye / hedef / metin, ses ve toplam kanal / rol sayısı / bir roldeki üye sayısı / çevrimiçi üye ve çevrimiçi rekoru (Presence Intent açıksa) / kripto para fiyatı (CoinGecko); 10 dk'da bir güncellenir |
| Doğum günleri | `/doğum-günü ayarla · sil · liste · kanal` | Kutlama mesajı + bir günlük rol; panelden kutlama saati, mesaj şablonu, yaşlı mesaj (`{yaş}`) ve doğum yılı izni |
| Geçici odalar | `/geçici-oda` · `/oda isim · limit · kilitle · aç · sahiplen · yasakla · yasağı-kaldır · at · gizle · göster · devret · sahip · temizle` | "Oda Oluştur" kanalına girene özel oda; boşalınca silinir (botta 'Üyeleri Taşı' gerekir) |
| YouTube | `/youtube ekle · sil · liste` | Yeni video duyurusu; API anahtarı gerekmez (RSS, 5 dk) |
| Sosyal akışlar | `/akış ekle · sil · liste` | Reddit (subreddit), Bluesky (kullanıcı), RSS/Atom ve podcast paylaşımlarını duyurur; hesap ya da anahtar gerekmez, 5 dk'da bir. Özel metin (`{başlık} {bağlantı} {kaynak}`), rol etiketi. X / Instagram / TikTok açık akış sunmaz: `.env`'e `RSS_BRIDGE_URL` (RSSHub uyumlu köprü) yazılırsa kullanıcı adıyla eklenir |
| Arama | `/ara youtube · twitch · anime · manga · pokemon · sözlük · imgur · kripto · gaz` | YouTube (yt-dlp), Twitch (anahtar gerekir), Kitsu, PokéAPI, Urban Dictionary, Imgur (`IMGUR_CLIENT_ID` gerekir) |
| Emoji | `/emoji ekle · büyüt · sil` | Başka sunucunun emojisini ya da bir resim bağlantısını sunucuya ekler (botta 'İfadeleri Yönet' gerekir) |
| Ekonomi | `/ekonomi bakiye · günlük · çalış · yatır · çek · gönder · soy · zenginler` | Sunucuya özel para (🪙), cüzdan + banka, günlük seri bonusu; `/ekonomi çalış tür:` ile balık tutma ve madencilik (20 dakikada bir, nadir buluntular); `/ekonomi soy` üye seçilmeden çalıştırılırsa herkesin butonla katıldığı banka soygunu başlar. Panelden sunucuya özel para birimi adı/simgesi ve günlük banka faizi (binde, üye başına üst sınır) |
| Şans oyunları | `/ekonomi yazı-tura · slot · blackjack` | Blackjack butonlarla oynanır |
| Sahte borsa | `/ekonomi borsa fiyatlar · al · sat · portföy` | 6 hayali şirketin hissesi sunucu parasıyla alınıp satılır; fiyatlar saatte bir değişir |
| Mağaza | `/ekonomi mağaza · satın-al · envanter`, `/mağaza-yönet`, `/ekonomi-yönet` | Satılık roller ve ürünler; yönetici para ver/al/sıfırla |
| Doğrulama | `/doğrulama kur · kapat` | Butonla rol alma; isteğe bağlı toplama sorusu ya da resimli kod (captcha). Cevap Redis'te 5 dk |
| Oyunlar | `/oyun xox · dört-bağla · adam-asmaca · bilgi-yarışması · müzik-yarışması` | Botla veya üyeyle; kazanana 🪙 ödül; adam asmaca ve bilgi yarışması kanaldaki herkese açık |
| Eğlence | `/eğlence 8top · zar · seç · doğruluk-cesaret · anket` | Anket Discord'un yerleşik anketiyle |
| Duyuru | `/duyuru gönder · düzenle` | Embed oluşturucu: metin açılan pencereye yazılır (çok satırlı), renk, görsel, rol etiketi |
| Otomatik cevap | `/otocevap ekle · sil · liste` | Kelime/tam/başında eşleşme, `{kullanıcı}`, kanal başına 5 sn bekleme; bellekte önbellekli |
| Rol menüleri | (panelden) | Emoji tepkisi, buton ya da açılır menüyle rol alma; tek seçim modu (renk rolleri); yetkili roller dağıtılamaz. Davranışlar: normal, doğrulama (yalnızca verir), ters (tepki rolü alır), bağlayıcı (geri alınamaz), geçici (süre sonunda geri alınır) |
| Özel komutlar | (panelden) `!ad` | Düz ya da gömülü cevap, özelden cevap, rol ver/al, role ve kanala kısıtlama, bekleme süresi, tetikleyen mesajı silme; regex tetikleyici ve değişkenler (`{kullanıcı}` `{isim}` `{kimlik}` `{sunucu}` `{kanal}` `{üye_sayısı}` `{argümanlar}` `{1}`–`{9}` `{rastgele:a/b}`) |
| Tekrarlı mesajlar | (panelden) | Seçilen kanala 5 dakika – 7 gün aralıkla aynı mesaj; kimseyi etiketlemez; "kanalı temizle" seçeneğiyle belirli aralıklarla kanal temizlenir (sabitlenenler kalır) |
| Gömülü mesajlar | (panelden) | Başlık, metin, renk, resimler ve alt yazıyla gömülü mesaj gönderme, sonradan düzenleme ve silme |
| Otomasyon | (panelden) | "Şu olunca şunu yap": mesaj, üye girişi/çıkışı, rol, tepki ve alt başlık olaylarına mesaj, özel mesaj, rol, tepki, alt başlık ya da silme ile karşılık |
| Başarımlar | `/seviye başarımlar` | Mesaj, tepki, ses dakikası ve alt başlık sayısına göre rozetler; 16 hazır + özel başarımlar, ödül rolü ve XP |
| Etkinlik planlama | `/etkinlik oluştur · liste · iptal` | Tarihli etkinlik mesajı, Katılıyorum / Belki / Katılmıyorum butonları, başlamadan 15 dk önce katılacaklara hatırlatma; aynı etkinlik Discord'un kendi **Etkinlikler** listesine de eklenir (iptalde silinir) |
| Sunucu yedeği | `/yedek al · liste · yükle · sil` | Rol ve kanal yapısını kaydeder; geri yükleme yalnızca EKSİK rolleri ve kanalları oluşturur, hiçbir şeyi silmez (mesajlar ve kanala özel izinler yedeklenmez). Panelden zamanlanmış otomatik yedek (1/3/7/30 günde bir, son 3 saklanır) ve eklenti ayarlarını JSON olarak dışa / içe aktarma |
| ModMail | (panelden) | Üye bota özelden yazar; mesaj yetkili kanalında o üyeye ait alt başlığa düşer, yetkililer oradan cevaplar (`.` ile başlayan mesaj iç not, `!kapat` konuşmayı bitirir) |
| Yıkım koruması | (panelden) | Kısa sürede çok kanal / rol silen ya da açan, üye yasaklayan / atan yetkilinin yetkili rollerini alır (ya da atar / yasaklar); kimin yaptığı denetim kaydından okunur. Ek kalkanlar: güvenilmeyen birinin açtığı webhook silinir, eklediği bot atılır, bir role verdiği yönetici yetkisi ya da üyeye verdiği yetkili rol geri alınır |
| Discord AutoMod | (panelden) | Discord'un kendi kuralları: hazır filtreler (küfür, cinsel içerik, hakaret), en fazla 1000 yasaklı kelime (+ isteğe bağlı susturma), toplu etiket sınırı, spam; engellenenler bir kanala bildirilir. Mesaj Discord tarafında engellenir, bot kapalıyken de çalışır |
| Sunucu kilidi | `/koruma kilit` | Baskında bütün yazı kanallarında @everyone'ın yazmasını tek seferde kapatır; açınca her kanalın eski izni aynen geri gelir |
| Takma ad denetimi | (panelden, Moderasyon) | Küfürlü takma adları ve listede üste çıkmak için başa konan işaretleri (!, ., 0…) düzeltir; girişte ve ad değişince |
| Takviye teşekkürü | (panelden, Topluluk) | Sunucuya takviye yapana seçilen kanalda teşekkür mesajı ({kullanıcı}, {sayı}…) ve isteğe bağlı ek rol |
| Roller | `/rol otomatik · panel · bilgi · süreli-ver · süreli-al · süreliler · toplu` | Yeni üyelere otomatik rol, butonlu rol paneli, rol bilgisi; **süreli rol**: süre dolunca kendiliğinden geri alınır (bot kapalıyken dolanlar açılışta); `/rol toplu` bir rolü herkese / insanlara / botlara / bir rolün sahiplerine toplu verir ya da alır |
| Çekiliş | `/çekiliş başlat · bitir · yeniden-çek` · (panelden) | Butonla katılım, süre dolunca otomatik çekiliş; katılım şartları (gereken rol, en az seviye, hesap yaşı) ve katılımı 2 kez sayılan ek şans rolü |
| Film / dizi / oyun önerisi | `/tavsiye film · dizi · oyun · ara` | Önce feelm.org (IPv6 üzerinden; bu ağdan IPv4 adreslerine ulaşılamıyor), ulaşılamazsa 10 dk boyunca: film/dizi TMDB'den (özet doğrudan Türkçe), oyun RAWG'dan (özet yapay zekâ ya da Google ile Türkçeye çevrilir); IMDb puanı IMDb'nin resmî günlük puan dosyasından; tür, yıl ve en düşük puana göre popüler ilk üç sayfadan rastgele öneri; 🎲 Başka öner butonu; kanalda son 30 öneri tekrar edilmez; yetişkin içerik atlanır. `.env`: `TMDB_API_KEY`, `RAWG_API_KEY` |
| Vurgu bildirimi | `/vurgu ekle · sil · liste` | Üyenin seçtiği kelime (en fazla 10) görebildiği bir kanalda geçince özelden haber verilir; o kanalda son 5 dk'da yazdıysa ya da 5 dk içinde bildirim aldıysa verilmez |
| Moderatör notları | `/üye-notu ekle · liste · sil` | Üyeye bildirilmeyen, yalnızca yetkililerin gördüğü notlar |
| Klip kanalı (AxT Capture) | `/eğlence klipler` · (panelden) | Seçilen kanala gönderilen videolar sayılır ve sıralanır; AxT Capture'ın Discord webhook ayarına o kanalın webhook adresi yazılınca klipler tek tıkla gelir |
| Sağ tık menüsü | (Uygulamalar menüsü) | Mesaja sağ tık: Çevir (Türkçe), Buraya kadar özetle (son ~100 mesaj), YZ cevap taslağı, Yetkiliye bildir, Starboard'a ekle, Hızlı ceza. Üyeye sağ tık: Sicil, Not ekle, Hızlı ceza (üye). Slash komut sınırına sayılmaz |
| Yapay zekâ yardımcıları | (panelden) | Yapay zekâ moderasyonu: kelime listesinin kaçırdığı gizlenmiş küfürleri (“a.q”, “4mk”) yalnızca şüpheli mesajlarda modele sorar. Bilet asistanı: bileti açanın ilk mesajına sunucunun SSS metninden cevap vermeyi dener, bulamazsa yetkililere bırakır. İkisi de varsayılan kapalı. **Bilgi bankası** (panel → YZ): yöneticilerin yazdığı sunucu bilgileri; sohbette ve bilet asistanında soruyla ilgili maddeler kullanılır, burada olmayan sunucu bilgisi uydurulmaz |
| Sağlık ve bildirim | (panelden) · `GET /saglik` | Sağlık kontrolü sayfası silinmiş kanal / rol, eksik izin ve eksik anahtarları listeler; silinen rol / kanal temel ayarlardan kendiliğinden temizlenir. Kritik hatalar `ALERT_WEBHOOK_URL` webhook'una (yoksa bot sahibine özelden) gelir; `/saglik` dış izleyiciler içindir |
| Topluluk araçları | `/afk` · `/itiraf` · (panelden) | AFK bildirimi; sabit (yapışkan) mesajlar; sayma oyunu (rekor tutulur); kelime zinciri; adsız itiraflar (isteğe bağlı yetkili kaydı); kalıcı roller (30 gün içinde dönen üyeye eski rolleri); ses kanalında dakika başına XP |
| Çeviri ve yardım | `/çevir` · `/yardım` | Yapay zekâ ile çeviri; komutların kategorilere göre listesi |
| Hayvan yakalama | `/hayvan koleksiyon · sıralama · liste · kanal` | Seçilen kanalda 10-20 mesajda bir hayvan belirir; ilk basan yakalar (18 tür, 4 nadirlik) |
| Başvurular | `/başvuru form-ekle · panel · form-sil · form-liste · itiraz-kanalı` | En fazla 5 soruluk form, yetkili kanalında Kabul/Ret (+sebep), sonuç DM'i; yasak DM'inde itiraz butonu, kabulde yasak kalkar |
| Yapay zekâ | `/ai kanal-ekle · kanal-sil · karakter · durum · sıfırla` · `/yaz` | NVIDIA NIM veya Gemini (iki anahtar da tanımlıysa biri yanıt vermeyince diğerine geçilir); güncel bilgi için web araması (yerel SearXNG). Sunucu kişiliği, kanal başına karakter (8 hazır + 15 özel); `/yaz` tek seferlik metin yazdırır |
| Gizlilik ve veri | `/bilgi gizlilik · verilerimi-sil · sunucu-verisini-sil` | Gizlilik metni (`/gizlilik`, İngilizcesi `/privacy` web sayfası); üye kendi verisini, sunucu sahibi sunucunun bütün verisini siler (bot sunucudan çıkar). Bot sunucudan çıkarılınca veriler 30 gün sonra kendiliğinden silinir. Komut adları Discord'u İngilizce kullananlara İngilizce görünür |
| Resim üretimi | `/hayal-et` | Yazdığın şeyin resmini çizer (NVIDIA NIM, FLUX.1-dev; `NVIDIA_API_KEY` ile). Sunucunun yasaklı kelimeleri ve modelin güvenlik süzgeci geçerli; üye başına 30 sn bekleme |
| Profil | `/kartım profil · rep · ayarla · sıfırla` | Profil: seviye kartı, bakiye, itibar, başarım sayısı ve katılım tarihi. İtibar günde bir kez verilir; `ayarla` ile kişisel seviye kartı rengi/arka planı |
| Kişisel rütbe kartı | `/kartım ayarla · sıfırla` | Üye kendi `/seviye göster` kartının vurgu rengini, arka plan resmini ve yazı tipini seçer; yöneticiler başkasınınkini sıfırlayabilir |
| Bot profili | (panelden) | Botun sunucuya özel takma adı, avatarı, afişi ve hakkında yazısı; durum ve etkinlik yazısı (bot sahibi, Sistem sayfası) |
| Yayın | `/yayın ekle · sil · liste` | Twitch / Kick canlı yayın duyurusu |
| Müzik | `/çal`, `/müzik geç · durdur · duraklat · devam · şimdi · sıra · ses · döngü · karıştır · ayrıl` | YouTube araması/bağlantısı, Spotify (şarkı/albüm/liste → YouTube'da aranır), SoundCloud; bağlantılar güvenlik için yalnızca bilinen müzik/video sitelerinden (YouTube, Spotify, SoundCloud, Bandcamp, Twitch, Vimeo…) kabul edilir; çalma listeleri (100 şarkıya kadar); 5 dk boşta kalınca veya kanal boşalınca ayrılır |
| Müzik+ | `/müzik 7-24 · otomatik · radyo · sözler · sar · sil · taşı · dj-rol · liste-kaydet · liste-çal · listelerim · liste-sil · söyle · filtre · efekt` | 7/24 modu, benzer şarkılarla otomatik çalma (YouTube Mix), 7 canlı radyo, lrclib şarkı sözleri; DJ rolü (kontrol komutlarını role kısıtlar); kişiye özel kayıtlı çalma listeleri (10 liste, 50'şer şarkı); `/müzik söyle` yazdığını ses kanalında sesli okur (200 karakter); `/müzik filtre` bass boost / nightcore / vaporwave / 8D (sıradaki şarkıdan itibaren); `/müzik efekt` sunucunun ve Discord'un ses panosu seslerini müziğin üstüne çalar |

## Çalıştırma

Bot normal bir Windows programı olarak çalışır. Veritabanları (PostgreSQL, Redis) ve web araması
(SearXNG) arka planda Docker'da çalışır; onlarla uğraşmanız gerekmez.

| Ne yapmak istiyorsun | Nasıl |
|---|---|
| Botu başlat | **`Botu Başlat.cmd`** dosyasına çift tıkla |
| Botu durdur | Bot penceresini kapat (veya içinde Ctrl+C) |
| Arka planda başlat (pencere yok) | `Botu Başlat.cmd -Arkaplan` → durdurmak için `Botu Durdur.cmd` |
| Çökerse? | Bot bir bekçiyle (`scripts\run-bot.cmd`) çalışır: beklenmedik kapanışta 10 sn sonra kendiliğinden açılır; Botu Durdur ya da pencereyi kapatmak kalıcı durdurur |
| Logları gör | Bot penceresi veya `logs\bot.YYYY-MM-DD.log` (son 7 gün) |

`Botu Başlat.cmd` sırasıyla: Docker Desktop kapalıysa açar, servisleri başlatır, kod değiştiyse
botu yeniden derler (`cargo build --release`) ve botu kendi penceresinde açar. Pencereyi kapatmak
botu düzgünce kapatır (bekleyen XP kaydedilir).

Gerekenler: [Rust](https://rustup.rs), Docker Desktop ve Visual Studio Build Tools (C++ ve CMake).
Müzik için ayrıca `yt-dlp`, `deno` (YouTube koruması için) ve `ffmpeg` (SoundCloud, filtreler, efektler) PATH'te ya da proje içindeki
`tools\` klasöründe olmalı: `winget install yt-dlp.yt-dlp DenoLand.Deno Gyan.FFmpeg`. yt-dlp'yi güncel tutun
(`yt-dlp -U` veya `winget upgrade yt-dlp.yt-dlp`); YouTube değiştikçe eski sürümler çalışmaz.

Servisleri elle yönetmek için:

```bash
docker compose up -d --wait      # PostgreSQL + Redis + SearXNG (sadece 127.0.0.1)
docker compose down              # durdur (veriler volume'larda kalır)
```

İstenirse bot da Docker'da çalıştırılabilir (exe ile **aynı anda değil**; aynı token ile iki bot bağlanır):
`docker compose --profile docker up -d --build`

## Web paneli

Bot `WEB_BIND` adresinde (varsayılan `127.0.0.1:8080`) bir web sunucusu açar: `/panel` (yönetim) ve
`/itiraz/<anahtar>` (ban itirazı). Giriş Discord'dan: `/panel` komutu tek kullanımlık, 10 dk geçerli bir
bağlantı verir; oturum Redis'te 7 gün tutulur, formlar CSRF korumalıdır. `.env`'e `DISCORD_CLIENT_SECRET` yazılırsa giriş sayfasında
"Discord ile giriş yap" düğmesi de çıkar (Developer Portal → OAuth2 → Redirects'e Sistem sayfasında yazan `…/panel/oauth` adresi eklenmeli). Bot sahibi (Developer Portal'daki
sahip/takım) durum kartlarını, canlı logları ve yeniden başlatmayı görür; diğerleri sadece 'Sunucuyu Yönet'
izni olan sunucuları. Sunucuda (Docker) `tunnel` servisi (cloudflared hızlı tünel, hesapsız) dışarıya
`https://….trycloudflare.com` adresi verir; bot adresi `TUNNEL_METRICS_URL/quicktunnel`'dan öğrenir.
Sabit adres istenirse `WEB_PUBLIC_URL`.

## 7/24 sunucuda çalıştırma

**Güncelleme:** `Sunucu Güncellemesi Hazırla.cmd` → `dist\AxBot-guncelleme.tar.gz` (veritabanı yedeği yok) →
arkadaşın aynı komutlarla kurar; sunucudaki veriler korunur. Panelde **Sistem → Derleme** tarihi değişir.

**Başkasının sunucusu (dosyaları o yükler):** `Sunucu Paketi Hazırla.cmd` → `dist\AxBot-sunucu.tar.gz`
(hazır x86_64 imaj + veritabanı yedeği + `.env` + `kur.sh` + `BENIOKU.txt`). Sunucuda:
`tar -xzf AxBot-sunucu.tar.gz && cd AxBot-sunucu && sudo bash kur.sh`. Bot `/opt/axbot`'a kurulur,
dışarıya port açmaz (`deploy/docker-compose.sunucu.yml`), gece 03:30 yedek alır. Güncellemede aynı
paket yeniden kurulur; sunucudaki veriler korunur. ARM sunucuda imaj orada derlenir.

**SSH erişimin varsa:** `deploy\sunucuya-gonder.ps1 -Sunucu root@IP [-Port 22] [-VeriTasi]`
(anahtarı kendisi oluşturur, şifre bir kez sorulur; Docker yoksa `deploy/server-setup.sh`'ı çalıştırır).

Her iki yolda da bot sunucuya taşınınca bu bilgisayarda kapatılmalı (aynı token ile iki bot olur);
SSH betiği `.sunucuda-calisiyor` dosyası bırakır ve `Botu Başlat.cmd` artık açmaz.
YouTube veri merkezi IP'lerini sık engeller: sunucuda YouTube müziği çalışmayabilir.

## Yapılandırma (`.env`)

`.env.example` dosyasını `.env` olarak kopyalayıp doldurun. `.env` asla paylaşılmamalı.

| Değişken | Zorunlu | Açıklama |
|---|---|---|
| `DISCORD_TOKEN` | ✅ | Developer Portal → Bot → Token |
| `DEV_GUILD_ID` | | Doluysa komutlar sadece bu sunucuya kaydedilir (test için). Boşsa bot bulunduğu **her sunucuya** anında kaydeder; yeni eklendiği sunucuya da katıldığı an (itiraz sunucuları hariç). Başka sunucuya ekleme: `SUNUCUYA-EKLEME.md`, kurulum kontrolü: `/kurulum` |
| `POSTGRES_*`, `DATABASE_URL`, `REDIS_PASSWORD`, `REDIS_URL` | ✅ | Docker'da adresler otomatik ayarlanır |
| `NVIDIA_API_KEY` / `GEMINI_API_KEY` | | Yapay zekâ (biri yeterli) |
| `AI_MODEL`, `AI_FALLBACK_MODEL`, `AI_PROVIDER`, `AI_SYSTEM_PROMPT` | | Varsayılan: `nvidia/nemotron-3.5-super-vl-120b-a12b` / `gemini-flash-lite-latest`; yedek sırası asıl model → `AI_FALLBACK_MODEL` (`nvidia/nemotron-3.5-lightning-30b-a3b`) → Gemini |
| `SEARXNG_URL`, `SEARXNG_SECRET` | | Yapay zekânın web araması (üyelik gerektirmez) |
| `TAVILY_API_KEY` | | SearXNG yerine Tavily kullanmak için |
| `TWITCH_CLIENT_ID/SECRET`, `KICK_CLIENT_ID/SECRET` | | Yayın duyuruları |
| `RUST_LOG` | | Log seviyesi, örn. `info,serenity=warn,sqlx=warn` |
| `LOG_DIR` | | Log dosyalarının klasörü (varsayılan `logs`); boşsa sadece konsola yazılır |

Discord Developer Portal → Bot → **Message Content Intent** açık olmalı (auto-mod ve yapay zekâ için).

## Geliştirme

```bash
cargo run                        # debug modunda çalıştır (servisler docker compose ile açık olmalı)
cargo test                       # birim testleri
cargo clippy --all-targets -- -W clippy::unwrap_used -W clippy::expect_used
```

Gerçek API'lerle yapay zekâ + arama testi:
`cargo test live_ai_answers_with_current_info -- --ignored --nocapture`

## Mimari

```
src/
├── main.rs           başlatma, framework, güvenli kapanış (Ctrl+C, pencere kapatma, SIGTERM)
├── config.rs         .env → tip güvenli yapılandırma (gizli değerler loglanmaz)
├── telemetry.rs      konsol + günlük dönen log dosyaları
├── data.rs           paylaşılan durum (PgPool, Redis, modül önbellekleri)
├── net.rs            ortak HTTP istemcisi (IPv4 öncelikli, önbellekli DNS)
├── db/  cache/       PostgreSQL havuzu + migration'lar, Redis bağlantısı
├── events/           merkezi gateway event yönlendiricisi
├── automod/          regex filtre (bellekte derlenmiş), Redis spam sayacı
├── moderation/ modlog/  vaka kaydı, hiyerarşi, süre ayrıştırma, log kanalı
├── tickets/          buton etkileşimleri, konuşma kaydı
├── leveling/         Lua script ile atomik XP, toplu PostgreSQL yedeği
├── ai/               NIM/Gemini, araç çağırma ile web araması, sohbet geçmişi (Redis)
├── streams/          Twitch Helix / Kick API, periyodik kontrol
├── music/            songbird (DAVE şifreli ses), yt-dlp, kuyruk ve olaylar
└── commands/         slash komutlar
migrations/           sqlx migration'ları (binary'ye gömülür, açılışta uygulanır)
```

Kurallar: hiçbir yerde `.unwrap()` / `.expect()` yok; hatalar `anyhow` / `thiserror` ile
taşınır ve `tracing` ile loglanır. Mesaj başına işlemler (auto-mod, XP) veritabanına gitmez.
