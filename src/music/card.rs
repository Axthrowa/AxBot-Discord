//! "Şimdi çalıyor" kartı: resim olarak çizilen kart (`render.rs`) + kontrol butonları.
//!
//! Butonlar `music:<işlem>` biçimindedir; bot yeniden başlasa da çalışır (o anki çalana uygulanır).
//! Sadece botla aynı ses kanalındaki üyeler kullanabilir. "Ses" ve "Ekle" butonları form açar.

use std::sync::Arc;
use std::time::Duration;

use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow, CreateButton,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateModal, EditInteractionResponse,
    GuildId, Mentionable, ModalInteraction,
};
use songbird::tracks::{LoopState, PlayMode};

use super::{Music, TrackInfo};
use crate::modals;

pub const PREFIX: &str = "music:";
const VOLUME_STEP: f32 = 0.1;
const MAX_VOLUME: f32 = 1.5;

/// Kartta gösterilen anlık durum.
pub struct CardState {
    pub volume: f32,
    pub paused: bool,
    pub looping: bool,
    /// Sırada bekleyen şarkı sayısı (çalan hariç).
    pub upcoming: usize,
    pub position: Duration,
    /// Sıradaki şarkının adı.
    pub next: Option<String>,
}

pub struct Source {
    pub name: &'static str,
    pub accent: [u8; 3],
}

pub fn source_of(info: &TrackInfo) -> Source {
    let url = info.url.as_deref().unwrap_or_default();
    if info.spotify_url.is_some() {
        Source { name: "SPOTIFY", accent: [30, 215, 96] }
    } else if url.contains("soundcloud.com") {
        Source { name: "SOUNDCLOUD", accent: [255, 110, 30] }
    } else if url.contains("youtube.com") || url.contains("youtu.be") {
        Source { name: "YOUTUBE", accent: [255, 48, 48] }
    } else if info.title.starts_with("📻") {
        Source { name: "CANLI RADYO", accent: [170, 100, 220] }
    } else {
        Source { name: "MÜZİK", accent: [88, 101, 242] }
    }
}

/// "Sanatçı - Şarkı (Official Video)" → ("Sanatçı", "Şarkı")
pub fn split_title(title: &str) -> (Option<String>, String) {
    let title = title.trim_start_matches("📻").trim();
    let cleaned = super::extra::clean_title(title);
    let cleaned = if cleaned.is_empty() { title.to_owned() } else { cleaned };
    match cleaned.split_once(" - ") {
        Some((artist, song)) if !artist.trim().is_empty() && !song.trim().is_empty() => {
            (Some(artist.trim().to_owned()), song.trim().to_owned())
        }
        _ => (None, cleaned),
    }
}

/// Kartın üstündeki satır. Discord'un göreli zamanı (`<t:…:R>`) istemcide saniye saniye güncellenir.
/// Şarkı adı bu satırda olduğu için Discord bildiriminde de görünür (resimdeki yazı bildirime düşmez).
pub fn content(info: &TrackInfo, state: &CardState) -> String {
    let (artist, song) = split_title(&info.title);
    let name = match artist {
        Some(artist) => format!("**{song}** — {artist}"),
        None => format!("**{song}**"),
    };
    if state.paused {
        return format!("⏸️ Duraklatıldı: {name}");
    }
    match info.duration {
        Some(total) if total > state.position => {
            let ends = serenity::Timestamp::now().unix_timestamp() + i64::try_from((total - state.position).as_secs()).unwrap_or(0);
            format!("🎶 Şimdi çalıyor: {name} · bitiş <t:{ends}:R>")
        }
        Some(_) => format!("🎶 Şimdi çalıyor: {name}"),
        None => format!("📻 Canlı yayın: {name}"),
    }
}

/// Atlama menüsündeki nokta sayısı (şarkının %0, %10 … %90'ı).
const SEEK_POINTS: u32 = 10;
const SEEK_STEP: Duration = Duration::from_secs(10);

