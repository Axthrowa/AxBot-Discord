//! 60 saniyelik periyodik ses takibi ve Ses XP motoru.
//!
//! Her dakika çalışan hafif bir döngüyle Serenity önbelleğindeki aktif ses kanalları
//! taranır; susturulmamış, sağırlaşmamış, AFK kanalında olmayan ve tek başına durmayan
//! uygun üyeler tespit edilerek analitik sayaçlarına ve seviye sistemine yazılır.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use poise::serenity_prelude::{self as serenity, ChannelId, GuildId, Mentionable, RoleId, UserId};
use redis::aio::ConnectionManager;
use sqlx::PgPool;

use crate::leveling::settings::{roles_on_level_up, Announce, LevelSettings};

/// Bir ses kanalındaki üyenin anlık durumu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceMemberState {
    pub is_bot: bool,
    pub is_afk_channel: bool,
    pub is_muted: bool,
    pub is_deafened: bool,
}

/// Üyenin ses XP'si ve ses istatistikleri için uygun olup olmadığını belirler.
pub fn is_voice_eligible(
    state: &VoiceMemberState,
    channel_human_count: usize,
    settings: &LevelSettings,
) -> bool {
    if !settings.voice_xp_enabled || state.is_bot {
        return false;
    }
    if settings.voice_ignore_afk && state.is_afk_channel {
        return false;
    }
    if settings.voice_ignore_muted && state.is_muted {
        return false;
    }
    if settings.voice_ignore_deafened && state.is_deafened {
        return false;
    }
    if channel_human_count < settings.voice_min_members {
        return false;
    }
    true
}

/// 60 saniyelik periyodik ses takibi görevini başlatır.
pub fn spawn_voice_tracker(
    ctx: serenity::Context,
    db: PgPool,
    redis: ConnectionManager,
    settings: crate::settings::PluginSettings,
    leveling: Arc<crate::leveling::Leveling>,
) {
    tokio::spawn(async move {
        // Açılışta diğer servislerin (veritabanı, Discord gateway) oturması için kademeli gecikme
        tokio::time::sleep(Duration::from_secs(35)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            crate::supervise::round("ses takipçisi", Duration::from_secs(120), async {
                tick_voice(&ctx, &db, &redis, &settings, &leveling).await;
            })
            .await;
        }
    });
}

/// Bir dakikalık ses döngüsü: tüm sunuculardaki aktif ses kanallarını tarar ve uygun üyelere kredi verir.
async fn tick_voice(
    ctx: &serenity::Context,
    db: &PgPool,
    redis: &ConnectionManager,
    settings_store: &crate::settings::PluginSettings,
    leveling: &crate::leveling::Leveling,
) {
    let guilds = ctx.cache.guilds();
    for guild_id in guilds {
        let (has_voice_states, channels_map) = {
            let Some(guild) = ctx.cache.guild(guild_id) else { continue };
            if guild.voice_states.is_empty() {
                (false, HashMap::new())
            } else {
                let afk_channel = guild.afk_metadata.as_ref().map(|a| a.afk_channel_id);
                let mut channels_map: HashMap<ChannelId, Vec<(UserId, VoiceMemberState, Vec<RoleId>)>> = HashMap::new();
                for vs in guild.voice_states.values() {
                    let Some(channel_id) = vs.channel_id else { continue };
                    let is_bot = vs.member.as_ref().map_or_else(
                        || guild.members.get(&vs.user_id).is_some_and(|m| m.user.bot),
                        |m| m.user.bot,
                    );
                    let roles = vs.member.as_ref().map_or_else(
                        || guild.members.get(&vs.user_id).map(|m| m.roles.clone()).unwrap_or_default(),
                        |m| m.roles.clone(),
                    );
                    let state = VoiceMemberState {
                        is_bot,
                        is_afk_channel: afk_channel == Some(channel_id),
                        is_muted: vs.mute || vs.self_mute,
                        is_deafened: vs.deaf || vs.self_deaf,
                    };
                    channels_map.entry(channel_id).or_default().push((vs.user_id, state, roles));
                }
                (true, channels_map)
            }
        };

        if !has_voice_states {
            continue;
        }

        let settings = match settings_store.get::<LevelSettings>(db, guild_id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(guild = %guild_id, error = ?e, "Ses takibi: seviye ayarları okunamadı");
                continue;
            }
        };

        if !settings.enabled || !settings.voice_xp_enabled {
            continue;
        }

        for (channel_id, members) in channels_map {
            let human_count = members.iter().filter(|(_, s, _)| !s.is_bot).count();

            for (user_id, state, roles) in members {
                if !is_voice_eligible(&state, human_count, &settings) {
                    continue;
                }

                let channels = [channel_id];
                match leveling.is_excluded(db, guild_id, &channels, &roles, &settings).await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(e) => {
                        tracing::debug!(guild = %guild_id, user = %user_id, error = ?e, "Ses muafiyet kontrolü başarısız");
                        continue;
                    }
                }

                let role_ids: Vec<u64> = roles.iter().map(|r| r.get()).collect();
                let rate = settings.effective_rate(&role_ids);
                let xp = crate::leveling::scaled_xp(u64::from(settings.voice_xp_per_minute), rate);

                // 1. Analitik Redis tamponuna kaydet
                super::record_voice_minute(redis, guild_id, channel_id, user_id, xp).await;

                // 2. XP ekle ve seviye atlama denetle
                if xp > 0 {
                    if let Ok(delta) = i64::try_from(xp) {
                        match leveling.add_xp(db, redis, guild_id, user_id, delta).await {
                            Ok((old_xp, new_xp)) => {
                                let old_lvl = crate::leveling::formula::level(old_xp);
                                let new_lvl = crate::leveling::formula::level(new_xp);
                                if new_lvl > old_lvl {
                                    handle_level_up(
                                        ctx,
                                        db,
                                        leveling,
                                        LevelUpEvent {
                                            guild_id,
                                            user_id,
                                            channel_id,
                                            old_lvl,
                                            new_lvl,
                                        },
                                        &settings,
                                    )
                                    .await;
                                }
                            }
                            Err(e) => {
                                tracing::debug!(guild = %guild_id, user = %user_id, error = ?e, "Ses XP'si eklenemedi");
                            }
                        }
                    }
                }

                // 3. Başarım sayacını güvenle güncelle
                let user_clone = ctx.cache.user(user_id).map(|u| (*u).clone());
                if let Some(user) = user_clone {
                    let actx = crate::achievements::AchievementContext {
                        ctx,
                        db,
                        redis,
                        settings: settings_store,
                        leveling,
                    };
                    crate::achievements::bump_with(actx, guild_id, &user, crate::achievements::Kind::Voice, 1, Some(channel_id)).await;
                }
            }
        }
    }
}

