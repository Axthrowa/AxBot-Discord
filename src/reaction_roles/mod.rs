//! Rol Menüleri (Reaction Roles & Role Panel).
//!
//! Üyelerin emoji tepkisi, buton (`rr:` veya `rrm:`) ya da açılır menü (`rrm:select`) ile
//! rol alıp bırakmasını sağlar.
//!
//! - `security`: Bot genelinde kullanılan rol güvenlik ve hiyerarşi denetimleri (`check_role`, `dangerous`).
//! - `stateless`: `/rol-menüsü` komutunun sıfır-DB hızlı `rr:` buton yapısı.
//! - `stateful`: Web panel destekli gelişmiş 3 modlu ve 5 davranışlı `rrm:` / `rrm:select` rol menüsü motoru.

pub mod security;
pub mod stateful;
pub mod stateless;

use poise::serenity_prelude::{self as serenity, ComponentInteraction};

use crate::data::Data;

// Doğrudan dışa aktarılanlar
pub use security::{check_role, dangerous};
pub use stateful::{
    emoji_key, publish, Behavior, Menu, Mode, Pair, ReactionRoleSettings, MAX_MENUS, MAX_PAIRS,
};
pub use stateless::{panel_message, MAX_ROLES};

/// Bir bileşen etkileşim kimliğinin bu modüle ait olup olmadığını kontrol eder.
/// Hem eski ve yeni `rr:` (stateless) hem `rrm:` (stateful) kimliklerini tanır.
pub fn matches_component(id: &str) -> bool {
    id.starts_with(stateless::PREFIX) || id.starts_with(stateful::PREFIX)
}

/// Gelen buton veya menü etkileşimini doğru alt modüle (stateless ya da stateful) yönlendirir.
pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
) -> anyhow::Result<()> {
    let id = &interaction.data.custom_id;
    if id.starts_with(stateless::PREFIX) {
        stateless::handle_button(ctx, interaction).await
    } else if id.starts_with(stateful::PREFIX) {
        stateful::handle_component(ctx, data, interaction).await
    } else {
        Ok(())
    }
}

pub use stateful::on_reaction;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_both_prefixes() {
        assert!(matches_component("rr:123456789"));
        assert!(matches_component("rrm:123456789"));
        assert!(matches_component("rrm:select"));
        assert!(!matches_component("ticket:open"));
        assert!(!matches_component("verify:button"));
        assert!(!matches_component(""));
        assert!(!matches_component("r:1234"));
    }

    #[test]
    fn prefix_separation() {
        assert_eq!(stateless::PREFIX, "rr:");
        assert_eq!(stateful::PREFIX, "rrm:");
        assert_eq!(stateful::SELECT_ID, "rrm:select");
        assert!(stateful::SELECT_ID.starts_with(stateful::PREFIX));
        assert!(!stateful::SELECT_ID.starts_with(stateless::PREFIX));
    }

    #[test]
    fn role_id_extraction_stateless_and_stateful() {
        let stateless_id = "rr:987654321012345678";
        let stateful_id = "rrm:987654321012345678";

        let parsed_stateless = stateless_id
            .strip_prefix(stateless::PREFIX)
            .and_then(|s| s.parse::<u64>().ok());
        let parsed_stateful = stateful_id
            .strip_prefix(stateful::PREFIX)
            .and_then(|s| s.parse::<u64>().ok());

        assert_eq!(parsed_stateless, Some(987654321012345678));
        assert_eq!(parsed_stateful, Some(987654321012345678));
    }

    #[test]
    fn dangerous_permissions_sanity() {
        let dangerous = dangerous();
        assert!(dangerous.intersects(serenity::Permissions::ADMINISTRATOR));
        assert!(dangerous.intersects(serenity::Permissions::MANAGE_ROLES));
        assert!(dangerous.intersects(serenity::Permissions::BAN_MEMBERS));
        assert!(dangerous.intersects(serenity::Permissions::KICK_MEMBERS));
        assert!(dangerous.intersects(serenity::Permissions::MANAGE_GUILD));
        assert!(!dangerous.intersects(serenity::Permissions::VIEW_CHANNEL));
        assert!(!dangerous.intersects(serenity::Permissions::SEND_MESSAGES));
    }
}
