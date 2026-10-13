//! Şarkı kartı görseli: kapaktan bulanık arka plan, yuvarlak köşeli kapak, kaynak rozeti,
//! şarkı/sanatçı, ilerleme çubuğu ve ses bilgisi. Saf Rust (image + imageproc + ab_glyph).
//!
//! Çizim CPU işidir; çağıran taraf `spawn_blocking` ile ayrı iş parçacığında çalıştırır.

use std::io::Cursor;
use std::sync::LazyLock;
use std::time::Duration;

use ab_glyph::{FontRef, PxScale};
use anyhow::Context as _;
use image::imageops::{self, FilterType};
use image::{RgbImage, Rgba, RgbaImage};
use imageproc::drawing::{draw_text_mut, text_size};

/// Discord sohbette resmi en fazla ~550 px genişlikte gösterir; daha "yüksek" bir oran kartın
/// sohbette büyük görünmesini sağlar.
pub const WIDTH: u32 = 1000;
pub const HEIGHT: u32 = 560;
const COVER: u32 = 300;
const MARGIN: i32 = 48;
const TEXT_X: i32 = MARGIN + COVER as i32 + 36;
const TEXT_W: u32 = WIDTH - TEXT_X as u32 - MARGIN as u32;

pub(crate) static BOLD: LazyLock<Option<FontRef<'static>>> =
    LazyLock::new(|| FontRef::try_from_slice(include_bytes!("../../assets/fonts/NotoSans-Bold.ttf")).ok());
pub(crate) static SEMI: LazyLock<Option<FontRef<'static>>> =
    LazyLock::new(|| FontRef::try_from_slice(include_bytes!("../../assets/fonts/NotoSans-SemiBold.ttf")).ok());
pub(crate) static REGULAR: LazyLock<Option<FontRef<'static>>> =
    LazyLock::new(|| FontRef::try_from_slice(include_bytes!("../../assets/fonts/NotoSans-Regular.ttf")).ok());

// Karşılama ve rütbe kartlarında seçilebilen ek yazı tipleri (SIL Open Font License).
pub(crate) static POPPINS_BOLD: LazyLock<Option<FontRef<'static>>> =
    LazyLock::new(|| FontRef::try_from_slice(include_bytes!("../../assets/fonts/Poppins-Bold.ttf")).ok());
pub(crate) static POPPINS_SEMI: LazyLock<Option<FontRef<'static>>> =
    LazyLock::new(|| FontRef::try_from_slice(include_bytes!("../../assets/fonts/Poppins-SemiBold.ttf")).ok());
pub(crate) static POPPINS_REGULAR: LazyLock<Option<FontRef<'static>>> =
    LazyLock::new(|| FontRef::try_from_slice(include_bytes!("../../assets/fonts/Poppins-Regular.ttf")).ok());
pub(crate) static PTSERIF_BOLD: LazyLock<Option<FontRef<'static>>> =
    LazyLock::new(|| FontRef::try_from_slice(include_bytes!("../../assets/fonts/PTSerif-Bold.ttf")).ok());
pub(crate) static PTSERIF_REGULAR: LazyLock<Option<FontRef<'static>>> =
    LazyLock::new(|| FontRef::try_from_slice(include_bytes!("../../assets/fonts/PTSerif-Regular.ttf")).ok());
pub(crate) static PACIFICO: LazyLock<Option<FontRef<'static>>> =
    LazyLock::new(|| FontRef::try_from_slice(include_bytes!("../../assets/fonts/Pacifico-Regular.ttf")).ok());

/// Kartta yazacak her şey.
pub struct CardData {
    pub title: String,
    pub artist: Option<String>,
    /// "SPOTIFY", "YOUTUBE", "RADYO"…
    pub source: String,
    pub accent: [u8; 3],
    pub position: Duration,
    /// `None` = canlı yayın.
    pub duration: Option<Duration>,
    pub paused: bool,
    pub looping: bool,
    pub volume_percent: u32,
    pub upcoming: usize,
    pub requester: String,
    /// Sıradaki şarkının adı.
    pub next: Option<String>,
    /// Alt ortadaki yazı: "<sunucu adı> MÜZİK".
    pub brand: String,
}

