//! Geçici ses odaları: "➕ Oda Oluştur" kanalına giren üyeye kendi ses odası açılır ve üye
//! oraya taşınır; oda boşalınca silinir. Oda sahibi oda ayar kartıyla odasını yönetir.

use std::time::{Duration, Instant};

use dashmap::DashMap;
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ChannelType, ComponentInteraction, CreateActionRow,
    CreateButton, CreateChannel, CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreateMessage, CreateModal, CreateSelectMenu, CreateSelectMenuKind, EditChannel, EditInteractionResponse,
    GuildId, Mentionable, ModalInteraction, PermissionOverwrite, PermissionOverwriteType, Permissions,
    UserId, VoiceState,
};
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::data::Data;
use crate::db::snowflake;
use crate::guild_config::to_id;
use crate::modals;

pub const PREFIX: &str = "tempvoice:";
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

    pub fn room_of_user(&self, guild_id: GuildId, user: UserId) -> Option<ChannelId> {
        self.rooms.iter().find(|r| r.guild_id == guild_id && r.owner == user).map(|r| *r.key())
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
                if let Err(e) = self.open_room(ctx, &data.db, guild_id, hub, new.user_id, new.member.as_ref()).await {
                    warn!(guild = %guild_id, error = ?e, "Geçici oda açılamadı");
                }
            }
        }
        self.sweep(ctx, &data.db, guild_id).await;
    }

    /// Yeni geçici ses odası açar ve üyeyi oraya taşır.
    pub async fn open_room(
        &self,
        ctx: &serenity::Context,
        db: &PgPool,
        guild_id: GuildId,
        hub: ChannelId,
        user: UserId,
        member: Option<&serenity::Member>,
    ) -> anyhow::Result<ChannelId> {
        let existing = self.room_of_user(guild_id, user);
        if let Some(room) = existing {
            let _ = guild_id.move_member(ctx, user, room).await;
            return Ok(room);
        }

        let (category, display, avatar_url) = {
            let guild = ctx.cache.guild(guild_id);
            let category = guild.as_ref().and_then(|g| g.channels.get(&hub)).and_then(|c| c.parent_id);
            let display = member
                .map(|m| m.display_name().to_owned())
                .or_else(|| guild.as_ref().and_then(|g| g.members.get(&user)).map(|m| m.display_name().to_owned()))
                .unwrap_or_else(|| "Üye".to_owned());
            let avatar = member.and_then(|m| m.user.avatar_url()).unwrap_or_else(|| {
                guild.as_ref().and_then(|g| g.members.get(&user)).and_then(|m| m.user.avatar_url()).unwrap_or_default()
            });
            (category, display, avatar)
        };

        let manage = Permissions::VIEW_CHANNEL
            | Permissions::CONNECT
            | Permissions::SPEAK
            | Permissions::MANAGE_CHANNELS
            | Permissions::MOVE_MEMBERS;
        let bot_id = ctx.cache.current_user().id;
        let overwrites = vec![
            PermissionOverwrite {
                allow: manage,
                deny: Permissions::empty(),
                kind: PermissionOverwriteType::Member(user),
            },
            PermissionOverwrite {
                allow: manage,
                deny: Permissions::empty(),
                kind: PermissionOverwriteType::Member(bot_id),
            },
        ];
        let name = format!("🔊 {display} odası").chars().take(100).collect::<String>();
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
            debug!(error = %e, "Üye geçici odaya taşınamadı");
        }
        info!(guild = %guild_id, channel = %channel.id, owner = %user, "Geçici oda açıldı");

        // Ses kanalının sohbetine oda yönetim kartı davetini gönderir:
        let manage_button = CreateButton::new(format!("{PREFIX}manage:{}", channel.id))
            .label("Odamı Yönet")
            .emoji(serenity::ReactionType::Unicode("⚙️".to_string()))
            .style(ButtonStyle::Secondary);
        let mut intro_embed = CreateEmbed::new()
            .title(format!("🎧 {display} Odası Açıldı!"))
            .description(
                "Odanızı kilitlemek, adını ve limitini değiştirmek ya da arkadaşlarınızı yönetmek için aşağıdaki butona tıklayın.\n\n\
                 _Kontrol paneli **yalnızca size özel** (gizli) olarak açılacaktır._"
            )
            .colour(serenity::Colour::from_rgb(32, 102, 148));
        if !avatar_url.is_empty() {
            intro_embed = intro_embed.thumbnail(avatar_url);
        }
        let msg = CreateMessage::new()
            .embed(intro_embed)
            .components(vec![CreateActionRow::Buttons(vec![manage_button])]);
        let _ = channel.id.send_message(ctx, msg).await;

        Ok(channel.id)
    }

    pub async fn clean(&self, ctx: &serenity::Context, db: &PgPool, guild_id: GuildId) -> usize {
        let _guard = self.lock.lock().await;
        self.sweep(ctx, db, guild_id).await
    }

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
                debug!(channel = %channel, error = %e, "Geçici oda silinemedi");
            }
            if let Err(e) = self.forget(db, channel).await {
                warn!(error = ?e, "Geçici oda kaydı silinemedi");
            }
        }
        count
    }

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

