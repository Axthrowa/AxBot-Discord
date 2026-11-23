//! Komutların `required_permissions` denetimi.
//!
//! poise üyeyi ve kanalı kendisi çekmeye çalışır; biri alınamazsa (ör. kanal önbellekte yoksa)
//! komutu "iznin yok" diyerek reddeder. Slash komutlarda Discord, üyenin o kanaldaki izinlerini
//! etkileşimle birlikte gönderir; denetim bu bilgiyle yapılır.

use poise::serenity_prelude as serenity;
use serenity::Permissions;

use crate::data::{Context, Data};
use crate::error::Error;

type Command = poise::Command<Data, Error>;

/// İzin gereksinimini poise'dan alıp komutun `custom_data`'sına taşır (üst komutunkiyle birlikte).
pub fn take_required(command: &mut Command, inherited: Permissions) {
    let required = inherited | std::mem::take(&mut command.required_permissions);
    for sub in &mut command.subcommands {
        take_required(sub, required);
    }
    command.custom_data = Box::new(required);
}

/// Gerekli izinlerden üyede eksik olanlar (bilinemiyorsa `None`).
async fn missing(ctx: Context<'_>, required: Permissions) -> Option<Permissions> {
    if ctx.guild_id().is_none() {
        return Some(Permissions::empty());
    }
    let have = match ctx {
        poise::Context::Application(app) => app.interaction.member.as_ref()?.permissions?,
        poise::Context::Prefix(_) => {
            let member = ctx.author_member().await?;
            let guild = ctx.guild()?;
            let channel = guild.channels.get(&ctx.channel_id())?;
            guild.user_permissions_in(channel, &member)
        }
    };
    Some(required - have)
}

pub async fn check(ctx: Context<'_>) -> Result<bool, Error> {
    if let Some(guild_id) = ctx.guild_id() {
        let data = ctx.data();
        let top = ctx.parent_commands().first().map_or(&ctx.command().name, |p| &p.name);
        if crate::modules::module_of(top).is_some() {
            let modules = crate::commands::modules_of(&data.db, &data.settings, guild_id).await;
            if !modules.allows(top) {
                ctx.send(poise::CreateReply::default().content("❌ Bu modül bu sunucuda kapalı (yöneticiler panelden açabilir).").ephemeral(true)).await?;
                return Ok(false);
            }
        }
    }
    let Some(required) = ctx.command().custom_data.downcast_ref::<Permissions>().copied() else {
        return Ok(true);
    };
    if required.is_empty() {
        return Ok(true);
    }
    let text = match missing(ctx, required).await {
        Some(m) if m.is_empty() => return Ok(true),
        Some(m) => format!("❌ Bu komut için **{}** iznin olmalı.", crate::error::permission_names(m)),
        None => "❌ İznin doğrulanamadı; komutu slash (/) olarak kullan.".to_owned(),
    };
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subcommands_inherit_parent_permissions() {
        let mut parent = Command { required_permissions: Permissions::MANAGE_GUILD, ..Default::default() };
        parent.subcommands.push(Command { required_permissions: Permissions::MANAGE_ROLES, ..Default::default() });
        take_required(&mut parent, Permissions::empty());
        assert!(parent.required_permissions.is_empty());
        assert!(parent.subcommands[0].required_permissions.is_empty());
        let sub = parent.subcommands[0].custom_data.downcast_ref::<Permissions>().copied();
        assert_eq!(sub, Some(Permissions::MANAGE_GUILD | Permissions::MANAGE_ROLES));
    }
}
