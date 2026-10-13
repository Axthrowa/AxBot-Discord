//! Panel: "Hoş geldin & Veda" (mesajlar, resimli kart, özelden karşılama, roller) ve "Doğrulama"
//! eklentilerinin formları, ayrıca karşılama kartı önizlemesi.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use poise::serenity_prelude::GuildId;
use tracing::warn;

use super::auth;
use super::can_manage;
use super::pages::forbidden;
use super::plugins::{checkbox, checklist, field, section, textarea, Input, Options, TEMPLATE_HINT};
use crate::cards::{self, WelcomeSettings, DEFAULT_SUBTITLE, DEFAULT_TITLE};
use crate::guild_config::Setting;
use crate::members::DEFAULT_DM;
use crate::web::{escape, WebState};

const CARD_TEXT_MAX: usize = 100;
const URL_MAX: usize = 300;

fn text_input(name: &str, value: Option<&str>, placeholder: &str, max: usize) -> String {
    format!(
        "<input type=\"text\" class=\"wide\" name=\"{name}\" value=\"{}\" maxlength=\"{max}\" placeholder=\"{}\">",
        escape(value.unwrap_or_default()),
        escape(placeholder)
    )
}

fn colour_input(name: &str, value: &str) -> String {
    let value = cards::parse_hex(value).map_or_else(|| "#ffffff".to_owned(), |[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}"));
    format!("<input type=\"color\" name=\"{name}\" value=\"{value}\">")
}

/// "Hoş geldin & Veda" sayfasının alanları ve eklentinin açık olup olmadığı.
pub async fn fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let cfg = state.configs.get(&state.db, guild_id).await?;
    let settings = state.settings.get::<WelcomeSettings>(&state.db, guild_id).await?;

    let mut themes = String::from("<div class=\"checks\">");
    for (key, name) in crate::greetings::THEMES {
        themes.push_str(&format!(
            "<label class=\"check\"><input type=\"checkbox\" name=\"theme\" value=\"{key}\"{}> {name}</label>",
            if settings.themes.iter().any(|t| t == key) { " checked" } else { "" },
        ));
    }
    themes.push_str("</div>");

    let mut out = section(
        "Hoş geldin mesajı",
        field("Kanal", &options.channel_select("welcome_channel", cfg.welcome_channel), "")
            + &field(
                "Mesaj",
                &textarea("welcome_message", cfg.welcome_message.as_deref(), "Boş: temalı hazır mesajlar"),
                TEMPLATE_HINT,
            ),
    );

    out.push_str(&section(
        "Hazır mesaj temaları",
        themes + "<p class=\"note\">Mesaj kutuları boşken karşılama ve vedalar bu temaların GIF'li hazır mesajlarından seçilir. \
                   Hiçbiri seçili değilse hepsi karışık kullanılır.</p>",
    ));

    out.push_str(&section(
        "Karşılama kartı",
        checkbox("card", settings.card, "Karşılama mesajına resimli kart ekle (temalı GIF'in yerine geçer)")
            + &field("Başlık", &text_input("card_title", settings.card_title.as_deref(), DEFAULT_TITLE, CARD_TEXT_MAX), "")
            + &field(
                "Alt yazı",
                &text_input("card_subtitle", settings.card_subtitle.as_deref(), DEFAULT_SUBTITLE, CARD_TEXT_MAX),
                "Değişkenler: <code>{isim}</code>, <code>{sunucu}</code>, <code>{üye}</code>.",
            )
            + &field(
                "Yazı tipi",
                &{
                    let mut select = String::from("<select name=\"font\">");
                    for (key, label) in cards::FONTS {
                        select.push_str(&format!("<option value=\"{key}\"{}>{label}</option>", if *key == settings.font { " selected" } else { "" }));
                    }
                    select + "</select>"
                },
                "",
            )
            + &field("Metin rengi", &colour_input("text_color", &settings.text_color), "")
            + &field("Arka plan rengi", &colour_input("bg_color", &settings.bg_color), "")
            + &field(
                "Arka plan resmi (https bağlantısı)",
                &text_input("background_url", settings.background_url.as_deref(), "https://…/resim.jpg", URL_MAX),
                "Boşsa düz arka plan rengi kullanılır. PNG, JPEG ya da WebP; en fazla 8 MB.",
            )
            + &field(
                "Kaplama opaklığı (%)",
                &format!("<input type=\"number\" name=\"overlay\" value=\"{}\" min=\"0\" max=\"100\">", settings.overlay),
                "Arka plan resminin üstüne arka plan rengi bu oranda bindirilir; yazı okunaklı kalsın diye.",
            )
            + &format!(
                "<p class=\"note\">Önizleme (kayıtlı ayarlarla, senin profilinle):</p>\
                 <img class=\"preview\" src=\"/panel/sunucu/{guild_id}/kart-onizleme\" alt=\"Kart önizlemesi\" loading=\"lazy\">"
            ),
    ));

    out.push_str(&section(
        "Özelden karşılama",
        checkbox("dm", settings.dm, "Yeni üyeye özel mesaj gönder")
            + &field(
                "Özel mesaj",
                &textarea("dm_message", settings.dm_message.as_deref(), DEFAULT_DM),
                "Değişkenler: <code>{isim}</code>, <code>{sunucu}</code>, <code>{üye}</code>. Üyenin özel mesajları kapalıysa gönderilemez.",
            ),
    ));

    out.push_str(&section(
        "Veda mesajı",
        field("Kanal", &options.channel_select("goodbye_channel", cfg.goodbye_channel), "")
            + &field(
                "Mesaj",
                &textarea("goodbye_message", cfg.goodbye_message.as_deref(), "Boş: temalı hazır mesajlar"),
                "Ayrılan kişi etiketlenmez; <code>{isim}</code> kullan.",
            ),
    ));

    out.push_str(&section(
        "Yeni üyelere verilecek roller",
        field(
            "Otomatik rol",
            &options.role_select("autorole", cfg.autorole),
            "Listede sadece botun verebileceği, yönetim yetkisi içermeyen roller görünür.",
        ) + "<p class=\"note\" style=\"margin:0 0 8px\">Ek roller:</p>"
            + &checklist("extra_role", &options.roles, "@ ", &settings.extra_roles)
            + &field(
                "Rolleri kaç dakika sonra ver",
                &format!("<input type=\"number\" name=\"role_delay\" value=\"{}\" min=\"0\" max=\"{}\">", settings.role_delay_mins, crate::members::MAX_ROLE_DELAY_MINS),
                "0 = katılır katılmaz. Gecikme, hemen yazıp çıkan hesaplara karşı işe yarar; bot bu sürede yeniden başlarsa bekleyen roller verilmez.",
            ),
    ));

    let enabled = cfg.welcome_channel.is_some() || cfg.goodbye_channel.is_some() || cfg.autorole.is_some() || settings.dm;
    Ok((out, enabled))
}

