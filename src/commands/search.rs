//! Arama komutları (`/ara youtube · twitch · anime · manga · pokemon · sözlük`) ve emoji araçları
//! (`/emoji ekle · büyüt · sil`).
//!
//! Dış servisler sabit, herkese açık API'lerdir; kullanıcının verdiği metin yalnızca sorgu
//! parametresi olarak gider. Kullanıcının verdiği *adresten* dosya yalnızca `/emoji ekle`de indirilir
//! ve o da `cards::fetch_bytes` ile (https + herkese açık adres + boyut sınırı) yapılır.

use std::time::Duration;

use anyhow::Context as _;
use poise::serenity_prelude::{self as serenity, CreateAttachment, CreateEmbed, CreateEmbedFooter, Mentionable};
use poise::CreateReply;
use serde_json::Value;

use super::moderation::reply_private;
use crate::data::Context;
use crate::error::Error;

const TIMEOUT: Duration = Duration::from_secs(10);
/// Discord'un emoji dosyası sınırı.
const EMOJI_MAX_BYTES: usize = 256 * 1024;

/// YouTube, Twitch, anime, manga, Pokémon ve sözlük araması.
#[poise::command(
    slash_command,
    rename = "ara",
    subcommands("ara_youtube", "ara_twitch", "ara_anime", "ara_manga", "ara_pokemon", "ara_sozluk", "ara_imgur", "ara_kripto", "ara_gaz"),
    subcommand_required,
    category = "Araçlar"
)]
pub async fn ara(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

async fn get_json(ctx: Context<'_>, url: &str, query: &[(&str, &str)]) -> anyhow::Result<Option<Value>> {
    let response = ctx
        .data()
        .http_client
        .get(url)
        .query(query)
        .header("Accept", "application/vnd.api+json, application/json")
        .timeout(TIMEOUT)
        .send()
        .await
        .context("arama servisine ulaşılamadı")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response.error_for_status().context("arama servisi hata döndü")?;
    Ok(Some(response.json().await.context("arama cevabı okunamadı")?))
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        text.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// YouTube'da video arar.
#[poise::command(slash_command, rename = "youtube")]
async fn ara_youtube(ctx: Context<'_>, #[description = "Aranacak video"] #[max_length = 150] sorgu: String) -> Result<(), Error> {
    ctx.defer().await?;
    let results = match ctx.data().music.search_youtube(&sorgu, 5).await {
        Ok(results) => results,
        Err(message) => return reply_private(ctx, message).await,
    };
    if results.is_empty() {
        return reply_private(ctx, "Sonuç bulunamadı.").await;
    }
    let lines: Vec<String> = results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let length = r.duration.map(|d| format!(" · {}", crate::music::render::clock(d))).unwrap_or_default();
            let channel = r.channel.as_deref().map(|c| format!(" — {}", clip(c, 40))).unwrap_or_default();
            format!("**{}.** [{}]({}){channel}{length}", i + 1, clip(&r.title.replace(['[', ']'], ""), 80), r.url)
        })
        .collect();
    let embed = CreateEmbed::new()
        .title(format!("▶️ YouTube: {}", clip(&sorgu, 100)))
        .description(lines.join("\n"))
        .colour(serenity::Colour::RED);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Bir Twitch yayıncısını arar.
#[poise::command(slash_command, rename = "twitch")]
async fn ara_twitch(ctx: Context<'_>, #[description = "Kanal adı"] #[max_length = 25] kanal: String) -> Result<(), Error> {
    let login = kanal.trim().trim_start_matches('@').to_lowercase();
    if login.is_empty() || !login.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return reply_private(ctx, "Geçerli bir Twitch kanal adı yaz (harf, rakam ve alt çizgi).").await;
    }
    ctx.defer().await?;
    let data = ctx.data();
    let profile = match data.streams.twitch_profile(&login).await {
        Ok(Some(profile)) => profile,
        Ok(None) => return reply_private(ctx, "Böyle bir Twitch kanalı bulunamadı.").await,
        Err(e) => {
            tracing::debug!(error = ?e, "Twitch araması yapılamadı");
            return reply_private(ctx, "Twitch araması şu an yapılamıyor (Twitch anahtarları `.env` dosyasında tanımlı olmalı).").await;
        }
    };
    let mut embed = CreateEmbed::new()
        .title(format!("🟣 {}", profile.display_name))
        .url(format!("https://twitch.tv/{login}"))
        .colour(serenity::Colour::from_rgb(145, 70, 255));
    if !profile.description.trim().is_empty() {
        embed = embed.description(clip(&profile.description, 500));
    }
    if !profile.profile_image_url.is_empty() {
        embed = embed.thumbnail(profile.profile_image_url);
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

async fn kitsu(ctx: Context<'_>, kind: &str, query: &str, emoji: &str) -> Result<(), Error> {
    ctx.defer().await?;
    let url = format!("https://kitsu.io/api/edge/{kind}");
    let found = get_json(ctx, &url, &[("filter[text]", query), ("page[limit]", "1")]).await;
    let Some(item) = found.ok().flatten().and_then(|v| v.pointer("/data/0").cloned()) else {
        return reply_private(ctx, "Sonuç bulunamadı ya da arama servisine ulaşılamadı.").await;
    };
    let title = text(&item, "/attributes/canonicalTitle").unwrap_or("Adsız");
    let slug = text(&item, "/attributes/slug").unwrap_or_default();
    let mut embed = CreateEmbed::new()
        .title(format!("{emoji} {}", clip(title, 200)))
        .url(format!("https://kitsu.io/{kind}/{slug}"))
        .colour(serenity::Colour::from_rgb(253, 117, 91))
        .footer(CreateEmbedFooter::new("Kaynak: Kitsu"));
    if let Some(synopsis) = text(&item, "/attributes/synopsis") {
        embed = embed.description(clip(synopsis, 700));
    }
    if let Some(rating) = text(&item, "/attributes/averageRating") {
        embed = embed.field("Puan", format!("{rating} / 100"), true);
    }
    if let Some(status) = text(&item, "/attributes/status") {
        let status = match status {
            "finished" => "Bitti",
            "current" => "Devam ediyor",
            "upcoming" | "unreleased" | "tba" => "Yakında",
            other => other,
        };
        embed = embed.field("Durum", status, true);
    }
    let count = item
        .pointer(if kind == "anime" { "/attributes/episodeCount" } else { "/attributes/chapterCount" })
        .and_then(Value::as_u64);
    if let Some(count) = count {
        embed = embed.field(if kind == "anime" { "Bölüm" } else { "Kısım" }, count.to_string(), true);
    }
    if let Some(start) = text(&item, "/attributes/startDate") {
        embed = embed.field("Başlangıç", start, true);
    }
    if let Some(poster) = text(&item, "/attributes/posterImage/medium") {
        embed = embed.thumbnail(poster);
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Bir animeyi arar.
#[poise::command(slash_command, rename = "anime")]
async fn ara_anime(ctx: Context<'_>, #[description = "Anime adı"] #[max_length = 100] ad: String) -> Result<(), Error> {
    kitsu(ctx, "anime", &ad, "🎬").await
}

/// Bir mangayı arar.
#[poise::command(slash_command, rename = "manga")]
async fn ara_manga(ctx: Context<'_>, #[description = "Manga adı"] #[max_length = 100] ad: String) -> Result<(), Error> {
    kitsu(ctx, "manga", &ad, "📖").await
}

/// Kripto paranın fiyatı ve 24 saatlik değişimi (CoinGecko, anahtar gerekmez).
#[poise::command(slash_command, rename = "kripto")]
async fn ara_kripto(ctx: Context<'_>, #[description = "Para adı ya da kısaltması (ör. bitcoin, eth)"] #[max_length = 40] para: String) -> Result<(), Error> {
    ctx.defer().await?;
    // Ad / kısaltma → CoinGecko kimliği (en üstteki eşleşme).
    let found = get_json(ctx, "https://api.coingecko.com/api/v3/search", &[("query", para.trim())]).await.ok().flatten();
    let Some(coin) = found.as_ref().and_then(|f| f.pointer("/coins/0")) else {
        return reply_private(ctx, "Böyle bir kripto para bulunamadı.").await;
    };
    let id = text(coin, "/id").unwrap_or_default();
    let query = [("ids", id), ("vs_currencies", "usd,try"), ("include_24hr_change", "true"), ("include_market_cap", "true")];
    let Some(prices) = get_json(ctx, "https://api.coingecko.com/api/v3/simple/price", &query).await?.and_then(|p| p.get(id).cloned()) else {
        return reply_private(ctx, "Fiyat şu an alınamadı, biraz sonra tekrar dene.").await;
    };
    let number = |key: &str| prices.get(key).and_then(Value::as_f64);
    let change = number("usd_24h_change").unwrap_or(0.0);
    let mut embed = CreateEmbed::new()
        .title(format!("{} ({})", text(coin, "/name").unwrap_or(id), text(coin, "/symbol").unwrap_or_default().to_uppercase()))
        .url(format!("https://www.coingecko.com/en/coins/{id}"))
        .colour(if change >= 0.0 { serenity::Colour::DARK_GREEN } else { serenity::Colour::RED })
        .field("Fiyat", format!("${}", crate::statchannels::format_price(number("usd").unwrap_or(0.0))), true)
        .field("TL", format!("₺{}", crate::statchannels::format_price(number("try").unwrap_or(0.0))), true)
        .field("24 saat", format!("{} %{:.2}", if change >= 0.0 { "📈" } else { "📉" }, change), true)
        .footer(CreateEmbedFooter::new("CoinGecko"));
    if let Some(cap) = number("usd_market_cap").filter(|c| *c > 0.0) {
        embed = embed.field("Piyasa değeri", format!("${}", crate::statchannels::format_price(cap.round())), true);
    }
    if let Some(image) = text(coin, "/large") {
        embed = embed.thumbnail(image);
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Ethereum ağındaki güncel işlem ücreti (gas fiyatı).
#[poise::command(slash_command, rename = "gaz")]
async fn ara_gaz(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;
    let request = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_gasPrice", "params": [] });
    let answer: anyhow::Result<Value> = async {
        Ok(ctx.data().http_client.post("https://ethereum-rpc.publicnode.com").json(&request).timeout(TIMEOUT).send().await?.error_for_status()?.json().await?)
    }
    .await;
    let Some(gwei) = answer.ok().as_ref().and_then(|a| text(a, "/result")).and_then(gwei) else {
        return reply_private(ctx, "Ethereum gas fiyatı şu an alınamadı, biraz sonra tekrar dene.").await;
    };
    let embed = CreateEmbed::new()
        .title("⛽ Ethereum gas fiyatı")
        .description(format!("**{gwei:.2} gwei**\n\nBasit bir ETH transferi (21.000 gas) ≈ **{:.6} ETH**", gwei * 21_000.0 / 1e9))
        .colour(serenity::Colour::from_rgb(98, 126, 234))
        .footer(CreateEmbedFooter::new("Ethereum ana ağı · publicnode.com"));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// JSON-RPC'nin "0x…" wei değerini gwei'ye çevirir.
fn gwei(hex: &str) -> Option<f64> {
    u128::from_str_radix(hex.strip_prefix("0x")?, 16).ok().map(|wei| wei as f64 / 1e9)
}

/// Bir Pokémon'u arar.
#[poise::command(slash_command, rename = "pokemon")]
async fn ara_pokemon(ctx: Context<'_>, #[description = "Pokémon adı ya da numarası"] #[max_length = 40] ad: String) -> Result<(), Error> {
    let name = ad.trim().to_lowercase().replace(' ', "-");
    // Ad adrese yazıldığı için yalnızca güvenli karakterler kabul edilir.
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return reply_private(ctx, "Pokémon adını İngilizce harflerle yaz (ör. `pikachu`) ya da numarasını ver.").await;
    }
    ctx.defer().await?;
    let Some(pokemon) = get_json(ctx, &format!("https://pokeapi.co/api/v2/pokemon/{name}"), &[]).await.ok().flatten() else {
        return reply_private(ctx, "Böyle bir Pokémon bulunamadı.").await;
    };
    let list = |pointer: &str, inner: &str| {
        pokemon
            .pointer(pointer)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|i| text(i, inner)).collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    };
    let number = pokemon.get("id").and_then(Value::as_u64).unwrap_or(0);
    let stats = pokemon
        .pointer("/stats")
        .and_then(Value::as_array)
        .map(|stats| {
            stats
                .iter()
                .filter_map(|s| Some(format!("{}: **{}**", text(s, "/stat/name")?, s.get("base_stat")?.as_u64()?)))
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_default();
    let mut embed = CreateEmbed::new()
        .title(format!("#{number} {}", text(&pokemon, "/name").unwrap_or(&name)))
        .colour(serenity::Colour::from_rgb(255, 203, 5))
        .field("Tür", list("/types", "/type/name"), true)
        .field("Yetenekler", list("/abilities", "/ability/name"), true)
        .field(
            "Boy / Ağırlık",
            format!(
                "{:.1} m / {:.1} kg",
                pokemon.get("height").and_then(Value::as_f64).unwrap_or(0.0) / 10.0,
                pokemon.get("weight").and_then(Value::as_f64).unwrap_or(0.0) / 10.0
            ),
            true,
        )
        .footer(CreateEmbedFooter::new("Kaynak: PokéAPI"));
    if !stats.is_empty() {
        embed = embed.field("Temel değerler", stats, false);
    }
    if let Some(sprite) = text(&pokemon, "/sprites/other/official-artwork/front_default").or_else(|| text(&pokemon, "/sprites/front_default")) {
        embed = embed.thumbnail(sprite);
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Urban Dictionary'de (İngilizce argo sözlüğü) bir terim arar.
#[poise::command(slash_command, rename = "sözlük")]
async fn ara_sozluk(ctx: Context<'_>, #[description = "Aranacak terim (İngilizce)"] #[max_length = 80] terim: String) -> Result<(), Error> {
    ctx.defer().await?;
    let found = get_json(ctx, "https://api.urbandictionary.com/v0/define", &[("term", terim.trim())]).await;
    let Some(entry) = found.ok().flatten().and_then(|v| v.pointer("/list/0").cloned()) else {
        return reply_private(ctx, "Bu terim için tanım bulunamadı.").await;
    };
    // Urban Dictionary bağlantı yapılacak kelimeleri köşeli parantezle işaretler.
    let clean = |s: &str| s.replace(['[', ']'], "");
    let mut embed = CreateEmbed::new()
        .title(format!("📚 {}", clip(text(&entry, "/word").unwrap_or(&terim), 200)))
        .description(clip(&clean(text(&entry, "/definition").unwrap_or_default()), 1500))
        .colour(serenity::Colour::from_rgb(29, 36, 57))
        .footer(CreateEmbedFooter::new("Kaynak: Urban Dictionary (kullanıcıların yazdığı tanımlar; kaba olabilir)"));
    if let Some(link) = text(&entry, "/permalink") {
        embed = embed.url(link);
    }
    if let Some(example) = text(&entry, "/example") {
        embed = embed.field("Örnek", clip(&clean(example), 600), false);
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Imgur galeri cevabından yetişkin içerik olmayan ilk resimleri seçer: (başlık, sayfa, resim).
fn imgur_results(json: &Value, limit: usize) -> Vec<(String, String, Option<String>)> {
    json.get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("nsfw").and_then(Value::as_bool) != Some(true))
                .filter_map(|item| {
                    let page = text(item, "/link").filter(|l| l.starts_with("https://"))?;
                    // Albümlerde resim `images` içindedir; tekil gönderide bağlantının kendisi resimdir.
                    let image = text(item, "/images/0/link")
                        .or_else(|| (item.get("is_album").and_then(Value::as_bool) != Some(true)).then_some(page))
                        .filter(|l| l.starts_with("https://i.imgur.com/") && !l.ends_with(".mp4"));
                    Some((text(item, "/title").unwrap_or("Başlıksız").to_owned(), page.to_owned(), image.map(str::to_owned)))
                })
                .take(limit)
                .collect()
        })
        .unwrap_or_default()
}

/// Imgur'da resim arar.
#[poise::command(slash_command, rename = "imgur")]
async fn ara_imgur(ctx: Context<'_>, #[description = "Aranacak resim"] #[max_length = 100] sorgu: String) -> Result<(), Error> {
    let Some(client_id) = crate::integrations::get().imgur_client_id.as_deref() else {
        return reply_private(ctx, "Imgur araması için `.env` dosyasında `IMGUR_CLIENT_ID` tanımlı olmalı (imgur.com → Settings → Applications).").await;
    };
    ctx.defer().await?;
    let response = ctx
        .data()
        .http_client
        .get("https://api.imgur.com/3/gallery/search/top/all/0")
        .query(&[("q", sorgu.trim())])
        .header("Authorization", format!("Client-ID {client_id}"))
        .timeout(TIMEOUT)
        .send()
        .await;
    let json: Option<Value> = match response {
        Ok(response) if response.status().is_success() => response.json().await.ok(),
        Ok(response) => {
            tracing::debug!(status = %response.status(), "Imgur araması reddedildi");
            None
        }
        Err(_) => None,
    };
    let Some(json) = json else {
        return reply_private(ctx, "Imgur araması şu an yapılamıyor (anahtar geçersiz olabilir ya da servis yanıt vermiyor).").await;
    };
    let results = imgur_results(&json, 5);
    let Some((title, page, image)) = results.first() else {
        return reply_private(ctx, "Sonuç bulunamadı.").await;
    };
    let mut embed = CreateEmbed::new()
        .title(format!("🖼️ {}", clip(title, 200)))
        .url(page)
        .colour(serenity::Colour::from_rgb(27, 183, 110))
        .footer(CreateEmbedFooter::new("Kaynak: Imgur"));
    if let Some(image) = image {
        embed = embed.image(image);
    }
    let others: Vec<String> = results.iter().skip(1).map(|(title, page, _)| format!("[{}]({page})", clip(&title.replace(['[', ']'], ""), 60))).collect();
    if !others.is_empty() {
        embed = embed.field("Diğer sonuçlar", others.join("\n"), false);
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

// ---------------------------------------------------------------- Emoji

/// `<:ad:kimlik>` ya da `<a:ad:kimlik>` → (ad, kimlik, hareketli mi).
pub fn parse_custom_emoji(raw: &str) -> Option<(String, u64, bool)> {
    let inner = raw.trim().strip_prefix('<')?.strip_suffix('>')?;
    let (animated, rest) = match inner.strip_prefix("a:") {
        Some(rest) => (true, rest),
        None => (false, inner.strip_prefix(':')?),
    };
    let (name, id) = rest.split_once(':')?;
    let valid_name = (2..=32).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let id = id.parse::<u64>().ok().filter(|id| *id > 0)?;
    valid_name.then(|| (name.to_owned(), id, animated))
}

/// Discord emoji adı kuralı: 2-32 karakter; harf, rakam ve alt çizgi.
pub fn valid_emoji_name(name: &str) -> bool {
    (2..=32).contains(&name.chars().count()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Sunucu emojileri: ekle, büyüt, sil.
#[poise::command(slash_command, guild_only, rename = "emoji", subcommands("emoji_ekle", "emoji_buyut", "emoji_sil"), subcommand_required, category = "Araçlar")]
pub async fn emoji(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Başka bir sunucunun emojisini ya da bir resim bağlantısını bu sunucuya emoji olarak ekler.
#[poise::command(
    slash_command,
    guild_only,
    rename = "ekle",
    required_permissions = "MANAGE_GUILD_EXPRESSIONS",
    required_bot_permissions = "MANAGE_GUILD_EXPRESSIONS"
)]
async fn emoji_ekle(
    ctx: Context<'_>,
    #[description = "Emojinin kendisi (başka sunucudan) ya da https resim bağlantısı"]
    #[max_length = 300]
    kaynak: String,
    #[description = "Emojinin adı (bağlantı için zorunlu; emoji için boşsa kendi adı)"]
    #[max_length = 32]
    ad: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let (url, default_name, file) = match parse_custom_emoji(&kaynak) {
        Some((name, id, animated)) => {
            let ext = if animated { "gif" } else { "png" };
            (format!("https://cdn.discordapp.com/emojis/{id}.{ext}"), Some(name), format!("emoji.{ext}"))
        }
        None if kaynak.trim().starts_with("https://") => {
            let path = kaynak.trim().split('?').next().unwrap_or_default().to_lowercase();
            let ext = ["png", "jpg", "jpeg", "gif", "webp"].into_iter().find(|e| path.ends_with(&format!(".{e}"))).unwrap_or("png");
            (kaynak.trim().to_owned(), None, format!("emoji.{ext}"))
        }
        None => return reply_private(ctx, "Bir sunucu emojisi (ör. `:emoji:` olarak seçtiğin) ya da `https://` ile başlayan bir resim bağlantısı ver.").await,
    };
    let Some(name) = ad.or(default_name) else {
        return reply_private(ctx, "Bağlantıdan eklerken `ad` da yazmalısın.").await;
    };
    if !valid_emoji_name(&name) {
        return reply_private(ctx, "Emoji adı 2-32 karakter olmalı; sadece İngilizce harf, rakam ve alt çizgi.").await;
    }
    ctx.defer_ephemeral().await?;
    let Some(bytes) = crate::cards::fetch_bytes(&url, EMOJI_MAX_BYTES).await else {
        return reply_private(ctx, "❌ Resim indirilemedi. Herkese açık bir https bağlantısı olmalı ve 256 KB'ı geçmemeli.").await;
    };
    let image = CreateAttachment::bytes(bytes, file).to_base64();
    match guild_id.create_emoji(ctx.http(), &name, &image).await {
        Ok(created) => reply_private(ctx, format!("✅ Emoji eklendi: {created} (`:{}:`)", created.name)).await,
        Err(e) => {
            tracing::debug!(guild = %guild_id, error = %e, "Emoji eklenemedi");
            reply_private(ctx, "❌ Emoji eklenemedi. Sunucunun emoji yuvası dolmuş olabilir ya da dosya uygun değil (PNG, JPEG, GIF; en fazla 256 KB).").await
        }
    }
}

/// Bir sunucu emojisini büyük gösterir.
#[poise::command(slash_command, rename = "büyüt")]
async fn emoji_buyut(ctx: Context<'_>, #[description = "Büyütülecek sunucu emojisi"] #[max_length = 80] emoji: String) -> Result<(), Error> {
    let Some((name, id, animated)) = parse_custom_emoji(&emoji) else {
        return reply_private(ctx, "Bu komut sunucu emojileriyle çalışır (standart emojiler büyütülemez).").await;
    };
    let url = format!("https://cdn.discordapp.com/emojis/{id}.{}?size=512", if animated { "gif" } else { "png" });
    let embed = CreateEmbed::new().title(format!(":{name}:")).image(url).colour(serenity::Colour::BLURPLE);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Bu sunucunun bir emojisini siler.
#[poise::command(
    slash_command,
    guild_only,
    rename = "sil",
    required_permissions = "MANAGE_GUILD_EXPRESSIONS",
    required_bot_permissions = "MANAGE_GUILD_EXPRESSIONS"
)]
async fn emoji_sil(ctx: Context<'_>, #[description = "Silinecek sunucu emojisi"] #[max_length = 80] emoji: String) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some((name, id, _)) = parse_custom_emoji(&emoji) else {
        return reply_private(ctx, "Silinecek sunucu emojisini seç.").await;
    };
    let emoji_id = serenity::EmojiId::new(id);
    // Yalnızca bu sunucunun emojisi silinebilir.
    if !ctx.guild().is_some_and(|g| g.emojis.contains_key(&emoji_id)) {
        return reply_private(ctx, "Bu emoji bu sunucuya ait değil.").await;
    }
    match guild_id.delete_emoji(ctx.http(), emoji_id).await {
        Ok(()) => reply_private(ctx, format!("🗑️ `:{name}:` silindi ({} tarafından).", ctx.author().id.mention())).await,
        Err(_) => reply_private(ctx, "❌ Emoji silinemedi; botun 'İfadeleri Yönet' izni olmalı.").await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_wei_to_gwei() {
        assert_eq!(gwei("0x3b9aca00"), Some(1.0));
        assert_eq!(gwei("0x0"), Some(0.0));
        assert_eq!(gwei("123"), None);
        assert_eq!(gwei("0xzz"), None);
    }

    #[test]
    fn parses_custom_emojis() {
        assert_eq!(parse_custom_emoji("<:pepe:123456789012345678>"), Some(("pepe".to_owned(), 123_456_789_012_345_678, false)));
        assert_eq!(parse_custom_emoji(" <a:dans_1:42> "), Some(("dans_1".to_owned(), 42, true)));
        for bad in ["😀", ":pepe:", "<:p:1>", "<:pepe:abc>", "<:pe pe:12>", "<@123>", "<:pepe:0>", ""] {
            assert_eq!(parse_custom_emoji(bad), None, "{bad}");
        }
    }

    #[test]
    fn imgur_skips_adult_and_reads_albums() {
        let json = serde_json::json!({ "data": [
            { "title": "Yetişkin", "link": "https://imgur.com/a/x", "nsfw": true },
            { "title": "Albüm", "link": "https://imgur.com/a/abc", "is_album": true, "nsfw": false, "images": [{ "link": "https://i.imgur.com/1.jpg" }] },
            { "title": "Tek", "link": "https://i.imgur.com/2.png", "is_album": false },
            { "title": "Video", "link": "https://i.imgur.com/3.mp4", "is_album": false },
            { "title": "Bağlantısız" },
        ]});
        let got = imgur_results(&json, 5);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], ("Albüm".to_owned(), "https://imgur.com/a/abc".to_owned(), Some("https://i.imgur.com/1.jpg".to_owned())));
        assert_eq!(got[1].2.as_deref(), Some("https://i.imgur.com/2.png"));
        assert_eq!(got[2].2, None);
        assert!(imgur_results(&serde_json::json!({}), 5).is_empty());
    }

    #[test]
    fn emoji_names() {
        assert!(valid_emoji_name("sunucu_emoji1"));
        assert!(!valid_emoji_name("a"));
        assert!(!valid_emoji_name("çiçek"));
        assert!(!valid_emoji_name("iki kelime"));
        assert!(!valid_emoji_name(&"a".repeat(33)));
    }

    #[test]
    fn clips_long_text() {
        assert_eq!(clip("kısa", 10), "kısa");
        assert_eq!(clip("abcdefghij", 5), "abcd…");
    }
}
