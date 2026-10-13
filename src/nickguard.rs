//! Takma ad denetimi (Dyno / Wick benzeri): küfürlü adlar ve listede üste çıkmak için başa konan
//! işaretler ("!!!Ali", ".Ayşe") üye girişinde ve adını değiştirdiğinde düzeltilir.
//!
//! Ayarlar moderasyon ayarlarında (`ModSettings::nick_filter / dehoist / nick_replacement`), panelden açılır.
//! Botun 'Takma Adları Yönet' izni olmalı; sunucu sahibinin ve botun rolünden yüksek üyelerin adı değiştirilemez.

use poise::serenity_prelude::{self as serenity, EditMember, GuildId, RoleId, User};
use tracing::debug;

use crate::automod::rules::ModSettings;
use crate::data::Data;

/// Discord takma ad sınırı.
const MAX_NICK: usize = 32;

/// Baştaki "üste çıkarma" işaretlerini atar: harf (her dilde) ya da rakam olmayan her şey.
/// Rakamla başlayan adlar da listede üste çıktığı için baştaki rakamlar da atılır ("000Ali" → "Ali").
pub fn dehoisted(name: &str) -> String {
    name.trim_start_matches(|c: char| !c.is_alphabetic()).to_owned()
}

/// Üyenin görünen adı için ne yapılmalı? `None` = dokunma, `Some(ad)` = bu adı ver.
pub fn decide(display: &str, settings: &ModSettings, profane: bool) -> Option<String> {
    let replacement = || {
        let r = settings.nick_replacement.trim();
        if r.is_empty() { "Üye".to_owned() } else { r.chars().take(MAX_NICK).collect() }
    };
    if settings.nick_filter && profane {
        return Some(replacement());
    }
    if settings.dehoist {
        let cleaned = dehoisted(display);
        if cleaned != display {
            return Some(if cleaned.trim().is_empty() { replacement() } else { cleaned.chars().take(MAX_NICK).collect() });
        }
    }
    None
}

/// Giriş ve ad değişikliğinde çağrılır. `nick` sunucu takma adıdır (yoksa genel ad / kullanıcı adı görünür).
pub async fn check(ctx: &serenity::Context, data: &Data, guild_id: GuildId, user: &User, nick: Option<&str>, roles: &[RoleId]) {
    if user.bot {
        return;
    }
    let Ok(settings) = data.settings.get::<ModSettings>(&data.db, guild_id).await else {
        return;
    };
    if !settings.nick_filter && !settings.dehoist {
        return;
    }
    if roles.iter().any(|r| settings.exempt_roles.contains(&r.get())) {
        return;
    }
    let display = nick.or(user.global_name.as_deref()).unwrap_or(&user.name).to_owned();
    let profane = settings.nick_filter && data.automod.has_profanity(&data.db, &data.redis, guild_id, &display).await;
    let Some(new_name) = decide(&display, &settings, profane) else {
        return;
    };
    if Some(new_name.as_str()) == nick {
        return;
    }
    let reason = if profane { "Takma ad denetimi: uygunsuz ad" } else { "Takma ad denetimi: üste çıkarma işareti" };
    if let Err(e) = guild_id.edit_member(&ctx.http, user.id, EditMember::new().nickname(&new_name).audit_log_reason(reason)).await {
        // Sunucu sahibi / bottan yüksek üye ya da izin yok: sessizce geç.
        debug!(guild = %guild_id, user = %user.id, error = %e, "Takma ad değiştirilemedi");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(filter: bool, dehoist: bool) -> ModSettings {
        ModSettings { nick_filter: filter, dehoist, ..ModSettings::default() }
    }

    #[test]
    fn strips_hoisting_characters() {
        assert_eq!(dehoisted("!!!Ali"), "Ali");
        assert_eq!(dehoisted(". Ayşe"), "Ayşe");
        assert_eq!(dehoisted("000Zeynep"), "Zeynep");
        assert_eq!(dehoisted("Ömer!"), "Ömer!");
        assert_eq!(dehoisted("ılık"), "ılık");
        assert_eq!(dehoisted("!!!"), "");
    }

    #[test]
    fn decides_new_name() {
        assert_eq!(decide("!Ali", &settings(false, true), false), Some("Ali".to_owned()));
        assert_eq!(decide("Ali", &settings(false, true), false), None);
        assert_eq!(decide("!!!", &settings(false, true), false), Some("Üye".to_owned()));
        assert_eq!(decide("kötü", &settings(true, false), true), Some("Üye".to_owned()));
        // Kapalıyken dokunulmaz.
        assert_eq!(decide("!Ali", &settings(false, false), true), None);
        let custom = ModSettings { nick_replacement: "Yeni Üye".to_owned(), ..settings(true, true) };
        assert_eq!(decide("!!", &custom, false), Some("Yeni Üye".to_owned()));
    }
}
