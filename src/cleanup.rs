//! Silinen rol ve kanalların ayarlardan temizlenmesi.
//!
//! Bir rol ya da kanal Discord'dan silinince ona işaret eden temel ayarlar (otorol, karşılama kanalı…)
//! kendiliğinden kapatılır ve olay kayıt kanalına not düşülür; böylece bot her seferinde
//! "Unknown Role / Unknown Channel" hatası alıp sessizce başarısız olmaz. Diğer eklentilerdeki
//! (rol menüsü, otomasyon…) kopuk bağlantılar paneldeki Sağlık kontrolü sayfasında listelenir.

use poise::serenity_prelude::{self as serenity, ChannelId, CreateAllowedMentions, CreateMessage, GuildId, RoleId};
use tracing::{info, warn};

use crate::cards::WelcomeSettings;
use crate::data::Data;
use crate::guild_config::{GuildConfig, Setting};

/// Silinen role bağlı temel ayarlar: (ne olduğu, yazılacak yeni ayar).
pub fn role_changes(cfg: &GuildConfig, role: RoleId) -> Vec<(&'static str, Setting)> {
    let mut changes = Vec::new();
    if cfg.autorole == Some(role) {
        changes.push(("otomatik rol", Setting::Autorole(None)));
    }
    if cfg.verify_role == Some(role) {
        changes.push(("doğrulama rolü", Setting::Verify(None, cfg.verify_math)));
    }
    if cfg.birthday_role == Some(role) {
        changes.push(("doğum günü rolü", Setting::Birthday(cfg.birthday_channel, None)));
    }
    changes
}

/// Silinen kanala bağlı temel ayarlar.
pub fn channel_changes(cfg: &GuildConfig, channel: ChannelId) -> Vec<(&'static str, Setting)> {
    let mut changes = Vec::new();
    if cfg.welcome_channel == Some(channel) {
        changes.push(("karşılama kanalı", Setting::Welcome(None, cfg.welcome_message.clone())));
    }
    if cfg.goodbye_channel == Some(channel) {
        changes.push(("veda kanalı", Setting::Goodbye(None, cfg.goodbye_message.clone())));
    }
    if cfg.event_log_channel == Some(channel) {
        changes.push(("olay kayıt kanalı", Setting::EventLog(None)));
    }
    if cfg.starboard_channel == Some(channel) {
        changes.push(("starboard kanalı", Setting::Starboard(None, cfg.starboard_threshold.max(1))));
    }
    if cfg.suggestion_channel == Some(channel) {
        changes.push(("öneri kanalı", Setting::SuggestionChannel(None)));
    }
    if cfg.birthday_channel == Some(channel) {
        changes.push(("doğum günü kanalı", Setting::Birthday(None, cfg.birthday_role)));
    }
    if cfg.tempvoice_hub == Some(channel) {
        changes.push(("oda oluşturma kanalı", Setting::TempVoiceHub(None)));
    }
    if cfg.animal_channel == Some(channel) {
        changes.push(("hayvan yakalama kanalı", Setting::AnimalChannel(None)));
    }
    changes
}

async fn apply(ctx: &serenity::Context, data: &Data, guild_id: GuildId, what: &str, changes: Vec<(&'static str, Setting)>, mut cleared: Vec<&'static str>, skip_log: Option<ChannelId>) {
    for (label, setting) in changes {
        match data.guild_configs.set(&data.db, guild_id, setting).await {
            Ok(()) => cleared.push(label),
            Err(e) => warn!(guild = %guild_id, error = ?e, "Silinen {what} ayardan temizlenemedi"),
        }
    }
    if cleared.is_empty() {
        return;
    }
    info!(guild = %guild_id, cleared = ?cleared, "Silinen {what} ayarlardan temizlendi");
    // Yöneticiye haber: olay kayıt kanalı (silinen kanalın kendisi değilse).
    let Ok(cfg) = data.guild_configs.get(&data.db, guild_id).await else { return };
    let Some(log) = cfg.event_log_channel.filter(|c| Some(*c) != skip_log) else { return };
    let text = format!("🧹 Silinen bir {what} şu ayarlarda kullanılıyordu; bu ayarlar kapatıldı: **{}**. Yeniden açmak için `/panel`.", cleared.join(", "));
    let _ = log.send_message(&ctx.http, CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await;
}

/// Bir rol silindi (ya da artık sunucuda yok): ona bağlı temel ayarları kapatır.
pub async fn role_deleted(ctx: &serenity::Context, data: &Data, guild_id: GuildId, role: RoleId) {
    let Ok(cfg) = data.guild_configs.get(&data.db, guild_id).await else { return };
    let changes = role_changes(&cfg, role);
    let mut cleared = Vec::new();
    // Karşılamadaki ek roller.
    if let Ok(welcome) = data.settings.get::<WelcomeSettings>(&data.db, guild_id).await {
        if welcome.extra_roles.contains(&role.get()) {
            let mut updated = (*welcome).clone();
            updated.extra_roles.retain(|id| *id != role.get());
            if data.settings.set(&data.db, guild_id, updated).await.is_ok() {
                cleared.push("karşılamadaki ek rol");
            }
        }
    }
    apply(ctx, data, guild_id, "rol", changes, cleared, None).await;
}

/// Bir kanal silindi: ona bağlı temel ayarları kapatır.
pub async fn channel_deleted(ctx: &serenity::Context, data: &Data, guild_id: GuildId, channel: ChannelId) {
    let Ok(cfg) = data.guild_configs.get(&data.db, guild_id).await else { return };
    let changes = channel_changes(&cfg, channel);
    apply(ctx, data, guild_id, "kanal", changes, Vec::new(), Some(channel)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_settings_pointing_at_the_deleted_item_change() {
        let cfg = GuildConfig {
            autorole: Some(RoleId::new(5)),
            birthday_role: Some(RoleId::new(6)),
            birthday_channel: Some(ChannelId::new(10)),
            welcome_channel: Some(ChannelId::new(10)),
            welcome_message: Some("selam".to_owned()),
            starboard_channel: Some(ChannelId::new(11)),
            ..GuildConfig::default()
        };
        let roles = role_changes(&cfg, RoleId::new(5));
        assert_eq!(roles.iter().map(|(label, _)| *label).collect::<Vec<_>>(), ["otomatik rol"]);
        // Doğum günü rolü silinince kanal ayarı korunur.
        assert!(matches!(role_changes(&cfg, RoleId::new(6)).as_slice(), [(_, Setting::Birthday(Some(_), None))]));
        assert!(role_changes(&cfg, RoleId::new(99)).is_empty());

        let channels = channel_changes(&cfg, ChannelId::new(10));
        assert_eq!(channels.iter().map(|(label, _)| *label).collect::<Vec<_>>(), ["karşılama kanalı", "doğum günü kanalı"]);
        // Karşılama metni silinmez, yalnızca kanal kapanır.
        assert!(matches!(channels.first(), Some((_, Setting::Welcome(None, Some(message)))) if message == "selam"));
        assert!(channel_changes(&cfg, ChannelId::new(99)).is_empty());
    }
}
