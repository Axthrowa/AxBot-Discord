//! `/akış`: Reddit, Bluesky, RSS ve podcast duyuruları.

use poise::serenity_prelude::{self as serenity, Mentionable};

use super::moderation::reply_private;
use crate::data::Context;
use crate::error::Error;
use crate::feeds::{self, FeedSettings, Kind};

/// Reddit, Bluesky, RSS ve podcast duyuruları.
#[poise::command(
    slash_command,
    guild_only,
    rename = "akış",
    subcommands("akis_ekle", "akis_sil", "akis_liste", "akis_haberler", "akis_haber_ozeti"),
    subcommand_required,
    required_permissions = "MANAGE_GUILD",
    category = "Sunucu"
)]
pub async fn akis(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Haber kategorileri (`news::CATEGORIES` ile aynı).
#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum NewsChoice {
    #[name = "Hepsi"]
    All,
    #[name = "Teknoloji"]
    Tech,
    #[name = "Oyun"]
    Games,
    #[name = "Sinema"]
    Cinema,
    #[name = "Dizi"]
    Series,
    #[name = "Ücretsiz oyunlar"]
    FreeGames,
}

impl NewsChoice {
    fn key(self) -> Option<&'static str> {
        Some(match self {
            Self::All => return None,
            Self::Tech => "teknoloji",
            Self::Games => "oyun",
            Self::Cinema => "sinema",
            Self::Series => "dizi",
            Self::FreeGames => "ucretsiz-oyun",
        })
    }
}

/// Haberlerin yazılacağı dil (`news::LANGUAGES`).
#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum NewsLanguage {
    #[name = "Orijinal (haberin kendi dili)"]
    Original,
    #[name = "Türkçe"]
    Turkish,
    #[name = "İngilizce"]
    English,
    #[name = "Almanca"]
    German,
    #[name = "İspanyolca"]
    Spanish,
    #[name = "Fransızca"]
    French,
}

impl NewsLanguage {
    fn code(self) -> &'static str {
        match self {
            Self::Original => "",
            Self::Turkish => "tr",
            Self::English => "en",
            Self::German => "de",
            Self::Spanish => "es",
            Self::French => "fr",
        }
    }
}

/// Haberleri bir kanala otomatik gönderir (kanal boşsa kapatır). Diller ve süzgeçler: web paneli.
#[poise::command(slash_command, guild_only, rename = "haberler")]
async fn akis_haberler(
    ctx: Context<'_>,
    #[description = "Haberlerin gideceği kanal (boş bırakırsan bu kategori kapanır)"]
    #[channel_types("Text", "News")]
    kanal: Option<serenity::GuildChannel>,
    #[description = "Hangi haberler (varsayılan: hepsi)"] kategori: Option<NewsChoice>,
    #[description = "Haberler hangi dilde yazılsın (farklı dildeki haberler yapay zekâyla çevrilir)"] dil: Option<NewsLanguage>,
) -> Result<(), Error> {
    use crate::news::{NewsSettings, CATEGORIES};
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let data = ctx.data();
    let mut settings = (*data.settings.get::<NewsSettings>(&data.db, guild_id).await?).clone();
    let key = kategori.and_then(NewsChoice::key);
    let keys: Vec<&str> = match key {
        Some(k) => vec![k],
        None => CATEGORIES.iter().map(|c| c.key).collect(),
    };
    if let Some(dil) = dil {
        settings.output_language = dil.code().to_owned();
    }
    let text = match &kanal {
        Some(channel) => {
            if let Some(problem) = super::community::cannot_send(ctx, channel) {
                return reply_private(ctx, problem).await;
            }
            settings.enabled = true;
            if key.is_some() || !settings.categories.is_empty() {
                for k in &keys {
                    if !settings.categories.iter().any(|c| c == k) {
                        settings.categories.push((*k).to_owned());
                    }
                }
            }
            match key {
                // Tek kategori: o kategoriye özel kanal (diğerleri kendi kanalında kalır).
                Some(k) if settings.channel.is_some() => {
                    settings.channels.insert(k.to_owned(), channel.id.get());
                }
                _ => {
                    settings.channel = Some(channel.id.get());
                    if key.is_none() {
                        settings.channels.clear();
                    }
                }
            }
            let language = crate::news::language(&settings.output_language).map_or("haberin kendi dili".to_owned(), |l| format!("{} {}", l.flag, l.name));
            format!(
                "📰 **{}** haberleri artık {} kanalına gelecek (dil: {language}). Yeni haber çıktıkça otomatik yazılır; \
                 her kategoriden en yeni haber birkaç dakika içinde örnek olarak gelir. Kaynak dilleri, anahtar kelimeler, \
                 görünüm ve günlük özet: `/panel` → **Haberler**.",
                keys.iter().filter_map(|k| crate::news::category(k)).map(|c| c.name).collect::<Vec<_>>().join(", "),
                channel.id.mention()
            )
        }
        None => {
            settings.categories = if settings.categories.is_empty() { CATEGORIES.iter().map(|c| c.key.to_owned()).collect() } else { settings.categories };
            settings.categories.retain(|c| !keys.contains(&c.as_str()));
            for k in &keys {
                settings.channels.remove(*k);
            }
            if settings.categories.is_empty() {
                settings.enabled = false;
            }
            "📰 Seçilen haberler kapatıldı.".to_owned()
        }
    };
    data.settings.set(&data.db, guild_id, settings).await?;
    reply_private(ctx, text).await
}