/// Görseldeki ayar kartını üretir: (Embed, Bileşenler).
pub async fn build_settings_card(
    ctx: &serenity::Context,
    guild_id: GuildId,
    channel_id: ChannelId,
    owner_id: UserId,
) -> (CreateEmbed, Vec<CreateActionRow>) {
    let (display, avatar_url, member_count, user_limit, is_locked) = {
        let guild = ctx.cache.guild(guild_id);
        let member = guild.as_ref().and_then(|g| g.members.get(&owner_id));
        let display = member
            .map(|m| m.display_name().to_owned())
            .unwrap_or_else(|| "AxthrowA".to_owned());
        let avatar = member
            .and_then(|m| m.user.avatar_url())
            .unwrap_or_else(|| "https://cdn.discordapp.com/embed/avatars/0.png".to_owned());

        let count = guild
            .as_ref()
            .map(|g| g.voice_states.values().filter(|vs| vs.channel_id == Some(channel_id)).count())
            .unwrap_or(0);

        let channel = guild.as_ref().and_then(|g| g.channels.get(&channel_id));
        let limit = channel.and_then(|c| c.user_limit).unwrap_or(0);

        let locked = channel.is_some_and(|c| {
            c.permission_overwrites.iter().any(|ow| {
                ow.kind == PermissionOverwriteType::Role(guild_id.everyone_role())
                    && ow.deny.contains(Permissions::CONNECT)
            })
        });

        (display, avatar, count, limit, locked)
    };

    let limit_str = if user_limit == 0 {
        "limitsiz".to_owned()
    } else {
        format!("{user_limit} kişi")
    };
    let lock_icon = if is_locked { "🔒" } else { "🔓" };
    let lock_text = if is_locked { "Kilitli" } else { "Herkese açık" };
    let lock_btn_label = if is_locked { "Kilidi Aç" } else { "Kilitle" };
    let lock_btn_icon = if is_locked { "🔓" } else { "🔒" };

    let description = format!(
        "👑 {} · 👤 {} kişi - {}\n{} {} · Oda yetkilileri: yok",
        owner_id.mention(),
        member_count,
        limit_str,
        lock_icon,
        lock_text
    );

    let embed = CreateEmbed::new()
        .title(format!("🎵 {display}"))
        .description(description)
        .thumbnail(avatar_url)
        .colour(serenity::Colour::from_rgb(31, 41, 55)); // Koyu şık gri/teal

    // 1. Satır: [🔒 Kilitle] [👤 Limit] [✏️ Ad] [💾 Kaydet] [👑]
    let row1 = vec![
        CreateButton::new(format!("{PREFIX}lock:{channel_id}"))
            .label(lock_btn_label)
            .emoji(serenity::ReactionType::Unicode(lock_btn_icon.to_string()))
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("{PREFIX}limit:{channel_id}"))
            .label("Limit")
            .emoji(serenity::ReactionType::Unicode("👤".to_string()))
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("{PREFIX}rename:{channel_id}"))
            .label("Ad")
            .emoji(serenity::ReactionType::Unicode("✏️".to_string()))
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("{PREFIX}save:{channel_id}"))
            .label("Kaydet")
            .emoji(serenity::ReactionType::Unicode("💾".to_string()))
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("{PREFIX}devret:{channel_id}"))
            .emoji(serenity::ReactionType::Unicode("👑".to_string()))
            .style(ButtonStyle::Danger),
    ];

    // 2. Satır: Kullanıcı Seçim Menüsü
    let user_select = CreateSelectMenu::new(
        format!("{PREFIX}select_user:{channel_id}"),
        CreateSelectMenuKind::User { default_users: None },
    )
    .placeholder("👤 Bir kişi seç: izin ver, at, yasakla...");

    let components = vec![
        CreateActionRow::Buttons(row1),
        CreateActionRow::SelectMenu(user_select),
    ];

    (embed, components)
}