/// Kartı JPEG olarak çizer. Tasarım: kapağın renginden gelen yumuşak ışık (neon), cam panel,
/// ince yazılar (Poppins), ikonlu kutucuklar ve renk geçişli ilerleme çubuğu.
pub fn render(card: &CardData, cover: Option<&RgbaImage>) -> anyhow::Result<Vec<u8>> {
    let (bold, semi, regular) = match (POPPINS_BOLD.as_ref(), POPPINS_SEMI.as_ref(), POPPINS_REGULAR.as_ref()) {
        (Some(b), Some(s), Some(r)) => (b, s, r),
        _ => anyhow::bail!("yazı tipleri yüklenemedi"),
    };
    // Vurgu rengi kapaktan alınır (yoksa platform rengi); bütün ışıklar bu renkte.
    let accent = cover.and_then(dominant_colour).unwrap_or(card.accent);
    let white = Rgba([255, 255, 255, 255]);
    let soft = Rgba([196, 202, 210, 255]);
    let dim = Rgba([140, 147, 156, 255]);
    let mut img = background(cover, accent);

    // Dış ışık + cam panel.
    let (px, py, pw, ph) = (16u32, 16u32, WIDTH - 32, HEIGHT - 32);
    for (grow, alpha) in [(6u32, 0.07f32), (4, 0.12), (2, 0.2)] {
        stroke_round_rect(&mut img, px - grow, py - grow, pw + 2 * grow, ph + 2 * grow, 30 + grow, accent, alpha);
    }
    fill_round_rect(&mut img, px, py, pw, ph, 30, [8, 10, 14], 0.46);
    stroke_round_rect(&mut img, px, py, pw, ph, 30, accent, 0.65);
    stroke_round_rect(&mut img, px + 1, py + 1, pw - 2, ph - 2, 29, [255, 255, 255], 0.06);

    // Üst satır: platform logosu + adı · durum hapı.
    let top = 42;
    let logo_size = 34;
    let (label, label_x) = match card.source.as_str() {
        "SPOTIFY" => {
            draw_spotify_logo(&mut img, MARGIN + logo_size / 2, top + 15, logo_size / 2);
            ("Spotify".to_owned(), MARGIN + logo_size + 12)
        }
        "YOUTUBE" => {
            draw_youtube_logo(&mut img, MARGIN, top, logo_size + 14, logo_size);
            ("YouTube".to_owned(), MARGIN + logo_size + 24)
        }
        _ => {
            fill_circle(&mut img, MARGIN + 11, top + 16, 10, accent);
            (title_case(&card.source), MARGIN + 32)
        }
    };
    draw_text_mut(&mut img, white, label_x, top - 1, PxScale::from(28.0), semi, &label);

    let (status, dot) = if card.paused {
        ("DURAKLATILDI", [250, 190, 60])
    } else if card.duration.is_none() {
        ("CANLI", [255, 80, 80])
    } else {
        ("ÇALIYOR", [52, 211, 120])
    };
    let (sw, _) = text_size(PxScale::from(17.0), semi, status);
    let pill_w = sw + 50;
    let pill_x = WIDTH - MARGIN as u32 - pill_w;
    fill_round_rect(&mut img, pill_x, top as u32, pill_w, 36, 18, [20, 24, 30], 0.7);
    stroke_round_rect(&mut img, pill_x, top as u32, pill_w, 36, 18, [255, 255, 255], 0.16);
    fill_circle(&mut img, pill_x as i32 + 20, top + 18, 6, dot);
    draw_text_mut(&mut img, white, pill_x as i32 + 34, top + 7, PxScale::from(17.0), semi, status);

    // Kapak: arkasında renkli ışık, ince parlak çerçeve.
    let cover_y = 104u32;
    let cx = MARGIN as u32;
    for (grow, alpha) in [(14u32, 0.05f32), (10, 0.08), (6, 0.12), (3, 0.2)] {
        fill_round_rect(&mut img, cx - grow, cover_y - grow, COVER + 2 * grow, COVER + 2 * grow, 26 + grow, accent, alpha);
    }
    match cover {
        Some(c) => {
            let square = crop_square(c);
            let resized = imageops::resize(&square, COVER, COVER, FilterType::Lanczos3);
            paste_rounded(&mut img, &resized, cx, cover_y, 24);
        }
        None => {
            fill_round_rect(&mut img, cx, cover_y, COVER, COVER, 24, accent, 0.35);
            draw_notes(&mut img, MARGIN + COVER as i32 / 2, cover_y as i32 + COVER as i32 / 2);
        }
    }
    stroke_round_rect(&mut img, cx, cover_y, COVER, COVER, 24, lighten(accent, 0.35), 0.85);

    // Sağ sütun: şarkı adı (2 satıra kadar) ve sanatçı.
    let mut y = cover_y as i32 + 2;
    for line in wrap(&card.title, bold, 54.0, TEXT_W, 2) {
        draw_text_mut(&mut img, white, TEXT_X, y, PxScale::from(54.0), bold, &line);
        y += 66;
    }
    let artist = card.artist.clone().unwrap_or_else(|| {
        if card.duration.is_none() { "Canlı radyo yayını" } else { "Bilinmeyen sanatçı" }.to_owned()
    });
    let artist = fit(&artist, regular, 30.0, TEXT_W);
    draw_text_mut(&mut img, soft, TEXT_X, y + 2, PxScale::from(30.0), regular, &artist);

    // Kutucuklar (ikon + yazı): SES · DÖNGÜ · İSTEYEN — kapağın alt kısmına hizalı.
    let chip_y = cover_y as i32 + COVER as i32 - 86;
    let chips = [
        ("volume", format!("SES %{}", card.volume_percent), false),
        ("loop", format!("DÖNGÜ {}", if card.looping { "AÇIK" } else { "KAPALI" }), card.looping),
        ("user", card.requester.to_uppercase(), false),
    ];
    let mut x = TEXT_X;
    for (icon, text, highlighted) in &chips {
        let text = fit(text, semi, 15.0, 220);
        let (tw, _) = text_size(PxScale::from(15.0), semi, &text);
        let w = tw + 54;
        if x + w as i32 > TEXT_X + TEXT_W as i32 {
            break;
        }
        if *highlighted {
            fill_round_rect(&mut img, x as u32, chip_y as u32, w, 36, 18, accent, 0.55);
        } else {
            fill_round_rect(&mut img, x as u32, chip_y as u32, w, 36, 18, [255, 255, 255], 0.08);
        }
        stroke_round_rect(&mut img, x as u32, chip_y as u32, w, 36, 18, [255, 255, 255], 0.16);
        if let Some(glyph) = small_icon(icon) {
            imageops::overlay(&mut img, glyph, i64::from(x + 14), i64::from(chip_y + 9));
        }
        draw_text_mut(&mut img, white, x + 38, chip_y + 9, PxScale::from(15.0), semi, &text);
        x += w as i32 + 10;
    }
    let next = match (&card.next, card.upcoming) {
        (Some(next), n) if n > 1 => format!("SIRADAKİ: {next}  (+{} şarkı daha)", n - 1),
        (Some(next), _) => format!("SIRADAKİ: {next}"),
        (None, _) => "SIRADAKİ: sıra boş (Ekle butonuyla şarkı ekleyebilirsiniz)".to_owned(),
    };
    let next = fit(&next, regular, 17.0, TEXT_W);
    draw_text_mut(&mut img, dim, TEXT_X, chip_y + 54, PxScale::from(17.0), regular, &next);

    // İlerleme çubuğu: ince iz, renk geçişli dolgu, ışıklı tutamaç.
    let bar_y = 448;
    let bar_x = MARGIN;
    let bar_w = WIDTH as i32 - 2 * MARGIN;
    fill_round_rect(&mut img, bar_x as u32, bar_y as u32, bar_w as u32, 8, 4, [255, 255, 255], 0.16);
    let ratio = match card.duration {
        Some(total) if !total.is_zero() => (card.position.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0),
        _ => 1.0,
    };
    let filled = ((bar_w as f64) * ratio).round() as i32;
    if filled > 0 {
        gradient_bar(&mut img, bar_x, bar_y, filled.max(8), 8, accent, lighten(accent, 0.4));
    }
    if card.duration.is_some() {
        let kx = bar_x + filled;
        for (r, alpha) in [(20, 0.08f32), (16, 0.14), (13, 0.25)] {
            fill_circle_alpha(&mut img, kx, bar_y + 4, r, lighten(accent, 0.3), alpha);
        }
        fill_circle(&mut img, kx, bar_y + 4, 11, lighten(accent, 0.55));
        stroke_circle(&mut img, kx, bar_y + 4, 11, [255, 255, 255]);
    }

    // Süreler + ortada sunucu adı.
    let times_y = bar_y + 24;
    draw_text_mut(&mut img, white, bar_x, times_y, PxScale::from(22.0), semi, &clock(card.position));
    match card.duration {
        Some(total) => {
            let left = format!("-{}", clock(total.saturating_sub(card.position)));
            let rest = format!(" / {}", clock(total));
            let (lw, _) = text_size(PxScale::from(22.0), semi, &left);
            let (rw, _) = text_size(PxScale::from(22.0), regular, &rest);
            let start = bar_x + bar_w - (lw + rw) as i32;
            draw_text_mut(&mut img, white, start, times_y, PxScale::from(22.0), semi, &left);
            draw_text_mut(&mut img, dim, start + lw as i32, times_y, PxScale::from(22.0), regular, &rest);
        }
        None => {
            let (lw, _) = text_size(PxScale::from(22.0), semi, "CANLI YAYIN");
            draw_text_mut(&mut img, soft, bar_x + bar_w - lw as i32, times_y, PxScale::from(22.0), semi, "CANLI YAYIN");
        }
    }
    draw_spaced(&mut img, &format!("{} MÜZİK", card.brand.to_uppercase()), regular, 14.0, 2, (WIDTH as i32) / 2, times_y + 5, dim);

    let rgb: RgbImage = image::DynamicImage::ImageRgba8(img).to_rgb8();
    let mut out = Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&rgb)
        .context("kart kodlanamadı")?;
    Ok(out.into_inner())
}

