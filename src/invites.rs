//! Davet takibi: yeni üyenin hangi davet bağlantısıyla (kimin davetiyle) geldiğini bulur.
//!
//! Discord bunu doğrudan söylemez; davetlerin kullanım sayıları bellekte tutulur ve her girişte
//! yeniden okunup karşılaştırılır (kullanımı artan davet = kullanılan davet). Bunun için botun
//! "Sunucuyu Yönet" izni olmalı. Aynı anda gelen girişler karışmasın diye girişler sıraya alınır.

use std::collections::HashMap;

use dashmap::DashMap;
use poise::serenity_prelude::{self as serenity, GuildId, Member, UserId};
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::db::snowflake;
use crate::guild_config::to_id;

/// Bu yaştan genç hesaplarla gelen davetler "sahte" sayılır.
pub const FAKE_ACCOUNT_DAYS: i64 = 7;

#[derive(Debug, Clone)]
struct CachedInvite {
    uses: u64,
    inviter: Option<UserId>,
}

/// Girişte bulunan davet.
#[derive(Debug, Clone)]
pub struct UsedInvite {
    pub code: String,
    pub inviter: Option<UserId>,
}

#[derive(Default)]
pub struct Invites {
    cache: DashMap<GuildId, HashMap<String, CachedInvite>>,
    join_lock: Mutex<()>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct InviteStats {
    /// Hâlâ sunucuda olan, sahte olmayan davetler.
    pub active: i64,
    pub left: i64,
    pub fake: i64,
}

impl Invites {
    /// Sunucunun davetlerini okuyup önbelleğe alır (açılışta ve bot sunucuya eklenince).
    pub async fn refresh(&self, http: &serenity::Http, guild_id: GuildId) {
        match guild_id.invites(http).await {
            Ok(list) => {
                let map = list
                    .into_iter()
                    .map(|i| (i.code, CachedInvite { uses: i.uses, inviter: i.inviter.map(|u| u.id) }))
                    .collect();
                self.cache.insert(guild_id, map);
            }
            Err(e) => debug!(guild = %guild_id, error = %e, "Davetler okunamadı ('Sunucuyu Yönet' izni yok olabilir)"),
        }
    }

    pub fn on_create(&self, guild_id: GuildId, code: &str, inviter: Option<UserId>, uses: u64) {
        self.cache
            .entry(guild_id)
            .or_default()
            .insert(code.to_owned(), CachedInvite { uses, inviter });
    }

    pub fn on_delete(&self, guild_id: GuildId, code: &str) {
        if let Some(mut map) = self.cache.get_mut(&guild_id) {
            map.remove(code);
        }
    }

