//! Müzik komutları.
//!
//! Kontrol komutları (geç, durdur, ses…) sadece botla aynı ses kanalındaki üyeler tarafından
//! kullanılabilir; böylece başka kanaldaki biri müziği bozamaz.

use std::sync::Arc;
use std::time::Instant;

use anyhow::anyhow;
use poise::serenity_prelude::{self as serenity, ChannelId, CreateEmbed, GuildId, Mentionable};
use poise::CreateReply;
use songbird::tracks::LoopState;
use songbird::Call;
use tokio::sync::Mutex;

use crate::data::Context;
use crate::error::Error;
use crate::music::{format_duration, TrackInfo, MAX_QUEUE};

const QUEUE_PAGE: usize = 10;

/// Şarkı çalar veya sıraya ekler (şarkı adı ya da YouTube/Spotify/SoundCloud bağlantısı).
#[poise::command(slash_command, guild_only, rename = "çal", category = "Müzik")]
pub async fn cal(
    ctx: Context<'_>,
    #[rename = "şarkı"]
    #[description = "Şarkı adı veya bağlantı (YouTube, Spotify, SoundCloud, çalma listesi)"]
    #[max_length = 300]
    query: String,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let music = &ctx.data().music;

    let Some(user_channel) = voice_channel_of(ctx, guild_id, ctx.author().id) else {
        return private(ctx, "🔇 Önce bir ses kanalına katıl.").await;
    };

    // Bot başka bir kanalda müzik çalıyorsa oraya taşınmaz.
    let mut queued = 0;
    if let Some(call) = music.songbird().get(guild_id) {
        let handler = call.lock().await;
        let bot_channel = handler.current_channel().map(|c| ChannelId::new(c.0.get()));
        if bot_channel.is_some_and(|c| c != user_channel) && !handler.queue().is_empty() {
            return private(ctx, "🎧 Şu an başka bir ses kanalında müzik çalıyorum.").await;
        }
        queued = handler.queue().len();
        if queued >= MAX_QUEUE {
            return private(ctx, "📜 Kuyruk dolu (en fazla 100 şarkı).").await;
        }
    }

    // yt-dlp arama/çözme birkaç saniye sürebilir.
    ctx.defer().await?;
    let started = Instant::now();

    if let Some((kind, id)) = crate::music::extra::spotify_link(&query) {
        return play_spotify(ctx, guild_id, user_channel, kind, &id, MAX_QUEUE - queued).await;
    }
    if crate::music::is_playlist_url(&query) {
        return play_playlist(ctx, guild_id, user_channel, &query, MAX_QUEUE - queued).await;
    }

    // Şarkı adı: önce Spotify'da aranır (anahtar varsa), ses YouTube'dan çalınır.
    let (input, info) = match music.resolve_preferring_spotify(&query, ctx.author().id, guild_id).await {
        Ok(resolved) => resolved,
        Err(message) => {
            ctx.say(message).await?;
            return Ok(());
        }
    };

    let enqueued = match music
        .enqueue(
            Arc::clone(&ctx.serenity_context().http),
            guild_id,
            user_channel,
            ctx.channel_id(),
            input,
            info.clone(),
        )
        .await
    {
        Ok(enqueued) => enqueued,
        Err(e) => {
            tracing::warn!(guild = %guild_id, error = ?e, "Ses kanalına katılınamadı");
            ctx.say("❌ Ses kanalına katılamadım. Kanalda **Bağlan** ve **Konuş** iznim olduğundan emin ol.")
                .await?;
            return Ok(());
        }
    };
    tracing::info!(
        guild = %guild_id,
        title = %info.title,
        position = enqueued.position,
        ms = started.elapsed().as_millis() as u64,
        "Şarkı eklendi"
    );

    reply_enqueued(ctx, guild_id, &info, enqueued.position).await
}

