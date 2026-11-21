//! Resim olarak çizilen kartlar: karşılama kartı, rütbe (seviye) kartı ve doğrulama captcha'sı.
//!
//! Çizim CPU işidir; çağıran taraf `spawn_blocking` ile ayrı iş parçacığında çalıştırır.
//! Yardımcılar şarkı kartıyla (`music::render`) ortaktır.

use std::io::Cursor;
use std::time::Duration;

use ab_glyph::PxScale;
use anyhow::Context as _;
use image::imageops::{self, FilterType};
use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_line_segment_mut, draw_text_mut, text_size};
use serde::{Deserialize, Serialize};

use crate::music::render::{blend, crop_square, fill_circle, fill_round_rect, fit, rgba, BOLD, REGULAR, SEMI};
use crate::rand_util;
use crate::settings::PluginConfig;

/// Eş zamanlı çoklu kart render isteklerinin threadpool'u boğmasını engelleyen semafor (2–4 paralel işlem).
pub static RENDER_SEMAPHORE: std::sync::LazyLock<tokio::sync::Semaphore> = std::sync::LazyLock::new(|| {
    let permits = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
    tokio::sync::Semaphore::new(permits)
});

const WELCOME_W: u32 = 1000;
const WELCOME_H: u32 = 400;
const RANK_W: u32 = 1000;
const RANK_H: u32 = 280;
const CAPTCHA_W: u32 = 420;
const CAPTCHA_H: u32 = 150;
/// İndirilen resimlerin en fazla boyutu ve bekleme süresi.
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(6);

/// Kartlarda seçilebilen yazı tipleri: (anahtar, görünen ad).
pub const FONTS: &[(&str, &str)] = &[
    ("noto", "Noto Sans (varsayılan)"),
    ("poppins", "Poppins"),
    ("ptserif", "PT Serif"),
    ("pacifico", "Pacifico (el yazısı)"),
];

pub struct FontSet {
    pub bold: &'static ab_glyph::FontRef<'static>,
    pub semi: &'static ab_glyph::FontRef<'static>,
    pub regular: &'static ab_glyph::FontRef<'static>,
}

/// Anahtarın yazı tipi ailesi; bilinmeyen anahtarda varsayılan. Tek kalınlığı olan ailelerde aynı dosya kullanılır.
pub fn font_set(key: &str) -> Option<FontSet> {
    use crate::music::render as r;
    let (bold, semi, regular) = match key {
        "poppins" => (&r::POPPINS_BOLD, &r::POPPINS_SEMI, &r::POPPINS_REGULAR),
        "ptserif" => (&r::PTSERIF_BOLD, &r::PTSERIF_BOLD, &r::PTSERIF_REGULAR),
        "pacifico" => (&r::PACIFICO, &r::PACIFICO, &r::PACIFICO),
        _ => (&BOLD, &SEMI, &REGULAR),
    };
    Some(FontSet { bold: bold.as_ref()?, semi: semi.as_ref()?, regular: regular.as_ref()? })
}

pub const DEFAULT_TITLE: &str = "{isim} sunucuya katıldı";
pub const DEFAULT_SUBTITLE: &str = "{üye}. üye";
/// Karıştırılabilen karakterler (0/O, 1/I/L) çıkarılmış captcha alfabesi.
const CAPTCHA_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
pub const CAPTCHA_LEN: usize = 5;