/// Seviye atlayan üye ve kanal bilgileri.
struct LevelUpEvent {
    guild_id: GuildId,
    user_id: UserId,
    channel_id: ChannelId,
    old_lvl: u32,
    new_lvl: u32,
}

/// Ses kanalındayken seviye atlayan üyeye ödül rollerini verir ve isteğe bağlı duyuruyu yapar.
async fn handle_level_up(
    ctx: &serenity::Context,
    db: &PgPool,
    leveling: &crate::leveling::Leveling,
    event: LevelUpEvent,
    settings: &LevelSettings,
) {
    let (earned, outdated) = match leveling.rewards(db, event.guild_id).await {
        Ok(rewards) => roles_on_level_up(&rewards, event.old_lvl, event.new_lvl, settings.stack_rewards),
        Err(e) => {
            tracing::warn!(guild = %event.guild_id, error = ?e, "Seviye ödülleri okunamadı");
            (Vec::new(), Vec::new())
        }
    };

    for role in earned {
        if let Err(e) = ctx.http.add_member_role(event.guild_id, event.user_id, role, Some("Ses seviyesi atlama")).await {
            tracing::debug!(guild = %event.guild_id, role = %role, error = %e, "Ödül rolü verilemedi");
        }
    }
    for role in outdated {
        if let Err(e) = ctx.http.remove_member_role(event.guild_id, event.user_id, role, Some("Eski seviye ödülü")).await {
            tracing::debug!(guild = %event.guild_id, role = %role, error = %e, "Eski ödül rolü geri alınamadı");
        }
    }

    if settings.announce == Announce::Off {
        return;
    }

    let (name, target) = match settings.announce {
        Announce::Dm => {
            let dm = match event.user_id.create_dm_channel(&ctx.http).await {
                Ok(dm) => dm,
                Err(e) => {
                    tracing::debug!(user = %event.user_id, error = %e, "DM açılamadı");
                    return;
                }
            };
            let name = ctx.cache.user(event.user_id).map_or_else(|| "Üye".to_owned(), |u| u.name.clone());
            (name, dm.id)
        }
        Announce::Channel => {
            let Some(target) = settings.announce_channel.filter(|id| *id != 0).map(ChannelId::new) else { return };
            let name = ctx.cache.user(event.user_id).map_or_else(|| "Üye".to_owned(), |u| u.name.clone());
            (name, target)
        }
        Announce::Current => {
            let name = ctx.cache.user(event.user_id).map_or_else(|| "Üye".to_owned(), |u| u.name.clone());
            (name, event.channel_id)
        }
        Announce::Off => return,
    };

    let guild_name = ctx.cache.guild(event.guild_id).map(|g| g.name.clone()).unwrap_or_else(|| "Sunucu".to_owned());
    let text = crate::leveling::settings::render_message(settings.template(), &event.user_id.mention().to_string(), &name, event.new_lvl, &guild_name);
    let message = serenity::CreateMessage::new()
        .content(text)
        .allowed_mentions(serenity::CreateAllowedMentions::new().users(vec![event.user_id]));

    if let Err(e) = target.send_message(&ctx.http, message).await {
        tracing::debug!(channel = %target, error = %e, "Ses seviye duyurusu gönderilemedi");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_eligibility_respects_settings() {
        let settings = LevelSettings::default();

        let normal = VoiceMemberState {
            is_bot: false,
            is_afk_channel: false,
            is_muted: false,
            is_deafened: false,
        };
        // 2 kişi varken uygun
        assert!(is_voice_eligible(&normal, 2, &settings));
        // Tek başınayken (human_count < voice_min_members: 2) uygun değil
        assert!(!is_voice_eligible(&normal, 1, &settings));

        // Bot asla uygun değil
        let bot = VoiceMemberState { is_bot: true, ..normal };
        assert!(!is_voice_eligible(&bot, 5, &settings));

        // AFK kanalındaysa uygun değil
        let afk = VoiceMemberState { is_afk_channel: true, ..normal };
        assert!(!is_voice_eligible(&afk, 3, &settings));

        // Susturulmuşsa uygun değil
        let muted = VoiceMemberState { is_muted: true, ..normal };
        assert!(!is_voice_eligible(&muted, 3, &settings));

        // Sağırlaştırılmışsa uygun değil
        let deaf = VoiceMemberState { is_deafened: true, ..normal };
        assert!(!is_voice_eligible(&deaf, 3, &settings));

        // Ses XP'si genel olarak kapalıysa uygun değil
        let disabled_settings = LevelSettings {
            voice_xp_enabled: false,
            ..LevelSettings::default()
        };
        assert!(!is_voice_eligible(&normal, 3, &disabled_settings));
    }
}
