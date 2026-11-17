//! Merkezi gateway event yönlendiricisi. Her event ilgili modüle dağıtılır.

use poise::serenity_prelude::{self as serenity, ChannelId, GuildId, Mentionable, Message, RoleId};
use tracing::{debug, info, warn};

use crate::ai;
use crate::automations::{self, Facts, Trigger};
use crate::automod::{self, Candidate};
use crate::data::Data;
use crate::error::Error;
use crate::leveling::settings::{render_message, roles_on_level_up, Announce, LevelSettings};
use crate::leveling::LevelUp;
use crate::{appeals, applications, eventlog, giveaway, members, rolepanel, starboard, suggestions, tickets, verification};

pub async fn handle(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Ready { data_about_bot } => {
            info!(
                user = %data_about_bot.user.tag(),
                guilds = data_about_bot.guilds.len(),
                shard = ?data_about_bot.shard,
                "Discord'a bağlanıldı"
            );
        }
        serenity::FullEvent::Resume { .. } => {
            debug!("Gateway oturumu sürdürüldü");
        }
        // Davet takibi için başlangıç kullanım sayıları.
        serenity::FullEvent::GuildCreate { guild, is_new } => {
            // Bot yeni bir sunucuya eklendi: komutlar orada hemen görünsün.
            if *is_new == Some(true) && data.dev_guild_id.is_none() {
                info!(guild = %guild.id, name = %guild.name, "Bot yeni bir sunucuya eklendi");
                crate::commands::register_guild(ctx, &framework.options.commands, guild.id).await;
            }
            data.invites.refresh(&ctx.http, guild.id).await;
            crate::privacy::on_present(&data.redis, guild.id).await;
        }
        // `unavailable` = Discord kesintisi; değilse bot sunucudan çıkarıldı.
        serenity::FullEvent::GuildDelete { incomplete, .. } if !incomplete.unavailable => {
            crate::privacy::on_removed(&data.redis, incomplete.id).await;
        }
        serenity::FullEvent::InviteCreate { data: invite } => {
            if let Some(guild_id) = invite.guild_id {
                data.invites.on_create(guild_id, &invite.code, invite.inviter.as_ref().map(|u| u.id), invite.uses);
                eventlog::invite_changed(ctx, data, guild_id, &invite.code, invite.channel_id, invite.inviter.as_ref(), true).await;
            }
        }
        serenity::FullEvent::InviteDelete { data: invite } => {
            if let Some(guild_id) = invite.guild_id {
                data.invites.on_delete(guild_id, &invite.code);
                eventlog::invite_changed(ctx, data, guild_id, &invite.code, invite.channel_id, None, false).await;
            }
        }
        serenity::FullEvent::Message { new_message: msg } => {
            let Some(guild_id) = msg.guild_id else {
                // Bota özelden yazılan mesajlar ModMail'e gider.
                if !msg.author.bot {
                    crate::modmail::on_dm(ctx, data, msg).await;
                }
                return Ok(());
            };
            // Klip kanalı: AxT Capture klipleri webhook ile gelir, bu yüzden aşağıdaki süzgeçten önce sayılır.
            if !msg.attachments.is_empty() && (msg.webhook_id.is_some() || !msg.author.bot) {
                crate::community::on_clip(ctx, data, guild_id, msg).await;
            }
            if msg.author.bot || msg.webhook_id.is_some() {
                return Ok(());
            }
            let roles = msg.member.as_deref().map_or(&[][..], |m| m.roles.as_slice());

            let deleted = automod::inspect(
                ctx,
                data,
                Candidate {
                    guild_id,
                    channel_id: msg.channel_id,
                    message_id: msg.id,
                    author: &msg.author,
                    roles,
                    content: &msg.content,
                    is_edit: false,
                    media: media_of(&msg.attachments),
                },
            )
            .await?;

            // Auto-mod'un sildiği mesajlar XP kazandırmaz ve yapay zekâya gitmez.
            if !deleted {
                // Kelime listesinin kaçırdığı gizlenmiş küfürler (ayar açıksa, yalnızca şüpheli mesajlarda).
                if crate::ai::assist::moderate(ctx, data, guild_id, msg, roles).await {
                    return Ok(());
                }
                if data.automod.has_invite(&msg.content) {
                    eventlog::invite_posted(ctx, data, guild_id, msg).await;
                }
                // Hayvan yakalama kanalı (ayar bellekte önbellekli; veritabanına gidilmez).
                if let Ok(cfg) = data.guild_configs.get(&data.db, guild_id).await {
                    if cfg.animal_channel == Some(msg.channel_id) {
                        data.animals.on_message(ctx, msg.channel_id).await;
                    }
                }
                // XP işlemi cevabı geciktirmesin: ikisi aynı anda yürür.
                let respond = async {
                    if crate::customcmd::handle(ctx, data, guild_id, msg, roles).await
                        || send_tag(ctx, data, guild_id, msg).await
                        || send_autoresponse(ctx, data, guild_id, msg).await
                    {
                        return Ok(());
                    }
                    ai::handle_message(ctx, data, guild_id, msg).await
                };
                let extras = async {
                    crate::community::on_message(ctx, data, guild_id, msg).await;
                    eventlog::remember(data, guild_id, msg).await;
                    crate::highlights::on_message(ctx, &data.db, guild_id, msg).await;
                    crate::modmail::on_staff_message(ctx, data, guild_id, msg).await;
                    crate::ai::assist::on_ticket_message(ctx, data, guild_id, msg).await;
                    tickets::on_message(ctx, &data.db, msg).await;
                    crate::achievements::bump(ctx, data, guild_id, &msg.author, crate::achievements::Kind::Messages, 1, Some(msg.channel_id)).await;
                    let facts = Facts { trigger: Trigger::Message, channel: Some(msg.channel_id.get()), content: &msg.content, member_roles: roles, ..Facts::default() };
                    automations::run(ctx, data, automations::Event { guild_id, user: &msg.author, facts, message: Some(msg) }).await;
                };
                let ((), responded, ()) = tokio::join!(award_xp(ctx, data, guild_id, msg, roles), respond, extras);
                responded?;
            }
        }
        // Mesajı gönderdikten sonra düzenleyerek filtreyi atlatmayı engeller.
        serenity::FullEvent::MessageUpdate {
            old_if_available,
            event: update,
            ..
        } => {
            if let Some(guild_id) = update.guild_id {
                eventlog::message_edited(ctx, data, guild_id, old_if_available.as_ref(), update).await;
            }
            let (Some(guild_id), Some(author), Some(content)) =
                (update.guild_id, &update.author, &update.content)
            else {
                return Ok(());
            };
            let roles = update
                .member
                .as_ref()
                .and_then(|m| m.as_deref())
                .map_or(&[][..], |m| m.roles.as_slice());

            automod::inspect(
                ctx,
                data,
                Candidate {
                    guild_id,
                    channel_id: update.channel_id,
                    message_id: update.id,
                    author,
                    roles,
                    content,
                    is_edit: true,
                    media: media_of(update.attachments.as_deref().unwrap_or_default()),
                },
            )
            .await?;
        }
        serenity::FullEvent::MessageDelete {
            channel_id,
            deleted_message_id,
            guild_id: Some(guild_id),
        } => {
            eventlog::message_deleted(ctx, data, *guild_id, *channel_id, *deleted_message_id).await;
        }
        serenity::FullEvent::MessageDeleteBulk {
            channel_id,
            multiple_deleted_messages_ids,
            guild_id: Some(guild_id),
        } => {
            eventlog::messages_bulk_deleted(ctx, data, *guild_id, *channel_id, multiple_deleted_messages_ids.len()).await;
        }
        // Butonlar: custom_id önekine göre ilgili modüle.
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Component(component),
        } => {
            let id = component.data.custom_id.as_str();
            if id.starts_with("ticket:") {
                tickets::handle_component(ctx, data, component).await?;
            } else if id.starts_with(crate::newsprefs::PREFIX) {
                crate::newsprefs::handle_component(ctx, data, component).await?;
            } else if id.starts_with(crate::reactionroles::PREFIX) {
                crate::reactionroles::handle_component(ctx, data, component).await?;
            } else if id.starts_with(rolepanel::PREFIX) {
                rolepanel::handle_button(ctx, component).await?;
            } else if id.starts_with(giveaway::BUTTON_PREFIX) {
                giveaway::handle_button(ctx, data, component).await?;
            } else if id.starts_with(suggestions::BUTTON_PREFIX) {
                suggestions::handle_button(ctx, &data.db, component).await?;
            } else if id.starts_with(verification::PREFIX) {
                verification::handle_button(ctx, data, component).await?;
            } else if id.starts_with(appeals::PREFIX) {
                appeals::handle_button(ctx, data, component).await?;
            } else if id.starts_with(applications::PREFIX) {
                applications::handle_button(ctx, data, component).await?;
            } else if id.starts_with(crate::games::animals::PREFIX) {
                data.animals.handle_button(ctx, data, component).await?;
            } else if id.starts_with(crate::music::card::PREFIX) {
                crate::music::card::handle_button(ctx, &data.music, component).await?;
            } else if id.starts_with(crate::rsvp::PREFIX) {
                crate::rsvp::handle_component(ctx, data, component).await?;
            } else if id.starts_with(crate::commands::RECOMMEND_PREFIX) {
                crate::commands::handle_recommend(ctx, data, component).await?;
            } else if id.starts_with(crate::commands::TOD_PREFIX) {
                crate::commands::handle_tod(ctx, component).await?;
            }
        }
        // Form pencereleri (doğrulama sorusu, başvuru, itiraz).
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Modal(modal),
        } => {
            let id = modal.data.custom_id.as_str();
            if id.starts_with(crate::music::card::PREFIX) {
                crate::music::card::handle_modal(ctx, &data.music, modal).await?;
            } else if id.starts_with(verification::PREFIX) {
                verification::handle_modal(ctx, data, modal).await?;
            } else if id.starts_with(appeals::PREFIX) {
                appeals::handle_modal(ctx, data, modal).await?;
            } else if id.starts_with(applications::PREFIX) {
                applications::handle_modal(ctx, data, modal).await?;
            }
        }
        // Starboard: sadece ⭐ tepkileri işlenir.
        serenity::FullEvent::ReactionAdd { add_reaction: reaction } => {
            if let (Some(guild_id), true) = (reaction.guild_id, starboard::is_star(&reaction.emoji)) {
                starboard::on_star(ctx, data, guild_id, reaction.channel_id, reaction.message_id).await;
            }
            if let Some(guild_id) = reaction.guild_id {
                crate::starboards::on_reaction(ctx, data, guild_id, reaction.channel_id, reaction.message_id, &reaction.emoji).await;
            }
            crate::reactionroles::on_reaction(ctx, data, reaction, true).await;
            if let (Some(guild_id), Some(member)) = (reaction.guild_id, reaction.member.as_ref()) {
                crate::achievements::bump(ctx, data, guild_id, &member.user, crate::achievements::Kind::Reactions, 1, Some(reaction.channel_id)).await;
                let emoji = match &reaction.emoji {
                    serenity::ReactionType::Unicode(text) => crate::reactionroles::emoji_key(text),
                    serenity::ReactionType::Custom { id, .. } => Some(id.to_string()),
                    _ => None,
                };
                let facts = Facts {
                    trigger: Trigger::Reaction,
                    channel: Some(reaction.channel_id.get()),
                    emoji: emoji.as_deref(),
                    member_roles: &member.roles,
                    ..Facts::default()
                };
                automations::run(ctx, data, automations::Event { guild_id, user: &member.user, facts, message: None }).await;
            }
        }
        serenity::FullEvent::ReactionRemove { removed_reaction: reaction } => {
            if let (Some(guild_id), true) = (reaction.guild_id, starboard::is_star(&reaction.emoji)) {
                starboard::on_star(ctx, data, guild_id, reaction.channel_id, reaction.message_id).await;
            }
            if let Some(guild_id) = reaction.guild_id {
                crate::starboards::on_reaction(ctx, data, guild_id, reaction.channel_id, reaction.message_id, &reaction.emoji).await;
            }
            crate::reactionroles::on_reaction(ctx, data, reaction, false).await;
        }
        serenity::FullEvent::ReactionRemoveEmoji { removed_reactions: reaction } => {
            if let (Some(guild_id), true) = (reaction.guild_id, starboard::is_star(&reaction.emoji)) {
                starboard::on_star(ctx, data, guild_id, reaction.channel_id, reaction.message_id).await;
            }
        }
        serenity::FullEvent::ReactionRemoveAll { channel_id, removed_from_message_id } => {
            let guild_id = ctx
                .cache
                .guilds()
                .into_iter()
                .find(|g| ctx.cache.guild(*g).is_some_and(|g| g.channels.contains_key(channel_id)));
            if let Some(guild_id) = guild_id {
                starboard::on_star(ctx, data, guild_id, *channel_id, *removed_from_message_id).await;
            }
        }
        serenity::FullEvent::VoiceStateUpdate { old, new } => {
            if let Some(guild_id) = new.guild_id {
                data.music.on_voice_state_update(ctx, guild_id).await;
            }
            data.tempvoice.on_voice_update(ctx, data, new).await;
            crate::achievements::on_voice(ctx, data, new).await;
            eventlog::voice_changed(ctx, data, old.as_ref(), new).await;
        }
        serenity::FullEvent::GuildMemberAddition { new_member } => {
            if new_member.user.bot {
                crate::antinuke::on_bot_join(ctx, data, new_member.guild_id, new_member.user.id).await;
            }
            members::on_join(ctx, data, new_member).await?;
            crate::nickguard::check(ctx, data, new_member.guild_id, &new_member.user, new_member.nick.as_deref(), &new_member.roles).await;
            crate::community::on_join(ctx, data, new_member).await;
            crate::community::member_flow(&data.redis, new_member.guild_id, true).await;
            crate::community::track_member(&data.redis, new_member.guild_id, new_member.user.id, true).await;
            let facts = Facts { trigger: Trigger::MemberJoin, ..Facts::default() };
            automations::run(ctx, data, automations::Event { guild_id: new_member.guild_id, user: &new_member.user, facts, message: None }).await;
        }
        serenity::FullEvent::GuildMemberRemoval {
            guild_id,
            user,
            member_data_if_available,
        } => {
            if let Err(e) = crate::invites::on_leave(&data.db, *guild_id, user.id).await {
                warn!(guild = %guild_id, error = ?e, "Davet kaydı güncellenemedi");
            }
            crate::antinuke::record(ctx, data, *guild_id, crate::antinuke::Kind::Kick, Some(user.id)).await;
            crate::community::member_flow(&data.redis, *guild_id, false).await;
            crate::community::track_member(&data.redis, *guild_id, user.id, false).await;
            if let Some(member) = member_data_if_available {
                crate::community::on_leave(data, *guild_id, user.id, &member.roles).await;
            }
            members::on_leave(ctx, data, *guild_id, user, member_data_if_available.as_ref()).await?;
            let facts = Facts { trigger: Trigger::MemberLeave, ..Facts::default() };
            automations::run(ctx, data, automations::Event { guild_id: *guild_id, user, facts, message: None }).await;
        }
        serenity::FullEvent::GuildMemberUpdate {
            old_if_available,
            event: update,
            ..
        } => {
            eventlog::member_updated(ctx, data, old_if_available.as_ref(), update).await;
            let name_changed = old_if_available.as_ref().is_none_or(|old| {
                old.nick != update.nick || old.user.global_name != update.user.global_name || old.user.name != update.user.name
            });
            if name_changed {
                crate::nickguard::check(ctx, data, update.guild_id, &update.user, update.nick.as_deref(), &update.roles).await;
            }
            if let Some(old) = old_if_available {
                if old.premium_since.is_none() && update.premium_since.is_some() {
                    crate::community::on_boost(ctx, data, update.guild_id, &update.user).await;
                }
            }
            if let Some(old) = old_if_available {
                let gained: Vec<RoleId> = update.roles.iter().filter(|r| !old.roles.contains(r)).copied().collect();
                if !gained.is_empty() {
                    crate::antinuke::on_member_roles_added(ctx, data, update.guild_id, update.user.id, &gained).await;
                }
                let added = update.roles.iter().filter(|r| !old.roles.contains(r)).map(|r| (Trigger::RoleAdded, *r));
                let removed = old.roles.iter().filter(|r| !update.roles.contains(r)).map(|r| (Trigger::RoleRemoved, *r));
                for (trigger, role) in added.chain(removed).collect::<Vec<_>>() {
                    let facts = Facts { trigger, role: Some(role.get()), member_roles: &update.roles, ..Facts::default() };
                    automations::run(ctx, data, automations::Event { guild_id: update.guild_id, user: &update.user, facts, message: None }).await;
                }
            }
        }
        serenity::FullEvent::GuildBanAddition { guild_id, banned_user } => {
            eventlog::ban_changed(ctx, data, *guild_id, banned_user, true).await;
            crate::antinuke::record(ctx, data, *guild_id, crate::antinuke::Kind::Ban, None).await;
        }
        serenity::FullEvent::GuildBanRemoval { guild_id, unbanned_user } => {
            eventlog::ban_changed(ctx, data, *guild_id, unbanned_user, false).await;
        }
        serenity::FullEvent::GuildRoleCreate { new } => {
            eventlog::role_created(ctx, data, new).await;
            crate::antinuke::on_role_permissions(ctx, data, new.guild_id, new, None).await;
        }
        serenity::FullEvent::GuildRoleUpdate { old_data_if_available, new } => {
            eventlog::role_updated(ctx, data, old_data_if_available.as_ref(), new).await;
            // Eski yetkiler bilinmiyorsa (önbellekte yok) yükseltme anlaşılamaz; atlanır.
            if let Some(old) = old_data_if_available {
                crate::antinuke::on_role_permissions(ctx, data, new.guild_id, new, Some(old.permissions)).await;
            }
        }
        serenity::FullEvent::WebhookUpdate { guild_id, belongs_to_channel_id } => {
            eventlog::webhooks_changed(ctx, data, *guild_id, *belongs_to_channel_id).await;
            crate::antinuke::on_webhook(ctx, data, *guild_id).await;
        }
        serenity::FullEvent::GuildRoleDelete { guild_id, removed_role_id, removed_role_data_if_available } => {
            eventlog::role_deleted(ctx, data, *guild_id, *removed_role_id, removed_role_data_if_available.as_ref()).await;
            crate::antinuke::record(ctx, data, *guild_id, crate::antinuke::Kind::RoleDelete, None).await;
            crate::cleanup::role_deleted(ctx, data, *guild_id, *removed_role_id).await;
        }
        serenity::FullEvent::ThreadCreate { thread } => {
            // Bot var olan bir alt başlığa eklendiğinde de bu olay gelir; yalnızca yeni açılanlar sayılır.
            let fresh = thread.id.created_at().unix_timestamp() + 15 > serenity::Timestamp::now().unix_timestamp();
            if fresh {
                eventlog::thread_changed(ctx, data, thread.guild_id, None, thread.id, thread.parent_id, true).await;
            }
            let owner = thread.owner_id.and_then(|id| ctx.cache.user(id).map(|u| u.clone()));
            if let (true, Some(owner)) = (fresh, owner) {
                crate::achievements::bump(ctx, data, thread.guild_id, &owner, crate::achievements::Kind::Threads, 1, Some(thread.id)).await;
                let facts = Facts { trigger: Trigger::ThreadCreated, channel: thread.parent_id.map(ChannelId::get), ..Facts::default() };
                let event = automations::Event { guild_id: thread.guild_id, user: &owner, facts, message: None };
                // Mesaj eylemi alt başlığın içine gider; koşuldaki kanal ise ana kanaldır.
                run_in_thread(ctx, data, event, thread.id).await;
            }
        }
        serenity::FullEvent::ThreadUpdate { old, new } => {
            eventlog::thread_updated(ctx, data, old.as_ref(), new).await;
        }
        serenity::FullEvent::ThreadDelete { thread, full_thread_data } => {
            let name = full_thread_data.as_ref().map(|t| t.name.as_str());
            eventlog::thread_changed(ctx, data, thread.guild_id, name, thread.id, Some(thread.parent_id), false).await;
        }
        serenity::FullEvent::GuildStickersUpdate { guild_id, current_state } => {
            eventlog::stickers_updated(ctx, data, *guild_id, current_state.len()).await;
        }
        serenity::FullEvent::ChannelCreate { channel } => {
            eventlog::channel_created(ctx, data, channel).await;
            crate::antinuke::record(ctx, data, channel.guild_id, crate::antinuke::Kind::ChannelCreate, None).await;
        }
        serenity::FullEvent::ChannelUpdate { old, new } => {
            eventlog::channel_updated(ctx, data, old.as_ref(), new).await;
        }
        serenity::FullEvent::GuildUpdate { old_data_if_available, new_data } => {
            let old = old_data_if_available.as_ref().map(|g| (g.name.clone(), g.icon_url()));
            eventlog::guild_updated(ctx, data, old, new_data).await;
        }
        serenity::FullEvent::GuildEmojisUpdate { guild_id, current_state } => {
            eventlog::emojis_updated(ctx, data, *guild_id, current_state.len()).await;
        }
        serenity::FullEvent::ChannelDelete { channel, .. } => {
            eventlog::channel_deleted(ctx, data, channel).await;
            crate::antinuke::record(ctx, data, channel.guild_id, crate::antinuke::Kind::ChannelDelete, None).await;
            crate::cleanup::channel_deleted(ctx, data, channel.guild_id, channel.id).await;
            tickets::on_channel_deleted(&data.db, channel.id).await?;
            crate::statchannels::on_channel_deleted(&data.db, channel.id).await?;
            data.tempvoice.forget(&data.db, channel.id).await?;
        }
        _ => {}
    }

    Ok(())
}

