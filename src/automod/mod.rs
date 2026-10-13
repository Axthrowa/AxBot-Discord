//! Gecikmesiz auto-mod: davet/link/küfür filtresi ve spam koruması.
//!
//! Önbellek katmanları:
//! - **L1 (bellek):** sunucu başına derlenmiş regex filtresi (`DashMap<GuildId, Arc<GuildFilter>>`).
//!   Mesaj başına kontrol tamamen bellekte ve senkron, I/O yok.
//! - **L2 (Redis):** serileştirilmiş ayarlar; bot yeniden başladığında PostgreSQL'e gitmeden doldurur.
//! - **L3 (PostgreSQL):** kalıcı kaynak.
//!
//! Spam sayaçları doğrudan Redis'te tutulur.

pub mod config;
mod filter;
pub mod rules;
mod spam;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use dashmap::DashMap;
use poise::serenity_prelude::{self as serenity, ChannelId, GuildId, Mentionable, MessageId, RoleId, User};
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use tracing::{debug, info, warn};

pub use config::AutoModSettings;
use filter::{BasePatterns, GuildFilter, Violation};
use rules::{Media, ModSettings, RestrictKind, Rule, RuleAction, RulePatterns};

use crate::data::Data;
use crate::modlog::AutoModLog;

/// Kanalda gösterilen uyarının otomatik silinme süresi.
const NOTICE_TTL: Duration = Duration::from_secs(5);

/// Aynı mesajın tekrar sayıldığı süre.
const REPEAT_WINDOW_SECS: u64 = 60;
/// Bundan kısa mesajlar ("ok", "evet") tekrar sayılmaz.
const REPEAT_MIN_CHARS: usize = 5;

pub struct AutoMod {
    base: Arc<BasePatterns>,
    rules: Arc<RulePatterns>,
    filters: DashMap<GuildId, Arc<GuildFilter>>,
    /// Auto-mod'un yeni sildiği mesajlar; olay logu bunları tekrar "silindi" diye loglamasın.
    recently_deleted: DashMap<MessageId, Instant>,
}

impl AutoMod {
    pub fn new() -> anyhow::Result<Self> {
        let base = BasePatterns::new().context("auto-mod regex kalıpları derlenemedi")?;
        let rules = RulePatterns::new().context("auto-mod kural kalıpları derlenemedi")?;
        Ok(Self {
            base: Arc::new(base),
            rules: Arc::new(rules),
            filters: DashMap::new(),
            recently_deleted: DashMap::new(),
        })
    }

    fn mark_deleted(&self, id: MessageId) {
        // Silme olayı birkaç saniye içinde gelir; eski kayıtlar ara sıra temizlenir.
        if self.recently_deleted.len() > 1_000 {
            self.recently_deleted.retain(|_, at| at.elapsed() < Duration::from_secs(60));
        }
        self.recently_deleted.insert(id, Instant::now());
    }

    /// Metinde Discord davet bağlantısı var mı? (denetim kaydı için)
    pub fn has_invite(&self, content: &str) -> bool {
        self.base.has_invite(content)
    }

    /// Metin sunucunun yasaklı kelime filtresine takılıyor mu? (resim isteği gibi mesaj dışı girdiler için)
    pub async fn has_profanity(&self, db: &PgPool, redis: &ConnectionManager, guild_id: GuildId, text: &str) -> bool {
        match self.filter_for(db, redis, guild_id).await {
            Ok(filter) => filter.has_profanity(&self.base, text),
            Err(_) => false,
        }
    }

    /// Mesaj auto-mod tarafından silindiyse `true` (ve kaydı düşürür).
    pub fn take_deleted(&self, id: MessageId) -> bool {
        self.recently_deleted.remove(&id).is_some()
    }

    /// L1'de varsa anında döner; yoksa L2/L3'ten yükleyip derler.
    async fn filter_for(
        &self,
        db: &PgPool,
        redis: &ConnectionManager,
        guild_id: GuildId,
    ) -> anyhow::Result<Arc<GuildFilter>> {
        if let Some(filter) = self.filters.get(&guild_id) {
            return Ok(Arc::clone(filter.value()));
        }

        let config = config::load(db, redis, guild_id).await?;
        // Uzun kelime listelerinde derleme birkaç ms sürebilir; async thread'leri bloklamasın.
        let filter = tokio::task::spawn_blocking(move || GuildFilter::compile(config))
            .await
            .context("filtre derleme görevi çöktü")?
            .context("auto-mod kelime filtresi derlenemedi")?;

        let filter = Arc::new(filter);
        self.filters.insert(guild_id, Arc::clone(&filter));
        debug!(guild = %guild_id, "Auto-mod filtresi derlendi ve önbelleğe alındı");
        Ok(filter)
    }