fn short_text(input: &Input<'_>, key: &str, max: usize) -> Result<Option<String>, &'static str> {
    let raw = input.raw(key);
    if raw.chars().count() > max {
        return Err("metin");
    }
    Ok((!raw.is_empty()).then(|| raw.to_owned()))
}

fn colour(input: &Input<'_>, key: &str) -> Result<String, &'static str> {
    let [r, g, b] = cards::parse_hex(input.raw(key)).ok_or("renk")?;
    Ok(format!("#{r:02X}{g:02X}{b:02X}"))
}

pub async fn save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = |e: anyhow::Error| {
        warn!(guild = %guild_id, error = ?e, "Panel: karşılama ayarı kaydedilemedi");
        "hata"
    };
    // 1) Doğrula.
    let welcome = Setting::Welcome(input.channel("welcome_channel")?, input.text("welcome_message")?);
    let goodbye = Setting::Goodbye(input.channel("goodbye_channel")?, input.text("goodbye_message")?);
    let autorole = Setting::Autorole(input.role("autorole")?);

    let background_url = short_text(input, "background_url", URL_MAX)?;
    if let Some(url) = &background_url {
        // Bot bu adresten dosya indirecek: yalnızca https ve herkese açık adresler kabul edilir.
        if !url.starts_with("https://") || !crate::music::extra::is_public_url(url).await {
            return Err("adres");
        }
    }
    let current = state.settings.get::<WelcomeSettings>(&state.db, guild_id).await.map_err(failed)?;
    let settings = WelcomeSettings {
        card: input.checked("card"),
        card_title: short_text(input, "card_title", CARD_TEXT_MAX)?,
        card_subtitle: short_text(input, "card_subtitle", CARD_TEXT_MAX)?,
        text_color: colour(input, "text_color")?,
        bg_color: colour(input, "bg_color")?,
        overlay: u8::try_from(input.number("overlay", 0, 100)?).map_err(|_| "sayi")?,
        font: match input.raw("font") {
            "" => "noto".to_owned(),
            key if cards::FONTS.iter().any(|(known, _)| *known == key) => key.to_owned(),
            _ => return Err("hata"),
        },
        background_url,
        dm: input.checked("dm"),
        dm_message: input.text("dm_message")?,
        extra_roles: input.ids("extra_role", &input.options.roles, "rol")?,
        role_delay_mins: if input.raw("role_delay").is_empty() { 0 } else { input.number("role_delay", 0, crate::members::MAX_ROLE_DELAY_MINS)? },
        themes: {
            let picked: Vec<String> = input.all("theme").into_iter().map(str::to_owned).collect();
            if picked.iter().any(|t| !crate::greetings::is_theme(t)) {
                return Err("hata");
            }
            picked
        },
        // Captcha ayarı Doğrulama sayfasındadır.
        captcha: current.captcha,
    };

    // 2) Kaydet.
    for setting in [welcome, goodbye, autorole] {
        state.configs.set(&state.db, guild_id, setting).await.map_err(failed)?;
    }
    state.settings.set(&state.db, guild_id, settings).await.map_err(failed)?;
    Ok(())
}

/// "Doğrulama" sayfasının alanları.
pub async fn verify_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let cfg = state.configs.get(&state.db, guild_id).await?;
    let settings = state.settings.get::<WelcomeSettings>(&state.db, guild_id).await?;
    let out = section(
        "Doğrulama",
        field(
            "Doğrulanan üyeye verilecek rol",
            &options.role_select("verify_role", cfg.verify_role),
            "Rol seçilmezse doğrulama kapalıdır. Kanalların @everyone için gizli, bu rol için görünür olmalı.",
        ) + &checkbox("captcha", settings.captcha, "Resimli kod (captcha) sor — botların otomatik geçmesini zorlaştırır")
            + &checkbox("math", cfg.verify_math, "Basit bir toplama sorusu sor (captcha açıksa onun yerine captcha sorulur)")
            + "<p class=\"note\">Üyelerin basacağı <b>Doğrula</b> butonlu mesajı Discord'da <code>/doğrulama kur</code> ile gönderirsin; \
               buradaki ayarlar o mesaja anında yansır.</p>",
    );
    Ok((out, cfg.verify_role.is_some()))
}