/// Hemen çalıyorsa "şimdi çalıyor" kartı, sıraya eklendiyse kısa bilgi.
async fn reply_enqueued(ctx: Context<'_>, guild_id: GuildId, info: &TrackInfo, position: usize) -> Result<(), Error> {
    if position == 0 {
        return send_card(ctx, guild_id).await;
    }
    let mut embed = CreateEmbed::new()
        .title(format!("➕ Sıraya eklendi (#{position})"))
        .description(crate::music::track_line(info))
        .field("İsteyen", ctx.author().id.mention().to_string(), true)
        .colour(serenity::Colour::BLURPLE);
    if let Some(thumb) = &info.thumbnail {
        embed = embed.thumbnail(thumb);
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Çalan şarkının kartını cevap olarak gönderir ve kaydeder.
async fn send_card(ctx: Context<'_>, guild_id: GuildId) -> Result<(), Error> {
    let music = &ctx.data().music;
    // Kart çizimi bir an sürebilir; komut zaten ertelenmediyse gizli olarak ertelenir.
    ctx.defer_ephemeral().await?;
    // Kart normal kanal mesajı olarak gönderilir: şarkı değişince/bitince güvenle silinebilsin.
    if music.post_card(&ctx.serenity_context().http, guild_id, ctx.channel_id()).await {
        // "Düşünüyor…" cevabı kaldırılır; sohbette sadece kart kalır.
        if let Context::Application(app) = ctx {
            if app.interaction.delete_response(ctx.http()).await.is_err() {
                ctx.send(CreateReply::default().content("🎶 Kart gönderildi.").ephemeral(true)).await?;
            }
        }
        return Ok(());
    }
    // Kanala yazılamadıysa (izin yok) kart komutun cevabı olarak gösterilir.
    let Some((jpeg, content, buttons)) = music.render_card(ctx.http(), guild_id).await else {
        ctx.say("🎶 Çalıyor.").await?;
        return Ok(());
    };
    let reply = ctx
        .send(
            CreateReply::default()
                .content(content)
                .attachment(serenity::CreateAttachment::bytes(jpeg, "kart.jpg"))
                .components(buttons),
        )
        .await?;
    if let Ok(message) = reply.message().await {
        music.set_card(&ctx.serenity_context().http, guild_id, message.channel_id, message.id).await;
    }
    Ok(())
}

/// Spotify şarkı/albüm/listesi: şarkı adları Spotify'dan okunur, YouTube'da aranıp çalınır.
async fn play_spotify(
    ctx: Context<'_>,
    guild_id: GuildId,
    voice_channel: ChannelId,
    kind: crate::music::extra::SpotifyKind,
    id: &str,
    room: usize,
) -> Result<(), Error> {
    let music = &ctx.data().music;
    let list = match crate::music::extra::spotify(&ctx.data().http_client, &kind, id).await {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(error = ?e, "Spotify okunamadı");
            ctx.say(format!("❌ Spotify bağlantısı okunamadı: {e}")).await?;
            return Ok(());
        }
    };

    let http = Arc::clone(&ctx.serenity_context().http);
    // Tek şarkı: hemen YouTube'da aranır (başlık ve kapak görseli için).
    if kind == crate::music::extra::SpotifyKind::Track {
        let Some(track) = list.tracks.first() else {
            ctx.say("❌ Spotify'da şarkı bulunamadı.").await?;
            return Ok(());
        };
        let (input, info) = match music.resolve_spotify_track(track, ctx.author().id, guild_id).await {
            Ok(resolved) => resolved,
            Err(message) => {
                ctx.say(message).await?;
                return Ok(());
            }
        };
        let enqueued = match music.enqueue(http, guild_id, voice_channel, ctx.channel_id(), input, info.clone()).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(guild = %guild_id, error = ?e, "Ses kanalına katılınamadı");
                ctx.say("❌ Ses kanalına katılamadım. Kanalda **Bağlan** ve **Konuş** iznim olduğundan emin ol.").await?;
                return Ok(());
            }
        };
        return reply_enqueued(ctx, guild_id, &info, enqueued.position).await;
    }

    // Liste/albüm: şarkılar sırası gelince YouTube'da aranır (ekleme anında beklenmez).
    let total = list.tracks.len();
    let tracks: Vec<_> = list.tracks.into_iter().take(room).collect();
    let count = tracks.len();
    let first = tracks.first().map(|t| format!("{} - {}", t.artists, t.title)).unwrap_or_default();
    let mut items = music.spotify_inputs(tracks, ctx.author().id, guild_id);
    // İlk şarkı çalmaya başlayınca "şimdi çalıyor" kartı gösterilsin.
    if let Some((_, first)) = items.first_mut() {
        first.announce = true;
    }
    if let Err(e) = music.enqueue_many(http, guild_id, voice_channel, ctx.channel_id(), items).await {
        tracing::warn!(guild = %guild_id, error = ?e, "Ses kanalına katılınamadı");
        ctx.say("❌ Ses kanalına katılamadım. Kanalda **Bağlan** ve **Konuş** iznim olduğundan emin ol.").await?;
        return Ok(());
    }
    let mut description = format!("**{}** listesinden **{count}** şarkı sıraya eklendi.", list.name);
    if total > count {
        description.push_str(&format!("\n_Kuyruk en fazla {MAX_QUEUE} şarkı alabildiği için {} şarkı eklenemedi._", total - count));
    }
    description.push_str("\n_Şarkılar sırası gelince YouTube'da bulunup çalınır._");
    let embed = CreateEmbed::new()
        .title("📜 Spotify listesi eklendi")
        .description(description)
        .field("İlk şarkı", first, false)
        .field("İsteyen", ctx.author().id.mention().to_string(), true)
        .colour(serenity::Colour::from_rgb(30, 215, 96));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Çalma listesindeki şarkıları (kuyrukta yer kaldığı kadar) sıraya ekler.