/// "CANLI RADYO" → "Canlı Radyo" (Türkçe büyük/küçük harf).
fn title_case(text: &str) -> String {
    text.split(' ')
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => {
                    let rest: String = chars.collect::<String>().replace('I', "ı").to_lowercase();
                    format!("{first}{rest}")
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Harf aralıklı, ortalanmış küçük yazı (sunucu adı).
#[allow(clippy::too_many_arguments)]
fn draw_spaced(img: &mut RgbaImage, text: &str, font: &FontRef<'_>, size: f32, spacing: i32, center_x: i32, y: i32, colour: Rgba<u8>) {
    let scale = PxScale::from(size);
    let text = fit(text, font, size, 360);
    let widths: Vec<i32> = text.chars().map(|c| text_size(scale, font, &c.to_string()).0 as i32).collect();
    let total: i32 = widths.iter().sum::<i32>() + spacing * (widths.len() as i32 - 1).max(0);
    let mut x = center_x - total / 2;
    for (c, w) in text.chars().zip(widths) {
        draw_text_mut(img, colour, x, y, scale, font, &c.to_string());
        x += w + spacing;
    }
}

/// Kart kutucuklarındaki 18 px beyaz ikonlar (bir kez çözülür).
fn small_icon(name: &str) -> Option<&'static RgbaImage> {
    static ICONS: LazyLock<Vec<(&'static str, RgbaImage)>> = LazyLock::new(|| {
        ["volume", "loop", "user"]
            .into_iter()
            .filter_map(|name| {
                let png = super::icons::png(name)?;
                let decoded = image::load_from_memory(png).ok()?.to_rgba8();
                Some((name, imageops::resize(&decoded, 18, 18, FilterType::Lanczos3)))
            })
            .collect()
    });
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, img)| img)
}