pub async fn verify_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = |e: anyhow::Error| {
        warn!(guild = %guild_id, error = ?e, "Panel: doğrulama ayarı kaydedilemedi");
        "hata"
    };
    let role = input.role("verify_role")?;
    let current = state.settings.get::<WelcomeSettings>(&state.db, guild_id).await.map_err(failed)?;
    let mut settings = (*current).clone();
    settings.captcha = input.checked("captcha");
    state.configs.set(&state.db, guild_id, Setting::Verify(role, input.checked("math"))).await.map_err(failed)?;
    state.settings.set(&state.db, guild_id, settings).await.map_err(failed)?;
    Ok(())
}

/// Kayıtlı ayarlarla, panele giren kişinin profiliyle çizilmiş karşılama kartı (JPEG).
pub async fn card_preview(State(state): State<WebState>, headers: HeaderMap, Path(guild): Path<u64>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await {
        return forbidden();
    }
    let Some((name, members)) = state.cache.guild(guild_id).map(|g| (g.name.clone(), g.member_count)) else {
        return forbidden();
    };
    let (Ok(settings), Ok(user)) = (
        state.settings.get::<WelcomeSettings>(&state.db, guild_id).await,
        state.http.get_user(session.user).await,
    ) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match crate::members::welcome_card(&settings, &user, &name, members).await {
        Some(bytes) => ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")], bytes).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options { guild_name: "T".to_owned(), channels: Vec::new(), categories: Vec::new(), roles: Vec::new(), all_roles: Vec::new() }
    }

    #[test]
    fn colours_and_short_texts_are_validated() {
        let options = options();
        let form: Vec<(String, String)> = [("text_color", "#12abEF"), ("bg_color", "kırmızı"), ("card_title", "  Merhaba  ")]
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let input = Input { form: &form, options: &options };
        assert_eq!(colour(&input, "text_color"), Ok("#12ABEF".to_owned()));
        assert_eq!(colour(&input, "bg_color"), Err("renk"));
        assert_eq!(short_text(&input, "card_title", 100), Ok(Some("Merhaba".to_owned())));
        assert_eq!(short_text(&input, "card_title", 3), Err("metin"));
        assert_eq!(short_text(&input, "yok", 3), Ok(None));
        // Renk kutusu tarayıcıya her zaman geçerli bir değer verir.
        assert!(colour_input("x", "bozuk").contains("#ffffff"));
        assert!(text_input("x", Some("\"><script>"), "", 10).contains("&quot;&gt;&lt;script&gt;"));
    }
}