async fn play_playlist(
    ctx: Context<'_>,
    guild_id: GuildId,
    voice_channel: ChannelId,
    url: &str,
    room: usize,
) -> Result<(), Error> {
    let music = &ctx.data().music;
    let playlist = match music.playlist(url, ctx.author().id, guild_id, room).await {
        Ok(playlist) => playlist,
        Err(message) => {
            ctx.say(message).await?;
            return Ok(());
        }
    };
    if playlist.tracks.is_empty() {
        ctx.say("❌ Çalma listesinde çalınabilir şarkı bulunamadı.").await?;
        return Ok(());
    }

    let mut tracks = playlist.tracks;
    // İlk şarkı çalmaya başlayınca "şimdi çalıyor" kartı gösterilsin.
    if let Some((_, first)) = tracks.first_mut() {
        first.announce = true;
    }
    let count = tracks.len();
    let total: std::time::Duration = tracks.iter().filter_map(|(_, info)| info.duration).sum();
    let first = tracks.first().map(|(_, info)| crate::music::track_line(info)).unwrap_or_default();
    let enqueued = match music
        .enqueue_many(
            Arc::clone(&ctx.serenity_context().http),
            guild_id,
            voice_channel,
            ctx.channel_id(),
            tracks,
        )
        .await
    {
        Ok(enqueued) => enqueued,
        Err(e) => {
            tracing::warn!(guild = %guild_id, error = ?e, "Ses kanalına katılınamadı");
            ctx.say("❌ Ses kanalına katılamadım. Kanalda **Bağlan** ve **Konuş** iznim olduğundan emin ol.")
                .await?;
            return Ok(());
        }
    };
    tracing::info!(guild = %guild_id, playlist = %playlist.title, count, "Çalma listesi eklendi");

    let mut description = format!("**{}** listesinden **{count}** şarkı sıraya eklendi.", playlist.title);
    if total.as_secs() > 0 {
        description.push_str(&format!(" Toplam süre: **{}**", format_duration(total)));
    }
    if playlist.skipped > 0 {
        description.push_str(&format!("\n_{} şarkı özel veya silinmiş olduğu için atlandı._", playlist.skipped));
    }
    if playlist.truncated {
        description.push_str(&format!("\n_Kuyruk en fazla {MAX_QUEUE} şarkı alabildiği için listenin tamamı eklenemedi._"));
    }
    let label = if enqueued.position == 0 { "Şimdi çalıyor" } else { "İlk şarkı" };
    let embed = CreateEmbed::new()
        .title("📜 Çalma listesi eklendi")
        .description(description)
        .field(label, first, false)
        .field("İsteyen", ctx.author().id.mention().to_string(), true)
        .colour(serenity::Colour::BLURPLE);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Çalan şarkıyı geçer.
#[poise::command(slash_command, guild_only, rename = "geç", category = "Müzik")]
pub async fn gec(ctx: Context<'_>) -> Result<(), Error> {
    let Some((_, call)) = same_channel_call(ctx).await? else {
        return Ok(());
    };
    let handler = call.lock().await;
    let Some(current) = handler.queue().current() else {
        return private(ctx, "Şu an çalan bir şarkı yok.").await;
    };
    let title = title_of(ctx, current.uuid());
    handler.queue().skip()?;
    drop(handler);
    ctx.say(format!("⏭️ Geçildi: **{title}**")).await?;
    Ok(())
}

/// Müziği durdurur, kuyruğu temizler ve ses kanalından ayrılır.
#[poise::command(slash_command, guild_only, rename = "durdur", category = "Müzik")]
pub async fn durdur(ctx: Context<'_>) -> Result<(), Error> {
    let Some((guild_id, _)) = same_channel_call(ctx).await? else {
        return Ok(());
    };
    ctx.data().music.stop_and_leave(guild_id).await;
    ctx.say("⏹️ Müzik durduruldu, kuyruk temizlendi.").await?;
    Ok(())
}

/// Ses kanalından ayrılır (kuyruk temizlenir).
#[poise::command(slash_command, guild_only, rename = "ayrıl", category = "Müzik")]
pub async fn ayril(ctx: Context<'_>) -> Result<(), Error> {
    let Some((guild_id, _)) = same_channel_call(ctx).await? else {
        return Ok(());
    };
    ctx.data().music.stop_and_leave(guild_id).await;
    ctx.say("👋 Ses kanalından ayrıldım.").await?;
    Ok(())
}

/// Çalan şarkıyı duraklatır.
#[poise::command(slash_command, guild_only, rename = "duraklat", category = "Müzik")]
pub async fn duraklat(ctx: Context<'_>) -> Result<(), Error> {
    let Some((_, call)) = same_channel_call(ctx).await? else {
        return Ok(());
    };
    if call.lock().await.queue().current().is_none() {
        return private(ctx, "Şu an çalan bir şarkı yok.").await;
    }
    call.lock().await.queue().pause()?;
    ctx.say("⏸️ Duraklatıldı. Devam etmek için `/müzik devam`.").await?;
    Ok(())
}

/// Duraklatılan şarkıyı devam ettirir.
#[poise::command(slash_command, guild_only, rename = "devam", category = "Müzik")]
pub async fn devam(ctx: Context<'_>) -> Result<(), Error> {
    let Some((_, call)) = same_channel_call(ctx).await? else {
        return Ok(());
    };
    if call.lock().await.queue().current().is_none() {
        return private(ctx, "Şu an çalan bir şarkı yok.").await;
    }
    call.lock().await.queue().resume()?;
    ctx.say("▶️ Devam ediliyor.").await?;
    Ok(())
}

/// Çalan şarkıyı ve ilerlemesini gösterir.
#[poise::command(slash_command, guild_only, rename = "şimdi", category = "Müzik")]
pub async fn simdi(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    if ctx.data().music.card_state(guild_id).await.is_none() {
        return private(ctx, "Şu an müzik çalmıyor.").await;
    }
    send_card(ctx, guild_id).await
}

/// Şarkı kuyruğunu gösterir.
#[poise::command(slash_command, guild_only, rename = "sıra", category = "Müzik")]
pub async fn sira(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let Some(call) = ctx.data().music.songbird().get(guild_id) else {
        return private(ctx, "Kuyruk boş.").await;
    };
    let handles = call.lock().await.queue().current_queue();
    if handles.is_empty() {
        return private(ctx, "Kuyruk boş.").await;
    }

    let music = &ctx.data().music;
    let infos: Vec<TrackInfo> = handles
        .iter()
        .filter_map(|h| music.track_info(h.uuid()))
        .collect();
    let total: std::time::Duration = infos.iter().filter_map(|i| i.duration).sum();

    let mut lines = Vec::new();
    for (i, info) in infos.iter().take(QUEUE_PAGE + 1).enumerate() {
        let prefix = if i == 0 { "▶️".to_owned() } else { format!("`{i}.`") };
        lines.push(format!(
            "{prefix} {} — {}",
            crate::music::track_line(info),
            info.requester.mention()
        ));
    }
    if infos.len() > QUEUE_PAGE + 1 {
        lines.push(format!("_…ve {} şarkı daha_", infos.len() - QUEUE_PAGE - 1));
    }

    let embed = CreateEmbed::new()
        .title(format!("📜 Kuyruk — {} şarkı", infos.len()))
        .description(lines.join("\n"))
        .footer(serenity::CreateEmbedFooter::new(format!(
            "Toplam süre: {}",
            format_duration(total)
        )))
        .colour(serenity::Colour::BLURPLE);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Ses seviyesini ayarlar (%0-150).
#[poise::command(slash_command, guild_only, rename = "ses", category = "Müzik")]
pub async fn ses(
    ctx: Context<'_>,
    #[rename = "seviye"]
    #[description = "Ses seviyesi yüzdesi (0-150, varsayılan 60)"]
    #[min = 0]
    #[max = 150]
    level: u8,
) -> Result<(), Error> {
    let Some((guild_id, _)) = same_channel_call(ctx).await? else {
        return Ok(());
    };
    let volume = f32::from(level) / 100.0;
    // Çalan ve sıradaki tüm şarkılara uygulanır; kart da hemen güncellenir.
    ctx.data().music.apply_volume(guild_id, volume).await;
    ctx.data().music.refresh_card_now(&ctx.serenity_context().http, guild_id).await;
    let icon = match level {
        0 => "🔇",
        1..=40 => "🔈",
        41..=90 => "🔉",
        _ => "🔊",
    };
    ctx.say(format!("{icon} Ses seviyesi **%{level}**")).await?;
    Ok(())
}

/// Çalan şarkının tekrarını açar/kapatır.
#[poise::command(slash_command, guild_only, rename = "döngü", category = "Müzik")]
pub async fn dongu(ctx: Context<'_>) -> Result<(), Error> {
    let Some((_, call)) = same_channel_call(ctx).await? else {
        return Ok(());
    };
    let Some(current) = call.lock().await.queue().current() else {
        return private(ctx, "Şu an çalan bir şarkı yok.").await;
    };
    let looping = current.get_info().await?.loops != LoopState::default();
    if looping {
        current.disable_loop()?;
        ctx.say("➡️ Döngü kapatıldı.").await?;
    } else {
        current.enable_loop()?;
        ctx.say("🔁 Bu şarkı tekrar edecek.").await?;
    }
    Ok(())
}

/// Sıradaki şarkıları karıştırır.
#[poise::command(slash_command, guild_only, rename = "karıştır", category = "Müzik")]
pub async fn karistir(ctx: Context<'_>) -> Result<(), Error> {
    let Some((_, call)) = same_channel_call(ctx).await? else {
        return Ok(());
    };
    let shuffled = call.lock().await.queue().modify_queue(|queue| {
        if queue.len() < 3 {
            return 0;
        }
        // İlk eleman çalan şarkı; yerinde kalır.
        let upcoming = &mut queue.make_contiguous()[1..];
        crate::rand_util::shuffle(upcoming);
        upcoming.len()
    });
    if shuffled == 0 {
        return private(ctx, "Karıştırmak için sırada en az 2 şarkı olmalı.").await;
    }
    ctx.say(format!("🔀 Sıradaki {shuffled} şarkı karıştırıldı.")).await?;
    Ok(())
}

// ---------------------------------------------------------------- /müzik

/// Müzik ayarları ve ek özellikler (7/24, otomatik çalma, radyo, sözler, kuyruk düzenleme).
#[poise::command(
    slash_command,
    guild_only,
    rename = "müzik",
    subcommands("gec", "durdur", "ayril", "duraklat", "devam", "simdi", "sira", "ses", "dongu", "karistir", "m_247", "m_otomatik", "m_radyo", "m_sozler", "m_sar", "m_sil", "m_tasi", "m_dj", "m_liste_kaydet", "m_liste_cal", "m_listelerim", "m_liste_sil", "m_soyle"),
    subcommand_required,
    category = "Müzik"
)]
pub async fn muzik(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// 7/24 modu: kanal boşalsa da kuyruk bitse de bot ayrılmaz (Sunucuyu Yönet izni gerekir).
#[poise::command(slash_command, guild_only, rename = "7-24", required_permissions = "MANAGE_GUILD")]
async fn m_247(
    ctx: Context<'_>,
    #[rename = "açık"]
    #[description = "Açık mı?"]
    on: bool,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let mut settings = data.music.settings(guild_id);
    settings.always_on = on;
    data.music.save_settings(&data.db, guild_id, settings).await?;
    let text = if on {
        "🌙 **7/24 modu açık:** Kanal boşalsa ya da kuyruk bitse de ses kanalından ayrılmayacağım. (`/müzik ayrıl` ile çıkarabilirsin.)"
    } else {
        "☀️ 7/24 modu kapalı: Kanal boşalınca veya 5 dakika müzik çalmazsa ayrılırım."
    };
    ctx.say(text).await?;
    Ok(())
}

/// Otomatik çalma: kuyruk bitince son şarkıya benzer şarkılarla devam eder.
#[poise::command(slash_command, guild_only, rename = "otomatik")]
async fn m_otomatik(
    ctx: Context<'_>,
    #[rename = "açık"]
    #[description = "Açık mı?"]
    on: bool,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let mut settings = data.music.settings(guild_id);
    settings.autoplay = on;
    data.music.save_settings(&data.db, guild_id, settings).await?;
    let text = if on {
        "🔄 **Otomatik çalma açık:** Kuyruk bitince son YouTube şarkısına benzer şarkılar çalacağım."
    } else {
        "⏹️ Otomatik çalma kapalı."
    };
    ctx.say(text).await?;
    Ok(())
}

/// Canlı radyo dinle (kuyruk temizlenir).
#[poise::command(slash_command, guild_only, rename = "radyo")]
async fn m_radyo(
    ctx: Context<'_>,
    #[rename = "istasyon"]
    #[description = "Radyo istasyonu"]
    choice: crate::music::extra::RadioChoice,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let Some(user_channel) = voice_channel_of(ctx, guild_id, ctx.author().id) else {
        return private(ctx, "🔇 Önce bir ses kanalına katıl.").await;
    };
    if let Some(call) = ctx.data().music.songbird().get(guild_id) {
        let handler = call.lock().await;
        let bot_channel = handler.current_channel().map(|c| ChannelId::new(c.0.get()));
        if bot_channel.is_some_and(|c| c != user_channel) && !handler.queue().is_empty() {
            return private(ctx, "🎧 Şu an başka bir ses kanalında müzik çalıyorum.").await;
        }
    }
    ctx.defer().await?;
    let station = choice.station();
    let result = ctx
        .data()
        .music
        .play_radio(
            Arc::clone(&ctx.serenity_context().http),
            guild_id,
            user_channel,
            ctx.channel_id(),
            &station,
            ctx.author().id,
        )
        .await;
    if let Err(e) = result {
        tracing::warn!(guild = %guild_id, error = ?e, "Radyo başlatılamadı");
        ctx.say("❌ Radyo başlatılamadı. Ses kanalında **Bağlan** ve **Konuş** iznim olduğundan emin ol.").await?;
        return Ok(());
    }
    send_card(ctx, guild_id).await
}

/// Şarkı sözlerini gösterir (boşsa çalan şarkı).
#[poise::command(slash_command, guild_only, rename = "sözler")]
async fn m_sozler(
    ctx: Context<'_>,
    #[rename = "şarkı"]
    #[description = "Şarkı adı (boşsa şu an çalan)"]
    #[max_length = 150]
    query: Option<String>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let music = &ctx.data().music;
    let query = match query {
        Some(q) => q,
        None => {
            let current = match music.songbird().get(guild_id) {
                Some(call) => call.lock().await.queue().current(),
                None => None,
            };
            match current.and_then(|c| music.track_info(c.uuid())) {
                Some(info) => crate::music::extra::clean_title(&info.title),
                None => return private(ctx, "Şu an çalan bir şarkı yok; `şarkı` kısmına bir ad yaz.").await,
            }
        }
    };
    ctx.defer().await?;
    let found = crate::music::extra::lyrics(&ctx.data().http_client, &query).await;
    let lyrics = match found {
        Ok(Some(l)) => l,
        Ok(None) => {
            ctx.say(format!("🔍 **{query}** için şarkı sözü bulunamadı.")).await?;
            return Ok(());
        }
        Err(e) => {
            tracing::warn!(error = ?e, "Şarkı sözü alınamadı");
            ctx.say("❌ Şarkı sözü servisine şu an ulaşılamıyor.").await?;
            return Ok(());
        }
    };
    let mut text: String = lyrics.text.chars().take(3900).collect();
    if lyrics.text.chars().count() > 3900 {
        text.push_str("\n…");
    }
    let embed = CreateEmbed::new()
        .title(format!("🎤 {} — {}", lyrics.artist, lyrics.title))
        .description(text)
        .footer(serenity::CreateEmbedFooter::new("Sözler: lrclib.net"))
        .colour(serenity::Colour::BLURPLE);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Çalan şarkıyı belirli bir ana sarar (örn. 1:30).
#[poise::command(slash_command, guild_only, rename = "sar")]
async fn m_sar(
    ctx: Context<'_>,
    #[rename = "konum"]
    #[description = "Gidilecek an (örn. 90, 1:30, 1:02:03)"]
    position: String,
) -> Result<(), Error> {
    let Some((guild_id, call)) = same_channel_call(ctx).await? else {
        return Ok(());
    };
    let Some(target) = crate::music::extra::parse_position(&position) else {
        return private(ctx, "❌ Konumu `1:30` veya `90` (saniye) gibi yaz.").await;
    };
    let Some(current) = call.lock().await.queue().current() else {
        return private(ctx, "Şu an çalan bir şarkı yok.").await;
    };
    let info = ctx.data().music.track_info(current.uuid());
    match info.as_ref().and_then(|i| i.duration) {
        None => return private(ctx, "Bu yayın/şarkı sarılamıyor.").await,
        Some(total) if target >= total => {
            return private(ctx, &format!("Şarkı {} uzunluğunda; daha ileri sarılamaz.", format_duration(total))).await;
        }
        Some(_) => {}
    }
    ctx.defer().await?;
    match tokio::time::timeout(std::time::Duration::from_secs(20), current.seek_async(target)).await {
        Ok(Ok(_)) => {
            ctx.say(format!("⏩ **{}** konumuna sarıldı.", format_duration(target))).await?;
            ctx.data().music.refresh_card_now(&ctx.serenity_context().http, guild_id).await;
        }
        other => {
            tracing::warn!(guild = %guild_id, result = ?other, "Sarılamadı");
            ctx.say("❌ Bu şarkı sarılamadı (kaynak desteklemiyor olabilir).").await?;
        }
    }
    Ok(())
}

/// Kuyruktan bir şarkıyı çıkarır.
#[poise::command(slash_command, guild_only, rename = "sil")]
async fn m_sil(
    ctx: Context<'_>,
    #[rename = "sıra"]
    #[description = "Kuyruktaki sırası (/müzik sıra listesindeki numara)"]
    #[min = 1]
    index: u32,
) -> Result<(), Error> {
    let Some((guild_id, _)) = same_channel_call(ctx).await? else {
        return Ok(());
    };
    match ctx.data().music.remove_at(guild_id, usize::try_from(index)?).await {
        Some(info) => {
            ctx.say(format!("🗑️ Kuyruktan çıkarıldı: **{}**", info.title)).await?;
        }
        None => return private(ctx, "Kuyrukta bu numarada şarkı yok. `/müzik sıra` ile bakabilirsin.").await,
    }
    Ok(())
}

/// Kuyruktaki bir şarkının sırasını değiştirir.
#[poise::command(slash_command, guild_only, rename = "taşı")]
async fn m_tasi(
    ctx: Context<'_>,
    #[rename = "nereden"]
    #[description = "Şarkının şimdiki sırası"]
    #[min = 1]
    from: u32,
    #[rename = "nereye"]
    #[description = "Yeni sırası (1 = sıradaki)"]
    #[min = 1]
    to: u32,
) -> Result<(), Error> {
    let Some((guild_id, _)) = same_channel_call(ctx).await? else {
        return Ok(());
    };
    match ctx.data().music.move_track(guild_id, usize::try_from(from)?, usize::try_from(to)?).await {
        Some(title) => {
            ctx.say(format!("↕️ **{title}** artık {to}. sırada.")).await?;
        }
        None => return private(ctx, "Bu numaralar kuyrukta yok. `/müzik sıra` ile bakabilirsin.").await,
    }
    Ok(())
}

// ---------------------------------------------------------------- yardımcılar

fn guild_id(ctx: Context<'_>) -> Result<GuildId, Error> {
    ctx.guild_id()
        .ok_or_else(|| anyhow!("guild_only komut sunucu dışında çalıştı"))
}

fn voice_channel_of(ctx: Context<'_>, guild_id: GuildId, user: serenity::UserId) -> Option<ChannelId> {
    ctx.serenity_context()
        .cache
        .guild(guild_id)?
        .voice_states
        .get(&user)?
        .channel_id
}

/// Bot bir ses kanalındaysa ve kullanıcı da aynı kanaldaysa çağrıyı döner;
/// değilse kullanıcıya açıklama yazıp `None` döner.
async fn same_channel_call(ctx: Context<'_>) -> Result<Option<(GuildId, Arc<Mutex<Call>>)>, Error> {
    let guild_id = guild_id(ctx)?;
    let Some(call) = ctx.data().music.songbird().get(guild_id) else {
        private(ctx, "Şu an bir ses kanalında değilim.").await?;
        return Ok(None);
    };
    let bot_channel = call.lock().await.current_channel().map(|c| ChannelId::new(c.0.get()));
    let user_channel = voice_channel_of(ctx, guild_id, ctx.author().id);
    match (bot_channel, user_channel) {
        (Some(bot), Some(user)) if bot == user => {
            if !dj_allowed(ctx, guild_id, bot).await {
                private(ctx, "🎧 Bu sunucuda müziği yalnızca DJ rolündekiler yönetebilir. (Kanalda tek başınaysan sen de yönetebilirsin.)").await?;
                return Ok(None);
            }
            Ok(Some((guild_id, call)))
        }
        (Some(bot), _) => {
            private(ctx, &format!("🎧 Bunu yapmak için {} kanalında olmalısın.", bot.mention())).await?;
            Ok(None)
        }
        (None, _) => {
            private(ctx, "Şu an bir ses kanalında değilim.").await?;
            Ok(None)
        }
    }
}

/// DJ rolü ayarlıysa: rolü olanlar, sunucuyu yönetenler ve kanalda botla baş başa kalan üye müziği yönetebilir.
async fn dj_allowed(ctx: Context<'_>, guild_id: GuildId, channel: ChannelId) -> bool {
    let data = ctx.data();
    let Some(dj_role) = data.settings.get::<crate::music::playlists::MusicExtras>(&data.db, guild_id).await.ok().and_then(|s| s.dj_role) else {
        return true;
    };
    let author = ctx.author().id;
    // Önbellek kilidi `await`'ten önce bırakılır.
    ctx.serenity_context().cache.guild(guild_id).is_none_or(|guild| {
        let Some(member) = guild.members.get(&author) else { return false };
        let manages = guild.owner_id == author || guild.member_permissions(member).intersects(serenity::Permissions::ADMINISTRATOR | serenity::Permissions::MANAGE_GUILD);
        let listeners = guild
            .voice_states
            .values()
            .filter(|v| v.channel_id == Some(channel) && guild.members.get(&v.user_id).is_some_and(|m| !m.user.bot))
            .count();
        manages || member.roles.iter().any(|r| r.get() == dj_role) || listeners <= 1
    })
}

/// Yazdığını ses kanalında sesli okur (mikrofonu olmayanlar için).
#[poise::command(slash_command, guild_only, rename = "söyle", user_cooldown = 8)]
async fn m_soyle(ctx: Context<'_>, #[description = "Okunacak metin (en fazla 200 karakter)"] #[max_length = 200] metin: String) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let Some(voice_channel) = voice_channel_of(ctx, guild_id, ctx.author().id) else {
        return private(ctx, "🔊 Önce bir ses kanalına gir.").await;
    };
    let data = ctx.data();
    // Sesli okunacağı için sunucunun kelime filtresinden geçer.
    if data.automod.has_profanity(&data.db, &data.redis, guild_id, &metin).await {
        return private(ctx, "Bu metin sunucunun yasaklı kelimelerini içeriyor.").await;
    }
    let name = ctx.author().global_name.clone().unwrap_or_else(|| ctx.author().name.clone());
    // Kimin yazdığı belli olsun diye adıyla birlikte okunur.
    let spoken = format!("{name} diyor ki: {}", metin.trim());
    let Some(input) = data.music.tts_input(&spoken) else {
        return private(ctx, "Okunacak bir metin yaz.").await;
    };
    ctx.defer_ephemeral().await?;
    let info = TrackInfo {
        guild_id,
        title: format!("🗣️ {name}: {}", metin.trim()),
        url: None,
        duration: None,
        thumbnail: None,
        requester: ctx.author().id,
        announce: false,
        spotify_url: None,
    };
    match data.music.enqueue(Arc::clone(&ctx.serenity_context().http), guild_id, voice_channel, ctx.channel_id(), input, info).await {
        Ok(enqueued) if enqueued.position == 0 => private(ctx, "🗣️ Okunuyor.").await,
        Ok(enqueued) => private(ctx, &format!("🗣️ Sıraya eklendi ({}. sırada); çalan şarkılardan sonra okunacak.", enqueued.position)).await,
        Err(e) => {
            tracing::warn!(guild = %guild_id, error = ?e, "Ses kanalına katılınamadı");
            private(ctx, "❌ Ses kanalına katılamadım. Kanalda **Bağlan** ve **Konuş** iznim olduğundan emin ol.").await
        }
    }
}

/// DJ rolünü ayarlar: ayarlıysa geç / durdur / ses gibi komutları yalnızca o roldekiler kullanır.
#[poise::command(slash_command, guild_only, rename = "dj-rol", required_permissions = "MANAGE_GUILD")]
async fn m_dj(ctx: Context<'_>, #[description = "DJ rolü (boş bırakırsan kısıtlama kalkar)"] rol: Option<serenity::Role>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let text = match &rol {
        Some(role) => format!("🎧 DJ rolü: **{}**. Müziği artık bu roldekiler, yöneticiler ve kanalda tek başına olan üye yönetebilir; şarkı eklemek herkese açık.", role.name),
        None => "🎧 DJ kısıtlaması kaldırıldı; müziği kanaldaki herkes yönetebilir.".to_owned(),
    };
    data.settings.set(&data.db, guild_id, crate::music::playlists::MusicExtras { dj_role: rol.map(|r| r.id.get()) }).await?;
    private(ctx, &text).await
}