pub fn buttons(info: &TrackInfo, state: &CardState) -> Vec<CreateActionRow> {
    // Sade düzen: ince beyaz ikonlar (botun kendi emojileri, bkz. `icons`), yazısız butonlar.
    let button = |id: &str, icon: &str, style: ButtonStyle| {
        CreateButton::new(format!("{PREFIX}{id}")).emoji(super::icons::emoji(icon)).style(style)
    };
    let live = info.duration.is_none();
    // 1. satır: Spotify gibi — önceki (baştan başlat) · oynat/duraklat · sonraki · kapat · karıştır
    let mut row1 = Vec::new();
    if !live {
        row1.push(button("restart", "prev", ButtonStyle::Secondary));
    }
    row1.push(if state.paused {
        button("pause", "play", ButtonStyle::Success)
    } else {
        button("pause", "pause", ButtonStyle::Secondary)
    });
    row1.push(button("skip", "next", ButtonStyle::Secondary));
    row1.push(button("stop", "stop", ButtonStyle::Secondary));
    row1.push(button("shuffle", "shuffle", ButtonStyle::Secondary));

    // 2. satır: ses (form açar) ve döngü
    let row2 = vec![
        button("volume", "volume", ButtonStyle::Secondary),
        button("loop", "loop", if state.looping { ButtonStyle::Success } else { ButtonStyle::Secondary }),
    ];

    // 3. satır: şarkının istediğin yerine git (sürükleme yerine).
    let mut rows = vec![CreateActionRow::Buttons(row1), CreateActionRow::Buttons(row2)];
    if let Some(total) = info.duration.filter(|d| d.as_secs() >= 20) {
        let current = state.position.as_secs_f64() / total.as_secs_f64();
        let options = (0..SEEK_POINTS)
            .map(|i| {
                let at = total.mul_f64(f64::from(i) / f64::from(SEEK_POINTS));
                let label = if i == 0 {
                    "0:00 — baştan başla".to_owned()
                } else {
                    format!("{} — şarkının %{}'i", super::render::clock(at), i * 10)
                };
                // Şu an çalınan bölüm işaretlenir.
                let here = (current * f64::from(SEEK_POINTS)).floor() as u32 == i;
                let mut option = serenity::CreateSelectMenuOption::new(label, at.as_secs().to_string());
                if here {
                    option = option.description("▶ şu an buradasın");
                }
                option
            })
            .collect();
        let menu = serenity::CreateSelectMenu::new(format!("{PREFIX}seekto"), serenity::CreateSelectMenuKind::String { options })
            .placeholder("⏱️ Şarkının istediğin yerine git…");
        rows.push(CreateActionRow::SelectMenu(menu));
    }

    // 4. satır: sıra · şarkı ekle · sırayı temizle · paylaş (şarkının bağlantısı)
    let mut row4 = vec![
        button("queue", "queue", ButtonStyle::Secondary),
        button("add", "search", ButtonStyle::Primary),
        button("clear", "clear", ButtonStyle::Secondary),
    ];
    let link = info.spotify_url.as_deref().or(info.url.as_deref()).filter(|u| u.starts_with("https://"));
    if let Some(link) = link {
        row4.push(CreateButton::new_link(link).emoji(super::icons::emoji("share")));
    }
    rows.push(CreateActionRow::Buttons(row4));
    rows
}

/// İleri/geri sarılacak yeni konum (şarkının başı ile sonundan 2 sn öncesi arasında).
fn shifted(position: Duration, total: Option<Duration>, forward: bool) -> Duration {
    let moved = if forward { position + SEEK_STEP } else { position.saturating_sub(SEEK_STEP) };
    match total {
        Some(total) => moved.min(total.saturating_sub(Duration::from_secs(2))),
        None => moved,
    }
}