/// Karşılama eklentisinin ek ayarları (kanal, mesaj ve otorol `guild_settings` tablosundadır).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WelcomeSettings {
    /// Karşılama mesajına resimli kart eklenir (temalı GIF'in yerine).
    pub card: bool,
    pub card_title: Option<String>,
    pub card_subtitle: Option<String>,
    /// `#RRGGBB`
    pub text_color: String,
    pub bg_color: String,
    /// Arka plan resminin üstündeki rengin opaklığı (0-100).
    pub overlay: u8,
    pub background_url: Option<String>,
    /// Yeni üyeye özelden mesaj gönderilir.
    pub dm: bool,
    pub dm_message: Option<String>,
    /// Otomatik role ek olarak verilecek roller.
    pub extra_roles: Vec<u64>,
    /// Doğrulamada işlem sorusu yerine resimli kod (captcha) sorulur.
    pub captcha: bool,
    /// Kartın yazı tipi (`FONTS` anahtarı).
    pub font: String,
    /// Otomatik rol ve ek roller katılımdan bu kadar dakika sonra verilir (0 = hemen).
    pub role_delay_mins: u32,
    /// Hazır karşılama / veda mesajlarının temaları (`greetings::THEMES` anahtarları; boşsa hepsi karışık).
    pub themes: Vec<String>,
}

impl Default for WelcomeSettings {
    fn default() -> Self {
        Self {
            card: false,
            card_title: None,
            card_subtitle: None,
            text_color: "#FFFFFF".to_owned(),
            bg_color: "#23272A".to_owned(),
            overlay: 60,
            background_url: None,
            dm: false,
            dm_message: None,
            extra_roles: Vec::new(),
            captcha: false,
            font: "noto".to_owned(),
            role_delay_mins: 0,
            themes: Vec::new(),
        }
    }
}

impl PluginConfig for WelcomeSettings {
    const KEY: &'static str = "welcome";
}

impl WelcomeSettings {
    pub fn title(&self) -> &str {
        self.card_title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(DEFAULT_TITLE)
    }

    pub fn subtitle(&self) -> &str {
        self.card_subtitle.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(DEFAULT_SUBTITLE)
    }
}

/// `#RRGGBB` ya da `RRGGBB`.
pub fn parse_hex(raw: &str) -> Option<[u8; 3]> {
    let hex = raw.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let part = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some([part(0)?, part(2)?, part(4)?])
}

/// Kart başlığı / alt yazısı şablonu: etiket yerine görünen ad kullanılır (resimde etiket olmaz).
pub fn card_text(template: &str, name: &str, guild: &str, members: u64) -> String {
    template
        .replace("{kullanıcı}", name)
        .replace("{kullanici}", name)
        .replace("{isim}", name)
        .replace("{sunucu}", guild)
        .replace("{üye}", &members.to_string())
        .replace("{uye}", &members.to_string())
}

fn encode_jpeg(img: RgbaImage) -> anyhow::Result<Vec<u8>> {
    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
    let mut out = Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82)
        .encode_image(&rgb)
        .context("kart kodlanamadı")?;
    Ok(out.into_inner())
}

/// Resmi hedef boyutu tamamen dolduracak şekilde ölçekleyip ortadan kırpar.
fn cover_fit(src: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 {
        return RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
    }
    let scale = (w as f32 / sw as f32).max(h as f32 / sh as f32);
    let (rw, rh) = (((sw as f32 * scale).ceil() as u32).max(w), ((sh as f32 * scale).ceil() as u32).max(h));
    let resized = imageops::resize(src, rw, rh, FilterType::Triangle);
    imageops::crop_imm(&resized, (rw - w) / 2, (rh - h) / 2, w, h).to_image()
}

/// Yuvarlak avatar (çevresinde halka). Avatar yoksa düz renkli daire.
fn draw_avatar(img: &mut RgbaImage, avatar: Option<&RgbaImage>, cx: i32, cy: i32, radius: i32, ring: [u8; 3]) {
    fill_circle(img, cx, cy, radius + 6, ring);
    let Some(avatar) = avatar else {
        fill_circle(img, cx, cy, radius, [88, 101, 242]);
        return;
    };
    let side = (radius * 2) as u32;
    let small = imageops::resize(&crop_square(avatar), side, side, FilterType::Triangle);
    for (dx, dy, p) in small.enumerate_pixels() {
        let (fx, fy) = (dx as f32 + 0.5 - radius as f32, dy as f32 + 0.5 - radius as f32);
        let cov = (radius as f32 - (fx * fx + fy * fy).sqrt() + 0.5).clamp(0.0, 1.0);
        let (px, py) = (cx - radius + dx as i32, cy - radius + dy as i32);
        if cov > 0.0 && px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
            blend(img.get_pixel_mut(px as u32, py as u32), [p.0[0], p.0[1], p.0[2]], cov * f32::from(p.0[3]) / 255.0);
        }
    }
}

