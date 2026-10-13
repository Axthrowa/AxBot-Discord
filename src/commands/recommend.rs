//! `/tavsiye`: feelm.org'dan film, dizi ve oyun önerisi (bkz. `crate::feelm`).

use std::collections::VecDeque;
use std::sync::LazyLock;

use dashmap::DashMap;
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseFollowup, EditInteractionResponse,
};
use poise::CreateReply;

use crate::data::{Context, Data};
use crate::error::Error;
use crate::feelm::{self, Item, Kind, Query};

/// "Başka öner" butonu: `feelm:<tür kodu>:<tür>:<yıl>:<puan>`.
pub const PREFIX: &str = "feelm:";
/// Kanal başına son önerilenler (art arda aynısı gelmesin).
const RECENT: usize = 30;
static RECENT_PICKS: LazyLock<DashMap<ChannelId, VecDeque<String>>> = LazyLock::new(DashMap::new);

/// feelm.org'dan film, dizi ya da oyun önerisi.
#[poise::command(slash_command, guild_only, rename = "tavsiye", subcommands("film", "dizi", "oyun", "ara"), subcommand_required, category = "Eğlence")]
pub async fn tavsiye(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

async fn genres(kind: Kind, partial: &str) -> Vec<serenity::AutocompleteChoice> {
    let partial = partial.to_lowercase();
    kind.genres()
        .iter()
        .filter(|(_, name)| name.to_lowercase().contains(&partial))
        .take(25)
        .map(|(slug, name)| serenity::AutocompleteChoice::new(*name, *slug))
        .collect()
}

async fn movie_genres(_ctx: Context<'_>, partial: &str) -> Vec<serenity::AutocompleteChoice> {
    genres(Kind::Movie, partial).await
}

async fn series_genres(_ctx: Context<'_>, partial: &str) -> Vec<serenity::AutocompleteChoice> {
    genres(Kind::Series, partial).await
}

async fn game_genres(_ctx: Context<'_>, partial: &str) -> Vec<serenity::AutocompleteChoice> {
    genres(Kind::Game, partial).await
}

/// Film önerir (feelm.org).
#[poise::command(slash_command, guild_only, user_cooldown = 3)]
async fn film(
    ctx: Context<'_>,
    #[rename = "tür"]
    #[description = "Tür (ör. Korku, Bilim kurgu)"]
    #[autocomplete = "movie_genres"]
    genre: Option<String>,
    #[rename = "yıldan"]
    #[description = "Bu yıldan yeni olsun (ör. 2015)"]
    #[min = 1900]
    #[max = 2100]
    year: Option<u16>,
    #[rename = "en_az_imdb"]
    #[description = "En az IMDb puanı (1-9)"]
    #[min = 1]
    #[max = 9]
    score: Option<u8>,
) -> Result<(), Error> {
    recommend(ctx, Kind::Movie, Query { genre, year_from: year, min_score: score }).await
}

/// Dizi önerir (feelm.org).
#[poise::command(slash_command, guild_only, user_cooldown = 3)]
async fn dizi(
    ctx: Context<'_>,
    #[rename = "tür"]
    #[description = "Tür (ör. Suç, Komedi)"]
    #[autocomplete = "series_genres"]
    genre: Option<String>,
    #[rename = "yıldan"]
    #[description = "Bu yıldan yeni olsun (ör. 2015)"]
    #[min = 1900]
    #[max = 2100]
    year: Option<u16>,
    #[rename = "en_az_imdb"]
    #[description = "En az IMDb puanı (1-9)"]
    #[min = 1]
    #[max = 9]
    score: Option<u8>,
) -> Result<(), Error> {
    recommend(ctx, Kind::Series, Query { genre, year_from: year, min_score: score }).await
}

/// Oyun önerir (feelm.org).
#[poise::command(slash_command, guild_only, user_cooldown = 3)]
async fn oyun(
    ctx: Context<'_>,
    #[rename = "tür"]
    #[description = "Tür (ör. Rol yapma, Strateji)"]
    #[autocomplete = "game_genres"]
    genre: Option<String>,
    #[rename = "yıldan"]
    #[description = "Bu yıldan yeni olsun (ör. 2015)"]
    #[min = 1900]
    #[max = 2100]
    year: Option<u16>,
    #[rename = "en_az_puan"]
    #[description = "En az puan, 10 üzerinden (IGDB; ör. 8 = 80/100)"]
    #[min = 1]
    #[max = 9]
    score: Option<u8>,
) -> Result<(), Error> {
    recommend(ctx, Kind::Game, Query { genre, year_from: year, min_score: score }).await
}

/// Film, dizi ya da oyun arar (feelm.org).
#[poise::command(slash_command, guild_only, user_cooldown = 3)]
async fn ara(
    ctx: Context<'_>,
    #[description = "Ad (ör. Batman, The Witcher)"]
    #[max_length = 80]
    sorgu: String,
) -> Result<(), Error> {
    ctx.defer().await?;
    let data = ctx.data();
    let found = match feelm::search(&data.http_client, &sorgu).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(error = ?e, "feelm araması yapılamadı");
            ctx.say("❌ feelm.org'a şu an ulaşılamıyor. Biraz sonra tekrar dene.").await?;
            return Ok(());
        }
    };
    let hits: Vec<&Item> = found.0.iter().filter(|i| !i.is_adult()).collect();
    let Some(best) = hits.first() else {
        ctx.say(format!("🔎 feelm.org'da **{}** için sonuç bulunamadı.", sorgu.chars().take(80).collect::<String>())).await?;
        return Ok(());
    };
    let mut embed = item_embed(data, best).await;
    let others: Vec<String> = hits
        .iter()
        .skip(1)
        .take(5)
        .map(|i| {
            let (emoji, _) = i.kind.map_or(("•", ""), Kind::label);
            format!("{emoji} [{}]({}){}", i.title, i.url(), i.year.map(|y| format!(" ({y})")).unwrap_or_default())
        })
        .collect();
    if !others.is_empty() {
        embed = embed.field("Diğer sonuçlar", others.join("\n"), false);
    }
    ctx.send(CreateReply::default().embed(embed).components(vec![link_row(best, None)])).await?;
    Ok(())
}

