//! Kişiselleştirilmiş haberler: herkes kendi kategorilerini seçer.
//!
//! - Haber kanalına gönderilen panelde "⚙️ Tercihlerim" (kategori + özelden alma) ve "📰 Haberlerim" butonları var.
//!   "Haberlerim" son haberleri yalnızca üyenin seçtiği kategorilerden, yalnızca ona görünen bir mesajla gösterir.
//! - İsteyen özelden alır: anlık (kişi başına saatte en fazla `DM_PER_HOUR`) ya da günlük özet (sunucunun özet saatinde).
//!   Aynı haber iki sunucudan iki kez gönderilmez; özel mesajları kapalı olanın DM tercihi kapatılır.

use std::time::Duration;

use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow, CreateButton, CreateEmbed,
    CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateSelectMenu, CreateSelectMenuKind,
    CreateSelectMenuOption, EditInteractionResponse, GuildId, UserId,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{debug, warn};

use crate::data::Data;
use crate::db::snowflake;
use crate::guild_config::to_id;
use crate::news::{self, Category, NewsContext, NewsItem, NewsSettings, CATEGORIES};

pub const PREFIX: &str = "newsp:";
const PREFS_BUTTON: &str = "newsp:prefs";
const MINE_BUTTON: &str = "newsp:mine";
const CATEGORY_SELECT: &str = "newsp:cat";
const DM_SELECT: &str = "newsp:dm";
/// Özelden anlık haberde kişi başına saatlik sınır.
const DM_PER_HOUR: i64 = 6;
/// "Haberlerim"de kategori başına en fazla bu kadar haber.
const MINE_PER_CATEGORY: usize = 4;
const MINE_MAX: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DmMode {
    #[default]
    Off,
    Instant,
    Daily,
}

impl DmMode {
    const ALL: [(Self, &'static str, &'static str); 3] =
        [(Self::Off, "off", "Özelden gönderme"), (Self::Instant, "instant", "Özelden anlık gönder"), (Self::Daily, "daily", "Özelden günde bir özet gönder")];

    fn code(self) -> &'static str {
        Self::ALL.iter().find(|(m, ..)| *m == self).map_or("off", |(_, c, _)| c)
    }

    fn parse(code: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, c, _)| *c == code).map(|(m, ..)| *m)
    }

    fn label(self) -> &'static str {
        Self::ALL.iter().find(|(m, ..)| *m == self).map_or("", |(_, _, l)| l)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prefs {
    pub categories: Vec<String>,
    pub dm: DmMode,
}

impl Prefs {
    /// Seçim yoksa sunucunun açık bütün kategorileri.
    fn wants(&self, category: &str) -> bool {
        self.categories.is_empty() || self.categories.iter().any(|c| c == category)
    }
}

pub async fn load(db: &PgPool, guild_id: GuildId, user: UserId) -> anyhow::Result<Prefs> {
    let row: Option<(Vec<String>, String)> = sqlx::query_as("SELECT categories, dm FROM news_prefs WHERE guild_id = $1 AND user_id = $2")
        .bind(snowflake(guild_id.get())?)
        .bind(snowflake(user.get())?)
        .fetch_optional(db)
        .await?;
    Ok(row.map_or_else(Prefs::default, |(categories, dm)| Prefs { categories, dm: DmMode::parse(&dm).unwrap_or_default() }))
}

async fn save(db: &PgPool, guild_id: GuildId, user: UserId, prefs: &Prefs) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO news_prefs (guild_id, user_id, categories, dm) VALUES ($1, $2, $3, $4)
         ON CONFLICT (guild_id, user_id) DO UPDATE SET categories = EXCLUDED.categories, dm = EXCLUDED.dm, updated_at = now()",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(snowflake(user.get())?)
    .bind(&prefs.categories)
    .bind(prefs.dm.code())
    .execute(db)
    .await?;
    Ok(())
}

/// Sunucuda özelden haber alan kaç kişi var?
pub async fn subscriber_count(db: &PgPool, guild_id: GuildId) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM news_prefs WHERE guild_id = $1 AND dm <> 'off'").bind(snowflake(guild_id.get())?).fetch_one(db).await?)
}

/// Haber kanalına gönderilen tercih paneli.
pub fn panel_message() -> CreateMessage {
    let embed = CreateEmbed::new()
        .title("📰 Kişisel haberler")
        .description(
            "Bu kanalda bütün haberler akar; sen yalnızca **ilgilendiğin kategorileri** görmek istersen:\n\n\
             ⚙️ **Tercihlerim** — kategorilerini seç; istersen haberleri özelden (anlık ya da günde bir özet) al.\n\
             📰 **Haberlerim** — son haberleri yalnızca seçtiğin kategorilerden, sadece sana görünen bir mesajla gösterir.",
        )
        .colour(serenity::Colour::from_rgb(59, 130, 246))
        .footer(CreateEmbedFooter::new("Tercihler kişiseldir; başkaları göremez."));
    let buttons = vec![
        CreateButton::new(PREFS_BUTTON).label("Tercihlerim").emoji('⚙').style(ButtonStyle::Primary),
        CreateButton::new(MINE_BUTTON).label("Haberlerim").emoji('📰').style(ButtonStyle::Secondary),
    ];
    CreateMessage::new().embed(embed).components(vec![CreateActionRow::Buttons(buttons)])
}

