//! Kalıcı zamanlanmış görevler: çekiliş bitişi, hatırlatma, süreli ban kaldırma, süreli rol.
//!
//! Görevler PostgreSQL'de tutulur (bot kapalıyken zamanı gelenler açılışta çalışır).
//! Çalıştırıcı periyodik sorgulamaz: bir sonraki görevin zamanına kadar uyur, daha yakın
//! bir görev eklenirse `Notify` ile anında uyanır.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context as _};
use chrono::{DateTime, Utc};
use poise::serenity_prelude::{ChannelId, CreateAllowedMentions, CreateMessage, GuildId, Http, Mentionable, RoleId, UserId};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::PgPool;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::modlog::{CaseLog, ModLog};
use crate::moderation::{self, ModAction};

/// Uyku en fazla bu kadar sürer (saat değişimi vb. durumlara karşı emniyet).
const MAX_SLEEP: Duration = Duration::from_secs(60);
const BATCH: i64 = 50;
const MAX_ATTEMPTS: i32 = 5;
const RETRY_AFTER_SECS: i64 = 60;
/// Çalıştırılmak üzere alınan görev bu kadar ileri ertelenir (yarıda kalırsa o zaman tekrar çalışır).
const CLAIM_SECS: i64 = 5 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Task {
    GiveawayEnd {
        giveaway_id: i64,
    },
    Reminder {
        user_id: u64,
        channel_id: u64,
        text: String,
        created_unix: i64,
    },
    Unban {
        guild_id: u64,
        user_id: u64,
    },
    /// `/rol süreli-ver` ile verilen rolün süresi doldu.
    TempRoleEnd {
        guild_id: u64,
        user_id: u64,
        role_id: u64,
    },
}

pub struct Scheduler {
    db: PgPool,
    wake: Notify,
}

/// Görevleri çalıştırmak için gereken paylaşılan nesneler.
pub struct Executor {
    pub http: Arc<Http>,
    pub modlog: Arc<ModLog>,
    pub bot_id: UserId,
}

impl Scheduler {
    pub fn new(db: PgPool) -> Arc<Self> {
        Arc::new(Self {
            db,
            wake: Notify::new(),
        })
    }

    pub async fn schedule(&self, run_at_unix: i64, task: &Task) -> anyhow::Result<i64> {
        let run_at = DateTime::<Utc>::from_timestamp(run_at_unix, 0)
            .ok_or_else(|| anyhow!("geçersiz zaman: {run_at_unix}"))?;
        let id: i64 =
            sqlx::query_scalar("INSERT INTO scheduled_tasks (run_at, payload) VALUES ($1, $2) RETURNING id")
                .bind(run_at)
                .bind(Json(task))
                .fetch_one(&self.db)
                .await?;
        // Çalıştırıcı daha geç bir zamana kadar uyuyor olabilir.
        self.wake.notify_one();
        Ok(id)
    }

    /// Kullanıcının bekleyen hatırlatma sayısı (kötüye kullanım sınırı için).
    pub async fn pending_reminders(&self, user_id: UserId) -> anyhow::Result<i64> {
        let n = sqlx::query_scalar(
            "SELECT COUNT(*) FROM scheduled_tasks
             WHERE payload->>'kind' = 'reminder' AND (payload->>'user_id')::BIGINT = $1",
        )
        .bind(crate::db::snowflake(user_id.get())?)
        .fetch_one(&self.db)
        .await?;
        Ok(n)
    }

    /// Sunucuda bekleyen süreli rol sayısı.
    pub async fn pending_temp_roles(&self, guild: GuildId) -> anyhow::Result<i64> {
        let n = sqlx::query_scalar(
            "SELECT COUNT(*) FROM scheduled_tasks
             WHERE payload->>'kind' = 'temp_role_end' AND (payload->>'guild_id')::BIGINT = $1",
        )
        .bind(crate::db::snowflake(guild.get())?)
        .fetch_one(&self.db)
        .await?;
        Ok(n)
    }