    /// Ayarlar değiştiğinde çağrılır: Redis güncellenir, L1 kopyası düşürülür
    /// (bir sonraki mesajda yeni ayarlarla derlenir).
    pub async fn refresh(
        &self,
        db: &PgPool,
        redis: &ConnectionManager,
        guild_id: GuildId,
    ) -> anyhow::Result<()> {
        let config = config::load_from_db(db, guild_id).await?;
        config::cache_in_redis(redis, guild_id, &config).await;
        self.filters.remove(&guild_id);
        Ok(())
    }
}

/// Kontrol edilecek mesajın, yeni mesaj ve düzenleme event'lerinde ortak olan kısmı.
pub struct Candidate<'a> {
    pub guild_id: GuildId,
    pub channel_id: ChannelId,
    pub message_id: MessageId,
    pub author: &'a User,
    /// Üyenin rolleri; event'te gelmediyse boş (o zaman yetkili muafiyeti uygulanamaz).
    pub roles: &'a [RoleId],
    pub content: &'a str,
    pub is_edit: bool,
    /// Eklerin özeti (kısıtlı kanallar için).
    pub media: Media,
}

/// Mesaj silindiyse `true` döner.
pub async fn inspect(ctx: &serenity::Context, data: &Data, msg: Candidate<'_>) -> anyhow::Result<bool> {
    if msg.author.bot {
        return Ok(false);
    }

    let automod = &data.automod;
    let filter = automod.filter_for(&data.db, &data.redis, msg.guild_id).await?;
    let legacy = &filter.settings;
    if !legacy.enabled {
        return Ok(false);
    }
    let settings = match data.settings.get::<ModSettings>(&data.db, msg.guild_id).await {
        Ok(settings) => settings,
        Err(e) => {
            warn!(guild = %msg.guild_id, error = ?e, "Moderasyon ayarları okunamadı, varsayılan kullanılıyor");
            Arc::new(ModSettings::default())
        }
    };
    if legacy.exempt_staff && is_staff(ctx, &msg) {
        return Ok(false);
    }
    if msg.roles.iter().any(|role| settings.exempt_roles.contains(&role.get())) {
        return Ok(false);
    }

    // Kısıtlı kanal: içerik türü uymuyorsa başka hiçbir kurala bakılmadan silinir.
    if let Some(kind) = settings.restriction(msg.channel_id.get()) {
        if !rules::allowed_in_restricted(&automod.rules, kind, msg.content, msg.media, crate::tags::PREFIX) {
            return enforce_restricted(ctx, data, &msg, kind, &settings).await;
        }
    }
    if msg.content.is_empty() {
        return Ok(false);
    }

    match find_violation(data, &msg, &filter, &settings).await {
        Some((violation, action)) => enforce(ctx, data, &msg, violation, action).await,
        None => Ok(false),
    }
}

