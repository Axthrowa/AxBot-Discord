//! Discord'un kendi AutoMod kuralları (MEE6 / Dyno gibi panelden kurulur). Mesaj daha gönderilmeden
//! Discord tarafından engellenir; bot kapalıyken de çalışır.
//!
//! Bot en fazla dört kural yönetir (adları "AxBot · …"): hazır filtreler, yasaklı kelimeler, toplu etiket, spam.
//! Kural kimlikleri ayarlarda saklanır; panelden kaydedince kurallar oluşturulur, güncellenir ya da silinir.
//! Botun 'Sunucuyu Yönet' izni olmalı (susturma eylemi için 'Üyelere Zaman Aşımı Uygula' da).

use std::time::Duration;

use poise::serenity_prelude::automod::{Action, EventType, KeywordPresetType, Trigger};
use poise::serenity_prelude::{self as serenity, ChannelId, EditAutoModRule, GuildId, RoleId, RuleId};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::settings::PluginConfig;

pub const MAX_KEYWORDS: usize = 1000;
pub const MAX_KEYWORD_LEN: usize = 60;
pub const MAX_BLOCK_MESSAGE: usize = 150;
pub const MAX_MENTIONS: u8 = 50;
/// En uzun susturma: 4 hafta (Discord sınırı).
pub const MAX_TIMEOUT_MINUTES: u32 = 40_320;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NativeAutomodSettings {
    pub profanity: bool,
    pub sexual: bool,
    pub slurs: bool,
    /// Yasaklı kelimeler (Discord joker karakterleri: `*kelime*` içinde geçen, `kelime*` ile başlayan).
    pub keywords: Vec<String>,
    /// Yasaklı kelime yazanı bu kadar dakika sustur (0 = susturma).
    pub timeout_minutes: u32,
    /// Bir mesajda en fazla bu kadar farklı etiket (0 = kapalı).
    pub mention_limit: u8,
    /// Discord'un spam sezgisi.
    pub spam: bool,
    /// Engellenen mesajlar bu kanala bildirilir.
    pub alert_channel: Option<u64>,
    /// Mesajı engellenen üyeye gösterilecek açıklama (en fazla 150 karakter).
    pub block_message: String,
    /// Kurallardan etkilenmeyen roller.
    pub exempt_roles: Vec<u64>,
    /// Botun Discord'da oluşturduğu kuralların kimlikleri.
    pub rule_ids: RuleIds,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuleIds {
    pub presets: Option<u64>,
    pub keywords: Option<u64>,
    pub mentions: Option<u64>,
    pub spam: Option<u64>,
}

impl PluginConfig for NativeAutomodSettings {
    const KEY: &'static str = "discord_automod";
}

impl NativeAutomodSettings {
    pub fn is_active(&self) -> bool {
        self.profanity || self.sexual || self.slurs || !self.keywords.is_empty() || self.mention_limit > 0 || self.spam
    }

    fn presets(&self) -> Vec<KeywordPresetType> {
        [(self.profanity, KeywordPresetType::Profanity), (self.sexual, KeywordPresetType::SexualContent), (self.slurs, KeywordPresetType::Slurs)]
            .into_iter()
            .filter_map(|(on, preset)| on.then_some(preset))
            .collect()
    }

    fn actions(&self, timeout: bool) -> Vec<Action> {
        let message = self.block_message.trim();
        let mut actions = vec![Action::BlockMessage { custom_message: (!message.is_empty()).then(|| message.chars().take(MAX_BLOCK_MESSAGE).collect()) }];
        if let Some(channel) = self.alert_channel.filter(|c| *c != 0) {
            actions.push(Action::Alert(ChannelId::new(channel)));
        }
        if timeout && self.timeout_minutes > 0 {
            actions.push(Action::Timeout(Duration::from_secs(u64::from(self.timeout_minutes.min(MAX_TIMEOUT_MINUTES)) * 60)));
        }
        actions
    }
}

/// Panelden girilen kelime listesini temizler (satır ya da virgülle ayrılmış). Uygun değilse sebep kodu.
pub fn parse_keywords(raw: &str) -> Result<Vec<String>, &'static str> {
    let mut words: Vec<String> = Vec::new();
    for word in raw.split(['\n', ',']).map(str::trim).filter(|w| !w.is_empty()) {
        if word.chars().count() > MAX_KEYWORD_LEN {
            return Err("kelime");
        }
        let word = word.to_lowercase();
        if !words.contains(&word) {
            words.push(word);
        }
    }
    if words.len() > MAX_KEYWORDS {
        return Err("limit");
    }
    Ok(words)
}