    /// Kullanılan daveti bulur ve kaydeder. Bulunamazsa (özel URL, izin yok) davet eden boş kalır.
    pub async fn on_join(&self, http: &serenity::Http, db: &PgPool, member: &Member, account_age_days: i64) -> Option<UsedInvite> {
        let guild_id = member.guild_id;
        let _guard = self.join_lock.lock().await;

        let used = match guild_id.invites(http).await {
            Ok(list) => {
                let now: HashMap<String, CachedInvite> = list
                    .into_iter()
                    .map(|i| (i.code, CachedInvite { uses: i.uses, inviter: i.inviter.map(|u| u.id) }))
                    .collect();
                let before = self.cache.get(&guild_id).map(|m| m.clone()).unwrap_or_default();
                let used = find_used(&before, &now);
                self.cache.insert(guild_id, now);
                used
            }
            Err(e) => {
                debug!(guild = %guild_id, error = %e, "Davetler okunamadı");
                None
            }
        };

        let fake = account_age_days < FAKE_ACCOUNT_DAYS;
        if let Err(e) = record_join(db, guild_id, member.user.id, used.as_ref(), fake).await {
            warn!(guild = %guild_id, error = ?e, "Davet kaydı yazılamadı");
        }
        used
    }
}

/// Kullanımı artan davet; yoksa (son kullanımla silinen tek kullanımlık davetler için)
/// önceden olup artık olmayan tek davet.
fn find_used(before: &HashMap<String, CachedInvite>, now: &HashMap<String, CachedInvite>) -> Option<UsedInvite> {
    let increased = now.iter().find(|(code, inv)| {
        let old = before.get(*code).map_or(0, |b| b.uses);
        inv.uses > old
    });
    if let Some((code, inv)) = increased {
        return Some(UsedInvite { code: code.clone(), inviter: inv.inviter });
    }
    let mut vanished = before.iter().filter(|(code, _)| !now.contains_key(*code));
    match (vanished.next(), vanished.next()) {
        (Some((code, inv)), None) => Some(UsedInvite { code: code.clone(), inviter: inv.inviter }),
        _ => None,
    }
}

async fn record_join(db: &PgPool, guild_id: GuildId, user: UserId, used: Option<&UsedInvite>, fake: bool) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO invite_joins (guild_id, user_id, inviter_id, code, fake) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (guild_id, user_id) DO UPDATE
             SET inviter_id = EXCLUDED.inviter_id, code = EXCLUDED.code, fake = EXCLUDED.fake,
                 joined_at = now(), left_at = NULL",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(user.get())?)
    .bind(used.and_then(|u| u.inviter).map(|u| snowflake(u.get())).transpose()?)
    .bind(used.map(|u| u.code.as_str()))
    .bind(fake)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn on_leave(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<()> {
    sqlx::query("UPDATE invite_joins SET left_at = now() WHERE guild_id = $1 AND user_id = $2")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn stats(db: &PgPool, guild_id: GuildId, inviter: UserId) -> anyhow::Result<InviteStats> {
    let (active, left, fake): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE left_at IS NULL AND NOT fake),
                COUNT(*) FILTER (WHERE left_at IS NOT NULL),
                COUNT(*) FILTER (WHERE fake AND left_at IS NULL)
         FROM invite_joins WHERE guild_id = $1 AND inviter_id = $2",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(inviter.get())?)
    .fetch_one(db)
    .await?;
    Ok(InviteStats { active, left, fake })
}

/// Üyeyi kimin davet ettiği (biliniyorsa) ve hangi kodla.
pub async fn invited_by(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<Option<(Option<UserId>, Option<String>)>> {
    let row: Option<(Option<i64>, Option<String>)> =
        sqlx::query_as("SELECT inviter_id, code FROM invite_joins WHERE guild_id = $1 AND user_id = $2")
            .bind(snowflake(guild_id.get())?)
            .bind(snowflake(user.get())?)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(inviter, code)| (to_id(inviter).map(UserId::new), code)))
}

/// En çok aktif daveti olan üyeler.
pub async fn leaderboard(db: &PgPool, guild_id: GuildId, limit: i64) -> anyhow::Result<Vec<(UserId, i64)>> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT inviter_id, COUNT(*) AS active FROM invite_joins
         WHERE guild_id = $1 AND inviter_id IS NOT NULL AND left_at IS NULL AND NOT fake
         GROUP BY inviter_id ORDER BY active DESC LIMIT $2",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, n)| to_id(Some(id)).map(|id| (UserId::new(id), n)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(items: &[(&str, u64)]) -> HashMap<String, CachedInvite> {
        items
            .iter()
            .map(|(c, u)| ((*c).to_owned(), CachedInvite { uses: *u, inviter: Some(UserId::new(7)) }))
            .collect()
    }

    #[test]
    fn finds_invite_with_increased_uses() {
        let before = map(&[("a", 1), ("b", 5)]);
        let now = map(&[("a", 1), ("b", 6)]);
        assert_eq!(find_used(&before, &now).map(|u| u.code), Some("b".to_owned()));
    }

    #[test]
    fn new_invite_used_for_the_first_time() {
        let before = map(&[("a", 1)]);
        let now = map(&[("a", 1), ("yeni", 1)]);
        assert_eq!(find_used(&before, &now).map(|u| u.code), Some("yeni".to_owned()));
    }

    #[test]
    fn single_use_invite_that_vanished() {
        let before = map(&[("a", 1), ("tek", 0)]);
        let now = map(&[("a", 1)]);
        assert_eq!(find_used(&before, &now).map(|u| u.code), Some("tek".to_owned()));
    }

    #[test]
    fn ambiguous_or_unknown_is_none() {
        let before = map(&[("a", 1), ("b", 1), ("c", 1)]);
        let now = map(&[("a", 1)]);
        assert!(find_used(&before, &now).is_none());
        assert!(find_used(&map(&[("a", 1)]), &map(&[("a", 1)])).is_none());
    }
}