/// Şu anki sırayı kendi adına bir çalma listesi olarak kaydeder.
#[poise::command(slash_command, guild_only, rename = "liste-kaydet")]
async fn m_liste_kaydet(ctx: Context<'_>, #[description = "Listenin adı"] #[max_length = 40] ad: String) -> Result<(), Error> {
    use crate::music::playlists::{self, SavedTrack};
    let guild_id = guild_id(ctx)?;
    let Some(name) = playlists::normalize_name(&ad) else {
        return private(ctx, "Liste adı 1-40 karakter olmalı.").await;
    };
    let Some(call) = ctx.data().music.songbird().get(guild_id) else {
        return private(ctx, "Kaydedilecek bir sıra yok; önce `/çal` ile şarkı ekle.").await;
    };
    let handles = call.lock().await.queue().current_queue();
    let music = &ctx.data().music;
    let tracks: Vec<SavedTrack> = handles.iter().filter_map(|h| music.track_info(h.uuid())).take(playlists::MAX_TRACKS).map(|i| SavedTrack { title: i.title, url: i.url }).collect();
    if tracks.is_empty() {
        return private(ctx, "Kaydedilecek bir sıra yok; önce `/çal` ile şarkı ekle.").await;
    }
    let db = &ctx.data().db;
    let exists = playlists::load(db, ctx.author().id, &name).await?.is_some();
    if !exists && playlists::count(db, ctx.author().id).await? >= playlists::MAX_LISTS {
        return private(ctx, &format!("En fazla {} listen olabilir; `/müzik liste-sil` ile birini kaldır.", playlists::MAX_LISTS)).await;
    }
    playlists::save(db, ctx.author().id, &name, &tracks).await?;
    private(ctx, &format!("💾 **{name}** listesi {} şarkıyla kaydedildi. Çalmak için: `/müzik liste-çal ad:{name}`", tracks.len())).await
}