/// Sunucuda açık olan kategoriler.
fn active_categories(settings: &NewsSettings) -> Vec<&'static Category> {
    CATEGORIES.iter().filter(|c| settings.channel_for(c.key).is_some()).collect()
}

fn prefs_view(prefs: &Prefs, available: &[&'static Category]) -> (String, Vec<CreateActionRow>) {
    let chosen: Vec<String> = available.iter().filter(|c| prefs.categories.iter().any(|k| k == c.key)).map(|c| format!("{} {}", c.emoji, c.name)).collect();
    let text = format!(
        "**⚙️ Haber tercihlerin**\nKategoriler: {}\nÖzelden: **{}**\n\n_Seçimini aşağıdaki menülerden değiştir; hemen kaydedilir._",
        if chosen.is_empty() { "hepsi (seçim yok)".to_owned() } else { chosen.join(", ") },
        prefs.dm.label()
    );
    let options: Vec<CreateSelectMenuOption> = available
        .iter()
        .map(|c| CreateSelectMenuOption::new(c.name, c.key).emoji(serenity::ReactionType::Unicode(c.emoji.to_owned())).default_selection(prefs.categories.iter().any(|k| k == c.key)))
        .collect();
    let count = u8::try_from(options.len()).unwrap_or(25);
    let categories = CreateSelectMenu::new(CATEGORY_SELECT, CreateSelectMenuKind::String { options })
        .placeholder("Kategorilerini seç (boş = hepsi)")
        .min_values(0)
        .max_values(count.max(1));
    let dm_options = DmMode::ALL.iter().map(|(mode, code, label)| CreateSelectMenuOption::new(*label, *code).default_selection(*mode == prefs.dm)).collect();
    let dm = CreateSelectMenu::new(DM_SELECT, CreateSelectMenuKind::String { options: dm_options }).placeholder("Özelden alma");
    (text, vec![CreateActionRow::SelectMenu(categories), CreateActionRow::SelectMenu(dm)])
}

/// `newsp:` önekli butonlar ve menüler.
pub async fn handle_component(ctx: &serenity::Context, data: &Data, interaction: &ComponentInteraction) -> anyhow::Result<()> {
    let Some(guild_id) = interaction.guild_id else { return Ok(()) };
    let settings = data.settings.get::<NewsSettings>(&data.db, guild_id).await?;
    let available = active_categories(&settings);
    let user = interaction.user.id;
    let reply = |text: &str| CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true));
    if available.is_empty() {
        interaction.create_response(&ctx.http, reply("Bu sunucuda haberler şu an kapalı.")).await?;
        return Ok(());
    }
    let values = match &interaction.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.clone(),
        _ => Vec::new(),
    };
    match interaction.data.custom_id.as_str() {
        PREFS_BUTTON => {
            let prefs = load(&data.db, guild_id, user).await?;
            let (text, rows) = prefs_view(&prefs, &available);
            let response = CreateInteractionResponseMessage::new().content(text).components(rows).ephemeral(true);
            interaction.create_response(&ctx.http, CreateInteractionResponse::Message(response)).await?;
        }
        CATEGORY_SELECT | DM_SELECT => {
            let mut prefs = load(&data.db, guild_id, user).await?;
            if interaction.data.custom_id == CATEGORY_SELECT {
                // Yalnızca sunucuda açık kategoriler kabul edilir.
                prefs.categories = values.into_iter().filter(|v| available.iter().any(|c| c.key == v)).collect();
            } else if let Some(mode) = values.first().and_then(|v| DmMode::parse(v)) {
                prefs.dm = mode;
            }
            save(&data.db, guild_id, user, &prefs).await?;
            let (text, rows) = prefs_view(&prefs, &available);
            let update = CreateInteractionResponseMessage::new().content(format!("✅ Kaydedildi.\n\n{text}")).components(rows);
            interaction.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await?;
        }
        MINE_BUTTON => {
            interaction.defer_ephemeral(&ctx.http).await?;
            let prefs = load(&data.db, guild_id, user).await?;
            let embed = my_news(&NewsContext::from_data(ctx, data), &settings, &prefs, &available).await;
            interaction.edit_response(&ctx.http, EditInteractionResponse::new().embed(embed)).await?;
        }
        _ => {}
    }
    Ok(())
}

