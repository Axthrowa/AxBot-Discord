//! Geçici ses odaları: "➕ Oda Oluştur" kanalına giren üyeye kendi ses odası açılır ve üye
//! oraya taşınır; oda boşalınca silinir. Oda sahibi `/oda` komutlarıyla odasını yönetir.
//!
//! Odalar bellekte (hızlı kontrol) ve veritabanında (bot yeniden başlarsa boş odalar yine
//! silinsin diye) tutulur. Doluluk bot önbelleğindeki ses durumlarından okunur.

use std::time::{Duration, Instant};

use dashmap::DashMap;
use poise::serenity_prelude::{
    self as serenity, ChannelId, ChannelType, CreateChannel, GuildId, PermissionOverwrite,
    PermissionOverwriteType, Permissions, UserId, VoiceState,
};
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::data::Data;
use crate::db::snowflake;
use crate::guild_config::to_id;

/// Yeni açılan oda, üye taşınana kadar boş görünür; bu süre dolmadan silinmez.
const GRACE: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy)]
struct Room {
    guild_id: GuildId,
    owner: UserId,
    created: Instant,
}

#[derive(Default)]
pub struct TempVoice {
    rooms: DashMap<ChannelId, Room>,
    /// Aynı anda iki oda açılmasını / yarım kalmış silmeleri önler.
    lock: Mutex<()>,
}

impl TempVoice {
    /// Açılışta önceki odaları yükler (boş olanlar ilk ses olayında silinir).
    pub async fn load(&self, db: &PgPool) -> anyhow::Result<()> {
        let rows: Vec<(i64, i64, i64)> = sqlx::query_as("SELECT channel_id, guild_id, owner_id FROM temp_voice_channels")
            .fetch_all(db)
            .await?;
        let old = Instant::now().checked_sub(GRACE).unwrap_or_else(Instant::now);
        for (channel, guild, owner) in rows {
            if let (Some(c), Some(g), Some(o)) = (to_id(Some(channel)), to_id(Some(guild)), to_id(Some(owner))) {
                self.rooms.insert(
                    ChannelId::new(c),
                    Room { guild_id: GuildId::new(g), owner: UserId::new(o), created: old },
                );
            }
        }
        Ok(())
    }

    pub fn owner_of(&self, channel: ChannelId) -> Option<UserId> {
        self.rooms.get(&channel).map(|r| r.owner)
    }

    pub async fn set_owner(&self, db: &PgPool, channel: ChannelId, owner: UserId) -> anyhow::Result<()> {
        if let Some(mut room) = self.rooms.get_mut(&channel) {
            room.owner = owner;
        }
        sqlx::query("UPDATE temp_voice_channels SET owner_id = $2 WHERE channel_id = $1")
            .bind(snowflake(channel.get())?)
            .bind(snowflake(owner.get())?)
            .execute(db)
            .await?;
        Ok(())
    }

    /// Ses durumu değişti: oda oluşturma kanalına girildiyse oda açar, boşalan odaları siler.
    pub async fn on_voice_update(&self, ctx: &serenity::Context, data: &Data, new: &VoiceState) {
        let Some(guild_id) = new.guild_id else {
            return;
        };
        let has_rooms = self.rooms.iter().any(|r| r.guild_id == guild_id);
        let hub = match data.guild_configs.get(&data.db, guild_id).await {
            Ok(cfg) => cfg.tempvoice_hub,
            Err(e) => {
                warn!(guild = %guild_id, error = ?e, "Sunucu ayarları okunamadı");
                return;
            }
        };
        if hub.is_none() && !has_rooms {
            return;
        }

        let _guard = self.lock.lock().await;
        if let (Some(hub), Some(joined)) = (hub, new.channel_id) {
            if hub == joined {
                if let Err(e) = self.open_room(ctx, &data.db, guild_id, hub, new).await {
                    warn!(guild = %guild_id, error = ?e, "Geçici oda açılamadı ('Kanalları Yönet' ve 'Üyeleri Taşı' izni gerekli)");
                }
            }
        }
        self.sweep(ctx, &data.db, guild_id).await;
    }