pub struct WelcomeCard {
    pub title: String,
    pub subtitle: String,
    pub text: [u8; 3],
    pub background: [u8; 3],
    /// 0-100
    pub overlay: u8,
    /// `FONTS` anahtarı.
    pub font: String,
}

/// Karşılama kartını JPEG olarak çizer.
pub fn render_welcome(card: &WelcomeCard, avatar: Option<&RgbaImage>, backdrop: Option<&RgbaImage>) -> anyhow::Result<Vec<u8>> {
    let Some(FontSet { bold, regular, .. }) = font_set(&card.font) else {
        anyhow::bail!("yazı tipleri yüklenemedi");
    };
    let mut img = match backdrop {
        Some(backdrop) => {
            let mut img = cover_fit(backdrop, WELCOME_W, WELCOME_H);
            let alpha = f32::from(card.overlay.min(100)) / 100.0;
            for px in img.pixels_mut() {
                blend(px, card.background, alpha);
                px.0[3] = 255;
            }
            img
        }
        None => RgbaImage::from_pixel(WELCOME_W, WELCOME_H, rgba(card.background)),
    };

    let center = WELCOME_W as i32 / 2;
    draw_avatar(&mut img, avatar, center, 130, 90, card.text);

    let max_w = WELCOME_W - 80;
    let title = fit(&card.title, bold, 50.0, max_w);
    let (tw, _) = text_size(PxScale::from(50.0), bold, &title);
    draw_text_mut(&mut img, rgba(card.text), center - tw as i32 / 2, 248, PxScale::from(50.0), bold, &title);

    let subtitle = fit(&card.subtitle, regular, 32.0, max_w);
    let (sw, _) = text_size(PxScale::from(32.0), regular, &subtitle);
    // Alt yazı biraz soluk: metin rengi arka plana %25 karıştırılır.
    let mut soft = rgba(card.text);
    blend(&mut soft, card.background, 0.25);
    draw_text_mut(&mut img, soft, center - sw as i32 / 2, 316, PxScale::from(32.0), regular, &subtitle);
    encode_jpeg(img)
}

pub struct RankCard {
    pub name: String,
    pub level: u32,
    /// 1'den başlayan sıra; hiç XP'si yoksa `None`.
    pub rank: Option<u64>,
    pub current: u64,
    pub needed: u64,
    pub total: u64,
    pub accent: [u8; 3],
    /// `FONTS` anahtarı.
    pub font: String,
}

