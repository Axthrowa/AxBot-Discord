//! Butonlu rol menüsü (Carl-bot "reaction roles" benzeri).
//!
//! Rol ID'si butonun `custom_id`'sinde saklanır (`rr:<rol>`); tıklamada veritabanına gidilmez.
//! Yönetici veya moderasyon yetkisi veren roller hiçbir zaman dağıtılmaz (hem kurulumda hem
//! tıklama anında kontrol edilir; rol sonradan yetki kazanırsa buton çalışmayı bırakır).

use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteraction, CreateActionRow, CreateButton,
    CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
    GuildId, Mentionable, Permissions, Role, RoleId,
};

pub const PREFIX: &str = "rr:";
pub const MAX_ROLES: usize = 10;

/// Butonla dağıtılması güvenli olmayan izinler.
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

fn bot_top_position(ctx: &serenity::Context, guild_id: GuildId) -> u16 {
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
