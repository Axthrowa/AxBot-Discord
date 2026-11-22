//! Çoklu dil (i18n) yerelleştirme motoru.
//!
//! JSON sözlükleri (`locales/tr.json` ve `locales/en.json`) gömülü olarak derlenir ve
//! `category.key` hiyerarşisiyle `t(lang, key, args)` üzerinden erişilir.

use std::collections::HashMap;
use std::sync::LazyLock;

use axum::http::HeaderMap;
use serde_json::Value;

pub const TR_JSON: &str = include_str!("../locales/tr.json");
pub const EN_JSON: &str = include_str!("../locales/en.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Lang {
    #[default]
    Tr,
    En,
}

impl Lang {
    pub const ALL: [Self; 2] = [Self::Tr, Self::En];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tr => "tr",
            Self::En => "en",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let clean = s.trim().to_lowercase();
        if clean.starts_with("tr") {
            Some(Self::Tr)
        } else if clean.starts_with("en") {
            Some(Self::En)
        } else {
            None
        }
    }
}

static DICTIONARIES: LazyLock<HashMap<Lang, HashMap<String, String>>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    let tr_map = parse_json_dict(TR_JSON);
    let en_map = parse_json_dict(EN_JSON);
    map.insert(Lang::Tr, tr_map);
    map.insert(Lang::En, en_map);
    map
});

fn parse_json_dict(json_str: &str) -> HashMap<String, String> {
    let mut flat = HashMap::new();
    if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(json_str) {
        flatten_object("", &obj, &mut flat);
    }
    flat
}

fn flatten_object(prefix: &str, obj: &serde_json::Map<String, Value>, out: &mut HashMap<String, String>) {
    for (key, val) in obj {
        let full_key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match val {
            Value::Object(nested) => flatten_object(&full_key, nested, out),
            Value::String(s) => {
                out.insert(full_key, s.clone());
            }
            _ => {}
        }
    }
}

/// İstenen dilde anahtarı arar; bulunamazsa diğer dile ve en son anahtar adına düşer (`fallback`).
/// Verilen `{arg}` argümanlarını metin içine yerleştirir.
pub fn t(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    let dicts = &*DICTIONARIES;

    let text = dicts
        .get(&lang)
        .and_then(|d| d.get(key))
        .or_else(|| {
            // Birinci dilde bulunamazsa alternatif dile geç
            let fallback_lang = match lang {
                Lang::Tr => Lang::En,
                Lang::En => Lang::Tr,
            };
            dicts.get(&fallback_lang).and_then(|d| d.get(key))
        })
        .map(|s| s.as_str())
        .unwrap_or(key);

    let mut result = text.to_owned();
    for (param, val) in args {
        let placeholder = format!("{{{param}}}");
        result = result.replace(&placeholder, val);
    }
    result
}

/// Discord Slash komut çağrısından istemci veya sunucu dilini çözümler.
pub fn resolve_discord_lang(ctx: &poise::Context<'_, crate::data::Data, crate::error::Error>) -> Lang {
    if let Some(l) = ctx.locale().and_then(Lang::parse) {
        return l;
    }
    if let poise::Context::Application(app) = ctx {
        if let Some(l) = app.interaction.guild_locale.as_deref().and_then(Lang::parse) {
            return l;
        }
    }
    Lang::default()
}

/// Web panel HTTP isteğinden (`?lang=`, çerez, `Accept-Language`) dili çözümler.
pub fn resolve_web_lang(headers: &HeaderMap, query_lang: Option<&str>) -> Lang {
    if let Some(l) = query_lang.and_then(Lang::parse) {
        return l;
    }

    if let Some(cookie) = headers.get(axum::http::header::COOKIE).and_then(|c| c.to_str().ok()) {
        for part in cookie.split(';') {
            let part = part.trim();
            if let Some(val) = part.strip_prefix("lang=") {
                if let Some(l) = Lang::parse(val) {
                    return l;
                }
            }
        }
    }

    if let Some(accept) = headers.get(axum::http::header::ACCEPT_LANGUAGE).and_then(|a| a.to_str().ok()) {
        for item in accept.split(',') {
            let item = item.split(';').next().unwrap_or("").trim();
            if let Some(l) = Lang::parse(item) {
                return l;
            }
        }
    }

    Lang::Tr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_t_tr_and_en_lookups() {
        let tr_str = t(Lang::Tr, "music.now_playing", &[("title", "Dönence")]);
        assert_eq!(tr_str, "Şu an çalan: Dönence");

        let en_str = t(Lang::En, "music.now_playing", &[("title", "Bohemian Rhapsody")]);
        assert_eq!(en_str, "Now playing: Bohemian Rhapsody");

        let tr_queue = t(Lang::Tr, "music.queue_empty", &[]);
        assert_eq!(tr_queue, "Kuyruk boş.");

        let en_queue = t(Lang::En, "music.queue_empty", &[]);
        assert_eq!(en_queue, "Queue is empty.");
    }

    #[test]
    fn test_t_fallback_to_other_language_and_key() {
        // Eksik anahtar durumunda key'in kendisine düşmeli
        let missing = t(Lang::Tr, "nonexistent.key", &[]);
        assert_eq!(missing, "nonexistent.key");

        // Argümanlar düzgünce yerleşmeli
        let formatted = t(Lang::En, "analytics.messages_count", &[("count", "42")]);
        assert_eq!(formatted, "Total Messages: 42");
    }

    #[test]
    fn test_resolve_web_lang() {
        let mut headers = HeaderMap::new();

        // 1. Query öncelikli
        assert_eq!(resolve_web_lang(&headers, Some("en")), Lang::En);
        assert_eq!(resolve_web_lang(&headers, Some("tr")), Lang::Tr);

        // 2. Cookie
        headers.insert(axum::http::header::COOKIE, "theme=dark; lang=en".parse().unwrap_or_else(|_| axum::http::HeaderValue::from_static("")));
        assert_eq!(resolve_web_lang(&headers, None), Lang::En);

        // 3. Accept-Language
        let mut headers2 = HeaderMap::new();
        headers2.insert(axum::http::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9,tr;q=0.8".parse().unwrap_or_else(|_| axum::http::HeaderValue::from_static("")));
        assert_eq!(resolve_web_lang(&headers2, None), Lang::En);
    }
}
