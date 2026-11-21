//! Durumsuz (stateless) butonlu rol menüsü (komut tabanlı: `/rol-menüsü`).
//!
//! Rol ID'si butonun `custom_id`'sinde saklanır (`rr:<rol>`); tıklamada veritabanına gidilmez.
//! Yönetici veya moderasyon yetkisi veren roller hiçbir zaman dağıtılmaz.

use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteraction, CreateActionRow, CreateButton,
    CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
    Mentionable, Role, RoleId,
};

use super::security::check_role;

pub const PREFIX: &str = "rr:";
pub const MAX_ROLES: usize = 10;

pub fn panel_message(title: &str, description: Option<&str>, roles: &[Role]) -> CreateMessage {
    let list = roles.iter().map(|r| format!("• {}", r.id.mention())).collect::<Vec<_>>().join("\n");
    let embed = CreateEmbed::new()
        .title(title)
        .description(format!(
            "{}\n\n{list}\n\n_Butona basarak rolü al; tekrar basarak bırak._",
            description.unwrap_or("Almak istediğin rollerin butonuna bas.")
        ))
        .colour(serenity::Colour::BLURPLE);

    let buttons: Vec<CreateButton> = roles
        .iter()
        .map(|r| {
            CreateButton::new(format!("{PREFIX}{}", r.id))
                .label(r.name.chars().take(80).collect::<String>())
                .style(ButtonStyle::Secondary)
        })
        .collect();
    let rows = buttons
        .chunks(5)
        .map(|chunk| CreateActionRow::Buttons(chunk.to_vec()))
        .collect();

    CreateMessage::new().embed(embed).components(rows)
}

pub async fn handle_button(ctx: &serenity::Context, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let text = toggle(ctx, interaction).await;
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true)),
        )
        .await?;
    Ok(())
}

async fn toggle(ctx: &serenity::Context, interaction: &ComponentInteraction) -> String {
    let (Some(guild_id), Some(member)) = (interaction.guild_id, interaction.member.as_ref()) else {
        return "Bu buton sadece sunucuda çalışır.".to_owned();
    };
    let Some(role_id) = interaction
        .data
        .custom_id
        .strip_prefix(PREFIX)
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|id| *id != 0)
        .map(RoleId::new)
    else {
        return "Geçersiz buton.".to_owned();
    };

    // Rol hâlâ var mı ve hâlâ güvenli mi? (Önbellekten; ağ isteği yok.)
    let role = ctx.cache.guild(guild_id).and_then(|g| g.roles.get(&role_id).cloned());
    let Some(role) = role else {
        return "Bu rol artık yok.".to_owned();
    };
    if let Err(reason) = check_role(ctx, guild_id, &role) {
        return format!("❌ {reason}");
    }

    let has_role = member.roles.contains(&role_id);
    let result = if has_role {
        ctx.http
            .remove_member_role(guild_id, member.user.id, role_id, Some("Rol menüsü"))
            .await
    } else {
        ctx.http
            .add_member_role(guild_id, member.user.id, role_id, Some("Rol menüsü"))
            .await
    };
    match (result, has_role) {
        (Ok(()), false) => format!("✅ {} rolü verildi.", role_id.mention()),
        (Ok(()), true) => format!("➖ {} rolü alındı.", role_id.mention()),
        (Err(e), _) => {
            tracing::warn!(guild = %guild_id, role = %role_id, error = %e, "Rol menüsü rol veremedi");
            "❌ Rol değiştirilemedi; botun 'Rolleri Yönet' izni ve rol sıralaması kontrol edilmeli.".to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_parsing() {
        let custom_id = "rr:123456789012345678";
        assert!(custom_id.starts_with(PREFIX));
        let role_id = custom_id.strip_prefix(PREFIX).and_then(|s| s.parse::<u64>().ok());
        assert_eq!(role_id, Some(123456789012345678));

        let bad_id = "rrm:123456789";
        assert!(!bad_id.starts_with(PREFIX));
    }
}