/// Kart butonları.
pub async fn handle_button(ctx: &serenity::Context, music: &Music, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let Some(guild_id) = interaction.guild_id else {
        return Ok(());
    };
    let action = interaction.data.custom_id.strip_prefix(PREFIX).unwrap_or_default();
    // Form açan butonların ilk cevabı form olmalı; diğerleri hemen onaylanır, çünkü ses bağlantısının
    // kilidi (otomatik çalma şarkı eklerken) Discord'un 3 sn sınırından uzun tutulabilir.
    let acked = !matches!(action, "volume" | "add");
    if acked {
        interaction.create_response(ctx, CreateInteractionResponse::Acknowledge).await?;
    }
    if let Some(problem) = same_channel_problem(ctx, music, guild_id, interaction.user.id).await {
        return reply(ctx, interaction, acked, &problem).await;
    }

    // Form açan butonlar.
    match action {
        "volume" => {
            let current = (music.volume_of(guild_id) * 100.0).round();
            let modal = CreateModal::new(format!("{PREFIX}setvolume"), "Ses seviyesi")
                .components(vec![modals::text_field("level", &format!("Ses (0-150) — şu an %{current}"), false, true, 3)]);
            interaction.create_response(ctx, CreateInteractionResponse::Modal(modal)).await?;
            return Ok(());
        }
        "add" => {
            let modal = CreateModal::new(format!("{PREFIX}addsong"), "Şarkı ekle")
                .components(vec![modals::text_field("query", "Şarkı adı veya YouTube/SoundCloud bağlantısı", false, true, 300)]);
            interaction.create_response(ctx, CreateInteractionResponse::Modal(modal)).await?;
            return Ok(());
        }
        _ => {}
    }

    let Some(call) = music.songbird().get(guild_id) else {
        return reply(ctx, interaction, acked, "Şu an müzik çalmıyor.").await;
    };
    let queue = call.lock().await.queue().clone();
    let Some(current) = queue.current() else {
        return reply(ctx, interaction, acked, "Şu an çalan bir şarkı yok.").await;
    };

    match action {
        "pause" => {
            if matches!(current.get_info().await?.playing, PlayMode::Pause) { queue.resume()? } else { queue.pause()? }
        }
        "loop" => {
            if current.get_info().await?.loops != LoopState::default() { current.disable_loop()? } else { current.enable_loop()? }
        }
        "back" | "forward" | "seekto" | "restart" => {
            let total = music.track_info(current.uuid()).and_then(|i| i.duration);
            let target = if action == "restart" {
                Duration::ZERO
            } else if action == "seekto" {
                let chosen = match &interaction.data.kind {
                    serenity::ComponentInteractionDataKind::StringSelect { values } => values.first().and_then(|v| v.parse::<u64>().ok()),
                    _ => None,
                };
                let Some(secs) = chosen else {
                    return Ok(());
                };
                Duration::from_secs(secs)
            } else {
                let offset = music.position_offset_of(guild_id);
                let position = current.get_info().await?.position + offset;
                shifted(position, total, action == "forward")
            };
            // Kaynak sarmayı desteklemezse veya takılırsa bekleme sınırlı tutulur.
            let seeked = music.seek_to(&ctx.http, guild_id, target).await;
            if let Err(e) = seeked {
                tracing::debug!(error = %e, "Şarkı sarılamadı");
                let _ = interaction
                    .create_followup(
                        ctx,
                        serenity::CreateInteractionResponseFollowup::new()
                            .content("❌ Bu şarkı sarılamıyor (kaynak desteklemiyor olabilir).")
                            .ephemeral(true),
                    )
                    .await;
            }
            music.refresh_card_now(&ctx.http, guild_id).await;
            return Ok(());
        }
        "voldown" | "volup" => {
            let step = if action == "volup" { VOLUME_STEP } else { -VOLUME_STEP };
            let level = ((music.volume_of(guild_id) + step) * 10.0).round() / 10.0;
            music.apply_volume(guild_id, level.clamp(0.0, MAX_VOLUME)).await;
        }
        "shuffle" => {
            let shuffled = queue.modify_queue(|q| {
                if q.len() < 3 {
                    return 0;
                }
                let upcoming = &mut q.make_contiguous()[1..];
                crate::rand_util::shuffle(upcoming);
                upcoming.len()
            });
            if shuffled == 0 {
                return reply(ctx, interaction, acked, "Karıştırmak için sırada en az 2 şarkı olmalı.").await;
            }
        }
        "clear" => {
            let removed = music.clear_upcoming(guild_id).await;
            if removed == 0 {
                return reply(ctx, interaction, acked, "Sırada bekleyen şarkı yok.").await;
            }
        }
        "skip" => {
            // Yeni şarkının kartı başlayınca gelir; bu kart o zaman silinir.
            queue.skip()?;
            return Ok(());
        }
        "stop" => {
            let mut message = interaction.message.clone();
            let _ = message
                .edit(
                    ctx,
                    serenity::EditMessage::new()
                        .content(format!("⏹️ Müzik {} tarafından kapatıldı.", interaction.user.id.mention()))
                        .components(Vec::new()),
                )
                .await;
            music.stop_and_leave(guild_id).await;
            return Ok(());
        }
        "queue" => return show_queue(ctx, music, interaction, &queue).await,
        _ => return Ok(()),
    }

    // Değişikliği hemen karta yansıt.
    music.refresh_card_now(&ctx.http, guild_id).await;
    Ok(())
}

