# AxBot'u başka bir sunucuya ekleme

Bot aynı anda birden fazla sunucuda çalışır. Her sunucunun ayarları, ekonomisi, XP'si ve
kayıtları ayrıdır; bir sunucudaki ayar diğerini etkilemez.

## 1. Botu ekle

Bu bağlantıyı aç ve listeden arkadaşının sunucusunu seç (gerekli izinler seçili gelir):

https://discord.com/oauth2/authorize?client_id=1557460476525420605&scope=bot+applications.commands&permissions=1477757496567

- Listede sunucuyu görmek için o sunucuda **Sunucuyu Yönet** iznin olmalı. Yoksa bağlantıyı
  arkadaşına gönder, o eklesin.
- Arkadaşın eklerken "bu bot herkese açık değil" hatası alırsa: Developer Portal → AxBot →
  **Bot** → **Public Bot** seçeneğini aç → kaydet.

Komutlar bot eklendiği anda o sunucuda görünür (beklemeye gerek yok).

## 2. Botun rolünü en üste taşı

Sunucu Ayarları → **Roller** → "AxBot" rolünü listenin **en üstüne** sürükle.
Aksi halde bot üstündeki rolleri veremez, o rollerdeki üyeleri susturamaz veya atamaz.

## 3. `/kurulum` yaz

Eksik izinleri, rol sırasını ve hangi özelliklerin açık olduğunu tek ekranda gösterir.
Her satırın yanında o özelliği açan komut yazar.

## 4. İstediğin özellikleri aç (örnekler)

| Özellik | Komut |
|---|---|
| Moderasyon logu | `/log-kanalı kanal:#mod-log` |
| Sunucu olay logu | `/log-kanalı kanal:#olay-log tür:Sunucu olayları` |
| Karşılama / veda | `/karşılama hoşgeldin` · `/karşılama veda` |
| Otomatik rol | `/rol otomatik rol:@Üye` |
| Destek biletleri | `/bilet kur kanal:#destek` |
| Doğrulama | `/doğrulama kur` |
| Seviye ödülleri | `/seviye-ödül ekle` |
| Starboard | `/starboard kanal:#yıldızlar` |
| Öneriler | `/öneri-kanalı kanal:#öneriler` |
| Yapay zekâ sohbet kanalı | `/ai kanal-ekle` |
| Ban itirazı | `/başvuru itiraz-kanalı kanal:#itirazlar` |

## Bilmen gerekenler

- Bot **bu bilgisayarda** çalışıyor: bilgisayar kapalıyken bot iki sunucuda da çevrimdışı olur.
  7/24 açık kalması için sunucuya taşıma adımı gerekiyor (README → "7/24 sunucu").
- Ban itirazında yasaklanan kişinin botla ortak bir sunucusu kalmaz. Arkadaşının sunucusu için ya
  ayrı bir itiraz sunucusu açılmalı ya da 7/24 sunucuda web formu açılmalı.
- Botun kendisi tek bir yerde çalışır; yeni sunucu için `.env` veya kod değişikliği gerekmez.