async fn recommend(ctx: Context<'_>, kind: Kind, query: Query) -> Result<(), Error> {
    ctx.defer().await?;
    let data = ctx.data();
    match pick(data, kind, &query, ctx.channel_id()).await {
        Ok(Some(item)) => {
            let embed = item_embed(data, &item).await;
            ctx.send(CreateReply::default().embed(embed).components(vec![link_row(&item, Some((kind, &query)))])).await?;
        }
        Ok(None) => {
            ctx.say("🔎 Bu süzgeçlere uyan bir şey bulunamadı. Türü, yılı ya da puanı gevşetip tekrar dene.").await?;
        }
        Err(e) => {
            tracing::warn!(error = ?e, "feelm önerisi alınamadı");
            ctx.say("❌ feelm.org'a şu an ulaşılamıyor. Biraz sonra tekrar dene.").await?;
        }
    }
    Ok(())
}

/// Popüler ilk üç sayfadan (210 öğe) rastgele biri; kanalda yakın zamanda önerilenler atlanır.
async fn pick(data: &Data, kind: Kind, query: &Query, channel: ChannelId) -> anyhow::Result<Option<Item>> {
    let page = 1 + u32::try_from(crate::rand_util::below(3)).unwrap_or(0);
    let mut found = feelm::list(&data.http_client, kind, query, page).await?;
    if found.0.is_empty() && page > 1 {
        found = feelm::list(&data.http_client, kind, query, 1).await?;
    }
    let pool: Vec<&Item> = found.0.iter().filter(|i| !i.is_adult()).collect();
    let recent = RECENT_PICKS.get(&channel).map(|r| r.clone()).unwrap_or_default();
    let fresh: Vec<&Item> = pool.iter().copied().filter(|i| !recent.contains(&i.slug)).collect();
    let choices = if fresh.is_empty() { &pool } else { &fresh };
    let Some(item) = choices.get(usize::try_from(crate::rand_util::below(choices.len() as u64)).unwrap_or(0)).map(|i| (*i).clone()) else {
        return Ok(None);
    };
    let mut entry = RECENT_PICKS.entry(channel).or_default();
    entry.push_back(item.slug.clone());
    while entry.len() > RECENT {
        entry.pop_front();
    }
    Ok(Some(item))
}

