//! İstatistik (sayaç) kanalları: "👥 Üyeler: 123" gibi adı otomatik güncellenen kilitli ses kanalları.
//!
//! Discord bir kanalın adının 10 dakikada en fazla 2 kez değiştirilmesine izin verir; bu yüzden
//! adlar arka planda 10 dakikada bir, sadece değer değiştiyse güncellenir. Sayılar bot
//! önbelleğinden okunur (ağ isteği yok).

use std::time::Duration;

use poise::serenity_prelude::{
    self as serenity, ChannelId, ChannelType, CreateChannel, EditChannel, GuildId,
    PermissionOverwrite, PermissionOverwriteType, Permissions,
};
use sqlx::PgPool;
use tracing::{debug, warn};

use crate::db::snowflake;
use crate::guild_config::to_id;

const UPDATE_INTERVAL: Duration = Duration::from_secs(10 * 60);
pub const MAX_PER_GUILD: i64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum StatKind {
    #[name = "Tüm üyeler"]
    Members,
    #[name = "İnsanlar (bot hariç)"]
    Humans,
    #[name = "Botlar"]
    Bots,
    #[name = "Sunucu takviyeleri (boost)"]
    Boosts,
    #[name = "Üye hedefi"]
    Goal,
    #[name = "Metin kanalları"]
    TextChannels,
    #[name = "Ses kanalları"]
    VoiceChannels,
    #[name = "Tüm kanallar"]
    Channels,
    #[name = "Rol sayısı"]
    Roles,
    #[name = "Roldeki üyeler"]
    RoleMembers,
    #[name = "Çevrimiçi üyeler"]
    Online,
    #[name = "Çevrimiçi rekoru"]
    RecordOnline,
    #[name = "Kripto para fiyatı"]
    Crypto,
}

impl StatKind {
    pub fn db(self) -> &'static str {
        match self {
            Self::Members => "members",
            Self::Humans => "humans",
            Self::Bots => "bots",
            Self::Boosts => "boosts",
            Self::Goal => "goal",
            Self::TextChannels => "text_channels",
            Self::VoiceChannels => "voice_channels",
            Self::Channels => "channels",
            Self::Roles => "roles",
            Self::RoleMembers => "role_members",
            Self::Online => "online",
            Self::RecordOnline => "record_online",
            Self::Crypto => "crypto",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        Some(match value {
            "members" => Self::Members,
            "humans" => Self::Humans,
            "bots" => Self::Bots,
            "boosts" => Self::Boosts,
            "goal" => Self::Goal,
            "text_channels" => Self::TextChannels,
            "voice_channels" => Self::VoiceChannels,
            "channels" => Self::Channels,
            "roles" => Self::Roles,
            "role_members" => Self::RoleMembers,
            "online" => Self::Online,
            "record_online" => Self::RecordOnline,
            "crypto" => Self::Crypto,
            _ => return None,
        })
    }

    pub fn default_template(self) -> &'static str {
        match self {
            Self::Members => "👥 Üyeler: {sayı}",
            Self::Humans => "👤 İnsanlar: {sayı}",
            Self::Bots => "🤖 Botlar: {sayı}",
            Self::Boosts => "🚀 Takviye: {sayı}",
            Self::Goal => "🎯 Hedef: {sayı}/{hedef}",
            Self::TextChannels => "💬 Metin kanalları: {sayı}",
            Self::VoiceChannels => "🔊 Ses kanalları: {sayı}",
            Self::Channels => "📁 Kanallar: {sayı}",
            Self::Roles => "🏷️ Roller: {sayı}",
            Self::RoleMembers => "👥 {rol}: {sayı}",
            Self::Online => "🟢 Çevrimiçi: {sayı}",
            Self::RecordOnline => "🏆 Çevrimiçi rekoru: {sayı}",
            Self::Crypto => "💰 {ad}: ${fiyat}",
        }
    }
}

struct Counts {
    members: u64,
    humans: u64,
    bots: u64,
    boosts: u64,
    text_channels: u64,
    voice_channels: u64,
    channels: u64,
    roles: u64,
    online: u64,
    /// Sayılan rolün adı ve (önbellekteki) üye sayısı.
    role: Option<(String, u64)>,
}

