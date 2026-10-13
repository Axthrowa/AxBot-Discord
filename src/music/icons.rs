//! Müzik kartı butonlarının ikonları: Lucide çizgi ikonları (ISC lisansı, `assets/icons/src/`),
//! 128 px beyaz PNG olarak `assets/icons/` içinde.
//!
//! Bot açılışta bu PNG'leri **uygulama emojisi** olarak bir kez yükler (botun kendi emojileri; hiçbir
//! sunucunun emoji sınırını kullanmaz) ve butonlar bunları gösterir. Yükleme yapılamazsa butonlar
//! Unicode emojilere düşer. İkon tasarımı değişirse `VERSION` artırılır (eski emojiler yerinde kalır).

use std::collections::HashMap;
use std::sync::OnceLock;

use poise::serenity_prelude::{CreateAttachment, EmojiId, Http, ReactionType};
use tracing::{info, warn};

/// Emoji adlarının öneki; ikonlar değişince artırılır.
const VERSION: &str = "axm1";

pub struct Icon {
    pub name: &'static str,
    pub png: &'static [u8],
    pub fallback: &'static str,
}

macro_rules! icon {
    ($name:literal, $fallback:literal) => {
        Icon { name: $name, png: include_bytes!(concat!("../../assets/icons/", $name, ".png")), fallback: $fallback }
    };
}

pub const ICONS: &[Icon] = &[
    icon!("prev", "⏮️"),
    icon!("next", "⏭️"),
    icon!("play", "▶️"),
    icon!("pause", "⏸️"),
    icon!("stop", "⏹️"),
    icon!("shuffle", "🔀"),
    icon!("volume", "🔊"),
    icon!("loop", "🔁"),
    icon!("queue", "📜"),
    icon!("search", "🔍"),
    icon!("clear", "🧹"),
    icon!("share", "🔗"),
    icon!("user", "👤"),
];

static EMOJIS: OnceLock<HashMap<&'static str, EmojiId>> = OnceLock::new();

/// Buton ikonu: yüklendiyse uygulama emojisi, değilse Unicode karşılığı.
pub fn emoji(name: &str) -> ReactionType {
    let Some(icon) = ICONS.iter().find(|i| i.name == name) else {
        return ReactionType::Unicode("❔".to_owned());
    };
    match EMOJIS.get().and_then(|m| m.get(name)) {
        Some(id) => ReactionType::Custom { animated: false, id: *id, name: Some(format!("{VERSION}_{name}")) },
        None => ReactionType::Unicode(icon.fallback.to_owned()),
    }
}

/// İkonun PNG'si (kart resmindeki küçük simgeler için).
pub fn png(name: &str) -> Option<&'static [u8]> {
    ICONS.iter().find(|i| i.name == name).map(|i| i.png)
}

/// Eksik ikonları uygulama emojisi olarak yükler (açılışta bir kez; var olanlar yeniden yüklenmez).
pub async fn ensure_uploaded(http: &Http) {
    let existing = match http.get_application_emojis().await {
        Ok(list) => list,
        Err(e) => {
            warn!(error = %e, "Uygulama emojileri okunamadı; müzik butonları normal emoji kullanacak");
            return;
        }
    };
    let mut map = HashMap::new();
    let mut uploaded = 0;
    for icon in ICONS {
        let name = format!("{VERSION}_{}", icon.name);
        if let Some(found) = existing.iter().find(|e| e.name == name) {
            map.insert(icon.name, found.id);
            continue;
        }
        let image = CreateAttachment::bytes(icon.png, format!("{name}.png")).to_base64();
        let body = serde_json::json!({ "name": name, "image": image });
        match http.create_application_emoji(&body).await {
            Ok(emoji) => {
                map.insert(icon.name, emoji.id);
                uploaded += 1;
            }
            Err(e) => warn!(icon = icon.name, error = %e, "Müzik ikonu yüklenemedi; bu buton normal emoji kullanacak"),
        }
    }
    if uploaded > 0 {
        info!(count = uploaded, "Müzik ikonları uygulama emojisi olarak yüklendi");
    }
    let _ = EMOJIS.set(map);
}

#[cfg(test)]
mod tests {
    /// SVG → 128 px PNG. Elle: `cargo test render_icon_pngs -- --ignored` (ikon değişince; sonra `VERSION`'ı artır).
    #[test]
    #[ignore = "assets/icons altına dosya yazar"]
    fn render_icon_pngs() -> anyhow::Result<()> {
        use resvg::{tiny_skia, usvg};
        // (çıktı adı, Lucide dosyası, renk, dolu mu)
        let list = [
            ("prev", "skip-back", "#ffffff", true),
            ("next", "skip-forward", "#ffffff", true),
            ("play", "play", "#ffffff", true),
            ("pause", "pause", "#ffffff", true),
            ("stop", "square", "#ef4444", true),
            ("shuffle", "shuffle", "#ffffff", false),
            ("volume", "volume-2", "#ffffff", false),
            ("loop", "repeat", "#ffffff", false),
            ("queue", "list-music", "#ffffff", false),
            ("search", "search", "#ffffff", false),
            ("clear", "brush-cleaning", "#ffffff", false),
            ("share", "share-2", "#ffffff", false),
            ("user", "user", "#ffffff", false),
        ];
        for (name, file, colour, filled) in list {
            let mut svg = std::fs::read_to_string(format!("assets/icons/src/{file}.svg"))?.replace("currentColor", colour);
            if filled {
                svg = svg.replacen("fill=\"none\"", &format!("fill=\"{colour}\""), 1);
            }
            let tree = usvg::Tree::from_str(&svg, &usvg::Options::default())?;
            let mut pixmap = tiny_skia::Pixmap::new(128, 128).ok_or_else(|| anyhow::anyhow!("pixmap"))?;
            let scale = 128.0 / tree.size().width();
            resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
            pixmap.save_png(format!("assets/icons/{name}.png"))?;
            println!("{name}.png");
        }
        Ok(())
    }

    #[test]
    fn every_icon_is_png() {
        for icon in super::ICONS {
            assert!(icon.png.starts_with(b"\x89PNG"), "{} PNG değil", icon.name);
        }
    }
}