/// Bir kuralı istenen hâle getirir: gerekmiyorsa siler, varsa düzenler, yoksa oluşturur. Yeni kimliği döner.
async fn sync_rule(http: &serenity::Http, guild_id: GuildId, current: Option<u64>, wanted: Option<EditAutoModRule<'_>>, errors: &mut Vec<String>, label: &str) -> Option<u64> {
    let current = current.filter(|id| *id != 0).map(RuleId::new);
    match (current, wanted) {
        (None, None) => None,
        (Some(id), None) => {
            if let Err(e) = guild_id.delete_automod_rule(http, id).await {
                // Elle silinmişse sorun değil.
                if !e.to_string().contains("Unknown") {
                    errors.push(format!("{label}: silinemedi ({e})"));
                    return Some(id.get());
                }
            }
            None
        }
        (Some(id), Some(builder)) => match guild_id.edit_automod_rule(http, id, builder.clone()).await {
            Ok(rule) => Some(rule.id.get()),
            // Discord'da elle silinmişse yeniden oluşturulur.
            Err(e) if e.to_string().contains("Unknown") => create(http, guild_id, builder, errors, label).await,
            Err(e) => {
                errors.push(format!("{label}: güncellenemedi ({e})"));
                Some(id.get())
            }
        },
        (None, Some(builder)) => create(http, guild_id, builder, errors, label).await,
    }
}

async fn create(http: &serenity::Http, guild_id: GuildId, builder: EditAutoModRule<'_>, errors: &mut Vec<String>, label: &str) -> Option<u64> {
    match guild_id.create_automod_rule(http, builder).await {
        Ok(rule) => Some(rule.id.get()),
        Err(e) => {
            errors.push(format!("{label}: oluşturulamadı ({e})"));
            None
        }
    }
}

/// Ayarları Discord'a uygular; güncel kural kimliklerini ayara yazar. Hatalar (varsa) metin olarak döner.
pub async fn apply(http: &serenity::Http, guild_id: GuildId, settings: &mut NativeAutomodSettings) -> Vec<String> {
    let exempt: Vec<RoleId> = settings.exempt_roles.iter().filter(|r| **r != 0).map(|r| RoleId::new(*r)).collect();
    let base = |name: &'static str, trigger: Trigger, actions: Vec<Action>| {
        EditAutoModRule::new()
            .name(name)
            .event_type(EventType::MessageSend)
            .trigger(trigger)
            .actions(actions)
            .exempt_roles(exempt.clone())
            .enabled(true)
            .audit_log_reason("AxBot paneli")
    };
    let presets = settings.presets();
    let wanted_presets = (!presets.is_empty()).then(|| base("AxBot · Hazır filtreler", Trigger::KeywordPreset { presets, allow_list: Vec::new() }, settings.actions(false)));
    let wanted_keywords = (!settings.keywords.is_empty()).then(|| {
        base(
            "AxBot · Yasaklı kelimeler",
            Trigger::Keyword { strings: settings.keywords.clone(), regex_patterns: Vec::new(), allow_list: Vec::new() },
            settings.actions(true),
        )
    });
    let wanted_mentions = (settings.mention_limit > 0)
        .then(|| base("AxBot · Toplu etiket", Trigger::MentionSpam { mention_total_limit: settings.mention_limit.min(MAX_MENTIONS) }, settings.actions(false)));
    let wanted_spam = settings.spam.then(|| base("AxBot · Spam", Trigger::Spam, settings.actions(false)));

    let mut errors = Vec::new();
    let ids = settings.rule_ids;
    settings.rule_ids = RuleIds {
        presets: sync_rule(http, guild_id, ids.presets, wanted_presets, &mut errors, "Hazır filtreler").await,
        keywords: sync_rule(http, guild_id, ids.keywords, wanted_keywords, &mut errors, "Yasaklı kelimeler").await,
        mentions: sync_rule(http, guild_id, ids.mentions, wanted_mentions, &mut errors, "Toplu etiket").await,
        spam: sync_rule(http, guild_id, ids.spam, wanted_spam, &mut errors, "Spam").await,
    };
    for error in &errors {
        warn!(guild = %guild_id, error, "Discord AutoMod kuralı uygulanamadı");
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keyword_lists() {
        assert_eq!(parse_keywords("Küfür\n*spam*, reklam*\n\n küfür "), Ok(vec!["küfür".to_owned(), "*spam*".to_owned(), "reklam*".to_owned()]));
        assert_eq!(parse_keywords(&"a".repeat(61)), Err("kelime"));
        assert_eq!(parse_keywords(""), Ok(Vec::new()));
    }

    #[test]
    fn actions_include_alert_and_timeout_only_when_set() {
        let mut settings = NativeAutomodSettings { block_message: "Bu kelime yasak".to_owned(), ..Default::default() };
        assert_eq!(settings.actions(true).len(), 1);
        settings.alert_channel = Some(5);
        settings.timeout_minutes = 10;
        assert_eq!(settings.actions(false).len(), 2);
        assert_eq!(settings.actions(true).len(), 3);
        assert!(!settings.is_active());
        settings.slurs = true;
        assert!(settings.is_active() && settings.presets() == vec![KeywordPresetType::Slurs]);
    }
}