/// Rütbe kartını JPEG olarak çizer.
pub fn render_rank(card: &RankCard, avatar: Option<&RgbaImage>, backdrop: Option<&RgbaImage>) -> anyhow::Result<Vec<u8>> {
    let Some(FontSet { bold, semi, regular }) = font_set(&card.font) else {
        anyhow::bail!("yazı tipleri yüklenemedi");
    };
    let white = Rgba([255, 255, 255, 255]);
    let grey = Rgba([160, 166, 178, 255]);
    // Kişisel arka plan varsa kartı doldurur; yazılar okunsun diye üstüne yarı saydam panel gelir.
    let mut img = match backdrop {
        Some(backdrop) => {
            let mut img = cover_fit(backdrop, RANK_W, RANK_H);
            for px in img.pixels_mut() {
                px.0[3] = 255;
            }
            img
        }
        None => RgbaImage::from_pixel(RANK_W, RANK_H, Rgba([17, 18, 22, 255])),
    };
    fill_round_rect(&mut img, 16, 16, RANK_W - 32, RANK_H - 32, 28, [30, 32, 40], if backdrop.is_some() { 0.74 } else { 1.0 });

    draw_avatar(&mut img, avatar, 140, RANK_H as i32 / 2, 86, card.accent);

    let text_x = 264;
    let right = RANK_W as i32 - 52;
    // Sağ üst: SIRA ve SEVİYE.
    let level = card.level.to_string();
    let (lw, _) = text_size(PxScale::from(64.0), bold, &level);
    draw_text_mut(&mut img, rgba(card.accent), right - lw as i32, 44, PxScale::from(64.0), bold, &level);
    let (llw, _) = text_size(PxScale::from(26.0), semi, "SEVİYE");
    let level_label_x = right - lw as i32 - 12 - llw as i32;
    draw_text_mut(&mut img, rgba(card.accent), level_label_x, 76, PxScale::from(26.0), semi, "SEVİYE");

    let rank = card.rank.map_or_else(|| "—".to_owned(), |r| format!("#{r}"));
    let (rw, _) = text_size(PxScale::from(64.0), bold, &rank);
    let rank_x = level_label_x - 28 - rw as i32;
    draw_text_mut(&mut img, white, rank_x, 44, PxScale::from(64.0), bold, &rank);
    let (rlw, _) = text_size(PxScale::from(26.0), semi, "SIRA");
    let rank_label_x = rank_x - 12 - rlw as i32;
    draw_text_mut(&mut img, grey, rank_label_x, 76, PxScale::from(26.0), semi, "SIRA");

    // İsim (sıra yazısına taşmayacak genişlikte) ve XP.
    let name_w = (rank_label_x - text_x - 24).max(120) as u32;
    let name = fit(&card.name, semi, 44.0, name_w.min(RANK_W - text_x as u32 - 60));
    draw_text_mut(&mut img, white, text_x, 132, PxScale::from(44.0), semi, &name);
    let xp = format!("{} / {} XP", compact(card.current), compact(card.needed));
    let (xw, _) = text_size(PxScale::from(28.0), regular, &xp);
    draw_text_mut(&mut img, grey, right - xw as i32, 146, PxScale::from(28.0), regular, &xp);

    // İlerleme çubuğu.
    let (bar_x, bar_y, bar_h) = (text_x as u32, 198, 36);
    let bar_w = right as u32 - bar_x;
    fill_round_rect(&mut img, bar_x, bar_y, bar_w, bar_h, bar_h / 2, [58, 62, 74], 1.0);
    let ratio = if card.needed == 0 { 0.0 } else { (card.current as f32 / card.needed as f32).clamp(0.0, 1.0) };
    let filled = ((bar_w as f32 * ratio) as u32).max(if card.current > 0 { bar_h } else { 0 });
    if filled > 0 {
        fill_round_rect(&mut img, bar_x, bar_y, filled.min(bar_w), bar_h, bar_h / 2, card.accent, 1.0);
    }
    let total = format!("Toplam {} XP", compact(card.total));
    draw_text_mut(&mut img, Rgba([110, 116, 128, 255]), text_x, 100, PxScale::from(22.0), regular, &total);
    encode_jpeg(img)
}