/// Özeti Türkçeye çevirir (Redis'te 7 gün saklanır).
async fn turkish_overview(data: &Data, item: &Item) -> Option<String> {
    let key = format!("feelm:ozet:{}", item.slug);
    let mut conn = data.redis.clone();
    if let Ok(Some(cached)) = redis::cmd("GET").arg(&key).query_async::<Option<String>>(&mut conn).await {
        return Some(cached);
    }
    let english = feelm::overview(&data.http_client, item).await?;
    let text = crate::news::machine_translate(&english, "tr").await.unwrap_or(english);
    let _: redis::RedisResult<()> = redis::cmd("SET").arg(&key).arg(&text).arg("EX").arg(7 * 86_400).query_async(&mut conn).await;
    Some(text)
}

fn minutes(total: i64) -> String {
    match (total / 60, total % 60) {
        (0, m) => format!("{m} dk"),
        (h, 0) => format!("{h} sa"),
        (h, m) => format!("{h} sa {m} dk"),
    }
}

/// "2026-07-15" → "15.07.2026"
fn turkish_date(iso: &str) -> String {
    let parts: Vec<&str> = iso.split('-').collect();
    match parts.as_slice() {
        [y, m, d] => format!("{}.{m}.{y}", d.get(..2).unwrap_or(d)),
        _ => iso.to_owned(),
    }
}

async fn item_embed(data: &Data, item: &Item) -> CreateEmbed {
    let (emoji, label) = item.kind.map_or(("🎞️", "Öneri"), Kind::label);
    let title = match item.year {
        Some(year) => format!("{emoji} {} ({year})", item.title),
        None => format!("{emoji} {}", item.title),
    };
    let overview = turkish_overview(data, item).await.map(|t| {
        let short: String = t.chars().take(700).collect();
        if short.chars().count() < t.chars().count() { format!("{short}…") } else { short }
    });
    let mut embed = CreateEmbed::new()
        .title(title.chars().take(250).collect::<String>())
        .url(item.url())
        .description(overview.unwrap_or_else(|| "_Özet bulunamadı._".to_owned()))
        .colour(serenity::Colour::new(item.color.unwrap_or(0x5865F2)))
        .footer(CreateEmbedFooter::new(format!("{label} önerisi · feelm.org")));
    if let Some(poster) = &item.poster {
        embed = embed.thumbnail(poster);
    }
    let mut scores = Vec::new();
    if let Some(imdb) = item.imdb {
        scores.push(format!("⭐ IMDb **{imdb:.1}**/10"));
    }
    if let Some(tmdb) = item.tmdb {
        scores.push(format!("TMDB %{tmdb:.0}"));
    }
    if let Some(igdb) = item.igdb {
        scores.push(format!("⭐ IGDB **{igdb:.0}**/100"));
    }
    if !scores.is_empty() {
        embed = embed.field("Puan", scores.join(" · "), true);
    }
    if let Some(runtime) = item.runtime.filter(|r| *r > 0) {
        embed = embed.field("Süre", minutes(runtime), true);
    }
    if let Some(seasons) = item.seasons.filter(|s| *s > 0) {
        let episodes = item.episodes.map(|e| format!(" · {e} bölüm")).unwrap_or_default();
        embed = embed.field("Sezon", format!("{seasons} sezon{episodes}"), true);
    }
    if let Some(release) = &item.release {
        embed = embed.field("Çıkış", turkish_date(release), true);
    }
    if let Some(cert) = item.certification.as_deref().filter(|c| !c.is_empty()) {
        embed = embed.field("Yaş sınırı", cert, true);
    }
    if !item.developers.is_empty() {
        embed = embed.field("Geliştirici", item.developers.join(", ").chars().take(200).collect::<String>(), true);
    }
    embed
}

