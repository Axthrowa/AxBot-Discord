//! Karantina: üyenin rolleri saklanıp alınır, yalnızca karantina rolü verilir; kaldırılınca roller geri gelir.
//! Rol listesi `quarantines` tablosunda durur (bot yeniden başlasa da kaybolmaz).

use poise::serenity_prelude::{self as serenity, EditMember, GuildId, Member, RoleId};
use sqlx::PgPool;

use crate::db::snowflake;

/// Üyeyi karantinaya alır. Zaten karantinadaysa `Ok(false)`.
/// Discord'un yönettiği roller (bot / Nitro takviye rolleri) alınamadığı için üyede kalır.
pub async fn apply(
    cache: &serenity::Cache,
    http: &serenity::Http,
    db: &PgPool,
    guild_id: GuildId,
    member: &Member,
    role: RoleId,
    reason: &str,
) -> anyhow::Result<bool> {
    let managed: Vec<RoleId> = match cache.guild(guild_id) {
        Some(guild) => member.roles.iter().filter(|r| guild.roles.get(r).is_some_and(|r| r.managed)).copied().collect(),
        None => Vec::new(),
    };
    let saved: Vec<i64> = member
        .roles
        .iter()
        .filter(|r| **r != role && !managed.contains(r))
        .map(|r| snowflake(r.get()))
        .collect::<Result<_, _>>()?;
    let inserted = sqlx::query(
        "INSERT INTO quarantines (guild_id, user_id, roles, reason) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(member.user.id.get())?)
    .bind(&saved)
    .bind(reason)
    .execute(db)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Ok(false);
    }
    let mut roles = managed;
    roles.push(role);
    let edit = EditMember::new().roles(roles).audit_log_reason(reason);
    if let Err(e) = guild_id.edit_member(http, member.user.id, edit).await {
        // Roller değişmediyse kayıt da kalmamalı; yoksa üye "karantinada" görünür.
        forget(db, guild_id, member.user.id).await?;
        return Err(e.into());
    }
    Ok(true)
}

/// Karantinayı kaldırır ve saklanan rolleri geri verir (o arada silinmiş roller atlanır).
/// Üye karantinada değilse `Ok(false)`.
pub async fn release(
    cache: &serenity::Cache,
    http: &serenity::Http,
    db: &PgPool,
    guild_id: GuildId,
    member: &Member,
    role: Option<RoleId>,
    reason: &str,
) -> anyhow::Result<bool> {
    let saved: Option<Vec<i64>> = sqlx::query_scalar("SELECT roles FROM quarantines WHERE guild_id = $1 AND user_id = $2")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(member.user.id.get())?)
        .fetch_optional(db)
        .await?;
    let Some(saved) = saved else {
        return Ok(false);
    };
    let existing = |id: &RoleId| cache.guild(guild_id).is_none_or(|g| g.roles.contains_key(id));
    let mut roles: Vec<RoleId> = saved.iter().filter_map(|r| u64::try_from(*r).ok()).map(RoleId::new).filter(existing).collect();
    // Karantina sırasında eklenen roller (karantina rolü hariç) korunur.
    let added: Vec<RoleId> = member.roles.iter().filter(|r| Some(**r) != role && !roles.contains(r)).copied().collect();
    roles.extend(added);
    guild_id.edit_member(http, member.user.id, EditMember::new().roles(roles).audit_log_reason(reason)).await?;
    forget(db, guild_id, member.user.id).await?;
    Ok(true)
}

async fn forget(db: &PgPool, guild_id: GuildId, user: serenity::UserId) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM quarantines WHERE guild_id = $1 AND user_id = $2")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .execute(db)
        .await?;
    Ok(())
}

/// Karantinadaki biri çıkıp geri girerse karantina rolü yeniden verilir (`members::on_join`). Karantinadaysa `true`.
pub async fn on_rejoin(http: &serenity::Http, data: &crate::data::Data, member: &Member) -> anyhow::Result<bool> {
    let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM quarantines WHERE guild_id = $1 AND user_id = $2")
        .bind(snowflake(member.guild_id.get())?)
        .bind(snowflake(member.user.id.get())?)
        .fetch_optional(&data.db)
        .await?;
    if found.is_none() {
        return Ok(false);
    }
    let settings = data.settings.get::<crate::automod::rules::ModSettings>(&data.db, member.guild_id).await?;
    if let Some(role) = settings.quarantine_role.filter(|r| *r != 0) {
        member.add_role(http, RoleId::new(role)).await?;
    }
    Ok(true)
}
