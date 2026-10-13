//! Otomatik cevaplar: "sa" yazılınca "Aleyküm selam!" gibi. Kurallar sunucu başına bellekte
//! tutulur (her mesajda kontrol edilir; veritabanına sadece ilk kullanımda ve değişiklikte gidilir).
//!
//! Aynı kural aynı kanalda 5 saniyede bir defadan fazla çalışmaz (sohbeti boğmasın).

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use poise::serenity_prelude::{ChannelId, GuildId, UserId};
use sqlx::PgPool;

use crate::db::snowflake;
use crate::games::hangman::tr_lower;

pub const MAX_RULES: usize = 50;
const COOLDOWN: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum MatchMode {
    #[name = "Kelime olarak geçerse (önerilen)"]
    Word,
    #[name = "Mesajın tamamı buysa"]
    Exact,
    #[name = "Mesaj bununla başlıyorsa"]
    Starts,
}

impl MatchMode {
    pub fn db(self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Exact => "exact",
            Self::Starts => "starts",
        }
    }
    fn from_db(value: &str) -> Self {
        match value {
            "exact" => Self::Exact,
            "starts" => Self::Starts,
            _ => Self::Word,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Word => "kelime",
            Self::Exact => "tam",
            Self::Starts => "başında",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub id: i64,
    pub trigger: String,
    pub response: String,
    pub mode: MatchMode,
}

/// Noktalama ve fazla boşlukları atar: "Selam,  nasılsın?" → "selam nasılsın"
pub fn normalize(text: &str) -> String {
    tr_lower(text)
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl Rule {
    pub fn matches(&self, normalized_message: &str) -> bool {
        let t = self.trigger.as_str();
        if t.is_empty() {
            return false;
        }
        match self.mode {
            MatchMode::Exact => normalized_message == t,
            MatchMode::Starts => normalized_message == t || normalized_message.starts_with(&format!("{t} ")),
            // Kelime sınırlarıyla: "sa" → "sa", "sa millet" eşleşir; "masa" eşleşmez.
            MatchMode::Word => format!(" {normalized_message} ").contains(&format!(" {t} ")),
        }
    }
}

#[derive(Default)]
pub struct AutoResponder {
    rules: DashMap<GuildId, Arc<Vec<Rule>>>,
    last_fired: DashMap<(i64, ChannelId), Instant>,
}

impl AutoResponder {
    pub async fn rules(&self, db: &PgPool, guild_id: GuildId) -> anyhow::Result<Arc<Vec<Rule>>> {
        if let Some(rules) = self.rules.get(&guild_id) {
            return Ok(Arc::clone(rules.value()));
        }
        let rows: Vec<(i64, String, String, String)> =
            sqlx::query_as("SELECT id, trigger, response, match_mode FROM autoresponses WHERE guild_id = $1 ORDER BY length(trigger) DESC")
                .bind(snowflake(guild_id.get())?)
                .fetch_all(db)
                .await?;
        let rules = Arc::new(
            rows.into_iter()
                .map(|(id, trigger, response, mode)| Rule { id, trigger, response, mode: MatchMode::from_db(&mode) })
                .collect::<Vec<_>>(),
        );
        self.rules.insert(guild_id, Arc::clone(&rules));
        Ok(rules)
    }

    /// Mesaja uyan ilk kuralın cevabı (`{kullanıcı}` doldurulmuş). Bekleme süresindeyse `None`.
    pub async fn response_for(&self, db: &PgPool, guild_id: GuildId, channel: ChannelId, author: UserId, content: &str) -> anyhow::Result<Option<String>> {
        let rules = self.rules(db, guild_id).await?;
        if rules.is_empty() {
            return Ok(None);
        }
        let message = normalize(content);
        let Some(rule) = rules.iter().find(|r| r.matches(&message)) else {
            return Ok(None);
        };
        let key = (rule.id, channel);
        if self.last_fired.get(&key).is_some_and(|t| t.elapsed() < COOLDOWN) {
            return Ok(None);
        }
        self.last_fired.insert(key, Instant::now());
        Ok(Some(rule.response.replace("{kullanıcı}", &format!("<@{author}>")).replace("{kullanici}", &format!("<@{author}>"))))
    }

    /// Ekler veya (aynı tetikleyici varsa) günceller; yeni eklendiyse `true`.
    pub async fn set(&self, db: &PgPool, guild_id: GuildId, trigger: &str, response: &str, mode: MatchMode, author: UserId) -> anyhow::Result<bool> {
        let inserted: bool = sqlx::query_scalar(
            "INSERT INTO autoresponses (guild_id, trigger, response, match_mode, created_by) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (guild_id, trigger) DO UPDATE SET response = EXCLUDED.response, match_mode = EXCLUDED.match_mode
             RETURNING (xmax = 0)",
        )
        .bind(snowflake(guild_id.get())?)
        .bind(trigger)
        .bind(response)
        .bind(mode.db())
        .bind(snowflake(author.get())?)
        .fetch_one(db)
        .await?;
        self.rules.remove(&guild_id);
        Ok(inserted)
    }

    pub async fn remove(&self, db: &PgPool, guild_id: GuildId, trigger: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM autoresponses WHERE guild_id = $1 AND trigger = $2")
            .bind(snowflake(guild_id.get())?)
            .bind(trigger)
            .execute(db)
            .await?;
        self.rules.remove(&guild_id);
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(trigger: &str, mode: MatchMode) -> Rule {
        Rule { id: 1, trigger: normalize(trigger), response: "x".to_owned(), mode }
    }

    #[test]
    fn normalizes_messages() {
        assert_eq!(normalize("  Selam,  NASILSIN?! "), "selam nasılsın");
        assert_eq!(normalize("İYİ GECELER"), "iyi geceler");
    }

    #[test]
    fn word_mode_respects_boundaries() {
        let r = rule("sa", MatchMode::Word);
        assert!(r.matches(&normalize("sa")));
        assert!(r.matches(&normalize("Sa millet!")));
        assert!(r.matches(&normalize("herkese sa")));
        assert!(!r.matches(&normalize("masa")));
        assert!(!r.matches(&normalize("saat kaç")));
        let phrase = rule("iyi geceler", MatchMode::Word);
        assert!(phrase.matches(&normalize("Herkese İYİ GECELER :)")));
    }

    #[test]
    fn exact_and_starts_modes() {
        let exact = rule("merhaba", MatchMode::Exact);
        assert!(exact.matches(&normalize("Merhaba!")));
        assert!(!exact.matches(&normalize("merhaba arkadaşlar")));
        let starts = rule("kural", MatchMode::Starts);
        assert!(starts.matches(&normalize("kural nedir")));
        assert!(!starts.matches(&normalize("kurallar")));
        assert!(!starts.matches(&normalize("bu kural")));
    }
}

/// Gerçek PostgreSQL (sahte sunucu ID'si 1). Elle: `cargo test live_autoresponder -- --ignored --nocapture`
#[cfg(test)]
mod live_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "gerçek PostgreSQL gerektirir"]
    async fn live_autoresponder() -> anyhow::Result<()> {
        let _ = dotenvy::dotenv();
        let config = crate::config::DatabaseConfig { url: std::env::var("DATABASE_URL")?, max_connections: 2 };
        let db = crate::db::connect(&config).await?;
        let guild = GuildId::new(1);
        sqlx::query("DELETE FROM autoresponses WHERE guild_id = 1").execute(&db).await?;
        let ar = AutoResponder::default();
        assert!(ar.set(&db, guild, "sa", "Aleyküm selam {kullanıcı}!", MatchMode::Word, UserId::new(1)).await?);
        assert!(!ar.set(&db, guild, "sa", "Aleyküm selam, hoş geldin {kullanıcı}!", MatchMode::Word, UserId::new(1)).await?);
        let channel = ChannelId::new(5);
        let reply = ar.response_for(&db, guild, channel, UserId::new(7), "Sa millet").await?;
        assert_eq!(reply.as_deref(), Some("Aleyküm selam, hoş geldin <@7>!"));
        // Bekleme süresi: hemen tekrar cevap vermez.
        assert_eq!(ar.response_for(&db, guild, channel, UserId::new(7), "sa").await?, None);
        assert_eq!(ar.response_for(&db, guild, ChannelId::new(6), UserId::new(7), "masa").await?, None);
        assert!(ar.remove(&db, guild, "sa").await?);
        assert!(ar.rules(&db, guild).await?.is_empty());
        Ok(())
    }
}