/// Üyenin kategorilerindeki son haberler (başlıklar sunucunun haber diline çevrilir).
async fn my_news(news: &NewsContext, settings: &NewsSettings, prefs: &Prefs, available: &[&'static Category]) -> CreateEmbed {
    let mut lines = Vec::new();
    for category in available.iter().filter(|c| prefs.wants(c.key)) {
        let entries = news::recent(news, category.key, settings, MINE_PER_CATEGORY).await;
        if entries.is_empty() {
            continue;
        }
        lines.push(format!("**{} {}**", category.emoji, category.name));
        for entry in entries {
            let title = news::title_in(news, &entry.title, &entry.lang, settings.target_language()).await;
            lines.push(format!("• [{}]({}) — {} · <t:{}:R>", title.chars().take(120).collect::<String>(), entry.link, entry.source, entry.at));
        }
        if lines.len() >= MINE_MAX + available.len() {
            break;
        }
    }
    let description = if lines.is_empty() {
        "Seçtiğin kategorilerde son günlerde haber yok. Biraz sonra tekrar bak ya da ⚙️ Tercihlerim'den kategori ekle.".to_owned()
    } else {
        lines.join("\n")
    };
    CreateEmbed::new()
        .title("📰 Senin haberlerin")
        .description(description.chars().take(4000).collect::<String>())
        .colour(serenity::Colour::from_rgb(59, 130, 246))
        .footer(CreateEmbedFooter::new("Yalnızca sen görüyorsun · ⚙️ Tercihlerim ile kategorilerini değiştir"))
}

/// Özelden haber isteyenler: (üye, mod) — bu kategoriyi isteyenler.
async fn subscribers(db: &PgPool, guild_id: GuildId, category: &str) -> anyhow::Result<Vec<(UserId, DmMode)>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT user_id, dm FROM news_prefs WHERE guild_id = $1 AND dm <> 'off' AND (cardinality(categories) = 0 OR $2 = ANY(categories))",
    )
    .bind(snowflake(guild_id.get())?)
    .bind(category)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().filter_map(|(u, dm)| Some((UserId::new(to_id(Some(u))?), DmMode::parse(&dm)?))).collect())
}

async fn set_dm_off(db: &PgPool, guild_id: GuildId, user: UserId) {
    let result = async {
        sqlx::query("UPDATE news_prefs SET dm = 'off', updated_at = now() WHERE guild_id = $1 AND user_id = $2")
            .bind(snowflake(guild_id.get())?)
            .bind(snowflake(user.get())?)
            .execute(db)
            .await?;
        anyhow::Ok(())
    }
    .await;
    if let Err(e) = result {
        warn!(guild = %guild_id, user = %user, error = ?e, "Kişisel haber DM tercihi kapatılamadı");
    }
}

/// Redis'te ilk kez mi? (`SET NX`)
async fn first_time(news: &NewsContext, key: String, ttl_secs: u64) -> bool {
    let mut conn = news.redis.clone();
    let set: Option<String> = redis::cmd("SET").arg(key).arg(1).arg("NX").arg("EX").arg(ttl_secs).query_async(&mut conn).await.unwrap_or(None);
    set.is_some()
}

#[derive(Serialize, Deserialize)]
struct Queued {
    category: String,
    title: String,
    link: String,
    source: String,
}