/// "Ses" ve "Ekle" formları.
pub async fn handle_modal(ctx: &serenity::Context, music: &Music, modal: &ModalInteraction) -> anyhow::Result<()> {
    let Some(guild_id) = modal.guild_id else {
        return Ok(());
    };
    let action = modal.data.custom_id.strip_prefix(PREFIX).unwrap_or_default();
    let value = modals::values(modal).into_iter().next().unwrap_or_default();
    if let Some(problem) = same_channel_problem(ctx, music, guild_id, modal.user.id).await {
        return modal_reply(ctx, modal, &problem).await;
    }
    match action {
        "setvolume" => {
            let Ok(level) = value.trim().trim_start_matches('%').parse::<u32>() else {
                return modal_reply(ctx, modal, "❌ 0 ile 150 arasında bir sayı yaz.").await;
            };
            let level = level.min(150);
            music.apply_volume(guild_id, level as f32 / 100.0).await;
            modal.create_response(ctx, CreateInteractionResponse::Acknowledge).await?;
            music.refresh_card_now(&ctx.http, guild_id).await;
            Ok(())
        }
        "addsong" => {
            let query = value.trim().to_owned();
            if super::is_playlist_url(&query) || super::extra::spotify_link(&query).is_some() {
                return modal_reply(ctx, modal, "📜 Çalma listesi ve Spotify bağlantıları için `/çal` komutunu kullan.").await;
            }
            modal.defer_ephemeral(ctx).await?;
            let Some(voice) = voice_channel_of(ctx, guild_id, modal.user.id) else {
                modal.edit_response(ctx, EditInteractionResponse::new().content("🔇 Önce ses kanalına katıl.")).await?;
                return Ok(());
            };
            let text = match music.resolve_preferring_spotify(&query, modal.user.id, guild_id).await {
                Err(message) => message,
                Ok((input, info)) => {
                    let title = info.title.clone();
                    match music.enqueue(Arc::clone(&ctx.http), guild_id, voice, modal.channel_id, input, info).await {
                        Ok(e) if e.position == 0 => format!("🎶 Çalıyor: **{title}**"),
                        Ok(e) => format!("➕ Sıraya eklendi (#{}): **{title}**", e.position),
                        Err(_) => "❌ Şarkı eklenemedi.".to_owned(),
                    }
                }
            };
            modal.edit_response(ctx, EditInteractionResponse::new().content(text)).await?;
            music.refresh_card_now(&ctx.http, guild_id).await;
            Ok(())
        }
        "seek" | "seekmodal" => {
            let Some(call) = music.songbird().get(guild_id) else {
                return modal_reply(ctx, modal, "❌ Şu an müzik çalmıyor.").await;
            };
            let (cur_pos, total) = {
                let handler = call.lock().await;
                let queue = handler.queue().clone();
                let Some(current) = queue.current() else {
                    return modal_reply(ctx, modal, "❌ Şu an çalan bir şarkı yok.").await;
                };
                let offset = music.position_offset_of(guild_id);
                let cur = current.get_info().await.ok().map(|s| s.position + offset);
                let tot = music.track_info(current.uuid()).and_then(|i| i.duration);
                (cur, tot)
            };
            let target = match super::extra::parse_seek_timestamp(&value, cur_pos) {
                Ok(t) => t,
                Err(msg) => return modal_reply(ctx, modal, &format!("❌ {msg}")).await,
            };
            if let Some(total) = total {
                if target >= total {
                    return modal_reply(
                        ctx,
                        modal,
                        &format!("❌ Şarkı {} uzunluğunda; daha ileri sarılamaz.", super::format_duration(total)),
                    )
                    .await;
                }
            }
            modal.create_response(ctx, CreateInteractionResponse::Acknowledge).await?;
            if let Err(e) = music.seek_to(&ctx.http, guild_id, target).await {
                tracing::warn!(guild = %guild_id, error = %e, "Modal ile sarılamadı");
            }
            music.refresh_card_now(&ctx.http, guild_id).await;
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn show_queue(ctx: &serenity::Context, music: &Music, interaction: &ComponentInteraction, queue: &songbird::tracks::TrackQueue) -> anyhow::Result<()> {
    let lines: Vec<String> = queue
        .current_queue()
        .iter()
        .filter_map(|h| music.track_info(h.uuid()))
        .take(11)
        .enumerate()
        .map(|(i, info)| {
            let prefix = if i == 0 { "▶️".to_owned() } else { format!("`{i}.`") };
            format!("{prefix} {}", super::track_line(&info))
        })
        .collect();
    let total = queue.len();
    let mut text = lines.join("\n");
    if total > 11 {
        text.push_str(&format!("\n_…ve {} şarkı daha_", total - 11));
    }
    reply(ctx, interaction, true, &format!("📜 **Kuyruk — {total} şarkı**\n{text}")).await
}

/// Kullanıcı botla aynı ses kanalında değilse açıklama.
async fn same_channel_problem(ctx: &serenity::Context, music: &Music, guild_id: GuildId, user: serenity::UserId) -> Option<String> {
    let Some(call) = music.songbird().get(guild_id) else {
        return Some("Şu an müzik çalmıyor.".to_owned());
    };
    let Ok(locked) = tokio::time::timeout(Duration::from_secs(2), call.lock()).await else {
        return Some("⏳ Bot şu an sıraya şarkı ekliyor; birkaç saniye sonra tekrar dene.".to_owned());
    };
    let bot = locked.current_channel().map(|c| ChannelId::new(c.0.get()));
    drop(locked);
    if bot.is_none() || bot != voice_channel_of(ctx, guild_id, user) {
        return Some("🎧 Müziği kontrol etmek için botla aynı ses kanalında olmalısın.".to_owned());
    }
    None
}

/// Yalnızca tıklayana görünen cevap; buton önceden onaylandıysa (`acked`) ek mesaj olarak gider.
async fn reply(ctx: &serenity::Context, interaction: &ComponentInteraction, acked: bool, text: &str) -> anyhow::Result<()> {
    if acked {
        interaction
            .create_followup(ctx, serenity::CreateInteractionResponseFollowup::new().content(text).ephemeral(true))
            .await?;
        return Ok(());
    }
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true)),
        )
        .await?;
    Ok(())
}