/// Sayıları bot önbelleğinden okur.
fn counts(cache: &serenity::Cache, guild_id: GuildId, role: Option<u64>) -> Option<Counts> {
    let guild = cache.guild(guild_id)?;
    let kind_count = |kinds: &[ChannelType]| guild.channels.values().filter(|c| kinds.contains(&c.kind)).count() as u64;
    let role = role.filter(|id| *id != 0).map(serenity::RoleId::new).and_then(|id| {
        let name = guild.roles.get(&id)?.name.clone();
        Some((name, guild.members.values().filter(|m| m.roles.contains(&id)).count() as u64))
    });
    let bots = guild.members.values().filter(|m| m.user.bot).count() as u64;
    let members = guild.member_count.max(guild.members.len() as u64);
    Some(Counts {
        members,
        humans: members.saturating_sub(bots),
        bots,
        boosts: guild.premium_subscription_count.unwrap_or(0),
        text_channels: kind_count(&[ChannelType::Text, ChannelType::News, ChannelType::Forum]),
        voice_channels: kind_count(&[ChannelType::Voice, ChannelType::Stage]),
        // Kategoriler kanal sayılmaz.
        channels: guild.channels.values().filter(|c| c.kind != ChannelType::Category).count() as u64,
        // @everyone rol sayılmaz.
        roles: (guild.roles.len() as u64).saturating_sub(1),
        // Presence Intent kapalıysa bu liste boştur.
        online: guild.presences.values().filter(|p| p.status != serenity::OnlineStatus::Offline).count() as u64,
        role,
    })
}

/// Developer Portal'da Presence Intent açık mı? (açılışta `main` belirler)
static PRESENCE_INTENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_presence_intent(enabled: bool) {
    PRESENCE_INTENT.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub fn presence_intent() -> bool {
    PRESENCE_INTENT.load(std::sync::atomic::Ordering::Relaxed)
}

fn online_count(cache: &serenity::Cache, guild_id: GuildId) -> Option<u64> {
    counts(cache, guild_id, None).map(|c| c.online)
}

/// CoinGecko para kimliği: küçük harf, rakam ve tire (ör. `bitcoin`, `shiba-inu`).
pub fn valid_coin(id: &str) -> bool {
    (1..=50).contains(&id.len()) && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// 67890.5 → "67.891", 3.14159 → "3,14", 0.00001234 → "0,00001234"
pub fn format_price(price: f64) -> String {
    if !price.is_finite() || price < 0.0 {
        return "—".to_owned();
    }
    if price >= 1000.0 {
        let whole = price.round() as u64;
        let digits = whole.to_string();
        let mut out = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push('.');
            }
            out.push(c);
        }
        out
    } else if price >= 1.0 {
        format!("{price:.2}").replace('.', ",")
    } else {
        let text = format!("{price:.8}");
        text.trim_end_matches('0').trim_end_matches('.').replace('.', ",")
    }
}

/// Paranın dolar fiyatı (CoinGecko, anahtar gerekmez). Para bulunamazsa hata.
pub async fn coin_price(client: &reqwest::Client, coin: &str) -> anyhow::Result<String> {
    anyhow::ensure!(valid_coin(coin), "geçersiz para kimliği");
    let json: serde_json::Value = client
        .get("https://api.coingecko.com/api/v3/simple/price")
        .query(&[("ids", coin), ("vs_currencies", "usd")])
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let price = json.get(coin).and_then(|c| c.get("usd")).and_then(serde_json::Value::as_f64).ok_or_else(|| anyhow::anyhow!("para bulunamadı"))?;
    Ok(format_price(price))
}