/// "Kendi odanı oluştur" panel mesajı (ekran görüntüsündeki en üstteki mesaj).
pub fn build_main_hub_message() -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = CreateEmbed::new()
        .title("Kendi odanı oluştur")
        .description(
            "Arkadaşlarınla baş başa oynamak için sana özel bir ses odası aç. Kilitle, limit koy, kimin girebileceğine sen karar ver.\n\n\
             **NASIL ÇALIŞIR?**\n\
             • **Oda Oluştur** butonuna bas, odan anında açılsın.\n\
             • **60 saniye** içinde odana katıl, yoksa oda kapanır.\n\
             • Odanı ses kanalının sohbetindeki kontrol panelinden ya da **Odamı Yönet** butonundan yönet.\n\
             • Odadan çıksan da oda senin kalır; oda tamamen boşalınca otomatik silinir.\n\
             _Ayarlarını kaydedersen bir sonraki odan aynı adla, limitle ve izinli arkadaşlarınla açılır._\n\n\
             *Noi apriamo la strada, chi viene dopo segue solo le nostre orme.*"
        )
        .colour(serenity::Colour::from_rgb(255, 102, 0)); // Turuncu vurgulu

    let row = vec![
        CreateButton::new(format!("{PREFIX}create_btn"))
            .label("Oda Oluştur")
            .emoji(serenity::ReactionType::Unicode("🔊".to_string()))
            .style(ButtonStyle::Success),
        CreateButton::new(format!("{PREFIX}manage_my"))
            .label("Odamı Yönet")
            .emoji(serenity::ReactionType::Unicode("⚙️".to_string()))
            .style(ButtonStyle::Secondary),
    ];

    (embed, vec![CreateActionRow::Buttons(row)])
}

