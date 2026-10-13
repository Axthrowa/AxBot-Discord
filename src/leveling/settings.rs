//! Seviye eklentisinin sunucuya özel ayarları ve ödül rolü kuralları.

use poise::serenity_prelude::RoleId;
use serde::{Deserialize, Serialize};

use super::Reward;
use crate::settings::PluginConfig;

/// Seviye atlama duyurusunun gideceği yer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Announce {
    Off,
    /// Mesajın yazıldığı kanal.
    #[default]
    Current,
    Dm,
    /// `announce_channel`.
    Channel,
}

impl Announce {
    pub const ALL: [(Self, &'static str, &'static str); 4] = [
        (Self::Off, "off", "Kapalı"),
        (Self::Current, "current", "Mesajın yazıldığı kanal"),
        (Self::Dm, "dm", "Özel mesaj"),
        (Self::Channel, "channel", "Belirli bir kanal"),
    ];

    #[cfg(test)]
    pub fn code(self) -> &'static str {
        Self::ALL.iter().find(|(a, _, _)| *a == self).map_or("current", |(_, code, _)| code)
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, c, _)| *c == code).map(|(a, _, _)| *a)
    }
}

/// Seçilebilir TP oranları (yüzde).
pub const RATES: [u16; 8] = [25, 50, 75, 100, 150, 200, 250, 300];
pub const DEFAULT_MESSAGE: &str = "🎉 {kullanıcı} **{seviye}. seviyeye** ulaştı!";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LevelSettings {
    pub enabled: bool,
    pub announce: Announce,
    pub announce_channel: Option<u64>,
    /// Boşsa `DEFAULT_MESSAGE`. Değişkenler: `{kullanıcı}`, `{isim}`, `{seviye}`, `{sunucu}`.
    pub message: Option<String>,
    /// `true`: kazanılan tüm ödül rolleri kalır; `false`: sadece en yükseği.
    pub stack_rewards: bool,
    /// Üye XP kaybedip seviyesi düşerse ödül rolü geri alınır.
    pub remove_on_loss: bool,
    /// Kazanılan XP'nin yüzdesi (100 = normal).
    pub xp_rate: u16,
    /// `true`: sadece listedeki roller XP kazanır; `false`: listedekiler kazanamaz.
    pub roles_allow_only: bool,
    /// `true`: sadece listedeki kanallarda XP kazanılır; `false`: listedekilerde kazanılmaz.
    pub channels_allow_only: bool,
    /// Sıralama sayfası giriş yapmadan da görülebilir.
    pub public_leaderboard: bool,
}

impl Default for LevelSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            announce: Announce::Current,
            announce_channel: None,
            message: None,
            stack_rewards: true,
            remove_on_loss: false,
            xp_rate: 100,
            roles_allow_only: false,
            channels_allow_only: false,
            public_leaderboard: false,
        }
    }
}

impl PluginConfig for LevelSettings {
    const KEY: &'static str = "levels";
}

impl LevelSettings {
    pub fn template(&self) -> &str {
        self.message.as_deref().filter(|m| !m.trim().is_empty()).unwrap_or(DEFAULT_MESSAGE)
    }
}

/// Duyuru şablonunu doldurur.
pub fn render_message(template: &str, mention: &str, name: &str, level: u32, guild: &str) -> String {
    template
        .replace("{kullanıcı}", mention)
        .replace("{kullanici}", mention)
        .replace("{isim}", name)
        .replace("{seviye}", &level.to_string())
        .replace("{sunucu}", guild)
}

/// Seviye `old` → `new` yükselince verilecek ve geri alınacak ödül rolleri.
///
/// Biriktirme açıksa aradaki tüm ödüller verilir. Kapalıysa sadece ulaşılan en yüksek ödül
/// verilir ve daha düşük seviyelerin ödülleri geri alınır.
pub fn roles_on_level_up(rewards: &[Reward], old: u32, new: u32, stack: bool) -> (Vec<RoleId>, Vec<RoleId>) {
    if stack {
        let grant = rewards.iter().filter(|r| r.level > old && r.level <= new).map(|r| r.role_id).collect();
        return (grant, Vec::new());
    }
    let Some(top) = rewards.iter().filter(|r| r.level <= new).max_by_key(|r| r.level) else {
        return (Vec::new(), Vec::new());
    };
    if top.level <= old {
        // Bu yükselişte yeni bir ödül seviyesine ulaşılmadı.
        return (Vec::new(), Vec::new());
    }
    let revoke = rewards
        .iter()
        .filter(|r| r.level < top.level && r.role_id != top.role_id)
        .map(|r| r.role_id)
        .collect();
    (vec![top.role_id], revoke)
}

/// Seviye `new`'e düşünce geri alınacak ödül rolleri (artık hak edilmeyenler).
pub fn roles_on_level_loss(rewards: &[Reward], new: u32) -> Vec<RoleId> {
    rewards.iter().filter(|r| r.level > new).map(|r| r.role_id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewards() -> Vec<Reward> {
        [(5, 50), (10, 100), (20, 200)]
            .into_iter()
            .map(|(level, role)| Reward { level, role_id: RoleId::new(role) })
            .collect()
    }

    fn ids(roles: &[RoleId]) -> Vec<u64> {
        roles.iter().map(|r| r.get()).collect()
    }

    #[test]
    fn stacking_grants_every_reward_in_range() {
        let (grant, revoke) = roles_on_level_up(&rewards(), 4, 10, true);
        assert_eq!(ids(&grant), vec![50, 100]);
        assert!(revoke.is_empty());
    }

    #[test]
    fn highest_only_replaces_lower_rewards() {
        let (grant, revoke) = roles_on_level_up(&rewards(), 9, 10, false);
        assert_eq!(ids(&grant), vec![100]);
        assert_eq!(ids(&revoke), vec![50]);
        // Yeni ödül seviyesine ulaşılmadıysa hiçbir şey değişmez.
        let (grant, revoke) = roles_on_level_up(&rewards(), 10, 12, false);
        assert!(grant.is_empty() && revoke.is_empty());
    }

    #[test]
    fn loss_revokes_rewards_above_new_level() {
        assert_eq!(ids(&roles_on_level_loss(&rewards(), 7)), vec![100, 200]);
        assert!(roles_on_level_loss(&rewards(), 20).is_empty());
    }

    #[test]
    fn old_records_without_new_fields_still_load() -> anyhow::Result<()> {
        let settings: LevelSettings = serde_json::from_str("{\"xp_rate\":200}")?;
        assert_eq!(settings.xp_rate, 200);
        assert!(settings.enabled && settings.stack_rewards);
        assert_eq!(settings.announce, Announce::Current);
        assert_eq!(render_message(settings.template(), "<@1>", "Ali", 3, "S"), "🎉 <@1> **3. seviyeye** ulaştı!");
        assert_eq!(Announce::from_code(Announce::Dm.code()), Some(Announce::Dm));
        Ok(())
    }
}