/// Kayıtlı çalma listeni sıraya ekler.
#[poise::command(slash_command, guild_only, rename = "liste-çal")]
async fn m_liste_cal(ctx: Context<'_>, #[description = "Listenin adı"] #[max_length = 40] ad: String) -> Result<(), Error> {
    use crate::music::playlists;
    let guild_id = guild_id(ctx)?;
    let Some(voice_channel) = voice_channel_of(ctx, guild_id, ctx.author().id) else {
        return private(ctx, "🔊 Önce bir ses kanalına gir.").await;
    };
    let Some(name) = playlists::normalize_name(&ad) else {
        return private(ctx, "Liste adı 1-40 karakter olmalı.").await;
    };
    let Some(tracks) = playlists::load(&ctx.data().db, ctx.author().id, &name).await? else {
        return private(ctx, "Bu adda bir listen yok. `/müzik listelerim` ile bakabilirsin.").await;
    };
    ctx.defer().await?;
    let music = &ctx.data().music;
    let count = tracks.len();
    let items = music.saved_inputs(tracks, ctx.author().id, guild_id);
    if let Err(e) = music.enqueue_many(Arc::clone(&ctx.serenity_context().http), guild_id, voice_channel, ctx.channel_id(), items).await {
        tracing::warn!(guild = %guild_id, error = ?e, "Ses kanalına katılınamadı");
        ctx.say("❌ Ses kanalına katılamadım. Kanalda **Bağlan** ve **Konuş** iznim olduğundan emin ol.").await?;
        return Ok(());
    }
    ctx.say(format!("📜 **{name}** listesinden **{count}** şarkı sıraya eklendi.")).await?;
    send_card(ctx, guild_id).await
}