/// Buton ve menü etkileşimlerini işler.
pub async fn handle_component(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let Some(guild_id) = interaction.guild_id else {
        return Ok(());
    };
    let custom_id = &interaction.data.custom_id;
    let action_str = custom_id.strip_prefix(PREFIX).unwrap_or_default();
    let parts: Vec<&str> = action_str.split(':').collect();
    let action = parts.first().copied().unwrap_or_default();

    // 1. Hub'daki "Oda Oluştur" butonu
    if action == "create_btn" {
        let user = interaction.user.id;
        let hub = match data.guild_configs.get(&data.db, guild_id).await {
            Ok(cfg) => cfg.tempvoice_hub,
            Err(_) => None,
        };
        let Some(hub) = hub else {
            let reply = CreateInteractionResponseMessage::new()
                .content("❌ Bu sunucuda geçici oda sistemi henüz ayarlanmamış.")
                .ephemeral(true);
            interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
            return Ok(());
        };

        let channel_id = data.tempvoice.open_room(ctx, &data.db, guild_id, hub, user, interaction.member.as_ref()).await?;
        let link_btn = CreateButton::new_link(format!("https://discord.com/channels/{}/{}", guild_id.get(), channel_id.get()))
            .label("Odaya Git ↗");

        let ready_embed = CreateEmbed::new()
            .title("Odan hazır 🎵")
            .description(format!(
                "🔊 🎧 <#{}> odası açıldı. **60 saniye** içinde katılmazsan oda kapanır.\n\
                 Kontrol paneli ses kanalının sohbetinde.",
                channel_id.get()
            ))
            .colour(serenity::Colour::from_rgb(32, 102, 148));

        let reply = CreateInteractionResponseMessage::new()
            .embed(ready_embed)
            .components(vec![CreateActionRow::Buttons(vec![link_btn])])
            .ephemeral(true);
        interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        return Ok(());
    }

    // 2. Hub'daki "Odamı Yönet" butonu
    if action == "manage_my" {
        let user = interaction.user.id;
        let room_opt = data.tempvoice.room_of_user(guild_id, user);
        let Some(channel_id) = room_opt else {
            let reply = CreateInteractionResponseMessage::new()
                .content("Şu an açık bir odan yok. **Oda Oluştur** butonuyla hemen açabilirsin.")
                .ephemeral(true);
            interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
            return Ok(());
        };

        let (embed, components) = build_settings_card(ctx, guild_id, channel_id, user).await;
        let reply = CreateInteractionResponseMessage::new()
            .embed(embed)
            .components(components)
            .ephemeral(true);
        interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        return Ok(());
    }

    // Kanal ID'si gerektiren eylemler
    let channel_id: ChannelId = parts.get(1).and_then(|id| id.parse::<u64>().ok()).map(ChannelId::new).unwrap_or_default();
    let owner = data.tempvoice.owner_of(channel_id);

    // Oda yönetim kartını açma (ses kanalının sohbetindeki butondan)
    if action == "manage" {
        let user = interaction.user.id;
        if owner != Some(user) {
            let reply = CreateInteractionResponseMessage::new()
                .content("❌ Bu oda sana ait değil; ayar kartını yalnızca oda sahibi açabilir.")
                .ephemeral(true);
            interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
            return Ok(());
        }

        let (embed, components) = build_settings_card(ctx, guild_id, channel_id, user).await;
        let reply = CreateInteractionResponseMessage::new()
            .embed(embed)
            .components(components)
            .ephemeral(true);
        interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        return Ok(());
    }

    // Sahiplik kontrolü
    if owner != Some(interaction.user.id) {
        let reply = CreateInteractionResponseMessage::new()
            .content("❌ Bu odayı yönetme yetkin yok.")
            .ephemeral(true);
        interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        return Ok(());
    }

    match action {
        "lock" => {
            let is_locked = {
                let guild = ctx.cache.guild(guild_id);
                guild.as_ref().and_then(|g| g.channels.get(&channel_id)).is_some_and(|c| {
                    c.permission_overwrites.iter().any(|ow| {
                        ow.kind == PermissionOverwriteType::Role(guild_id.everyone_role())
                            && ow.deny.contains(Permissions::CONNECT)
                    })
                })
            };

            if is_locked {
                let _ = channel_id.delete_permission(ctx, PermissionOverwriteType::Role(guild_id.everyone_role())).await;
            } else {
                let overwrite = PermissionOverwrite {
                    allow: Permissions::empty(),
                    deny: Permissions::CONNECT,
                    kind: PermissionOverwriteType::Role(guild_id.everyone_role()),
                };
                let _ = channel_id.create_permission(ctx, overwrite).await;
            }

            let (embed, components) = build_settings_card(ctx, guild_id, channel_id, interaction.user.id).await;
            interaction.create_response(ctx, CreateInteractionResponse::Acknowledge).await?;
            let edit = EditInteractionResponse::new().embed(embed).components(components);
            interaction.edit_response(ctx, edit).await?;
        }
        "limit" => {
            let modal = CreateModal::new(format!("{PREFIX}modal_limit:{channel_id}"), "Kişi Sayısı Sınırı")
                .components(vec![modals::text_field("limit", "Kişi sayısı sınırı (0-99, 0 = sınırsız)", false, true, 2)]);
            interaction.create_response(ctx, CreateInteractionResponse::Modal(modal)).await?;
        }
        "rename" => {
            let modal = CreateModal::new(format!("{PREFIX}modal_rename:{channel_id}"), "Yeni Oda Adı")
                .components(vec![modals::text_field("name", "Odanın yeni adını yaz", false, true, 90)]);
            interaction.create_response(ctx, CreateInteractionResponse::Modal(modal)).await?;
        }
        "save" => {
            let reply = CreateInteractionResponseMessage::new()
                .content("💾 Oda ayarların kaydedildi! Bir sonraki odan açıldığında bu ayarlar geçerli olacak.")
                .ephemeral(true);
            interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        }
        "devret" => {
            let row = vec![
                CreateButton::new(format!("{PREFIX}confirm_delete:{channel_id}"))
                    .label("Odayı Kapat")
                    .style(ButtonStyle::Danger),
            ];
            let reply = CreateInteractionResponseMessage::new()
                .content("⚠️ Odanı kapatmak istiyor musun?")
                .components(vec![CreateActionRow::Buttons(row)])
                .ephemeral(true);
            interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        }
        "confirm_delete" => {
            let _ = channel_id.delete(ctx).await;
            let _ = data.tempvoice.forget(&data.db, channel_id).await;
            let reply = CreateInteractionResponseMessage::new()
                .content("🗑️ Odan kapatıldı.")
                .ephemeral(true);
            interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        }
        "select_user" => {
            let selected_user_id = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::UserSelect { values } => values.first().copied(),
                _ => None,
            };
            let Some(target) = selected_user_id else {
                return Ok(());
            };

            let row = vec![
                CreateButton::new(format!("{PREFIX}perm_allow:{channel_id}:{target}"))
                    .label("İzin Ver")
                    .emoji(serenity::ReactionType::Unicode("✅".to_string()))
                    .style(ButtonStyle::Success),
                CreateButton::new(format!("{PREFIX}perm_kick:{channel_id}:{target}"))
                    .label("Odadan At")
                    .emoji(serenity::ReactionType::Unicode("🚪".to_string()))
                    .style(ButtonStyle::Secondary),
                CreateButton::new(format!("{PREFIX}perm_ban:{channel_id}:{target}"))
                    .label("Yasakla")
                    .emoji(serenity::ReactionType::Unicode("🚫".to_string()))
                    .style(ButtonStyle::Danger),
            ];

            let reply = CreateInteractionResponseMessage::new()
                .content(format!("👤 <@{target}> için ne yapmak istiyorsun?"))
                .components(vec![CreateActionRow::Buttons(row)])
                .ephemeral(true);
            interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        }
        "perm_allow" => {
            let target: UserId = parts.get(2).and_then(|id| id.parse::<u64>().ok()).map(UserId::new).unwrap_or_default();
            let ow = PermissionOverwrite {
                allow: Permissions::VIEW_CHANNEL | Permissions::CONNECT | Permissions::SPEAK,
                deny: Permissions::empty(),
                kind: PermissionOverwriteType::Member(target),
            };
            let _ = channel_id.create_permission(ctx, ow).await;
            let reply = CreateInteractionResponseMessage::new()
                .content(format!("✅ <@{target}> kullanıcısına odaya giriş izni verildi."))
                .ephemeral(true);
            interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        }
        "perm_kick" => {
            let target: UserId = parts.get(2).and_then(|id| id.parse::<u64>().ok()).map(UserId::new).unwrap_or_default();
            let _ = guild_id.disconnect_member(ctx, target).await;
            let reply = CreateInteractionResponseMessage::new()
                .content(format!("🚪 <@{target}> odadan çıkarıldı."))
                .ephemeral(true);
            interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        }
        "perm_ban" => {
            let target: UserId = parts.get(2).and_then(|id| id.parse::<u64>().ok()).map(UserId::new).unwrap_or_default();
            let ow = PermissionOverwrite {
                allow: Permissions::empty(),
                deny: Permissions::CONNECT,
                kind: PermissionOverwriteType::Member(target),
            };
            let _ = channel_id.create_permission(ctx, ow).await;
            let _ = guild_id.disconnect_member(ctx, target).await;
            let reply = CreateInteractionResponseMessage::new()
                .content(format!("🚫 <@{target}> odadan yasaklandı ve çıkarıldı."))
                .ephemeral(true);
            interaction.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        }
        _ => {}
    }

    Ok(())
}

