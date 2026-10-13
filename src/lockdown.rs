//! Sunucu kilidi (Wick / Dyno "lockdown"): baskın sırasında bütün yazı kanallarında @everyone'ın mesaj
//! göndermesi tek seferde kapatılır; kilit açılınca her kanalın ÖNCEKİ izni aynen geri yüklenir.
//!
//! Hangi kanalın kilitlendiği ve eski izni Redis'te (`kilit:{sunucu}`) tutulur; zaten kapalı olan kanallara
//! dokunulmaz (kilit açılınca yanlışlıkla açılmasınlar).

use poise::serenity_prelude::{self as serenity, ChannelId, ChannelType, GuildId, PermissionOverwrite, PermissionOverwriteType, Permissions};
use redis::aio::ConnectionManager;
use tracing::warn;

/// Kilitte kapatılan izinler (tek kanal kilidiyle aynı).
pub fn send_permissions() -> Permissions {
    Permissions::SEND_MESSAGES | Permissions::SEND_MESSAGES_IN_THREADS | Permissions::CREATE_PUBLIC_THREADS | Permissions::CREATE_PRIVATE_THREADS
}

fn key(guild_id: GuildId) -> String {
    format!("kilit:{guild_id}")
}

/// Kanalın eski @everyone izni: `"-"` = hiç yoktu, yoksa `"izin:yasak"` bitleri.
fn encode(previous: Option<(Permissions, Permissions)>) -> String {
    previous.map_or_else(|| "-".to_owned(), |(allow, deny)| format!("{}:{}", allow.bits(), deny.bits()))
}

fn decode(raw: &str) -> Option<Option<(Permissions, Permissions)>> {
    if raw == "-" {
        return Some(None);
    }
    let (allow, deny) = raw.split_once(':')?;
    Some(Some((Permissions::from_bits_truncate(allow.parse().ok()?), Permissions::from_bits_truncate(deny.parse().ok()?))))
}

pub async fn is_locked(redis: &ConnectionManager, guild_id: GuildId) -> bool {
    let mut conn = redis.clone();
    redis::cmd("EXISTS").arg(key(guild_id)).query_async::<bool>(&mut conn).await.unwrap_or(false)
}

/// Bütün yazı kanallarını kilitler. (kilitlenen, atlanan, başarısız) sayıları.
pub async fn lock_all(ctx: &serenity::Context, redis: &ConnectionManager, guild_id: GuildId, reason: &str) -> anyhow::Result<(usize, usize, usize)> {
    let everyone = guild_id.everyone_role();
    let bot = ctx.cache.current_user().id;
    // Önbellek kilidi `await`'ten önce bırakılır.
    let channels: Vec<(ChannelId, Option<(Permissions, Permissions)>)> = ctx
        .cache
        .guild(guild_id)
        .map(|g| {
            g.channels
                .values()
                .filter(|c| matches!(c.kind, ChannelType::Text | ChannelType::News | ChannelType::Forum))
                .map(|c| {
                    let current = c.permission_overwrites.iter().find(|o| o.kind == PermissionOverwriteType::Role(everyone)).map(|o| (o.allow, o.deny));
                    (c.id, current)
                })
                .collect()
        })
        .unwrap_or_default();
    let (mut locked, mut skipped, mut failed) = (0, 0, 0);
    let mut conn = redis.clone();
    for (channel, current) in channels {
        // Zaten yazılamayan kanallar (duyuru, kurallar…) olduğu gibi kalır.
        if current.is_some_and(|(_, deny)| deny.contains(Permissions::SEND_MESSAGES)) {
            skipped += 1;
            continue;
        }
        let (allow, deny) = current.unwrap_or((Permissions::empty(), Permissions::empty()));
        let overwrite = PermissionOverwrite { allow: allow - send_permissions(), deny: deny | send_permissions(), kind: PermissionOverwriteType::Role(everyone) };
        match channel.create_permission(&ctx.http, overwrite).await {
            Ok(()) => {
                locked += 1;
                let _: () = redis::cmd("HSET").arg(key(guild_id)).arg(channel.get()).arg(encode(current)).query_async(&mut conn).await?;
                // Bot kilitli kanalda da yazabilsin.
                let bot_overwrite =
                    PermissionOverwrite { allow: Permissions::SEND_MESSAGES | Permissions::VIEW_CHANNEL, deny: Permissions::empty(), kind: PermissionOverwriteType::Member(bot) };
                let _ = channel.create_permission(&ctx.http, bot_overwrite).await;
            }
            Err(e) => {
                failed += 1;
                warn!(guild = %guild_id, channel = %channel, error = %e, reason, "Sunucu kilidi: kanal kilitlenemedi");
            }
        }
    }
    Ok((locked, skipped, failed))
}

/// Kilidi açar: kilitlenen kanalların eski izinleri geri yüklenir. (açılan, başarısız) sayıları.
pub async fn unlock_all(ctx: &serenity::Context, redis: &ConnectionManager, guild_id: GuildId) -> anyhow::Result<(usize, usize)> {
    let everyone = guild_id.everyone_role();
    let mut conn = redis.clone();
    let saved: Vec<(u64, String)> = redis::cmd("HGETALL").arg(key(guild_id)).query_async(&mut conn).await?;
    let (mut opened, mut failed) = (0, 0);
    for (channel, raw) in saved {
        let (Some(previous), true) = (decode(&raw), channel != 0) else { continue };
        let channel = ChannelId::new(channel);
        let result = match previous {
            Some((allow, deny)) => channel.create_permission(&ctx.http, PermissionOverwrite { allow, deny, kind: PermissionOverwriteType::Role(everyone) }).await,
            None => channel.delete_permission(&ctx.http, PermissionOverwriteType::Role(everyone)).await,
        };
        match result {
            Ok(()) => opened += 1,
            // Kanal silinmişse sayılmaz.
            Err(e) if e.to_string().contains("Unknown Channel") => {}
            Err(e) => {
                failed += 1;
                warn!(guild = %guild_id, channel = %channel, error = %e, "Sunucu kilidi: kanal açılamadı");
            }
        }
    }
    if failed == 0 {
        let _: () = redis::cmd("DEL").arg(key(guild_id)).query_async(&mut conn).await?;
    }
    Ok((opened, failed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_permissions_round_trip() {
        assert_eq!(decode(&encode(None)), Some(None));
        let perms = (Permissions::VIEW_CHANNEL, Permissions::ATTACH_FILES);
        assert_eq!(decode(&encode(Some(perms))), Some(Some(perms)));
        assert_eq!(decode("bozuk"), None);
    }
}