/// Kapağın baskın, canlı rengi: renk tonları 24 dilime ayrılır, canlı (doygun + parlak) pikseller
/// oy verir; kazanan tonun doygun, parlak hâli ışık rengi olur (neon görünüm).
fn dominant_colour(cover: &RgbaImage) -> Option<[u8; 3]> {
    const BINS: usize = 24;
    let small = imageops::resize(cover, 48, 48, FilterType::Triangle);
    let mut votes = [0f32; BINS];
    let mut hue_sum = [0f32; BINS];
    for px in small.pixels() {
        let (h, s, v) = rgb_to_hsv(px.0[0], px.0[1], px.0[2]);
        if s < 0.25 || v < 0.2 {
            continue;
        }
        let w = s * s * v;
        let bin = ((h / 360.0) * BINS as f32) as usize % BINS;
        votes[bin] += w;
        hue_sum[bin] += h * w;
    }
    let (best, weight) = votes.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1))?;
    if weight < 1.0 {
        return None;
    }
    Some(hsv_to_rgb(hue_sum[best] / weight, 0.72, 0.96))
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r, g, b].map(|ch| ((ch + m) * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// Rengi beyaza doğru `amount` (0-1) kadar açar.
fn lighten(c: [u8; 3], amount: f32) -> [u8; 3] {
    c.map(|v| (f32::from(v) + (255.0 - f32::from(v)) * amount).round() as u8)
}

/// Soldan sağa `from` → `to` renk geçişli, yuvarlak uçlu çubuk.
fn gradient_bar(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, from: [u8; 3], to: [u8; 3]) {
    let r = h as f32 / 2.0;
    for py in 0..h {
        for px in 0..w {
            let a = coverage(px as f32 + 0.5, py as f32 + 0.5, w as f32, h as f32, r);
            if a <= 0.0 {
                continue;
            }
            let t = px as f32 / w.max(1) as f32;
            let c = [0, 1, 2].map(|i| (f32::from(from[i]) * (1.0 - t) + f32::from(to[i]) * t).round() as u8);
            let (gx, gy) = (x + px, y + py);
            if gx >= 0 && gy >= 0 && (gx as u32) < img.width() && (gy as u32) < img.height() {
                blend(img.get_pixel_mut(gx as u32, gy as u32), c, a);
            }
        }
    }
}

/// Yarı saydam daire (ışık halkası).
fn fill_circle_alpha(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, c: [u8; 3], alpha: f32) {
    for y in (cy - r - 1)..=(cy + r + 1) {
        for x in (cx - r - 1)..=(cx + r + 1) {
            if x < 0 || y < 0 || x as u32 >= img.width() || y as u32 >= img.height() {
                continue;
            }
            let d = (((x - cx) * (x - cx) + (y - cy) * (y - cy)) as f32).sqrt();
            let a = (r as f32 + 0.5 - d).clamp(0.0, 1.0) * alpha;
            if a > 0.0 {
                blend(img.get_pixel_mut(x as u32, y as u32), c, a);
            }
        }
    }
}

/// 2 px kalınlığında daire çizgisi.
fn stroke_circle(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, c: [u8; 3]) {
    for y in (cy - r - 2)..=(cy + r + 2) {
        for x in (cx - r - 2)..=(cx + r + 2) {
            if x < 0 || y < 0 || x as u32 >= img.width() || y as u32 >= img.height() {
                continue;
            }
            let d = (((x - cx) * (x - cx) + (y - cy) * (y - cy)) as f32).sqrt();
            let a = (1.0 - (d - r as f32).abs()).clamp(0.0, 1.0);
            if a > 0.0 {
                blend(img.get_pixel_mut(x as u32, y as u32), c, a);
            }
        }
    }
}

/// Metni kelime kelime en fazla `max_lines` satıra böler; sığmayan son satır "…" ile biter.
fn wrap(text: &str, font: &FontRef<'_>, size: f32, max_w: u32, max_lines: usize) -> Vec<String> {
    let scale = PxScale::from(size);
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for (i, word) in words.iter().enumerate() {
        let candidate = if current.is_empty() { (*word).to_owned() } else { format!("{current} {word}") };
        if text_size(scale, font, &candidate).0 <= max_w {
            current = candidate;
            continue;
        }
        if lines.len() + 1 == max_lines {
            // Son satır: kalan her şey sığdırılıp "…" ile kısaltılır.
            let rest = words.get(i..).map(|w| w.join(" ")).unwrap_or_default();
            let last = if current.is_empty() { rest } else { format!("{current} {rest}") };
            lines.push(fit(&last, font, size, max_w));
            return lines;
        }
        if current.is_empty() {
            lines.push(fit(word, font, size, max_w));
        } else {
            lines.push(std::mem::take(&mut current));
            current = (*word).to_owned();
        }
    }
    if !current.is_empty() {
        lines.push(fit(&current, font, size, max_w));
    }
    lines
}

/// Spotify logosu: yeşil daire ve üç kavisli ses dalgası (kodla çizilir; her boyutta keskin).
fn draw_spotify_logo(img: &mut RgbaImage, cx: i32, cy: i32, r: i32) {
    let green = [30, 215, 96];
    let dark = [18, 18, 18];
    fill_circle(img, cx, cy, r, green);
    // (yükseklik, yarı genişlik, kalınlık) — daire yarıçapına göre oranlar.
    let arcs = [(-0.30_f32, 0.62_f32, 0.17_f32), (0.03, 0.52, 0.145), (0.33, 0.41, 0.12)];
    let big_r = 1.7_f32;
    let rf = r as f32;
    for py in (cy - r)..=(cy + r) {
        for px in (cx - r)..=(cx + r) {
            if px < 0 || py < 0 || px as u32 >= img.width() || py as u32 >= img.height() {
                continue;
            }
            let (u, v) = ((px - cx) as f32 / rf, (py - cy) as f32 / rf);
            let mut best = 0.0_f32;
            for (y_top, half_w, thickness) in arcs {
                let (ax, ay) = (0.0, y_top + big_r);
                let (dx, dy) = (u - ax, v - ay);
                let len = (dx * dx + dy * dy).sqrt().max(1e-6);
                let on_arc_x = ax + dx / len * big_r;
                let d = if on_arc_x.abs() <= half_w && dy < 0.0 {
                    (len - big_r).abs()
                } else {
                    // Yuvarlak uçlar: en yakın uç noktaya uzaklık.
                    let end_y = ay - (big_r * big_r - half_w * half_w).sqrt();
                    let ex = half_w.copysign(u);
                    ((u - ex).powi(2) + (v - end_y).powi(2)).sqrt()
                };
                let coverage = ((thickness / 2.0 - d) * rf + 0.5).clamp(0.0, 1.0);
                best = best.max(coverage);
            }
            // Daire içinde kalan kısımlar boyanır.
            let inside = ((u * u + v * v).sqrt() * rf <= rf - 0.5) as u8 as f32;
            if best > 0.0 && inside > 0.0 {
                blend(img.get_pixel_mut(px as u32, py as u32), dark, best);
            }
        }
    }
}

/// YouTube logosu: kırmızı yuvarlak dikdörtgen ve beyaz oynat üçgeni.
fn draw_youtube_logo(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32) {
    fill_round_rect(img, x as u32, y as u32, w as u32, h as u32, (h as f32 * 0.28) as u32, [255, 0, 0], 1.0);
    // Yumuşak kenarlı üçgen: her pikselin üçgen kenarlarına uzaklığına bakılır.
    let (x0, y0) = (x as f32 + w as f32 * 0.40, y as f32 + h as f32 * 0.29);
    let (x1, y1) = (x0, y as f32 + h as f32 * 0.71);
    let (x2, y2) = (x as f32 + w as f32 * 0.68, y as f32 + h as f32 * 0.5);
    let edge = |ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32| {
        let (ex, ey) = (bx - ax, by - ay);
        ((px - ax) * ey - (py - ay) * ex) / (ex * ex + ey * ey).sqrt()
    };
    for py in y..y + h {
        for px in x..x + w {
            if px < 0 || py < 0 || px as u32 >= img.width() || py as u32 >= img.height() {
                continue;
            }
            let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
            let d = edge(x0, y0, x1, y1, fx, fy).min(edge(x1, y1, x2, y2, fx, fy)).min(edge(x2, y2, x0, y0, fx, fy));
            // Üç kenarın da iç tarafındaki noktalar pozitif uzaklıktadır.
            let coverage = (d + 0.5).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend(img.get_pixel_mut(px as u32, py as u32), [255, 255, 255], coverage);
            }
        }
    }
}

/// Kapaktan bulanık, koyulaştırılmış arka plan (kapak yoksa vurgu renginde degrade).
fn background(cover: Option<&RgbaImage>, accent: [u8; 3]) -> RgbaImage {
    let mut img = match cover {
        Some(c) => {
            // Önce küçültüp bulanıklaştırmak hızlıdır; sonra büyütülür.
            let small = imageops::resize(c, 80, 45, FilterType::Triangle);
            let blurred = imageops::blur(&small, 4.0);
            imageops::resize(&blurred, WIDTH, HEIGHT, FilterType::Triangle)
        }
        None => RgbaImage::from_pixel(WIDTH, HEIGHT, Rgba([18, 20, 26, 255])),
    };
    // Koyulaştır ve kapağın olduğu sol üstten, vurgu renginde yumuşak bir ışık yay.
    let (gx, gy) = ((MARGIN + COVER as i32 / 2) as f32, 250.0f32);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let dx = (x as f32 - gx) / WIDTH as f32;
        let dy = (y as f32 - gy) / HEIGHT as f32;
        let glow = (1.0 - (dx * dx + dy * dy).sqrt() * 1.3).clamp(0.0, 1.0);
        for (channel, tint) in px.0.iter_mut().zip(accent) {
            let base = f32::from(*channel) * 0.42;
            let light = f32::from(tint) * 0.42 * glow * glow;
            *channel = (base + light).min(255.0) as u8;
        }
        px.0[3] = 255;
    }
    img
}

