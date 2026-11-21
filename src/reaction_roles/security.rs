//! Rol güvenliği ve hiyerarşi denetimleri.
//!
//! Dağıtılacak rollerin yönetici veya moderasyon yetkisi içermediğini ve botun en yüksek
//! rolünün altında kaldığını doğrular. Tüm komutlar ve rol dağıtım sistemleri bu modülü kullanır.

use poise::serenity_prelude::{self as serenity, GuildId, Permissions, Role};

/// Butonla veya otomatik olarak dağıtılması güvenli olmayan izinler.
pub fn dangerous() -> Permissions {
    Permissions::ADMINISTRATOR
        | Permissions::MANAGE_GUILD
        | Permissions::MANAGE_ROLES
        | Permissions::MANAGE_CHANNELS
        | Permissions::MANAGE_MESSAGES
        | Permissions::MANAGE_WEBHOOKS
        | Permissions::MANAGE_NICKNAMES
        | Permissions::BAN_MEMBERS
        | Permissions::KICK_MEMBERS
        | Permissions::MODERATE_MEMBERS
        | Permissions::MENTION_EVERYONE
        | Permissions::VIEW_AUDIT_LOG
}

/// Rol dağıtılabilir mi? Değilse sebebini döner.
pub fn check_role(ctx: &serenity::Context, guild_id: GuildId, role: &Role) -> Result<(), String> {
    if role.id == guild_id.everyone_role() {
        return Err("@everyone dağıtılamaz.".to_owned());
    }
    if role.managed {
        return Err(format!("{} bir bota/entegrasyona ait, elle verilemez.", role.name));
    }
    if role.permissions.intersects(dangerous()) {
        return Err(format!(
            "{} yönetici/moderasyon yetkisi içeriyor; güvenlik için butonla dağıtılamaz.",
            role.name
        ));
    }
    let bot_top = bot_top_position(ctx, guild_id);
    if role.position >= bot_top {
        return Err(format!("{} rolü benim rolümün üstünde (veya eşit); rolümü yukarı taşı.", role.name));
    }
    Ok(())
}

pub fn bot_top_position(ctx: &serenity::Context, guild_id: GuildId) -> u16 {
    let bot_id = ctx.cache.current_user().id;
    ctx.cache.guild(guild_id).map_or(0, |guild| {
        guild.members.get(&bot_id).map_or(0, |bot| {
            bot.roles
                .iter()
                .filter_map(|r| guild.roles.get(r))
                .map(|r| r.position)
                .max()
                .unwrap_or(0)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dangerous_permissions_cover_admin_and_moderation() {
        let perms = dangerous();
        assert!(perms.contains(Permissions::ADMINISTRATOR));
        assert!(perms.contains(Permissions::BAN_MEMBERS));
        assert!(perms.contains(Permissions::MANAGE_ROLES));
        assert!(perms.contains(Permissions::MODERATE_MEMBERS));
        assert!(!perms.contains(Permissions::SEND_MESSAGES));
        assert!(!perms.contains(Permissions::VIEW_CHANNEL));
    }
}
