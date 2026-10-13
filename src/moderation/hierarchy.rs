//! Rol hiyerarşisi kontrolü: Discord'un sunucu tarafında yaptığı kontrolü önceden yapıp
//! kullanıcıya anlaşılır bir mesaj verir (aksi halde sadece "Missing Permissions" döner).

use poise::serenity_prelude::{self as serenity, Guild, GuildId, Member, RoleId};

/// İşlem yapılabiliyorsa `Ok`, yapılamıyorsa kullanıcıya gösterilecek sebebi döner.
pub async fn check(
    ctx: &serenity::Context,
    guild_id: GuildId,
    moderator: &Member,
    target: &Member,
) -> Result<(), String> {
    let bot_id = ctx.cache.current_user().id;

    if target.user.id == moderator.user.id {
        return Err("Kendine moderasyon işlemi uygulayamazsın.".to_owned());
    }
    if target.user.id == bot_id {
        return Err("Bana moderasyon işlemi uygulayamazsın.".to_owned());
    }

    // Botun kendi üyeliği önbellekte olmalı; değilse Discord'dan çekilir.
    let bot_member = match guild_id.member(ctx, bot_id).await {
        Ok(member) => member,
        Err(e) => return Err(format!("Botun sunucu bilgisi alınamadı: {e}")),
    };

    let Some(guild) = ctx.cache.guild(guild_id) else {
        return Err("Sunucu bilgisi henüz yüklenmedi, birkaç saniye sonra tekrar dene.".to_owned());
    };

    if target.user.id == guild.owner_id {
        return Err("Sunucu sahibine moderasyon işlemi uygulanamaz.".to_owned());
    }

    let target_pos = top_position(&guild, &target.roles);
    let moderator_pos = if moderator.user.id == guild.owner_id {
        u16::MAX
    } else {
        top_position(&guild, &moderator.roles)
    };
    let bot_pos = top_position(&guild, &bot_member.roles);

    if moderator_pos <= target_pos {
        return Err("Bu üyenin en yüksek rolü seninkine eşit veya daha yüksek.".to_owned());
    }
    if bot_pos <= target_pos {
        return Err(
            "Bu üyenin en yüksek rolü benimkine eşit veya daha yüksek. \
             Sunucu Ayarları → Roller'den rolümü yukarı taşı."
                .to_owned(),
        );
    }
    Ok(())
}

fn top_position(guild: &Guild, roles: &[RoleId]) -> u16 {
    roles
        .iter()
        .filter_map(|id| guild.roles.get(id))
        .map(|role| role.position)
        .max()
        .unwrap_or(0)
}
