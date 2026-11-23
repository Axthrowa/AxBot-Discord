//! Hata tipleri ve framework seviyesindeki merkezi hata yakalayıcı.

use poise::serenity_prelude::{self as serenity, Permissions};
use poise::CreateReply;
use tracing::{error, warn};

/// İzinlerin Discord'daki Türkçe adları.
pub(crate) fn permission_names(perms: Permissions) -> String {
    let names: [(Permissions, &str); 23] = [
        (Permissions::VIEW_CHANNEL, "Kanalı Görüntüle"),
        (Permissions::SEND_MESSAGES, "Mesaj Gönder"),
        (Permissions::READ_MESSAGE_HISTORY, "Mesaj Geçmişini Oku"),
        (Permissions::ADD_REACTIONS, "Tepki Ekle"),
        (Permissions::EMBED_LINKS, "Bağlantı Yerleştir"),
        (Permissions::SEND_POLLS, "Anket Oluştur"),
        (Permissions::MANAGE_MESSAGES, "Mesajları Yönet"),
        (Permissions::MANAGE_CHANNELS, "Kanalları Yönet"),
        (Permissions::MANAGE_ROLES, "Rolleri Yönet"),
        (Permissions::MANAGE_GUILD, "Sunucuyu Yönet"),
        (Permissions::MOVE_MEMBERS, "Üyeleri Taşı"),
        (Permissions::KICK_MEMBERS, "Üyeleri At"),
        (Permissions::BAN_MEMBERS, "Üyeleri Yasakla"),
        (Permissions::MODERATE_MEMBERS, "Üyelere Zaman Aşımı Uygula"),
        (Permissions::SEND_MESSAGES_IN_THREADS, "Alt Başlıklarda Mesaj Gönder"),
        (Permissions::CREATE_PUBLIC_THREADS, "Herkese Açık Alt Başlık Oluştur"),
        (Permissions::CREATE_PRIVATE_THREADS, "Özel Alt Başlık Oluştur"),
        (Permissions::ATTACH_FILES, "Dosya Ekle"),
        (Permissions::USE_EXTERNAL_EMOJIS, "Harici Emoji Kullan"),
        (Permissions::CREATE_INSTANT_INVITE, "Davet Oluştur"),
        (Permissions::VIEW_AUDIT_LOG, "Denetim Kaydını Görüntüle"),
        (Permissions::CONNECT, "Bağlan"),
        (Permissions::SPEAK, "Konuş"),
    ];
    let known: Vec<&str> = names.iter().filter(|(p, _)| perms.contains(*p)).map(|(_, n)| *n).collect();
    if known.is_empty() {
        perms.to_string()
    } else {
        known.join(", ")
    }
}

use crate::data::Data;

/// Komut ve event'lerin ortak hata tipi.
pub type Error = anyhow::Error;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("zorunlu ortam değişkeni eksik: {0}")]
    Missing(&'static str),

    #[error("ortam değişkeni geçersiz: {key} ({reason})")]
    Invalid { key: &'static str, reason: String },
}

/// "💡 Kullanım: `/komut <zorunlu> [isteğe bağlı]` — açıklama" satırı.
pub fn usage_hint(command: &poise::Command<Data, Error>) -> String {
    let mut usage = format!("/{}", command.qualified_name);
    for p in &command.parameters {
        if p.required {
            usage.push_str(&format!(" <{}>", p.name));
        } else {
            usage.push_str(&format!(" [{}]", p.name));
        }
    }
    let mut line = format!("💡 Kullanım: `{usage}`");
    if let Some(desc) = command.description.as_deref().filter(|d| !d.is_empty()) {
        line.push_str(&format!(" — {desc}"));
    }
    line.push_str("\n_`<...>` zorunlu, `[...]` isteğe bağlı. Tüm komutlar için `/yardım`._");
    line
}

pub async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    match error {
        poise::FrameworkError::Setup { error, .. } => {
            error!(error = ?error, "Framework kurulumu başarısız");
        }
        poise::FrameworkError::Command { error, ctx, .. } => {
            error!(
                command = %ctx.command().qualified_name,
                user = %ctx.author().id,
                error = ?error,
                "Komut hatası"
            );

            // Discord'un "erişim yok / izin yok" hataları kullanıcıya anlaşılır gösterilir.
            let text = match error.downcast_ref::<serenity::Error>() {
                Some(serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(resp)))
                    if matches!(resp.error.code, 50001 | 50013) =>
                {
                    "❌ Bu kanalda gerekli iznim yok (kanalı göremiyor veya yazamıyorum). Kanal izinlerinden botun rolüne **Kanalı Görüntüle** ve **Mesaj Gönder** izni ver."
                }
                _ => "⚠️ Komut çalıştırılırken beklenmeyen bir hata oluştu.",
            };
            let text = format!("{text}\n{}", usage_hint(ctx.command()));
            let reply = CreateReply::default().content(text).ephemeral(true);

            if let Err(e) = ctx.send(reply).await {
                warn!(error = %e, "Hata mesajı kullanıcıya gönderilemedi");
            }
        }
        poise::FrameworkError::MissingBotPermissions { missing_permissions, ctx, .. } => {
            let text = format!(
                "❌ Bu kanalda şu iznim eksik: **{}**. Kanal ayarlarından botun rolüne bu izni ver.",
                permission_names(missing_permissions)
            );
            if let Err(e) = ctx.send(CreateReply::default().content(text).ephemeral(true)).await {
                warn!(error = %e, "İzin uyarısı gönderilemedi");
            }
        }
        poise::FrameworkError::MissingUserPermissions { missing_permissions, ctx, .. } => {
            let text = match missing_permissions {
                Some(p) => format!("❌ Bu komut için **{}** iznin olmalı.", permission_names(p)),
                None => "❌ Bu komutu kullanma iznin yok.".to_owned(),
            };
            if let Err(e) = ctx.send(CreateReply::default().content(text).ephemeral(true)).await {
                warn!(error = %e, "İzin uyarısı gönderilemedi");
            }
        }
        poise::FrameworkError::CooldownHit { remaining_cooldown, ctx, .. } => {
            let text = format!("⏳ Biraz yavaş! **{} sn** sonra tekrar dene.", remaining_cooldown.as_secs().max(1));
            if let Err(e) = ctx.send(CreateReply::default().content(text).ephemeral(true)).await {
                warn!(error = %e, "Bekleme uyarısı gönderilemedi");
            }
        }
        poise::FrameworkError::ArgumentParse { ctx, input, .. } => {
            let what = input.map(|i| format!(" (`{}`)", i.chars().take(50).collect::<String>())).unwrap_or_default();
            let text = format!("❌ Girdiğin değer anlaşılamadı{what}.\n{}", usage_hint(ctx.command()));
            if let Err(e) = ctx.send(CreateReply::default().content(text).ephemeral(true)).await {
                warn!(error = %e, "Kullanım ipucu gönderilemedi");
            }
        }
        // İzin denetimi (perms::check) cevabı zaten verdi.
        poise::FrameworkError::CommandCheckFailed { error: None, .. } => {}
        poise::FrameworkError::EventHandler { error, event, .. } => {
            error!(event = event.snake_case_name(), error = ?error, "Event handler hatası");
        }
        other => {
            if let Err(e) = poise::builtins::on_error(other).await {
                error!(error = %e, "Framework hatası işlenemedi");
            }
        }
    }
}