/// Bir kategorinin bu haftaki özetini ve en yeni haberini şimdi gösterir.
#[poise::command(slash_command, guild_only, rename = "haber-özeti")]
async fn akis_haber_ozeti(ctx: Context<'_>, #[description = "Hangi kategori"] kategori: NewsChoice) -> Result<(), Error> {
    use crate::news::{self, NewsContext, NewsSettings};
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    ctx.defer().await?;
    let data = ctx.data();
    let settings = data.settings.get::<NewsSettings>(&data.db, guild_id).await?;
    let category = news::category(kategori.key().unwrap_or("teknoloji")).ok_or_else(|| anyhow::anyhow!("kategori yok"))?;
    let context = NewsContext::from_data(ctx.serenity_context(), data);
    let mut embeds = Vec::new();
    if let Some(weekly) = news::weekly_embed(&context, category, &settings).await {
        embeds.push(weekly);
    }
    match news::latest(category, &settings).await {
        Ok((source, item)) => {
            let rendered = news::render_for_command(&context, &item, &settings).await;
            embeds.push(news::news_embed(category, source, &item, &rendered, settings.style));
        }
        Err(e) => tracing::debug!(error = ?e, "Son haber alınamadı"),
    }
    if embeds.is_empty() {
        ctx.say("📰 Seçili dil ve süzgeçlerle şu an gösterilecek haber bulunamadı.").await?;
        return Ok(());
    }
    let mut reply = poise::CreateReply::default().allowed_mentions(serenity::CreateAllowedMentions::new());
    if embeds.len() == 1 {
        reply = reply.content("ℹ️ Haftalık özet için henüz yeterli haber toplanmadı (haberler açıldıktan sonra birikir). En yeni haber:");
    }
    for embed in embeds {
        reply = reply.embed(embed);
    }
    ctx.send(reply).await?;
    Ok(())
}

/// Bir kaynağı takip eder; yeni paylaşımlar seçilen kanala duyurulur.
#[poise::command(slash_command, guild_only, rename = "ekle")]
async fn akis_ekle(
    ctx: Context<'_>,
    #[description = "Kaynağın türü"]
    #[rename = "tür"]
    kind: Kind,
    #[description = "Subreddit adı, Bluesky kullanıcı adı ya da https akış adresi"]
    #[max_length = 400]
    kaynak: String,
    #[description = "Duyuruların gönderileceği kanal"]
    #[channel_types("Text", "News")]
    kanal: serenity::GuildChannel,
    #[description = "Duyuruda etiketlenecek rol"] rol: Option<serenity::Role>,
    #[description = "Özel metin: {başlık} {bağlantı} {kaynak}"]
    #[max_length = 500]
    mesaj: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    ctx.defer_ephemeral().await?;
    let data = ctx.data();
    let current = data.settings.get::<FeedSettings>(&data.db, guild_id).await?;
    if current.feeds.len() >= feeds::MAX_FEEDS {
        return reply_private(ctx, format!("En fazla {} akış takip edilebilir.", feeds::MAX_FEEDS)).await;
    }
    let feed = match feeds::prepare(kind, &kaynak, kanal.id.get(), rol.map(|r| r.id.get()), mesaj, current.next_id()).await {
        Ok(feed) => feed,
        Err(problem) => return reply_private(ctx, format!("❌ {problem}")).await,
    };
    // Okuma sürerken liste değişmiş olabilir: güncel hâli alınır.
    let mut settings = (*data.settings.get::<FeedSettings>(&data.db, guild_id).await?).clone();
    if settings.feeds.iter().any(|f| f.url == feed.url && f.channel == feed.channel) {
        return reply_private(ctx, "Bu kaynak bu kanalda zaten takip ediliyor.").await;
    }
    let feed = feeds::Feed { id: settings.next_id(), ..feed };
    let text = format!("✅ **{}** ({}) takip ediliyor; yeni paylaşımlar {} kanalına duyurulacak.", feed.name, kind.label(), kanal.id.mention());
    settings.feeds.push(feed);
    data.settings.set(&data.db, guild_id, settings).await?;
    reply_private(ctx, text).await
}

/// Takip edilen bir akışı kaldırır.
#[poise::command(slash_command, guild_only, rename = "sil")]
async fn akis_sil(ctx: Context<'_>, #[description = "Akışın numarası (/akış liste)"] #[min = 1] numara: u32) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let data = ctx.data();
    let mut settings = (*data.settings.get::<FeedSettings>(&data.db, guild_id).await?).clone();
    let before = settings.feeds.len();
    settings.feeds.retain(|f| f.id != numara);
    if settings.feeds.len() == before {
        return reply_private(ctx, "Bu numarada bir akış yok. `/akış liste` ile bakabilirsin.").await;
    }
    data.settings.set(&data.db, guild_id, settings).await?;
    reply_private(ctx, "🗑️ Akış kaldırıldı.").await
}

/// Takip edilen akışları listeler.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn akis_liste(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let data = ctx.data();
    let settings = data.settings.get::<FeedSettings>(&data.db, guild_id).await?;
    if settings.feeds.is_empty() {
        return reply_private(ctx, "Takip edilen akış yok. `/akış ekle` ile ekleyebilirsin.").await;
    }
    let lines: Vec<String> = settings
        .feeds
        .iter()
        .map(|f| format!("`{}` · {} · **{}** → <#{}>", f.id, f.kind.label(), f.name, f.channel))
        .collect();
    reply_private(ctx, lines.join("\n")).await
}