/// Alt başlık otomasyonları: koşullar ana kanala göre denetlenir, mesaj alt başlığa gönderilir.
async fn run_in_thread(ctx: &serenity::Context, data: &Data, event: automations::Event<'_>, thread: ChannelId) {
    automations::run_with_target(ctx, data, event, Some(thread)).await;
}

/// Eklerin tür özeti (kısıtlı kanal denetimi için).
fn media_of(attachments: &[serenity::Attachment]) -> crate::automod::rules::Media {
    let mut media = crate::automod::rules::Media::default();
    for attachment in attachments {
        media.add(attachment.content_type.as_deref(), &attachment.filename);
    }
    media
}

/// Otomatik cevap kuralına uyuyorsa cevaplar; cevapladıysa `true`.
async fn send_autoresponse(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message) -> bool {
    match data.autoresponder.response_for(&data.db, guild_id, msg.channel_id, msg.author.id, &msg.content).await {
        Ok(Some(text)) => {
            let reply = serenity::CreateMessage::new()
                .content(text)
                .reference_message(msg)
                .allowed_mentions(serenity::CreateAllowedMentions::new().users(vec![msg.author.id]));
            if let Err(e) = msg.channel_id.send_message(&ctx.http, reply).await {
                debug!(channel = %msg.channel_id, error = %e, "Otomatik cevap gönderilemedi");
            }
            true
        }
        Ok(None) => false,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Otomatik cevaplar okunamadı");
            false
        }
    }
}