async fn modal_reply(ctx: &serenity::Context, modal: &ModalInteraction, text: &str) -> anyhow::Result<()> {
    modal
        .create_response(
            ctx,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true)),
        )
        .await?;
    Ok(())
}

fn voice_channel_of(ctx: &serenity::Context, guild_id: GuildId, user: serenity::UserId) -> Option<ChannelId> {
    ctx.cache.guild(guild_id)?.voice_states.get(&user)?.channel_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use poise::serenity_prelude::UserId;

    fn info(title: &str, url: Option<&str>) -> TrackInfo {
        TrackInfo {
            guild_id: GuildId::new(1),
            title: title.to_owned(),
            url: url.map(str::to_owned),
            duration: Some(Duration::from_secs(178)),
            thumbnail: None,
            requester: UserId::new(1),
            announce: false,
            spotify_url: None,
        }
    }

    #[test]
    fn splits_artist_and_song() {
        assert_eq!(split_title("TARKAN - Şımarık (Official Music Video)"), (Some("TARKAN".to_owned()), "Şımarık".to_owned()));
        assert_eq!(split_title("Güneş"), (None, "Güneş".to_owned()));
        assert_eq!(split_title("📻 Joy Türk"), (None, "Joy Türk".to_owned()));
    }

    #[test]
    fn detects_sources() {
        assert_eq!(source_of(&info("x", Some("https://www.youtube.com/watch?v=a"))).name, "YOUTUBE");
        assert_eq!(source_of(&info("x", Some("https://soundcloud.com/a/b"))).name, "SOUNDCLOUD");
        assert_eq!(source_of(&info("📻 Joy Türk", None)).name, "CANLI RADYO");
        let mut sp = info("x", None);
        sp.spotify_url = Some("https://open.spotify.com/track/x".to_owned());
        assert_eq!(source_of(&sp).name, "SPOTIFY");
    }

    #[test]
    fn content_line() {
        let i = info("x", None);
        let playing = CardState { volume: 0.6, paused: false, looping: false, upcoming: 0, position: Duration::from_secs(10), next: None };
        assert!(content(&i, &playing).contains("<t:"));
        let paused = CardState { paused: true, ..playing };
        assert!(content(&i, &paused).contains("Duraklatıldı"));
    }
}

#[cfg(test)]
mod seek_tests {
    use super::*;

    #[test]
    fn shifting_stays_inside_the_song() {
        let total = Some(Duration::from_secs(100));
        assert_eq!(shifted(Duration::from_secs(5), total, false), Duration::ZERO);
        assert_eq!(shifted(Duration::from_secs(50), total, true), Duration::from_secs(60));
        assert_eq!(shifted(Duration::from_secs(95), total, true), Duration::from_secs(98));
    }
}