    /// Bir üyedeki süreli rol görevini siler; silinen görev sayısını döndürür.
    pub async fn cancel_temp_role(&self, guild: GuildId, user: UserId, role: RoleId) -> anyhow::Result<u64> {
        let done = sqlx::query(
            "DELETE FROM scheduled_tasks
             WHERE payload->>'kind' = 'temp_role_end' AND (payload->>'guild_id')::BIGINT = $1
               AND (payload->>'user_id')::BIGINT = $2 AND (payload->>'role_id')::BIGINT = $3",
        )
        .bind(crate::db::snowflake(guild.get())?)
        .bind(crate::db::snowflake(user.get())?)
        .bind(crate::db::snowflake(role.get())?)
        .execute(&self.db)
        .await?;
        Ok(done.rows_affected())
    }

    /// Bekleyen süreli roller: (üye, rol, bitiş unix), en yakın biten önce.
    pub async fn temp_roles(&self, guild: GuildId, limit: i64) -> anyhow::Result<Vec<(i64, i64, i64)>> {
        let rows = sqlx::query_as(
            "SELECT (payload->>'user_id')::BIGINT, (payload->>'role_id')::BIGINT, EXTRACT(EPOCH FROM run_at)::BIGINT
             FROM scheduled_tasks
             WHERE payload->>'kind' = 'temp_role_end' AND (payload->>'guild_id')::BIGINT = $1
             ORDER BY run_at LIMIT $2",
        )
        .bind(crate::db::snowflake(guild.get())?)
        .bind(limit)
        .fetch_all(&self.db)
        .await?;
        Ok(rows)
    }

    pub fn spawn(self: &Arc<Self>, executor: Executor) {
        let scheduler = Arc::clone(self);
        tokio::spawn(async move {
            info!("Zamanlayıcı başladı");
            loop {
                crate::supervise::round("zamanlayıcı", Duration::from_secs(300), async {
                    if let Err(e) = scheduler.run_due(&executor).await {
                        warn!(error = ?e, "Zamanlanmış görevler çalıştırılamadı");
                    }
                })
                .await;
                let sleep = match scheduler.next_due_in().await {
                    Ok(Some(d)) => d.min(MAX_SLEEP),
                    Ok(None) => MAX_SLEEP,
                    Err(e) => {
                        warn!(error = ?e, "Sıradaki görev zamanı okunamadı");
                        MAX_SLEEP
                    }
                };
                tokio::select! {
                    () = tokio::time::sleep(sleep) => {}
                    () = scheduler.wake.notified() => {}
                }
            }
        });
    }

    async fn next_due_in(&self) -> anyhow::Result<Option<Duration>> {
        let secs: Option<f64> = sqlx::query_scalar(
            "SELECT EXTRACT(EPOCH FROM (MIN(run_at) - now()))::FLOAT8 FROM scheduled_tasks",
        )
        .fetch_one(&self.db)
        .await?;
        Ok(secs.map(|s| Duration::from_secs_f64(s.max(0.0))))
    }

    async fn run_due(&self, executor: &Executor) -> anyhow::Result<()> {
        loop {
            // Zamanı gelenler silinmez, ileri ertelenerek "ayrılır": bot görevi çalıştırırken kapanırsa görev
            // kaybolmaz, CLAIM_SECS sonra yeniden çalışır (görevler iki kez çalışsa da zarar vermez).
            let due: Vec<(i64, Json<Task>, i32)> = sqlx::query_as(
                "UPDATE scheduled_tasks SET run_at = now() + make_interval(secs => $2) WHERE id IN (
                     SELECT id FROM scheduled_tasks WHERE run_at <= now()
                     ORDER BY run_at LIMIT $1 FOR UPDATE SKIP LOCKED)
                 RETURNING id, payload, attempts",
            )
            .bind(BATCH)
            .bind(CLAIM_SECS as f64)
            .fetch_all(&self.db)
            .await?;
            if due.is_empty() {
                return Ok(());
            }

            for (id, Json(task), attempts) in due {
                debug!(id, ?task, "Zamanlanmış görev çalışıyor");
                let done = match execute(&self.db, executor, &task).await {
                    Ok(()) => true,
                    Err(e) if attempts + 1 >= MAX_ATTEMPTS => {
                        warn!(id, ?task, error = ?e, "Görev {MAX_ATTEMPTS} denemede başarısız, vazgeçildi");
                        true
                    }
                    Err(e) => {
                        warn!(id, ?task, error = ?e, "Görev başarısız, 1 dk sonra tekrar denenecek");
                        false
                    }
                };
                if done {
                    sqlx::query("DELETE FROM scheduled_tasks WHERE id = $1").bind(id).execute(&self.db).await?;
                } else {
                    sqlx::query("UPDATE scheduled_tasks SET run_at = now() + make_interval(secs => $2), attempts = attempts + 1 WHERE id = $1")
                        .bind(id)
                        .bind(RETRY_AFTER_SECS as f64)
                        .execute(&self.db)
                        .await?;
                }
            }
        }
    }
}