pub(crate) fn rgba(c: [u8; 3]) -> Rgba<u8> {
    Rgba([c[0], c[1], c[2], 255])
}

/// "3:05" / "1:02:03"
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Metni genişliğe sığdırır, sığmazsa "…" ile kısaltır.
pub(crate) fn fit(text: &str, font: &FontRef<'_>, size: f32, max_w: u32) -> String {
    let scale = PxScale::from(size);
    if text_size(scale, font, text).0 <= max_w {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>().trim_end().to_owned() + "…";
        if text_size(scale, font, &candidate).0 <= max_w {
            return candidate;
        }
    }
    "…".to_owned()
}

/// Kapak yokken çizilen ♫ (yazı tipinde müzik simgesi yok).
fn draw_notes(img: &mut RgbaImage, cx: i32, cy: i32) {
    let white = [255, 255, 255];
    // İki nota başı, iki sap, kalın bir bağ.
    fill_circle(img, cx - 38, cy + 42, 22, white);
    fill_circle(img, cx + 42, cy + 28, 22, white);
    fill_round_rect(img, (cx - 20) as u32, (cy - 62) as u32, 9, 106, 3, white, 1.0);
    fill_round_rect(img, (cx + 60) as u32, (cy - 76) as u32, 9, 106, 3, white, 1.0);
    for t in 0..22 {
        // Hafif eğimli bağ: soldan sağa 14 px yükselir.
        let y = cy - 62 - (t * 14 / 22);
        let x0 = cx - 20 + t * 4;
        fill_round_rect(img, x0 as u32, y as u32, 5, 22, 1, white, 1.0);
    }
}

