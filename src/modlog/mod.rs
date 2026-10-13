//! Log kanalına (audit log) embed gönderimi.
//!
//! Log kanalı ayarı her auto-mod silmesinde okunur; bu yüzden bellekte önbelleklenir
//! ve sadece ilk okumada / ayar değişince PostgreSQL'e gidilir.

use std::time::Duration;

use dashmap::DashMap;
use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateAttachment, CreateEmbed, CreateEmbedFooter, CreateMessage,
    GuildId, Http,
    Mentionable, User, UserId,
};
use sqlx::PgPool;
use tracing::warn;

use crate::db::snowflake;
use crate::moderation::{duration, ModAction};

pub struct ModLog {
    channels: DashMap<GuildId, Option<ChannelId>>,
    /// Hangi moderatör olaylarının yazılacağı (denetim kaydı ayarları).
    settings: crate::settings::PluginSettings,
}

/// Log embed'inde gösterilecek vaka bilgisi.
pub struct CaseLog<'a> {
    pub action: ModAction,
    pub case_number: i32,
    pub target: &'a User,
    pub moderator: UserId,
    pub reason: Option<&'a str>,
    pub duration: Option<Duration>,
}

/// Auto-mod'un sildiği bir mesajın log bilgisi.
pub struct AutoModLog<'a> {
    pub channel_id: ChannelId,
    pub author: &'a User,
    pub reason: &'a str,
    pub content: &'a str,
    /// Mesaj silindi mi (yalnızca uyarı verilen kurallarda silinmez).
    pub deleted: bool,
}

impl ModLog {
    pub fn new(settings: crate::settings::PluginSettings) -> Self {
        Self { channels: DashMap::new(), settings }
    }

    pub async fn channel(&self, db: &PgPool, guild_id: GuildId) -> anyhow::Result<Option<ChannelId>> {
        if let Some(entry) = self.channels.get(&guild_id) {
            return Ok(*entry.value());
        }

        let id: Option<i64> =
            sqlx::query_scalar("SELECT log_channel_id FROM guild_settings WHERE guild_id = $1")
                .bind(snowflake(guild_id.get())?)
                .fetch_optional(db)
                .await?
                .flatten();
        let channel = id
            .and_then(|id| u64::try_from(id).ok())
            .filter(|id| *id != 0)
            .map(ChannelId::new);

        self.channels.insert(guild_id, channel);
        Ok(channel)
    }

    pub async fn set_channel(
        &self,
        db: &PgPool,
        guild_id: GuildId,
        channel: Option<ChannelId>,
    ) -> anyhow::Result<()> {
        let channel_id = channel.map(|c| snowflake(c.get())).transpose()?;
        sqlx::query(
            "INSERT INTO guild_settings (guild_id, log_channel_id) VALUES ($1, $2)
             ON CONFLICT (guild_id) DO UPDATE SET log_channel_id = EXCLUDED.log_channel_id,
                                                  updated_at = now()",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(channel_id)
        .execute(db)
        .await?;

        self.channels.insert(guild_id, channel);
        Ok(())
    }

    pub async fn case(&self, http: &Http, db: &PgPool, guild_id: GuildId, log: CaseLog<'_>) {
        let event = match log.action {
            ModAction::Timeout => "mod_mute",
            ModAction::Untimeout => "mod_unmute",
            ModAction::Unban => "mod_unban",
            ModAction::Ban | ModAction::Kick | ModAction::Warn => "mod_ban",
        };
        let enabled = self
            .settings
            .get::<crate::automod::rules::ModSettings>(db, guild_id)
            .await
            .map_or(true, |s| s.log.logs(event));
        if !enabled {
            return;
        }
        let mut embed = CreateEmbed::new()
            .title(format!("{} | Vaka #{}", log.action.title(), log.case_number))
            .colour(log.action.colour())
            .field("Üye", user_line(log.target), true)
            .field("Yetkili", log.moderator.mention().to_string(), true);
        if let Some(d) = log.duration {
            embed = embed.field("Süre", duration::format(d), true);
        }
        embed = embed
            .field("Sebep", log.reason.unwrap_or("Sebep belirtilmedi"), false)
            .footer(CreateEmbedFooter::new(format!("Kullanıcı ID: {}", log.target.id)))
            .timestamp(serenity::Timestamp::now());

        self.send(http, db, guild_id, CreateMessage::new().embed(embed)).await;
    }

    pub async fn automod(&self, http: &Http, db: &PgPool, guild_id: GuildId, log: AutoModLog<'_>) {
        let embed = CreateEmbed::new()
            .title(if log.deleted { "🛡️ Auto-mod | Mesaj silindi" } else { "🛡️ Auto-mod | Kural ihlali" })
            .colour(serenity::Colour::DARK_GREY)
            .field("Üye", user_line(log.author), true)
            .field("Kanal", log.channel_id.mention().to_string(), true)
            .field("Sebep", log.reason, true)
            .field("İçerik", quote_content(log.content), false)
            .footer(CreateEmbedFooter::new(format!("Kullanıcı ID: {}", log.author.id)))
            .timestamp(serenity::Timestamp::now());

        self.send(http, db, guild_id, CreateMessage::new().embed(embed)).await;
    }

    /// Kapanan destek biletinin özetini ve konuşma kaydını (transcript) gönderir.
    pub async fn ticket_closed(
        &self,
        http: &Http,
        db: &PgPool,
        guild_id: GuildId,
        embed: CreateEmbed,
        transcript: CreateAttachment,
    ) {
        let message = CreateMessage::new().embed(embed).add_file(transcript);
        self.send(http, db, guild_id, message).await;
    }

    /// Bilete verilen memnuniyet puanı (bilet kayıtlarının gittiği kanala).
    pub async fn ticket_rated(&self, http: &Http, db: &PgPool, guild_id: GuildId, embed: CreateEmbed) {
        self.send(http, db, guild_id, CreateMessage::new().embed(embed)).await;
    }

    /// Log gönderilemezse moderasyon işlemi başarısız sayılmaz; sadece loglanır.
    async fn send(&self, http: &Http, db: &PgPool, guild_id: GuildId, message: CreateMessage) {
        let channel = match self.channel(db, guild_id).await {
            Ok(Some(channel)) => channel,
            Ok(None) => return,
            Err(e) => {
                warn!(guild = %guild_id, error = ?e, "Log kanalı ayarı okunamadı");
                return;
            }
        };

        if let Err(e) = channel.send_message(http, message).await {
            warn!(
                guild = %guild_id,
                channel = %channel,
                error = %e,
                "Log kanalına yazılamadı (botun o kanalda 'Mesaj Gönder', 'Bağlantı Yerleştir' ve 'Dosya Ekle' izni var mı?)"
            );
        }
    }
}

pub fn user_line(user: &User) -> String {
    format!("{} (`{}`)", user.id.mention(), user.tag())
}

/// Kod bloğu içinde, Discord'un alan sınırına (1024) sığacak şekilde gösterir.
fn quote_content(content: &str) -> String {
    const MAX: usize = 1000;
    let safe = content.replace("```", "ʼʼʼ");
    let mut text: String = safe.chars().take(MAX).collect();
    if safe.chars().count() > MAX {
        text.push('…');
    }
    format!("```\n{text}\n```")
}