async fn execute(db: &PgPool, ex: &Executor, task: &Task) -> anyhow::Result<()> {
    match task {
        Task::GiveawayEnd { giveaway_id } => crate::giveaway::end(&ex.http, db, *giveaway_id).await,
        Task::Reminder {
            user_id,
            channel_id,
            text,
            created_unix,
        } => {
            let user = UserId::new(nonzero(*user_id)?);
            let content = format!(
                "⏰ {} hatırlatma (<t:{created_unix}:R> istemiştin):\n> {text}",
                user.mention()
            );
            let message = CreateMessage::new()
                .content(&content)
                .allowed_mentions(CreateAllowedMentions::new().users(vec![user]));
            // Önce istendiği kanala; yazılamıyorsa DM.
            let channel = ChannelId::new(nonzero(*channel_id)?);
            if channel.send_message(&ex.http, message).await.is_err() {
                let dm = user.create_dm_channel(&ex.http).await.context("DM açılamadı")?;
                dm.id
                    .send_message(&ex.http, CreateMessage::new().content(content))
                    .await
                    .context("hatırlatma gönderilemedi")?;
            }
            Ok(())
        }
        Task::TempRoleEnd { guild_id, user_id, role_id } => {
            let guild = GuildId::new(nonzero(*guild_id)?);
            let user = UserId::new(nonzero(*user_id)?);
            let role = RoleId::new(nonzero(*role_id)?);
            match ex.http.remove_member_role(guild, user, role, Some("Süreli rolün süresi doldu")).await {
                Ok(()) => Ok(()),
                // Üye ayrılmış ya da rol silinmişse yapılacak bir şey yok.
                Err(e) if ["Unknown Member", "Unknown Role", "Unknown Guild"].iter().any(|m| e.to_string().contains(m)) => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
        Task::Unban { guild_id, user_id } => {
            let guild = GuildId::new(nonzero(*guild_id)?);
            let user = UserId::new(nonzero(*user_id)?);
            let reason = "Süreli yasağın süresi doldu";
            if let Err(e) = ex.http.remove_ban(guild, user, Some(reason)).await {
                // Elle kaldırılmışsa sorun değil.
                if !e.to_string().contains("Unknown Ban") {
                    return Err(e.into());
                }
                return Ok(());
            }
            let case = moderation::record_case(db, guild, ModAction::Unban, user, ex.bot_id, Some(reason), None).await?;
            if let Ok(target) = user.to_user(&ex.http).await {
                ex.modlog
                    .case(
                        &ex.http,
                        db,
                        guild,
                        CaseLog {
                            action: ModAction::Unban,
                            case_number: case.number,
                            target: &target,
                            moderator: ex.bot_id,
                            reason: Some(reason),
                            duration: None,
                        },
                    )
                    .await;
            }
            Ok(())
        }
    }
}

fn nonzero(id: u64) -> anyhow::Result<u64> {
    if id == 0 {
        Err(anyhow!("sıfır Discord ID"))
    } else {
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_json_roundtrip_is_tagged() -> anyhow::Result<()> {
        let task = Task::Unban { guild_id: 1, user_id: 2 };
        let json = serde_json::to_string(&task)?;
        assert_eq!(json, r#"{"kind":"unban","guild_id":1,"user_id":2}"#);
        let back: Task = serde_json::from_str(&json)?;
        assert!(matches!(back, Task::Unban { guild_id: 1, user_id: 2 }));
        Ok(())
    }
}