/// YouTube'un 4:3 kapaklarındaki üst/alt siyah şeritleri atar.
fn trim_letterbox(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let dark_row = |y: u32| {
        (0..w).step_by(8).all(|x| {
            let p = img.get_pixel(x, y).0;
            u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) < 60
        })
    };
    if w * 3 == h * 4 && h >= 40 && dark_row(2) && dark_row(h - 3) {
        let band = w * 9 / 16;
        return imageops::crop_imm(img, 0, (h - band) / 2, w, band).to_image();
    }
    img.clone()
}

pub(crate) fn crop_square(img: &RgbaImage) -> RgbaImage {
    let img = &trim_letterbox(img);
    let side = img.width().min(img.height());
    let x = (img.width() - side) / 2;
    let y = (img.height() - side) / 2;
    imageops::crop_imm(img, x, y, side, side).to_image()
}

/// (x, y) yuvarlak köşeli dikdörtgenin içinde mi? Kenarlar için 0-1 kapsama döner (yumuşak kenar).
pub(crate) fn coverage(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let cx = x.clamp(r, w - r);
    let cy = y.clamp(r, h - r);
    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}

pub(crate) fn blend(px: &mut Rgba<u8>, c: [u8; 3], a: f32) {
    for (channel, target) in px.0.iter_mut().zip(c) {
        *channel = (f32::from(*channel) * (1.0 - a) + f32::from(target) * a).round() as u8;
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn fill_round_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, r: u32, c: [u8; 3], alpha: f32) {
    for dy in 0..h {
        for dx in 0..w {
            let (px, py) = (x + dx, y + dy);
            if px >= img.width() || py >= img.height() {
                continue;
            }
            let cov = coverage(dx as f32 + 0.5, dy as f32 + 0.5, w as f32, h as f32, r as f32);
            if cov > 0.0 {
                blend(img.get_pixel_mut(px, py), c, alpha * cov);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stroke_round_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, r: u32, c: [u8; 3], alpha: f32) {
    for dy in 0..h {
        for dx in 0..w {
            let (px, py) = (x + dx, y + dy);
            if px >= img.width() || py >= img.height() {
                continue;
            }
            let (fx, fy) = (dx as f32 + 0.5, dy as f32 + 0.5);
            let outer = coverage(fx, fy, w as f32, h as f32, r as f32);
            let inner = coverage(fx - 1.5, fy - 1.5, w as f32 - 3.0, h as f32 - 3.0, r as f32 - 1.5);
            let edge = (outer - inner).max(0.0);
            if edge > 0.0 {
                blend(img.get_pixel_mut(px, py), c, alpha * edge);
            }
        }
    }
}

fn paste_rounded(img: &mut RgbaImage, src: &RgbaImage, x: u32, y: u32, r: u32) {
    let (w, h) = src.dimensions();
    for (dx, dy, p) in src.enumerate_pixels() {
        let (px, py) = (x + dx, y + dy);
        if px >= img.width() || py >= img.height() {
            continue;
        }
        let cov = coverage(dx as f32 + 0.5, dy as f32 + 0.5, w as f32, h as f32, r as f32);
        if cov > 0.0 {
            blend(img.get_pixel_mut(px, py), [p.0[0], p.0[1], p.0[2]], cov * f32::from(p.0[3]) / 255.0);
        }
    }
}

pub(crate) fn fill_circle(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, c: [u8; 3]) {
    for dy in -r - 1..=r + 1 {
        for dx in -r - 1..=r + 1 {
            let (px, py) = (cx + dx, cy + dy);
            if px < 0 || py < 0 || px >= img.width() as i32 || py >= img.height() as i32 {
                continue;
            }
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            let cov = (r as f32 - d + 0.5).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend(img.get_pixel_mut(px as u32, py as u32), c, cov);
            }
        }
    }
}

/// Kapak indirildikten sonra çözümlenir (JPEG/PNG/WebP).
pub fn decode_cover(bytes: &[u8]) -> Option<RgbaImage> {
    image::load_from_memory(bytes).ok().map(|i| i.to_rgba8())
}

/// Önizleme: gerçek kapaklarla kartları çizip `CARD_PREVIEW_DIR` klasörüne yazar.
/// `CARD_PREVIEW_DIR=... cargo test live_card_preview -- --ignored --nocapture`
#[cfg(test)]
mod live_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "kapak indirir ve dosya yazar"]
    async fn live_card_preview() -> anyhow::Result<()> {
        let dir = std::env::var("CARD_PREVIEW_DIR")?;
        let client = crate::net::http_client()?;
        let spotify = client
            .get("https://image-cdn-fa.spotifycdn.com/image/ab67616d00001e02255e131abc1410833be95673")
            .send().await?.bytes().await?;
        let youtube = client.get("https://i.ytimg.com/vi/cpp69ghR1IM/hqdefault.jpg").send().await?.bytes().await?;
        let cases = [
            ("spotify", decode_cover(&spotify), "Never Gonna Give You Up", Some("Rick Astley"), "SPOTIFY", [30, 215, 96], false),
            ("youtube", decode_cover(&youtube), "Şımarık (Uzun Versiyon) — Akustik Konser Kaydı İstanbul", Some("TARKAN"), "YOUTUBE", [255, 48, 48], true),
            ("radyo", None, "Joy Türk", None, "CANLI RADYO", [155, 89, 182], false),
        ];
        for (name, cover, title, artist, source, accent, paused) in cases {
            let card = CardData {
                title: title.to_owned(),
                artist: artist.map(str::to_owned),
                source: source.to_owned(),
                accent,
                position: Duration::from_secs(62),
                duration: if name == "radyo" { None } else { Some(Duration::from_secs(213)) },
                paused,
                looping: name == "youtube",
                volume_percent: 60,
                upcoming: if name == "radyo" { 0 } else { 4 },
                requester: "axthrowa".to_owned(),
                brand: "AxthrowA".to_owned(),
                next: if name == "radyo" { None } else { Some("Sezen Aksu - Firuze".to_owned()) },
            };
            let started = std::time::Instant::now();
            let jpeg = render(&card, cover.as_ref())?;
            println!("{name}: {} KB, {} ms", jpeg.len() / 1024, started.elapsed().as_millis());
            std::fs::write(format!("{dir}/kart-{name}.jpg"), jpeg)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_format() {
        assert_eq!(clock(Duration::from_secs(10)), "0:10");
        assert_eq!(clock(Duration::from_secs(185)), "3:05");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn renders_without_cover() -> anyhow::Result<()> {
        let card = CardData {
            title: "Çok Uzun Bir Şarkı Adı Ki Kesinlikle Karta Sığmayacak Kadar Uzun".to_owned(),
            artist: Some("Sanatçı".to_owned()),
            source: "YOUTUBE".to_owned(),
            accent: [255, 60, 60],
            position: Duration::from_secs(62),
            duration: Some(Duration::from_secs(192)),
            paused: false,
            looping: true,
            volume_percent: 60,
            upcoming: 4,
            requester: "axthrowa".to_owned(),
            brand: "AxthrowA".to_owned(),
            next: Some("Sezen Aksu - Firuze".to_owned()),
        };
        let jpeg = render(&card, None)?;
        let decoded = image::load_from_memory(&jpeg)?;
        assert_eq!((decoded.width(), decoded.height()), (WIDTH, HEIGHT));
        Ok(())
    }
}