/// 12345 → "12.3B", 1500000 → "1.5M" (kart dar olduğu için büyük sayılar kısaltılır).
fn compact(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{:.1}B", n as f64 / 1000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Rastgele captcha kodu.
pub fn captcha_code() -> String {
    (0..CAPTCHA_LEN)
        .filter_map(|_| CAPTCHA_ALPHABET.get(rand_util::below(CAPTCHA_ALPHABET.len() as u64) as usize))
        .map(|b| char::from(*b))
        .collect()
}

/// Kodu, botların okumasını zorlaştıran çizgi ve kaymalarla resme çizer.
pub fn render_captcha(code: &str) -> anyhow::Result<Vec<u8>> {
    let Some(bold) = BOLD.as_ref() else {
        anyhow::bail!("yazı tipleri yüklenemedi");
    };
    let mut img = RgbaImage::from_pixel(CAPTCHA_W, CAPTCHA_H, Rgba([28, 30, 38, 255]));
    let tone = || 90 + rand_util::below(140) as u8;
    let point = |max: u32| rand_util::below(u64::from(max)) as f32;
    for _ in 0..7 {
        let colour = Rgba([tone(), tone(), tone(), 255]);
        draw_line_segment_mut(&mut img, (point(CAPTCHA_W), point(CAPTCHA_H)), (point(CAPTCHA_W), point(CAPTCHA_H)), colour);
    }
    for _ in 0..260 {
        let (x, y) = (rand_util::below(u64::from(CAPTCHA_W)) as u32, rand_util::below(u64::from(CAPTCHA_H)) as u32);
        img.put_pixel(x, y, Rgba([tone(), tone(), tone(), 255]));
    }
    let step = (CAPTCHA_W as i32 - 60) / code.chars().count().max(1) as i32;
    for (i, c) in code.chars().enumerate() {
        let size = 62.0 + rand_util::below(22) as f32;
        let x = 34 + i as i32 * step + rand_util::below(14) as i32;
        let y = 22 + rand_util::below(36) as i32;
        let colour = Rgba([170 + rand_util::below(85) as u8, 170 + rand_util::below(85) as u8, 170 + rand_util::below(85) as u8, 255]);
        draw_text_mut(&mut img, colour, x, y, PxScale::from(size), bold, &c.to_string());
    }
    for _ in 0..3 {
        let colour = Rgba([tone(), tone(), tone(), 255]);
        draw_line_segment_mut(&mut img, (0.0, point(CAPTCHA_H)), (CAPTCHA_W as f32, point(CAPTCHA_H)), colour);
    }
    encode_jpeg(img)
}

/// Bir dosyayı indirir. Adres https değilse, herkese açık değilse ya da dosya `max` bayttan büyükse `None`.
/// Yönlendirmeler izlenmez (bkz. `net::fetch_client`).
pub async fn fetch_bytes(url: &str, max: usize) -> Option<Vec<u8>> {
    if !url.starts_with("https://") || !crate::music::extra::is_public_url(url).await {
        return None;
    }
    let client = crate::net::fetch_client().ok()?;
    let mut response = client.get(url).timeout(FETCH_TIMEOUT).send().await.ok()?;
    if !response.status().is_success() || response.content_length().is_some_and(|len| len > max as u64) {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > max {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
}

/// Bir resmi indirip çözer. Adres herkese açık değilse, çok büyükse ya da resim değilse `None`.
pub async fn fetch_image(url: &str) -> Option<RgbaImage> {
    let bytes = fetch_bytes(url, MAX_IMAGE_BYTES).await?;
    // Çözme CPU işidir; büyük resimlerde async iş parçacığını bekletmesin.
    tokio::task::spawn_blocking(move || image::load_from_memory(&bytes).ok().map(|i| i.to_rgba8())).await.ok()?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_jpeg(bytes: &[u8]) -> bool {
        bytes.starts_with(&[0xFF, 0xD8]) && bytes.len() > 1000
    }

    #[test]
    fn parses_hex_colours() {
        assert_eq!(parse_hex("#FFffff"), Some([255, 255, 255]));
        assert_eq!(parse_hex("23272A"), Some([0x23, 0x27, 0x2A]));
        assert_eq!(parse_hex("#FFF"), None);
        assert_eq!(parse_hex("#GGGGGG"), None);
        assert_eq!(parse_hex("#ççç"), None);
    }

    #[test]
    fn card_text_uses_names_not_mentions() {
        assert_eq!(card_text(DEFAULT_TITLE, "Ali", "S", 7), "Ali sunucuya katıldı");
        assert_eq!(card_text("{kullanıcı} · {sunucu} · {üye}", "Ali", "Sunucu", 42), "Ali · Sunucu · 42");
    }

    #[test]
    fn renders_cards_with_and_without_images() -> anyhow::Result<()> {
        let avatar = RgbaImage::from_pixel(64, 48, Rgba([200, 50, 50, 255]));
        let backdrop = RgbaImage::from_pixel(300, 500, Rgba([10, 120, 200, 255]));
        let welcome = WelcomeCard {
            title: "Çok uzun bir isim sunucuya katıldı ".repeat(4),
            subtitle: "42. üye".to_owned(),
            text: [255, 255, 255],
            background: [35, 39, 42],
            overlay: 60,
            font: "noto".to_owned(),
        };
        assert!(is_jpeg(&render_welcome(&welcome, Some(&avatar), Some(&backdrop))?));
        assert!(is_jpeg(&render_welcome(&welcome, None, None)?));

        let rank = RankCard { name: "Şükrü Öğüt".to_owned(), level: 12, rank: Some(3), current: 429, needed: 1337, total: 15_000, accent: [88, 101, 242], font: "poppins".to_owned() };
        assert!(is_jpeg(&render_rank(&rank, Some(&avatar), None)?));
        assert!(is_jpeg(&render_rank(&rank, Some(&avatar), Some(&backdrop))?));
        let empty = RankCard { name: "Y".to_owned(), level: 0, rank: None, current: 0, needed: 0, total: 0, accent: [255, 0, 0], font: "yok-boyle-bir-tip".to_owned() };
        assert!(is_jpeg(&render_rank(&empty, None, None)?));
        // Her yazı tipi ailesi yüklenebilmeli ve Türkçe harfleri çizebilmeli.
        for (key, _) in FONTS {
            let styled = WelcomeCard { font: (*key).to_owned(), title: "Şükrü Öğüt İçin Güneş".to_owned(), ..WelcomeCard { title: String::new(), subtitle: "42. üye".to_owned(), text: [255, 255, 255], background: [35, 39, 42], overlay: 60, font: String::new() } };
            assert!(is_jpeg(&render_welcome(&styled, None, None)?), "{key}");
        }

        // Göz kontrolü: CARD_PREVIEW_DIR verilirse kartlar o klasöre yazılır.
        if let Ok(dir) = std::env::var("CARD_PREVIEW_DIR") {
            let dir = std::path::Path::new(&dir);
            std::fs::write(dir.join("hosgeldin.jpg"), render_welcome(&welcome, Some(&avatar), Some(&backdrop))?)?;
            std::fs::write(dir.join("hosgeldin-duz.jpg"), render_welcome(&welcome, None, None)?)?;
            std::fs::write(dir.join("seviye.jpg"), render_rank(&rank, Some(&avatar), None)?)?;
            std::fs::write(dir.join("seviye-arkaplan.jpg"), render_rank(&rank, Some(&avatar), Some(&backdrop))?)?;
            for (key, _) in FONTS {
                let styled = WelcomeCard { font: (*key).to_owned(), title: "Şükrü Öğüt sunucuya katıldı".to_owned(), subtitle: "42. üye · İyi eğlenceler".to_owned(), text: [255, 255, 255], background: [35, 39, 42], overlay: 60 };
                std::fs::write(dir.join(format!("yazi-{key}.jpg")), render_welcome(&styled, Some(&avatar), None)?)?;
            }
            std::fs::write(dir.join("captcha.jpg"), render_captcha("AB7KX")?)?;
        }
        Ok(())
    }

    #[test]
    fn captcha_codes_avoid_ambiguous_characters() -> anyhow::Result<()> {
        for _ in 0..200 {
            let code = captcha_code();
            assert_eq!(code.chars().count(), CAPTCHA_LEN);
            assert!(code.chars().all(|c| !"0O1IL".contains(c)), "{code}");
        }
        assert!(is_jpeg(&render_captcha("AB7KX")?));
        Ok(())
    }

    #[test]
    fn compact_numbers() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(12_340), "12.3B");
        assert_eq!(compact(1_500_000), "1.5M");
    }

    #[tokio::test]
    async fn refuses_non_https_and_local_addresses() {
        assert!(fetch_image("http://example.com/a.png").await.is_none());
        assert!(fetch_image("https://127.0.0.1/a.png").await.is_none());
        assert!(fetch_image("https://localhost/a.png").await.is_none());
        assert!(fetch_image("dosya.png").await.is_none());
    }
}