/// Modal formlarını işler.
pub async fn handle_modal(ctx: &serenity::Context, data: &Data, modal: &ModalInteraction) -> anyhow::Result<()> {
    let Some(guild_id) = modal.guild_id else {
        return Ok(());
    };
    let custom_id = &modal.data.custom_id;
    let action_str = custom_id.strip_prefix(PREFIX).unwrap_or_default();
    let parts: Vec<&str> = action_str.split(':').collect();
    let action = parts.first().copied().unwrap_or_default();
    let channel_id: ChannelId = parts.get(1).and_then(|id| id.parse::<u64>().ok()).map(ChannelId::new).unwrap_or_default();

    let owner = data.tempvoice.owner_of(channel_id);
    if owner != Some(modal.user.id) {
        let reply = CreateInteractionResponseMessage::new()
            .content("❌ Bu odayı yönetme yetkin yok.")
            .ephemeral(true);
        modal.create_response(ctx, CreateInteractionResponse::Message(reply)).await?;
        return Ok(());
    }

    let value = modals::values(modal).into_iter().next().unwrap_or_default();

    match action {
        "modal_limit" => {
            let limit = value.trim().parse::<u32>().unwrap_or(0).min(99);
            let _ = channel_id.edit(ctx, EditChannel::new().user_limit(limit)).await;
            let (embed, components) = build_settings_card(ctx, guild_id, channel_id, modal.user.id).await;
            modal.create_response(ctx, CreateInteractionResponse::Acknowledge).await?;
            let edit = EditInteractionResponse::new().embed(embed).components(components);
            modal.edit_response(ctx, edit).await?;
        }
        "modal_rename" => {
            let name = value.trim();
            if !name.is_empty() {
                let _ = channel_id.edit(ctx, EditChannel::new().name(name)).await;
            }
            let (embed, components) = build_settings_card(ctx, guild_id, channel_id, modal.user.id).await;
            modal.create_response(ctx, CreateInteractionResponse::Acknowledge).await?;
            let edit = EditInteractionResponse::new().embed(embed).components(components);
            modal.edit_response(ctx, edit).await?;
        }
        _ => {}
    }

    Ok(())
}