/// Yeni bir haber sunucuya düştüğünde özelden isteyenlere iletir (anlık) ya da günlük listeye ekler.
pub async fn dispatch(news: &NewsContext, guild_id: GuildId, settings: &NewsSettings, category: &Category, source: &str, item: &NewsItem) {
    let subs = match subscribers(&news.db, guild_id, category.key).await {
        Ok(subs) if !subs.is_empty() => subs,
        Ok(_) => return,
        Err(e) => {
            debug!(guild = %guild_id, error = ?e, "Kişisel haber aboneleri okunamadı");
            return;
        }
    };
    let link_hash = news::link_hash(&item.link);
    // Çeviri (gerekirse) bir kez yapılır; sonuç zaten önbellekte.
    let rendered = news::render(news, item, settings.target_language(), false, Duration::from_secs(15)).await;
    // Çevrilemediyse özelden de yabancı dilde gönderilmez (kanal için yeniden denenince buraya tekrar gelir).
    if news::untranslated(item, settings.target_language(), &rendered) {
        return;
    }
    for (user, mode) in subs {
        // Aynı haber birden çok sunucudan gelse de bir kez.
        if !first_time(news, format!("haber:dm:{user}:{link_hash:x}"), 3 * 86_400).await {
            continue;
        }
        match mode {
            DmMode::Off => {}
            DmMode::Instant => {
                let hour = serenity::Timestamp::now().unix_timestamp() / 3600;
                let key = format!("haber:dmsaat:{user}:{hour}");
                let mut conn = news.redis.clone();
                let count: i64 =
                    redis::pipe().cmd("INCR").arg(&key).cmd("EXPIRE").arg(&key).arg(3700).ignore().query_async::<(i64,)>(&mut conn).await.map_or(1, |(c,)| c);
                if count > DM_PER_HOUR {
                    continue;
                }
                let embed = news::news_embed(category, source, item, &rendered, news::Style::Full);
                if let Err(e) = user.direct_message(&news.ctx.http, CreateMessage::new().embed(embed)).await {
                    debug!(user = %user, error = %e, "Kişisel haber özelden gönderilemedi; tercihi kapatılıyor");
                    set_dm_off(&news.db, guild_id, user).await;
                }
            }
            DmMode::Daily => {
                let queued = Queued { category: category.key.to_owned(), title: rendered.title.clone(), link: item.link.clone(), source: source.to_owned() };
                let Ok(json) = serde_json::to_string(&queued) else { continue };
                let key = format!("haber:dmbekleyen:{guild_id}:{user}");
                let mut conn = news.redis.clone();
                let _: redis::RedisResult<()> = redis::pipe()
                    .cmd("RPUSH").arg(&key).arg(json).ignore()
                    .cmd("LTRIM").arg(&key).arg(-60).arg(-1).ignore()
                    .cmd("EXPIRE").arg(&key).arg(3 * 86_400).ignore()
                    .query_async(&mut conn)
                    .await;
            }
        }
    }
}

/// Sunucunun özet saatinde, günlük özet isteyenlere özelden liste gönderir. `today`: Türkiye tarihi.
pub async fn daily_digests(news: &NewsContext, guild_id: GuildId, today: &str) {
    if !first_time(news, format!("haber:dmgunluk:{guild_id}:{today}"), 2 * 86_400).await {
        return;
    }
    let rows: Vec<i64> = match async {
        anyhow::Ok(sqlx::query_scalar("SELECT user_id FROM news_prefs WHERE guild_id = $1 AND dm = 'daily'").bind(snowflake(guild_id.get())?).fetch_all(&news.db).await?)
    }
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            debug!(guild = %guild_id, error = ?e, "Günlük kişisel haber listesi okunamadı");
            return;
        }
    };
    let guild_name = news.ctx.cache.guild(guild_id).map(|g| g.name.clone()).unwrap_or_default();
    for user in rows.into_iter().filter_map(|u| to_id(Some(u))).map(UserId::new) {
        let key = format!("haber:dmbekleyen:{guild_id}:{user}");
        let mut conn = news.redis.clone();
        let Ok((raw, ())): Result<(Vec<String>, ()), _> = redis::pipe().cmd("LRANGE").arg(&key).arg(0).arg(-1).cmd("DEL").arg(&key).ignore().query_async(&mut conn).await else {
            continue;
        };
        let queued: Vec<Queued> = raw.iter().filter_map(|r| serde_json::from_str(r).ok()).collect();
        if queued.is_empty() {
            continue;
        }
        let mut lines = Vec::new();
        for category in CATEGORIES {
            let items: Vec<&Queued> = queued.iter().filter(|q| q.category == category.key).collect();
            if items.is_empty() {
                continue;
            }
            lines.push(format!("**{} {}**", category.emoji, category.name));
            lines.extend(items.iter().rev().take(8).map(|q| format!("• [{}]({}) — {}", q.title.chars().take(120).collect::<String>(), q.link, q.source)));
        }
        let embed = CreateEmbed::new()
            .title("📰 Günün haberleri — sana özel")
            .description(lines.join("\n").chars().take(4000).collect::<String>())
            .colour(serenity::Colour::from_rgb(59, 130, 246))
            .footer(CreateEmbedFooter::new(format!("{guild_name} · tercihlerini haber kanalındaki ⚙️ Tercihlerim butonundan değiştir")));
        if let Err(e) = user.direct_message(&news.ctx.http, CreateMessage::new().embed(embed)).await {
            debug!(user = %user, error = %e, "Günlük kişisel haber özeti gönderilemedi; tercihi kapatılıyor");
            set_dm_off(&news.db, guild_id, user).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_modes_round_trip() {
        for (mode, code, _) in DmMode::ALL {
            assert_eq!(DmMode::parse(code), Some(mode));
            assert_eq!(mode.code(), code);
        }
        assert_eq!(DmMode::parse("hep"), None);
    }

    #[test]
    fn empty_choice_means_everything() {
        let all = Prefs::default();
        assert!(all.wants("oyun") && all.wants("dizi"));
        let some = Prefs { categories: vec!["oyun".to_owned()], dm: DmMode::Off };
        assert!(some.wants("oyun") && !some.wants("dizi"));
    }
}