/// Kanal adını hesaplar (Discord sınırı: 100 karakter).
/// `extra`: kripto sayacında (para adı, fiyat metni).
pub fn render(
    template: &str,
    kind: StatKind,
    cache: &serenity::Cache,
    guild_id: GuildId,
    goal: Option<u32>,
    role: Option<u64>,
    extra: Option<(&str, &str)>,
) -> Option<String> {
    let c = counts(cache, guild_id, role)?;
    let (role_name, role_members) = c.role.clone().unwrap_or_else(|| ("silinmiş rol".to_owned(), 0));
    let value = match kind {
        StatKind::Members | StatKind::Goal => c.members,
        StatKind::Humans => c.humans,
        StatKind::Bots => c.bots,
        StatKind::Boosts => c.boosts,
        StatKind::TextChannels => c.text_channels,
        StatKind::VoiceChannels => c.voice_channels,
        StatKind::Channels => c.channels,
        StatKind::Roles => c.roles,
        StatKind::RoleMembers => role_members,
        StatKind::Online => c.online,
        // Rekor `goal` sütununda saklanır.
        StatKind::RecordOnline => c.online.max(u64::from(goal.unwrap_or(0))),
        StatKind::Crypto => 0,
    };
    let (coin, price) = extra.unwrap_or(("?", "—"));
    let goal_text = goal.map_or_else(String::new, |g| g.to_string());
    let name = if kind == StatKind::Goal && goal.is_some_and(|g| value >= u64::from(g)) {
        format!("🎉 Hedefe ulaşıldı: {value}")
    } else {
        template
            .replace("{sayı}", &value.to_string())
            .replace("{sayi}", &value.to_string())
            .replace("{hedef}", &goal_text)
            .replace("{rol}", &role_name)
            .replace("{ad}", coin)
            .replace("{fiyat}", price)
    };
    Some(name.chars().take(100).collect())
}

/// Kilitli ses kanalını oluşturur ve kaydeder.
#[allow(clippy::too_many_arguments)]
pub async fn create(
    cache: &serenity::Cache,
    http: &serenity::Http,
    db: &PgPool,
    guild_id: GuildId,
    kind: StatKind,
    goal: Option<u32>,
    role: Option<serenity::RoleId>,
    category: Option<ChannelId>,
    coin: Option<(&str, &str)>,
) -> anyhow::Result<ChannelId> {
    let template = kind.default_template();
    let role = role.map(serenity::RoleId::get);
    let name = render(template, kind, cache, guild_id, goal, role, coin).unwrap_or_else(|| template.to_owned());
    let bot_id = cache.current_user().id;
    let overwrites = vec![
        PermissionOverwrite {
            allow: Permissions::VIEW_CHANNEL,
            deny: Permissions::CONNECT,
            kind: PermissionOverwriteType::Role(guild_id.everyone_role()),
        },
        PermissionOverwrite {
            allow: Permissions::VIEW_CHANNEL | Permissions::CONNECT | Permissions::MANAGE_CHANNELS,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(bot_id),
        },
    ];
    let mut builder = CreateChannel::new(&name)
        .kind(ChannelType::Voice)
        .permissions(overwrites)
        .audit_log_reason("Sayaç kanalı");
    if let Some(category) = category {
        builder = builder.category(category);
    }
    let channel = guild_id.create_channel(http, builder).await?;

    sqlx::query(
        "INSERT INTO stat_channels (channel_id, guild_id, kind, template, goal, last_name, role_id, extra)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(snowflake(channel.id.get())?)
    .bind(snowflake(guild_id.get())?)
    .bind(kind.db())
    .bind(template)
    .bind(goal.map(i32::try_from).transpose()?)
    .bind(&name)
    .bind(role.map(snowflake).transpose()?)
    .bind(coin.map(|(id, _)| id))
    .execute(db)
    .await?;
    Ok(channel.id)
}

pub async fn count(db: &PgPool, guild_id: GuildId) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM stat_channels WHERE guild_id = $1")
        .bind(snowflake(guild_id.get())?)
        .fetch_one(db)
        .await?)
}

/// (kanal, tür, hedef) listesi.
pub async fn list(db: &PgPool, guild_id: GuildId) -> anyhow::Result<Vec<(ChannelId, StatKind, Option<u32>)>> {
    let rows: Vec<(i64, String, Option<i32>)> =
        sqlx::query_as("SELECT channel_id, kind, goal FROM stat_channels WHERE guild_id = $1 ORDER BY channel_id")
            .bind(snowflake(guild_id.get())?)
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, kind, goal)| {
            Some((
                ChannelId::new(to_id(Some(id))?),
                StatKind::from_db(&kind)?,
                goal.and_then(|g| u32::try_from(g).ok()),
            ))
        })
        .collect())
}

/// Kanal elle silindiyse (veya `/sayaç sil`) kaydı kaldırır.
pub async fn on_channel_deleted(db: &PgPool, channel_id: ChannelId) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM stat_channels WHERE channel_id = $1")
        .bind(snowflake(channel_id.get())?)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub fn spawn_updater(ctx: serenity::Context, db: PgPool) {
    tokio::spawn(async move {
        let client = match crate::net::http_client() {
            Ok(client) => client,
            Err(e) => {
                warn!(error = ?e, "Sayaç kanalları için HTTP istemcisi kurulamadı");
                return;
            }
        };
        let mut ticker = tokio::time::interval(UPDATE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            crate::supervise::round("sayaç kanalları", std::time::Duration::from_secs(300), async {
                if let Err(e) = update_all(&ctx, &db, &client).await {
                    warn!(error = ?e, "Sayaç kanalları güncellenemedi");
                }
            })
            .await;
        }
    });
}