/// Kayıtlı çalma listelerini gösterir.
#[poise::command(slash_command, guild_only, rename = "listelerim")]
async fn m_listelerim(ctx: Context<'_>) -> Result<(), Error> {
    let lists = crate::music::playlists::list(&ctx.data().db, ctx.author().id).await?;
    if lists.is_empty() {
        return private(ctx, "Kayıtlı listen yok. Sırada şarkı varken `/müzik liste-kaydet` ile kaydedebilirsin.").await;
    }
    let lines: Vec<String> = lists.iter().map(|(name, count)| format!("📜 **{name}** — {count} şarkı")).collect();
    private(ctx, &lines.join("\n")).await
}

/// Kayıtlı bir çalma listeni siler.
#[poise::command(slash_command, guild_only, rename = "liste-sil")]
async fn m_liste_sil(ctx: Context<'_>, #[description = "Listenin adı"] #[max_length = 40] ad: String) -> Result<(), Error> {
    let name = crate::music::playlists::normalize_name(&ad).unwrap_or_default();
    let deleted = crate::music::playlists::delete(&ctx.data().db, ctx.author().id, &name).await?;
    private(ctx, if deleted { "🗑️ Liste silindi." } else { "Bu adda bir listen yok." }).await
}

fn title_of(ctx: Context<'_>, id: uuid::Uuid) -> String {
    ctx.data()
        .music
        .track_info(id)
        .map_or_else(|| "şarkı".to_owned(), |i| i.title)
}

async fn private(ctx: Context<'_>, text: &str) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(text).ephemeral(true))
        .await?;
    Ok(())
}
