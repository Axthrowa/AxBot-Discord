//! Ayar dışa / içe aktarma: sunucunun eklenti ayarları (`plugin_settings`) tek JSON dosyası olarak indirilir ve
//! başka bir sunucuya (ya da aynısına) yüklenebilir. Yükleme önce bütün kayıtları doğrular, sonra yazar.
//! Belirli mesajlara bağlı ayarlar (rol menüleri, gömülü mesajlar, etkinlikler) ve sunucu profili taşınmaz.
//! Temel ayarlar (`guild_settings`: kayıt / karşılama kanalı vb.) bu dosyada YOK; kanal ve rol kimlikleri
//! başka sunucuda geçersiz kalır ve Sağlık kontrolü sayfasında görünür.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use poise::serenity_prelude::GuildId;
use serde_json::{Map, Value};


use super::pages::forbidden;
use super::{auth, is_guild_admin};
use crate::db::snowflake;
use crate::settings::PluginConfig;
use crate::web::WebState;

const FORMAT: &str = "axbot-ayarlar";
/// Mesaj kimliğine ya da sunucunun kendi görünümüne bağlı ayarlar başka yere taşınmaz.
const SKIP: [&str; 4] = ["reaction_roles", "embeds", "events", "bot_profile"];

/// Anahtarı bilinen ayar türüyle doğrular; `write` ise kaydeder. Bilinmeyen anahtar `Ok(false)`.
macro_rules! typed {
    ($state:expr, $guild:expr, $key:expr, $value:expr, $write:expr, [$($t:ty),* $(,)?]) => {{
        let mut known = false;
        $(
            if !known && $key == <$t as PluginConfig>::KEY {
                known = true;
                let parsed: $t = serde_json::from_value($value.clone()).map_err(|_| "ice")?;
                if $write {
                    $state.settings.set(&$state.db, $guild, parsed).await.map_err(|e| {
                        tracing::warn!(guild = %$guild, error = ?e, "Panel: içe aktarılan ayar kaydedilemedi");
                        "hata"
                    })?;
                }
            }
        )*
        Ok::<bool, &'static str>(known)
    }};
}

async fn apply(state: &WebState, guild_id: GuildId, key: &str, value: &Value, write: bool) -> Result<bool, &'static str> {
    use crate::*;
    typed!(state, guild_id, key, value, write, [
        achievements::AchievementSettings,
        ai::characters::AiSettings,
        antinuke::AntiNukeSettings,
        automations::AutomationSettings,
        automod::rules::ModSettings,
        birthdays::BirthdaySettings,
        cards::WelcomeSettings,
        community::CommunitySettings,
        customcmd::CustomCommandSettings,
        economy::EconomySettings,
        feeds::FeedSettings,
        leveling::settings::LevelSettings,
        modmail::ModmailSettings,
        music::playlists::MusicExtras,
        nativeautomod::NativeAutomodSettings,
        news::NewsSettings,
        crate::starboard::StarboardSettings,
        tickets::TicketExtras,
        crate::scheduler::recurring::TimerSettings,
        backup::BackupSettings,
        invites::InviteSettings,
    ])
}

/// İçe aktarır; yazılan ayar sayısını döner. Hatalı dosyada hiçbir şey yazılmaz.
pub async fn import(state: &WebState, guild_id: GuildId, raw: &str) -> Result<usize, &'static str> {
    let root: Value = serde_json::from_str(raw.trim()).map_err(|_| "ice")?;
    if root.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err("ice");
    }
    let plugins = root.get("plugins").and_then(Value::as_object).ok_or("ice")?;
    let wanted: Vec<(&String, &Value)> = plugins.iter().filter(|(key, _)| !SKIP.contains(&key.as_str())).collect();
    // 1) Hepsi doğrulanır; bilinmeyen anahtar da hata sayılır (yanlış dosya).
    for (key, value) in &wanted {
        if !apply(state, guild_id, key, value, false).await? {
            return Err("ice");
        }
    }
    // 2) Yazılır.
    for (key, value) in &wanted {
        apply(state, guild_id, key, value, true).await?;
    }
    Ok(wanted.len())
}

/// Güvenli eklenti ayarlarını JSON nesnesi olarak çeker.
pub async fn export_safe_settings(db: &sqlx::PgPool, guild_id: GuildId) -> anyhow::Result<Value> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT plugin, data FROM plugin_settings WHERE guild_id = $1 ORDER BY plugin",
    )
    .bind(snowflake(guild_id.get())?)
    .fetch_all(db)
    .await?;
    let mut plugins = Map::new();
    for (key, data) in rows {
        if SKIP.contains(&key.as_str()) {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&data) {
            plugins.insert(key, value);
        }
    }
    Ok(Value::Object(plugins))
}

/// `GET /panel/sunucu/{id}/ayarlar-disa-aktar`: yalnızca gerçek sunucu yöneticileri.
pub async fn export(State(state): State<WebState>, headers: HeaderMap, Path(guild): Path<u64>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !is_guild_admin(&state, session.user, guild_id).await {
        return forbidden();
    }
    let plugins = match export_safe_settings(&state.db, guild_id).await {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let body = serde_json::json!({ "format": FORMAT, "version": 1, "guild": guild_id.get().to_string(), "plugins": plugins });
    let text = serde_json::to_string_pretty(&body).unwrap_or_default();
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8".to_owned()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"axbot-ayarlar-{guild_id}.json\"")),
        ],
        text,
    )
        .into_response()
}