/// İlk tetiklenen kuralı ve o kuralın eylemini döner. Sıra: en ciddi ve en ucuz denetimler önce;
/// Redis'e giden sayaçlar (tekrar, spam) en sonda.
async fn find_violation(
    data: &Data,
    msg: &Candidate<'_>,
    filter: &GuildFilter,
    settings: &ModSettings,
) -> Option<(Violation, RuleAction)> {
    let automod = &data.automod;
    let legacy = &filter.settings;
    let content = msg.content;
    let action = |rule: Rule| Some(settings.action(rule, legacy)).filter(|a| *a != RuleAction::Off);

    if let Some(a) = action(Rule::Invites) {
        if automod.base.has_invite(content) {
            return Some((Violation::Invite, a));
        }
    }
    if let Some(a) = action(Rule::Links) {
        if automod.rules.has_blocked_link(content, &settings.allowed_domains) {
            return Some((Violation::Link, a));
        }
    }
    if let Some(a) = action(Rule::BadWords) {
        if filter.has_profanity(&automod.base, content) {
            return Some((Violation::Profanity, a));
        }
    }
    if let Some(a) = action(Rule::Zalgo) {
        if rules::is_zalgo(content) {
            return Some((Violation::Zalgo, a));
        }
    }
    if let Some(a) = action(Rule::Mentions) {
        if automod.rules.count_mentions(content) > usize::from(settings.mention_limit) {
            return Some((Violation::Mentions, a));
        }
    }
    if let Some(a) = action(Rule::Caps) {
        if rules::too_many_caps(content, settings.caps_percent, settings.caps_min_letters) {
            return Some((Violation::Caps, a));
        }
    }
    if let Some(a) = action(Rule::Emojis) {
        if automod.rules.count_emojis(content) > usize::from(settings.emoji_limit) {
            return Some((Violation::Emojis, a));
        }
    }
    if let Some(a) = action(Rule::Spoilers) {
        if rules::count_spoilers(content) > usize::from(settings.spoiler_limit) {
            return Some((Violation::Spoilers, a));
        }
    }
    // Düzenlemeler tekrar ya da spam sayılmaz.
    if msg.is_edit {
        return None;
    }
    if let Some(a) = action(Rule::Repeated) {
        if content.chars().count() >= REPEAT_MIN_CHARS && is_repeated(data, msg, settings.repeat_limit).await {
            return Some((Violation::Repeated, a));
        }
    }
    if let Some(a) = action(Rule::Spam) {
        if let Some(violation) = check_spam(data, msg, legacy).await {
            return Some((violation, a));
        }
    }
    None
}

/// Aynı üye aynı mesajı bir dakika içinde `limit` kez gönderdiyse `true`.
async fn is_repeated(data: &Data, msg: &Candidate<'_>, limit: u16) -> bool {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    msg.content.trim().to_lowercase().hash(&mut hasher);
    let key = format!("automod:rep:{}:{}:{:x}", msg.guild_id, msg.author.id, hasher.finish());
    let mut conn = data.redis.clone();
    let counted: redis::RedisResult<(i64,)> = redis::pipe()
        .atomic()
        .cmd("SET")
        .arg(&key)
        .arg(0)
        .arg("EX")
        .arg(REPEAT_WINDOW_SECS)
        .arg("NX")
        .ignore()
        .cmd("INCR")
        .arg(&key)
        .query_async(&mut conn)
        .await;
    match counted {
        Ok((count,)) => count >= i64::from(limit.max(2)),
        Err(e) => {
            // Redis geçici olarak yoksa kimse cezalandırılmaz.
            warn!(error = %e, "Tekrar sayacı güncellenemedi");
            false
        }
    }
}

async fn check_spam(data: &Data, msg: &Candidate<'_>, settings: &AutoModSettings) -> Option<Violation> {
    let count = match spam::register_message(
        &data.redis,
        msg.guild_id,
        msg.author.id,
        settings.spam_window_secs,
    )
    .await
    {
        Ok(count) => count,
        Err(e) => {
            // Redis geçici olarak yoksa mesajları cezalandırmak yerine spam kontrolünü atla.
            warn!(error = %e, "Spam sayacı güncellenemedi");
            return None;
        }
    };

    let limit = i64::from(settings.spam_max_messages);
    (count > limit).then_some(Violation::Spam {
        first: count == limit + 1,
    })
}

/// Sunucu sahibi, Yönetici veya Mesajları Yönet iznine sahip üyeler muaftır.
/// Tamamen önbellekten hesaplanır, HTTP isteği yok.
fn is_staff(ctx: &serenity::Context, msg: &Candidate<'_>) -> bool {
    let Some(guild) = ctx.cache.guild(msg.guild_id) else {
        return false;
    };
    if guild.owner_id == msg.author.id {
        return true;
    }

    let mut permissions = guild
        .roles
        .get(&msg.guild_id.everyone_role())
        .map(|role| role.permissions)
        .unwrap_or_default();
    for role_id in msg.roles {
        if let Some(role) = guild.roles.get(role_id) {
            permissions |= role.permissions;
        }
    }

    permissions.intersects(serenity::Permissions::ADMINISTRATOR | serenity::Permissions::MANAGE_MESSAGES)
}

