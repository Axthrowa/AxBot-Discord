//! Bir sunucudaki kurulum durumu: botun eksik izinleri, rol sırası ve özelliklerin ayarları.
//! Hem `/kurulum` komutu hem web paneli kullanır.

use std::sync::Arc;

use poise::serenity_prelude::{Cache, ChannelId, GuildId, Http, Permissions, RoleId};
use sqlx::PgPool;

use crate::guild_config::GuildConfigs;

pub enum Target {
    Channel(ChannelId),
    Role(RoleId),
    Text(&'static str),
}

pub enum State {
    On(Target),
    Off,
    /// Ayarlı ama kanal/rol silinmiş.
    Broken,
}

pub struct Feature {
    pub name: &'static str,
    pub state: State,
    /// Ayarı açan / düzelten komut.
    pub command: &'static str,
}

pub struct Report {
    pub guild_name: String,
    pub icon_url: Option<String>,
    pub member_count: u64,
    /// Botun bu sunucuda eksik olan izinleri (Yönetici varsa boş).
    pub missing: Permissions,
    /// Botun rolü (bot rolleri hariç) en üstte mi?
    pub role_on_top: bool,
    pub features: Vec<Feature>,
}

impl Report {
    pub fn healthy(&self) -> bool {
        self.missing.is_empty() && self.role_on_top
    }
}

pub async fn build(http: &Http, cache: &Arc<Cache>, db: &PgPool, configs: &GuildConfigs, guild_id: GuildId) -> anyhow::Result<Report> {
    let bot_id = cache.current_user().id;
    // Botun üyeliği önbellekte yoksa Discord'dan alınır (izin hesabı için gerekli).
    let me = match cache.guild(guild_id).and_then(|g| g.members.get(&bot_id).cloned()) {
        Some(m) => m,
        None => guild_id.member(http, bot_id).await?,
    };
    let cfg = configs.get(db, guild_id).await?;
    let has_tickets: Option<(i64,)> = sqlx::query_as("SELECT guild_id FROM ticket_settings WHERE guild_id = $1")
        .bind(i64::try_from(guild_id.get())?)
        .fetch_optional(db)
        .await?;

    let guild = cache.guild(guild_id).ok_or_else(|| anyhow::anyhow!("sunucu önbellekte yok"))?;
    let perms = guild.member_permissions(&me);
    let missing = if perms.contains(Permissions::ADMINISTRATOR) {
        Permissions::empty()
    } else {
        crate::commands::BOT_PERMISSIONS - perms
    };
    let bot_top = me.roles.iter().filter_map(|r| guild.roles.get(r)).map(|r| r.position).max().unwrap_or(0);
    let highest = guild.roles.values().filter(|r| !r.managed).map(|r| r.position).max().unwrap_or(0);

    let channel = |name, c: Option<ChannelId>, command| Feature {
        name,
        state: match c {
            Some(c) if guild.channels.contains_key(&c) => State::On(Target::Channel(c)),
            Some(_) => State::Broken,
            None => State::Off,
        },
        command,
    };
    let role = |name, r: Option<RoleId>, command| Feature {
        name,
        state: match r {
            Some(r) if guild.roles.contains_key(&r) => State::On(Target::Role(r)),
            Some(_) => State::Broken,
            None => State::Off,
        },
        command,
    };
    let features = vec![
        channel("📜 Olay logu", cfg.event_log_channel, "/mod log-kanalı tür:Sunucu olayları"),
        channel("👋 Karşılama", cfg.welcome_channel, "/karşılama hoşgeldin"),
        role("🎭 Otomatik rol", cfg.autorole, "/rol otomatik"),
        role("✅ Doğrulama", cfg.verify_role, "/doğrulama kur"),
        Feature {
            name: "🎫 Bilet",
            state: if has_tickets.is_some() { State::On(Target::Text("kurulu")) } else { State::Off },
            command: "/bilet kur",
        },
        channel("⭐ Starboard", cfg.starboard_channel, "/starboard"),
        channel("💡 Öneriler", cfg.suggestion_channel, "/öneri kanal"),
        channel("🎂 Doğum günü", cfg.birthday_channel, "/doğum-günü kanal"),
        channel("🔊 Geçici oda", cfg.tempvoice_hub, "/geçici-oda"),
        channel("⚖️ Ban itirazı", cfg.appeal_channel, "/başvuru itiraz-kanalı"),
        channel("🦝 Hayvan yakalama", cfg.animal_channel, "/hayvan kanal"),
    ];

    Ok(Report {
        guild_name: guild.name.clone(),
        icon_url: guild.icon_url(),
        member_count: guild.member_count,
        missing,
        role_on_top: bot_top >= highest,
        features,
    })
}
