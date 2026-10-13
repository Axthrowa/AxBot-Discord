//! Özel komutlar: panelden tanımlanan `!komut`lar. Etiketlerden (`/etiket`) farkı: cevap gömülü
//! mesaj olabilir, rol verip alabilir, belirli rollere/kanallara kısıtlanabilir, bekleme süresi
//! olabilir, tetikleyen mesajı silebilir ya da cevabı özelden gönderebilir.
//!
//! Komut listesi bellekte durur (`PluginSettings`); mesaj başına veritabanına gidilmez.

use std::sync::Arc;

use poise::serenity_prelude::{self as serenity, CreateAllowedMentions, CreateEmbed, CreateMessage, GuildId, Mentionable, Message, RoleId};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::data::Data;
use crate::settings::PluginConfig;
use crate::{rolepanel, tags};

pub const MAX_COMMANDS: usize = 100;
pub const MAX_RESPONSE: usize = 2000;
pub const MAX_COOLDOWN_SECS: u32 = 3600;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Command {
    /// Küçük harfli, öneksiz ad (`kurallar`).
    pub name: String,
    pub response: String,
    /// Cevap gömülü mesaj (embed) olarak gönderilir.
    pub embed: bool,
    /// Cevap kanala değil, komutu yazana özelden gider.
    pub dm: bool,
    /// Komutu tetikleyen mesaj silinir.
    pub delete_trigger: bool,
    /// Aynı komut kanalda bu kadar saniyede bir çalışır (0 = sınırsız).
    pub cooldown_secs: u32,
    pub add_roles: Vec<u64>,
    pub remove_roles: Vec<u64>,
    /// Doluysa yalnızca bu rollerden birine sahip üyeler kullanabilir.
    pub required_roles: Vec<u64>,
    /// Doluysa yalnızca bu kanallarda çalışır.
    pub channels: Vec<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomCommandSettings {
    pub commands: Vec<Command>,
}

impl PluginConfig for CustomCommandSettings {
    const KEY: &'static str = "custom_commands";
}

/// Üye bu komutu bu kanalda kullanabilir mi?
pub fn allowed(command: &Command, channel: u64, roles: &[RoleId]) -> bool {
    let channel_ok = command.channels.is_empty() || command.channels.contains(&channel);
    let role_ok = command.required_roles.is_empty() || roles.iter().any(|r| command.required_roles.contains(&r.get()));
    channel_ok && role_ok
}

/// Cevap şablonu: `{kullanıcı}` (etiket), `{isim}`, `{sunucu}`, `{kanal}`.
pub fn render(template: &str, mention: &str, name: &str, guild: &str, channel: &str) -> String {
    template
        .replace("{kullanıcı}", mention)
        .replace("{kullanici}", mention)
        .replace("{isim}", name)
        .replace("{sunucu}", guild)
        .replace("{kanal}", channel)
}

/// Mesaj bir özel komutsa çalıştırır; çalıştırdıysa (ya da izin vermediyse) `true`.
pub async fn handle(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message, roles: &[RoleId]) -> bool {
    // Çoğu mesaj `!` ile başlamaz; bunlar için hiçbir iş yapılmaz.
    let Some(name) = tags::trigger(&msg.content) else {
        return false;
    };
    let settings: Arc<CustomCommandSettings> = match data.settings.get(&data.db, guild_id).await {
        Ok(settings) => settings,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Özel komutlar okunamadı");
            return false;
        }
    };
    let Some(command) = settings.commands.iter().find(|c| c.name == name) else {
        return false;
    };
    if !allowed(command, msg.channel_id.get(), roles) {
        // Komut var ama bu üyeye/kanala kapalı: sessizce yutulur (etiket olarak da aranmaz).
        return true;
    }
    if command.cooldown_secs > 0 && !claim_cooldown(data, guild_id, msg, command).await {
        return true;
    }

    let guild = ctx.cache.guild(guild_id).map(|g| g.name.clone()).unwrap_or_default();
    let display = msg.author.global_name.clone().unwrap_or_else(|| msg.author.name.clone());
    let text = render(
        &command.response,
        &msg.author.id.mention().to_string(),
        &display,
        &guild,
        &msg.channel_id.mention().to_string(),
    );
    // Cevap yöneticinin yazdığı metindir; yine de yalnızca komutu yazan kişi etiketlenir.
    let mut reply = CreateMessage::new().allowed_mentions(CreateAllowedMentions::new().users(vec![msg.author.id]));
    reply = if command.embed {
        reply.embed(CreateEmbed::new().description(text).colour(serenity::Colour::BLURPLE))
    } else {
        reply.content(text)
    };
    let sent = if command.dm {
        msg.author.direct_message(ctx, reply).await.map(|_| ())
    } else {
        msg.channel_id.send_message(&ctx.http, reply).await.map(|_| ())
    };
    if let Err(e) = sent {
        debug!(channel = %msg.channel_id, command = command.name, error = %e, "Özel komut cevabı gönderilemedi");
    }

    for (list, add) in [(&command.add_roles, true), (&command.remove_roles, false)] {
        for role in list.iter().filter(|id| **id != 0).map(|id| RoleId::new(*id)) {
            change_role(ctx, guild_id, msg, role, add).await;
        }
    }
    if command.delete_trigger {
        if let Err(e) = msg.delete(&ctx.http).await {
            debug!(channel = %msg.channel_id, error = %e, "Özel komut mesajı silinemedi");
        }
    }
    true
}