/// Kanalda kısa süre görünen uyarı.
async fn notice(ctx: &serenity::Context, channel_id: ChannelId, text: String) {
    match channel_id.say(&ctx.http, text).await {
        Ok(sent) => {
            let http = Arc::clone(&ctx.http);
            tokio::spawn(async move {
                tokio::time::sleep(NOTICE_TTL).await;
                if let Err(e) = channel_id.delete_message(&http, sent.id).await {
                    debug!(error = %e, "Auto-mod uyarısı silinemedi");
                }
            });
        }
        Err(e) => warn!(channel = %channel_id, error = %e, "Auto-mod uyarısı gönderilemedi"),
    }
}

async fn delete(ctx: &serenity::Context, data: &Data, msg: &Candidate<'_>) -> bool {
    if let Err(e) = msg.channel_id.delete_message(&ctx.http, msg.message_id).await {
        warn!(
            guild = %msg.guild_id,
            channel = %msg.channel_id,
            error = %e,
            "Auto-mod mesajı silemedi (botun 'Mesajları Yönet' izni var mı?)"
        );
        return false;
    }
    data.automod.mark_deleted(msg.message_id);
    true
}

/// Kısıtlı kanala uymayan mesajı siler; ayara göre kanalda ve/veya özelden haber verir.
async fn enforce_restricted(
    ctx: &serenity::Context,
    data: &Data,
    msg: &Candidate<'_>,
    kind: RestrictKind,
    settings: &ModSettings,
) -> anyhow::Result<bool> {
    if !delete(ctx, data, msg).await {
        return Ok(false);
    }
    let what = match kind {
        RestrictKind::Commands => "sadece komut kullanılabilir",
        RestrictKind::Images => "sadece resim paylaşılabilir",
        RestrictKind::Videos => "sadece video paylaşılabilir",
    };
    info!(guild = %msg.guild_id, channel = %msg.channel_id, user = %msg.author.id, kind = kind.code(), "Kısıtlı kanalda mesaj silindi");
    if settings.restricted_reply {
        notice(ctx, msg.channel_id, format!("⚠️ {}, bu kanalda {what}.", msg.author.id.mention())).await;
    }
    if settings.restricted_dm {
        let text = format!("⚠️ {} kanalında {what}; mesajın silindi.", msg.channel_id.mention());
        if let Err(e) = msg.author.direct_message(ctx, serenity::CreateMessage::new().content(text)).await {
            debug!(user = %msg.author.id, error = %e, "Kısıtlı kanal DM'i gönderilemedi");
        }
    }
    Ok(true)
}

async fn enforce(
    ctx: &serenity::Context,
    data: &Data,
    msg: &Candidate<'_>,
    violation: Violation,
    action: RuleAction,
) -> anyhow::Result<bool> {
    let deleted = action.deletes() && delete(ctx, data, msg).await;
    if action.deletes() && !deleted {
        return Ok(false);
    }

    info!(
        guild = %msg.guild_id,
        channel = %msg.channel_id,
        user = %msg.author.id,
        reason = violation.reason(),
        action = action.code(),
        edited = msg.is_edit,
        "Auto-mod kuralı tetiklendi"
    );

    // Spam'de sadece ilk ihlal bildirilir, loglanır ve uyarılır; log kanalı taşmasın.
    if !violation.should_notify() {
        return Ok(deleted);
    }

    data.modlog
        .automod(
            &ctx.http,
            &data.db,
            msg.guild_id,
            AutoModLog {
                channel_id: msg.channel_id,
                author: msg.author,
                reason: violation.reason(),
                content: msg.content,
                deleted,
            },
        )
        .await;

    notice(ctx, msg.channel_id, format!("⚠️ {}, {}", msg.author.id.mention(), violation.notice())).await;

    if action.warns() {
        let reason = format!("Otomatik moderasyon: {}", violation.reason());
        match crate::moderation::auto::warn_by_bot(ctx, data, msg.guild_id, msg.author, &reason).await {
            // Otomatik ceza uygulandıysa kanalda kalıcı olarak duyurulur.
            Ok(Some(punishment)) => {
                if let Err(e) = msg.channel_id.say(&ctx.http, punishment).await {
                    debug!(error = %e, "Otomatik ceza duyurusu gönderilemedi");
                }
            }
            Ok(None) => {}
            Err(e) => warn!(guild = %msg.guild_id, user = %msg.author.id, error = ?e, "Otomatik uyarı kaydedilemedi"),
        }
    }

    Ok(deleted)
}