    async fn open_room(&self, ctx: &serenity::Context, db: &PgPool, guild_id: GuildId, hub: ChannelId, state: &VoiceState) -> anyhow::Result<()> {
        let user = state.user_id;
        // Zaten açık bir odası varsa oraya taşı.
        let existing = self.rooms.iter().find(|r| r.guild_id == guild_id && r.owner == user).map(|r| *r.key());
        if let Some(room) = existing {
            guild_id.move_member(ctx, user, room).await?;
            return Ok(());
        }

        let (category, name) = {
            let guild = ctx.cache.guild(guild_id);
            let category = guild.as_ref().and_then(|g| g.channels.get(&hub)).and_then(|c| c.parent_id);
            let display = state
                .member
                .as_ref()
                .map(|m| m.display_name().to_owned())
                .or_else(|| guild.as_ref().and_then(|g| g.members.get(&user)).map(|m| m.display_name().to_owned()))
                .unwrap_or_else(|| "Üye".to_owned());
            (category, format!("🔊 {display} odası").chars().take(100).collect::<String>())
        };

        let manage = Permissions::VIEW_CHANNEL | Permissions::CONNECT | Permissions::SPEAK | Permissions::MANAGE_CHANNELS | Permissions::MOVE_MEMBERS;
        let bot_id = ctx.cache.current_user().id;
        let overwrites = vec![
            PermissionOverwrite { allow: manage, deny: Permissions::empty(), kind: PermissionOverwriteType::Member(user) },
            PermissionOverwrite { allow: manage, deny: Permissions::empty(), kind: PermissionOverwriteType::Member(bot_id) },
        ];
        let mut builder = CreateChannel::new(name)
            .kind(ChannelType::Voice)
            .permissions(overwrites)
            .audit_log_reason("Geçici ses odası");
        if let Some(category) = category {
            builder = builder.category(category);
        }
        let channel = guild_id.create_channel(ctx, builder).await?;
        self.rooms.insert(channel.id, Room { guild_id, owner: user, created: Instant::now() });
        sqlx::query("INSERT INTO temp_voice_channels (channel_id, guild_id, owner_id) VALUES ($1, $2, $3)")
            .bind(snowflake(channel.id.get())?)
            .bind(snowflake(guild_id.get())?)
            .bind(snowflake(user.get())?)
            .execute(db)
            .await?;

        if let Err(e) = guild_id.move_member(ctx, user, channel.id).await {
            // Üye bu arada kanaldan çıktıysa oda boş kalır; sweep siler.
            debug!(error = %e, "Üye geçici odaya taşınamadı");
        }
        info!(guild = %guild_id, channel = %channel.id, owner = %user, "Geçici oda açıldı");
        Ok(())
    }

    /// Boş odaları hemen siler (`/oda temizle`); silinen oda sayısını döner.
    pub async fn clean(&self, ctx: &serenity::Context, db: &PgPool, guild_id: GuildId) -> usize {
        let _guard = self.lock.lock().await;
        self.sweep(ctx, db, guild_id).await
    }

    /// Boşalan odaları siler; silinen oda sayısını döner.
    async fn sweep(&self, ctx: &serenity::Context, db: &PgPool, guild_id: GuildId) -> usize {
        let candidates: Vec<ChannelId> = self
            .rooms
            .iter()
            .filter(|r| r.guild_id == guild_id && r.created.elapsed() >= GRACE)
            .map(|r| *r.key())
            .collect();
        if candidates.is_empty() {
            return 0;
        }
        let empty: Vec<ChannelId> = match ctx.cache.guild(guild_id) {
            Some(guild) => candidates
                .into_iter()
                .filter(|c| !guild.voice_states.values().any(|vs| vs.channel_id == Some(*c)))
                .collect(),
            None => return 0,
        };
        let count = empty.len();
        for channel in empty {
            if let Err(e) = channel.delete(ctx).await {
                debug!(channel = %channel, error = %e, "Geçici oda silinemedi (elle silinmiş olabilir)");
            }
            if let Err(e) = self.forget(db, channel).await {
                warn!(error = ?e, "Geçici oda kaydı silinemedi");
            }
        }
        count
    }

    /// Kanal silindiğinde (bot ya da elle) kaydı kaldırır.
    pub async fn forget(&self, db: &PgPool, channel: ChannelId) -> anyhow::Result<()> {
        if self.rooms.remove(&channel).is_some() {
            sqlx::query("DELETE FROM temp_voice_channels WHERE channel_id = $1")
                .bind(snowflake(channel.get())?)
                .execute(db)
                .await?;
        }
        Ok(())
    }
}