/// Rol hâlâ güvenle dağıtılabiliyorsa verir / alır (yetkili roller hiçbir zaman dağıtılmaz).
async fn change_role(ctx: &serenity::Context, guild_id: GuildId, msg: &Message, role_id: RoleId, add: bool) {
    let role = ctx.cache.guild(guild_id).and_then(|g| g.roles.get(&role_id).cloned());
    let Some(role) = role else { return };
    if let Err(reason) = rolepanel::check_role(ctx, guild_id, &role) {
        debug!(guild = %guild_id, role = %role_id, reason, "Özel komut: rol değiştirilmedi");
        return;
    }
    let result = if add {
        ctx.http.add_member_role(guild_id, msg.author.id, role_id, Some("Özel komut")).await
    } else {
        ctx.http.remove_member_role(guild_id, msg.author.id, role_id, Some("Özel komut")).await
    };
    if let Err(e) = result {
        warn!(guild = %guild_id, role = %role_id, error = %e, "Özel komut rol değiştiremedi");
    }
}

/// Bekleme süresi dolmuşsa `true` döner ve yeni süreyi başlatır.
async fn claim_cooldown(data: &Data, guild_id: GuildId, msg: &Message, command: &Command) -> bool {
    let key = format!("cc:cd:{guild_id}:{}:{}", msg.channel_id, command.name);
    let mut conn = data.redis.clone();
    let claimed: redis::RedisResult<Option<String>> = redis::cmd("SET")
        .arg(key)
        .arg(1)
        .arg("EX")
        .arg(command.cooldown_secs)
        .arg("NX")
        .query_async(&mut conn)
        .await;
    // Redis geçici olarak yoksa komut çalışmaya devam eder.
    claimed.map_or(true, |c| c.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restrictions() {
        let open = Command::default();
        assert!(allowed(&open, 1, &[]));
        let limited = Command { required_roles: vec![5], channels: vec![10], ..Command::default() };
        assert!(allowed(&limited, 10, &[RoleId::new(5), RoleId::new(6)]));
        assert!(!allowed(&limited, 11, &[RoleId::new(5)]));
        assert!(!allowed(&limited, 10, &[RoleId::new(6)]));
    }

    #[test]
    fn renders_placeholders_and_loads_old_records() -> anyhow::Result<()> {
        assert_eq!(render("{kullanıcı} {isim} {sunucu} {kanal}", "<@1>", "Ali", "S", "<#2>"), "<@1> Ali S <#2>");
        let settings: CustomCommandSettings = serde_json::from_str("{\"commands\":[{\"name\":\"a\",\"response\":\"b\"}]}")?;
        assert_eq!(settings.commands.len(), 1);
        assert!(!settings.commands[0].embed && settings.commands[0].cooldown_secs == 0);
        Ok(())
    }
}