fn link_row(item: &Item, again: Option<(Kind, &Query)>) -> CreateActionRow {
    let mut buttons = Vec::new();
    if let Some((kind, query)) = again {
        let id = format!(
            "{PREFIX}{}:{}:{}:{}",
            kind.code(),
            query.genre.as_deref().unwrap_or("-"),
            query.year_from.unwrap_or(0),
            query.min_score.unwrap_or(0)
        );
        buttons.push(CreateButton::new(id).label("Başka öner").emoji('🎲').style(ButtonStyle::Primary));
    }
    buttons.push(CreateButton::new_link(item.url()).label("feelm.org'da aç"));
    CreateActionRow::Buttons(buttons)
}

/// `feelm:<kod>:<tür>:<yıl>:<puan>` → (tür, süzgeç)
fn parse_again(rest: &str) -> Option<(Kind, Query)> {
    let mut parts = rest.split(':');
    let kind = Kind::from_code(parts.next()?)?;
    let genre = parts.next()?;
    let year: u16 = parts.next()?.parse().ok()?;
    let score: u8 = parts.next()?.parse().ok()?;
    let query = Query {
        genre: (genre != "-" && kind.genre_name(genre).is_some()).then(|| genre.to_owned()),
        year_from: (year != 0).then_some(year),
        min_score: (score != 0).then_some(score),
    };
    Some((kind, query))
}

/// "Başka öner" butonu: aynı süzgeçle yeni öneri, mesaj yerinde güncellenir.
pub async fn handle_component(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let Some((kind, query)) = interaction.data.custom_id.strip_prefix(PREFIX).and_then(parse_again) else {
        return Ok(());
    };
    interaction.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
    match pick(data, kind, &query, interaction.channel_id).await {
        Ok(Some(item)) => {
            let embed = item_embed(data, &item).await;
            interaction
                .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed).components(vec![link_row(&item, Some((kind, &query)))]))
                .await?;
        }
        Ok(None) | Err(_) => {
            let note = CreateInteractionResponseFollowup::new().content("❌ Şu an yeni bir öneri bulunamadı, biraz sonra tekrar dene.").ephemeral(true);
            interaction.create_followup(&ctx.http, note).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn again_button_round_trips() {
        let query = Query { genre: Some("horror".to_owned()), year_from: Some(2010), min_score: Some(7) };
        let row_id = format!("{}:{}:{}:{}", Kind::Movie.code(), "horror", 2010, 7);
        assert_eq!(parse_again(&row_id), Some((Kind::Movie, query)));
        assert_eq!(parse_again("o:-:0:0"), Some((Kind::Game, Query::default())));
        // Listede olmayan tür yok sayılır.
        assert_eq!(parse_again("d:erotic:0:0"), Some((Kind::Series, Query::default())));
        assert_eq!(parse_again("x:-:0:0"), None);
        assert!(format!("{PREFIX}{row_id}").len() <= 100);
    }

    #[test]
    fn formats_runtime_and_dates() {
        assert_eq!(minutes(173), "2 sa 53 dk");
        assert_eq!(minutes(45), "45 dk");
        assert_eq!(minutes(120), "2 sa");
        assert_eq!(turkish_date("2026-07-15"), "15.07.2026");
    }
}