/// `!ad` bir etiketse içeriğini gönderir; gönderildiyse `true`.
async fn send_tag(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message) -> bool {
    // Çoğu mesaj `!` ile başlamaz; bunlar için hiçbir iş yapılmaz.
    if !msg.content.starts_with(crate::tags::PREFIX) {
        return false;
    }
    match data.tags.for_message(&data.db, guild_id, &msg.content).await {
        Ok(Some(content)) => {
            // Etiketi "Mesajları Yönet" izni olan biri yazar; @everyone / rol etiketi bot üzerinden çalışmasın
            // (yazanın kendi etiketleme izni olmayabilir) ve herkes `!ad` ile tekrar tekrar tetikleyebilir.
            let message = serenity::CreateMessage::new().content(content).allowed_mentions(serenity::CreateAllowedMentions::new());
            if let Err(e) = msg.channel_id.send_message(&ctx.http, message).await {
                debug!(channel = %msg.channel_id, error = %e, "Etiket gönderilemedi");
            }
            true
        }
        Ok(None) => false,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Etiketler okunamadı");
            false
        }
    }
}

/// XP verir; seviye atlandıysa duyurur ve ödül rollerini verir.
/// Hatalar mesaj akışını etkilememesi için sadece loglanır.
async fn award_xp(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message, roles: &[RoleId]) {
    // XP'siz kanal (veya kategorisi / alt başlığın ana kanalı) ya da rol.
    let channels: Vec<ChannelId> = ctx.cache.guild(guild_id).map_or_else(
        || vec![msg.channel_id],
        |guild| {
            let parent_of = |id: ChannelId| {
                guild
                    .channels
                    .get(&id)
                    .and_then(|c| c.parent_id)
                    .or_else(|| guild.threads.iter().find(|t| t.id == id).and_then(|t| t.parent_id))
            };
            let parent = parent_of(msg.channel_id);
            let grandparent = parent.and_then(parent_of);
            [Some(msg.channel_id), parent, grandparent].into_iter().flatten().collect()
        },
    );
    let settings = match data.settings.get::<LevelSettings>(&data.db, guild_id).await {
        Ok(settings) => settings,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Seviye ayarları okunamadı, varsayılan kullanılıyor");
            std::sync::Arc::new(LevelSettings::default())
        }
    };
    if !settings.enabled {
        return;
    }
    match data.leveling.is_excluded(&data.db, guild_id, &channels, roles, &settings).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => warn!(guild = %guild_id, error = ?e, "XP muafiyetleri okunamadı"),
    }

    let level_up = match data
        .leveling
        .on_message(&data.db, &data.redis, guild_id, msg.author.id, settings.effective_rate(&roles.iter().map(|r| r.get()).collect::<Vec<_>>()))
        .await
    {
        Ok(Some(level_up)) => level_up,
        Ok(None) => return,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "XP verilemedi");
            return;
        }
    };
    let LevelUp { old, new } = level_up;

    let (earned, outdated): (Vec<RoleId>, Vec<RoleId>) = match data.leveling.rewards(&data.db, guild_id).await {
        Ok(rewards) => roles_on_level_up(&rewards, old, new, settings.stack_rewards),
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Seviye ödülleri okunamadı");
            (Vec::new(), Vec::new())
        }
    };
    // "Sadece en yüksek ödül" ayarında eski ödül rolleri geri alınır.
    for role in outdated.into_iter().filter(|r| roles.contains(r)) {
        if let Err(e) = ctx.http.remove_member_role(guild_id, msg.author.id, role, Some("Daha yüksek seviye ödülü kazanıldı")).await {
            warn!(guild = %guild_id, role = %role, error = %e, "Eski seviye ödül rolü alınamadı");
        }
    }

    // Rol sonradan yönetici yetkisi almış olabilir: verirken yeniden denetlenir (önbellek kilidi `await`'ten önce bırakılır).
    let earned: Vec<RoleId> = {
        let roles: Vec<serenity::Role> = ctx
            .cache
            .guild(guild_id)
            .map(|g| earned.iter().filter_map(|r| g.roles.get(r).cloned()).collect())
            .unwrap_or_default();
        roles
            .into_iter()
            .filter(|role| match crate::rolepanel::check_role(ctx, guild_id, role) {
                Ok(()) => true,
                Err(reason) => {
                    warn!(guild = %guild_id, role = %role.id, reason, "Seviye ödülü verilmedi");
                    false
                }
            })
            .map(|role| role.id)
            .collect()
    };
    let mut granted = Vec::new();
    for role in earned {
        match ctx
            .http
            .add_member_role(guild_id, msg.author.id, role, Some("Seviye ödülü"))
            .await
        {
            Ok(()) => granted.push(role),
            Err(e) => warn!(
                guild = %guild_id,
                role = %role,
                error = %e,
                "Seviye ödül rolü verilemedi (botun rolü ödül rolünün üstünde mi?)"
            ),
        }
    }

    if settings.announce == Announce::Off {
        return;
    }
    let guild_name = ctx.cache.guild(guild_id).map(|g| g.name.clone()).unwrap_or_default();
    let name = msg.author.global_name.clone().unwrap_or_else(|| msg.author.name.clone());
    let mut text = render_message(settings.template(), &msg.author.id.mention().to_string(), &name, new, &guild_name);
    if !granted.is_empty() {
        let roles = granted
            .iter()
            .map(|r| r.mention().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        text.push_str(&format!(" Yeni rol: {roles}"));
    }
    // Şablon yönetici tarafından yazılır; yine de sadece seviye atlayan kişi etiketlenir.
    let message = serenity::CreateMessage::new()
        .content(text)
        .allowed_mentions(serenity::CreateAllowedMentions::new().users(vec![msg.author.id]));
    let sent = match settings.announce {
        Announce::Dm => msg.author.direct_message(ctx, message).await.map(|_| ()),
        Announce::Channel => {
            let target = settings.announce_channel.filter(|id| *id != 0).map_or(msg.channel_id, ChannelId::new);
            target.send_message(&ctx.http, message).await.map(|_| ())
        }
        Announce::Current | Announce::Off => msg.channel_id.send_message(&ctx.http, message).await.map(|_| ()),
    };
    if let Err(e) = sent {
        debug!(channel = %msg.channel_id, error = %e, "Seviye duyurusu gönderilemedi");
    }
}