#[derive(sqlx::FromRow)]
struct Row {
    channel_id: i64,
    guild_id: i64,
    kind: String,
    template: String,
    goal: Option<i32>,
    last_name: Option<String>,
    role_id: Option<i64>,
    extra: Option<String>,
}

async fn update_all(ctx: &serenity::Context, db: &PgPool, client: &reqwest::Client) -> anyhow::Result<()> {
    // Aynı para birden çok sayaçta olabilir; tur başına bir kez sorulur.
    let mut prices: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    let rows: Vec<Row> = sqlx::query_as("SELECT channel_id, guild_id, kind, template, goal, last_name, role_id, extra FROM stat_channels")
        .fetch_all(db)
        .await?;
    for row in rows {
        let (Some(channel), Some(guild), Some(kind)) = (to_id(Some(row.channel_id)), to_id(Some(row.guild_id)), StatKind::from_db(&row.kind)) else {
            continue;
        };
        let (channel, guild) = (ChannelId::new(channel), GuildId::new(guild));
        let mut goal = row.goal.and_then(|g| u32::try_from(g).ok());
        if kind == StatKind::RecordOnline {
            // Rekor kırıldıysa kalıcı olarak yazılır.
            let online = online_count(&ctx.cache, guild).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
            if online > goal.unwrap_or(0) {
                goal = Some(online);
                sqlx::query("UPDATE stat_channels SET goal = $2 WHERE channel_id = $1")
                    .bind(row.channel_id)
                    .bind(i32::try_from(online)?)
                    .execute(db)
                    .await?;
            }
        }
        let price = match (kind, row.extra.as_deref()) {
            (StatKind::Crypto, Some(coin)) => {
                if !prices.contains_key(coin) {
                    let fetched = coin_price(client, coin).await.ok();
                    prices.insert(coin.to_owned(), fetched);
                }
                // Fiyat okunamadıysa kanal adı olduğu gibi kalır.
                let Some(Some(price)) = prices.get(coin).cloned() else {
                    continue;
                };
                Some((coin.to_owned(), price))
            }
            _ => None,
        };
        let extra = price.as_ref().map(|(coin, price)| (coin.as_str(), price.as_str()));
        let Some(name) = render(&row.template, kind, &ctx.cache, guild, goal, to_id(row.role_id), extra) else {
            continue; // Sunucu önbellekte değil (bot çıkarılmış olabilir).
        };
        if row.last_name.as_deref() == Some(name.as_str()) {
            continue;
        }
        match channel.edit(ctx, EditChannel::new().name(&name)).await {
            Ok(_) => {
                sqlx::query("UPDATE stat_channels SET last_name = $2 WHERE channel_id = $1")
                    .bind(row.channel_id)
                    .bind(&name)
                    .execute(db)
                    .await?;
            }
            Err(e) => debug!(channel = %channel, error = %e, "Sayaç kanalı yeniden adlandırılamadı"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_prices() {
        assert_eq!(format_price(67_890.5), "67.891");
        assert_eq!(format_price(1_234_567.0), "1.234.567");
        assert_eq!(format_price(7.891), "7,89");
        assert_eq!(format_price(0.000_012_34), "0,00001234");
        assert_eq!(format_price(0.5), "0,5");
        assert_eq!(format_price(f64::NAN), "—");
    }

    #[test]
    fn validates_coin_ids() {
        assert!(valid_coin("bitcoin") && valid_coin("shiba-inu") && valid_coin("usd-coin2"));
        assert!(!valid_coin("") && !valid_coin("Bitcoin") && !valid_coin("btc&vs=x") && !valid_coin(&"a".repeat(51)));
    }

    #[test]
    fn kinds_round_trip() {
        for kind in [StatKind::Members, StatKind::RoleMembers, StatKind::Online, StatKind::RecordOnline, StatKind::Crypto] {
            assert_eq!(StatKind::from_db(kind.db()), Some(kind));
        }
    }
}
