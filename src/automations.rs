//! Otomasyonlar: "şu olunca şunu yap" kuralları. Tetikleyiciler: mesaj, üye girişi/çıkışı, rol
//! verilmesi/alınması, tepki, alt başlık açılması. Eylemler: mesaj gönder, özelden yaz, rol ver/al,
//! tepki ekle, mesajı sil, alt başlık aç.
//!
//! Kurallar bellekte durur (`PluginSettings`); olay başına veritabanına gidilmez.
//! Güvenlik: roller her seferinde `rolepanel::check_role` ile denetlenir; bir üye için kısa sürede
//! çalışabilecek otomasyon sayısı sınırlıdır (birbirini tetikleyen kurallar sonsuz döngüye giremez).

use std::net::IpAddr;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateAllowedMentions, CreateMessage, CreateThread, GuildId, Mentionable, Message,
    ReactionType, RoleId, User, UserId,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use sqlx::PgPool;

use crate::data::Data;
use crate::reaction_roles;
use crate::settings::PluginConfig;

pub const MAX_AUTOMATIONS: usize = 20;
/// Bir üye için bu sürede en fazla bu kadar otomasyon çalışır.
const BURST_WINDOW: Duration = Duration::from_secs(10);
const BURST_LIMIT: u8 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    #[default]
    Message,
    MemberJoin,
    MemberLeave,
    RoleAdded,
    RoleRemoved,
    Reaction,
    ThreadCreated,
    StreamWentLive,
    StreamWentOffline,
}

impl Trigger {
    pub const ALL: [(Self, &'static str, &'static str); 9] = [
        (Self::Message, "message", "Mesaj gönderildiğinde"),
        (Self::MemberJoin, "member_join", "Üye sunucuya katıldığında"),
        (Self::MemberLeave, "member_leave", "Üye sunucudan ayrıldığında"),
        (Self::RoleAdded, "role_added", "Üyeye rol verildiğinde"),
        (Self::RoleRemoved, "role_removed", "Üyeden rol alındığında"),
        (Self::Reaction, "reaction", "Mesaja tepki eklendiğinde"),
        (Self::ThreadCreated, "thread_created", "Alt başlık açıldığında"),
        (Self::StreamWentLive, "stream_went_live", "Yayın açıldığında"),
        (Self::StreamWentOffline, "stream_went_offline", "Yayın kapandığında"),
    ];

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, c, _)| *c == code).map(|(t, _, _)| *t)
    }

    pub fn code(self) -> &'static str {
        Self::ALL.iter().find(|(t, _, _)| *t == self).map_or("", |(_, c, _)| c)
    }

    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|(t, _, _)| *t == self).map_or("", |(_, _, label)| label)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Automation {
    pub id: u32,
    pub name: String,
    pub enabled: bool,
    pub trigger: Trigger,
    // --- Koşullar (boş olan koşul aranmaz)
    /// Mesaj tetikleyicisi: mesaj bu metni içermeli (büyük/küçük harf fark etmez).
    pub keyword: Option<String>,
    /// Olay bu kanalda olmalı.
    pub channel: Option<u64>,
    /// Mesaj/tepki: üyede bu rol olmalı. Rol tetikleyicileri: verilen/alınan rol bu olmalı.
    pub role: Option<u64>,
    /// Tepki tetikleyicisi: eklenen emoji bu olmalı.
    pub emoji: Option<String>,
    /// Yayın tetikleyicisi: yayıncı adı/login ("elraenn") veya platform ("twitch", "kick").
    pub streamer: Option<String>,
    // --- Eylemler
    pub message: Option<String>,
    /// Mesajın gideceği kanal; boşsa olayın olduğu kanal.
    pub message_channel: Option<u64>,
    pub dm: Option<String>,
    pub add_roles: Vec<u64>,
    pub remove_roles: Vec<u64>,
    /// Tetikleyen mesaja eklenecek tepki.
    pub react: Option<String>,
    pub delete_message: bool,
    /// Tetikleyen mesajdan bu adla alt başlık açılır.
    pub thread_name: Option<String>,
    /// Gönderilen mesajdaki rol etiketleri bildirim göndersin.
    pub ping_roles: bool,
    // --- Webhook / Yerel Tetikleyici Eylemi
    /// Tetiklendiğinde HTTP POST isteği gönderilecek hedef URL.
    pub webhook_url: Option<String>,
    /// İsteğe bağlı Authorization veya x-api-key başlığı / gizli anahtar.
    pub webhook_secret: Option<String>,
    /// Gönderilecek özel JSON payload şablonu (boşsa varsayılan olay JSON'u gider).
    pub webhook_payload: Option<String>,
    /// `127.0.0.1` ve `localhost` adreslerine istek atılmasına açıkça izin verilsin mi?
    pub allow_localhost: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DryRunInput {
    pub trigger: Trigger,
    pub channel_id: Option<u64>,
    pub role_id: Option<u64>,
    pub member_roles: Vec<u64>,
    pub user_id: Option<u64>,
    pub user_name: Option<String>,
    pub content: Option<String>,
    pub emoji: Option<String>,
    pub platform: Option<String>,
    pub streamer: Option<String>,
    pub title: Option<String>,
    pub game: Option<String>,
    pub viewers: Option<u64>,
    pub guild_name: Option<String>,
    pub probe_webhook: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionCheck {
    pub name: String,
    pub passed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedAction {
    pub action_type: String,
    pub target: String,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDryRunResult {
    pub url_valid: bool,
    pub target_url: Option<String>,
    pub allow_localhost: bool,
    pub ssrf_status: String,
    pub headers_preview: Vec<String>,
    pub payload_preview: Option<String>,
    pub probe_attempted: bool,
    pub probe_status_code: Option<u16>,
    pub probe_latency_ms: Option<u64>,
    pub probe_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DryRunReport {
    pub matched: bool,
    pub condition_results: Vec<ConditionCheck>,
    pub rendered_message: Option<String>,
    pub rendered_dm: Option<String>,
    pub rendered_payload: Option<String>,
    pub planned_actions: Vec<PlannedAction>,
    pub webhook_check: Option<WebhookDryRunResult>,
}

impl Automation {
    pub async fn evaluate_dry_run(&self, input: &DryRunInput, client: Option<&reqwest::Client>) -> DryRunReport {
        let mut conditions = Vec::new();

        // 1. Durum / Aktiflik
        conditions.push(ConditionCheck {
            name: "Kural Durumu".to_owned(),
            passed: self.enabled,
            reason: if self.enabled {
                "Otomasyon kuralı aktif.".to_owned()
            } else {
                "Otomasyon kuralı devre dışı bırakılmış (kapalı).".to_owned()
            },
        });

        // 2. Tetikleyici Türü
        let trigger_passed = self.trigger == input.trigger;
        conditions.push(ConditionCheck {
            name: "Tetikleyici Türü".to_owned(),
            passed: trigger_passed,
            reason: if trigger_passed {
                format!("Tetikleyici eşleşti: {}", self.trigger.label())
            } else {
                format!(
                    "Tetikleyici uyuşmuyor (Kural: {}, Gelen Olay: {})",
                    self.trigger.label(),
                    input.trigger.label()
                )
            },
        });

        // 3. Kanal Koşulu
        if let Some(wanted_channel) = self.channel {
            let passed = input.channel_id == Some(wanted_channel);
            conditions.push(ConditionCheck {
                name: "Kanal Koşulu".to_owned(),
                passed,
                reason: if passed {
                    format!("Hedef kanal eşleşti (<#{wanted_channel}>).")
                } else {
                    format!(
                        "Kanal uyuşmuyor: beklenen <#{wanted_channel}>, gelen {}",
                        input.channel_id.map(|id| format!("<#{id}>")).unwrap_or_else(|| "yok".to_owned())
                    )
                },
            });
        } else {
            conditions.push(ConditionCheck {
                name: "Kanal Koşulu".to_owned(),
                passed: true,
                reason: "Kanal filtresi yok (tüm kanallar geçerli).".to_owned(),
            });
        }

        // 4. Rol Koşulu
        if let Some(wanted_role) = self.role {
            let passed = match self.trigger {
                Trigger::RoleAdded | Trigger::RoleRemoved => input.role_id == Some(wanted_role),
                _ => input.member_roles.contains(&wanted_role) || input.role_id == Some(wanted_role),
            };
            conditions.push(ConditionCheck {
                name: "Rol Koşulu".to_owned(),
                passed,
                reason: if passed {
                    format!("Rol koşulu sağlandı (<@&{wanted_role}>).")
                } else {
                    format!("Rol koşulu sağlanamadı: beklenen <@&{wanted_role}> üyede/olayda bulunamadı.")
                },
            });
        } else {
            conditions.push(ConditionCheck {
                name: "Rol Koşulu".to_owned(),
                passed: true,
                reason: "Rol filtresi yok.".to_owned(),
            });
        }

        // 5. Mesaj / Anahtar Kelime Koşulu
        if let Some(ref keyword) = self.keyword {
            let content_str = input.content.as_deref().unwrap_or_default();
            let passed = fold(content_str).contains(&fold(keyword));
            conditions.push(ConditionCheck {
                name: "Anahtar Kelime".to_owned(),
                passed,
                reason: if passed {
                    format!("İçerikte \"{keyword}\" anahtar kelimesi bulundu.")
                } else {
                    format!("İçerik \"{keyword}\" anahtar kelimesini içermiyor.")
                },
            });
        }

        // 6. Tepki Emojisi Koşulu
        if let Some(ref emoji) = self.emoji {
            let wanted = crate::reaction_roles::emoji_key(emoji);
            let input_key = input.emoji.as_deref().and_then(crate::reaction_roles::emoji_key);
            let passed = wanted.is_some() && wanted == input_key;
            conditions.push(ConditionCheck {
                name: "Tepki Emojisi".to_owned(),
                passed,
                reason: if passed {
                    format!("Tepki emojisi eşleşti: {emoji}")
                } else {
                    format!("Tepki emojisi uyuşmuyor: beklenen {emoji}, gelen {}", input.emoji.as_deref().unwrap_or("yok"))
                },
            });
        }

        // 7. Yayıncı veya Platform Koşulu
        if let Some(ref wanted_streamer) = self.streamer {
            let ws = fold(wanted_streamer.trim());
            let streamer_match = input.streamer.as_deref().map(|s| fold(s) == ws).unwrap_or(false);
            let platform_match = input.platform.as_deref().map(|p| fold(p) == ws).unwrap_or(false);
            let passed = streamer_match || platform_match;
            conditions.push(ConditionCheck {
                name: "Yayıncı / Platform".to_owned(),
                passed,
                reason: if passed {
                    format!("Yayıncı veya platform filtresi eşleşti: \"{wanted_streamer}\".")
                } else {
                    format!(
                        "Yayın filtresi uyuşmuyor: beklenen \"{}\", gelen yayıncı: \"{}\", platform: \"{}\".",
                        wanted_streamer,
                        input.streamer.as_deref().unwrap_or("yok"),
                        input.platform.as_deref().unwrap_or("yok")
                    )
                },
            });
        }

        let matched = conditions.iter().all(|c| c.passed);

        // Şablon Çözümleme (Render)
        let guild = input.guild_name.as_deref().unwrap_or("Örnek Sunucu");
        let content_text = input.content.as_deref().unwrap_or_default();
        let is_stream = matches!(self.trigger, Trigger::StreamWentLive | Trigger::StreamWentOffline);

        let mut mock_user = User::default();
        mock_user.id = UserId::new(input.user_id.unwrap_or(1001));
        mock_user.name = input.user_name.clone().unwrap_or_else(|| "OrnekUye".to_owned());

        let fill_text = |tpl: &str| -> String {
            if is_stream {
                render_stream(
                    tpl,
                    guild,
                    self.message_channel.or(input.channel_id),
                    input.streamer.as_deref().unwrap_or("Yayinci"),
                    input.platform.as_deref().unwrap_or("twitch"),
                    input.title.as_deref().unwrap_or("Yayın Başlığı"),
                )
            } else {
                render(
                    tpl,
                    &mock_user,
                    guild,
                    self.message_channel.or(input.channel_id),
                    self.role.or(input.role_id),
                    content_text,
                )
            }
        };

        let rendered_message = self.message.as_deref().map(fill_text);
        let rendered_dm = self.dm.as_deref().map(fill_text);

        // Planlanan Aksiyonlar
        let mut planned_actions = Vec::new();
        if let Some(ref msg) = rendered_message {
            let target_ch = self.message_channel.or(input.channel_id);
            planned_actions.push(PlannedAction {
                action_type: "Mesaj Gönder".to_owned(),
                target: target_ch.map(|c| format!("<#{c}>")).unwrap_or_else(|| "Olayın olduğu kanal".to_owned()),
                preview: msg.clone(),
            });
        }
        if let Some(ref dm) = rendered_dm {
            planned_actions.push(PlannedAction {
                action_type: "Özel Mesaj (DM)".to_owned(),
                target: format!("@{}", mock_user.name),
                preview: dm.clone(),
            });
        }
        if !self.add_roles.is_empty() {
            let roles_str = self.add_roles.iter().map(|r| format!("<@&{r}>")).collect::<Vec<_>>().join(", ");
            planned_actions.push(PlannedAction {
                action_type: "Rol Ver".to_owned(),
                target: format!("@{}", mock_user.name),
                preview: format!("Verilecek roller: {roles_str}"),
            });
        }
        if !self.remove_roles.is_empty() {
            let roles_str = self.remove_roles.iter().map(|r| format!("<@&{r}>")).collect::<Vec<_>>().join(", ");
            planned_actions.push(PlannedAction {
                action_type: "Rol Al".to_owned(),
                target: format!("@{}", mock_user.name),
                preview: format!("Alınacak roller: {roles_str}"),
            });
        }
        if let Some(ref emoji) = self.react {
            planned_actions.push(PlannedAction {
                action_type: "Tepki Ekle".to_owned(),
                target: "Tetikleyen Mesaj".to_owned(),
                preview: format!("Emoji: {emoji}"),
            });
        }
        if let Some(ref thread) = self.thread_name {
            planned_actions.push(PlannedAction {
                action_type: "Alt Başlık Aç".to_owned(),
                target: "Tetikleyen Mesaj".to_owned(),
                preview: format!("Başlık: {}", fill_text(thread)),
            });
        }
        if self.delete_message {
            planned_actions.push(PlannedAction {
                action_type: "Mesajı Sil".to_owned(),
                target: "Tetikleyen Mesaj".to_owned(),
                preview: "Tetikleyen mesaj sohbet kanalından silinir.".to_owned(),
            });
        }

        // Webhook Kontrolü ve Probe
        let mut webhook_check = None;
        let mut rendered_payload = None;

        if let Some(ref raw_url) = self.webhook_url.as_deref().filter(|u| !u.trim().is_empty()) {
            let val_res = validate_webhook_url(raw_url, self.allow_localhost);
            let url_valid = val_res.is_ok();
            let ssrf_status = match val_res {
                Ok(ref u) => {
                    let host = u.host_str().unwrap_or_default();
                    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
                        "Yerel Hedef (allow_localhost açık, izin verildi)".to_owned()
                    } else {
                        "SSRF Güvenli (Genel Webhook Uç Noktası)".to_owned()
                    }
                }
                Err(err) => format!("SSRF Engeli: {err}"),
            };

            let mut headers_preview = vec!["content-type: application/json".to_owned()];
            if self.webhook_secret.is_some() {
                headers_preview.push("Authorization: Bearer <GİZLİ_ANAHTAR>".to_owned());
                headers_preview.push("x-webhook-secret: <GİZLİ_ANAHTAR>".to_owned());
            }

            let default_payload = if is_stream {
                serde_json::json!({
                    "event": self.trigger.label(),
                    "trigger": self.trigger,
                    "platform": input.platform.as_deref().unwrap_or("twitch"),
                    "streamer": input.streamer.as_deref().unwrap_or("Yayinci"),
                    "title": input.title,
                    "game": input.game,
                    "viewers": input.viewers,
                })
            } else {
                serde_json::json!({
                    "event": self.trigger.label(),
                    "trigger": self.trigger,
                    "user_id": input.user_id.unwrap_or(1001),
                    "user_name": mock_user.name,
                    "channel_id": input.channel_id,
                    "content": input.content,
                })
            };

            let payload_str = if let Some(custom) = self.webhook_payload.as_deref().filter(|p| !p.trim().is_empty()) {
                fill_text(custom)
            } else {
                default_payload.to_string()
            };
            rendered_payload = Some(payload_str.clone());

            planned_actions.push(PlannedAction {
                action_type: "Webhook İsteği".to_owned(),
                target: raw_url.to_string(),
                preview: format!("Payload ({})", if self.webhook_payload.is_some() { "özel şablon" } else { "varsayılan JSON" }),
            });

            let mut probe_attempted = false;
            let mut probe_status_code = None;
            let mut probe_latency_ms = None;
            let mut probe_error = None;

            if input.probe_webhook && url_valid {
                if let (Some(client), Ok(target_url)) = (client, val_res) {
                    probe_attempted = true;
                    let start_t = Instant::now();
                    let mut req = client
                        .post(target_url.as_str())
                        .timeout(Duration::from_secs(4))
                        .header("X-AxBot-Dry-Run", "true")
                        .header("content-type", "application/json");

                    if let Some(ref secret) = self.webhook_secret {
                        req = req.header("x-webhook-secret", secret)
                            .header("Authorization", format!("Bearer {secret}"));
                    }

                    match req.body(payload_str.clone()).send().await {
                        Ok(resp) => {
                            probe_latency_ms = u64::try_from(start_t.elapsed().as_millis()).ok();
                            probe_status_code = Some(resp.status().as_u16());
                        }
                        Err(err) => {
                            probe_latency_ms = u64::try_from(start_t.elapsed().as_millis()).ok();
                            probe_error = Some(err.to_string());
                        }
                    }
                }
            }

            webhook_check = Some(WebhookDryRunResult {
                url_valid,
                target_url: Some(raw_url.to_string()),
                allow_localhost: self.allow_localhost,
                ssrf_status,
                headers_preview,
                payload_preview: Some(payload_str),
                probe_attempted,
                probe_status_code,
                probe_latency_ms,
                probe_error,
            });
        }

        DryRunReport {
            matched,
            condition_results: conditions,
            rendered_message,
            rendered_dm,
            rendered_payload,
            planned_actions,
            webhook_check,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationSettings {
    pub automations: Vec<Automation>,
}

impl PluginConfig for AutomationSettings {
    const KEY: &'static str = "automations";
}

impl AutomationSettings {
    pub fn next_id(&self) -> u32 {
        self.automations.iter().map(|a| a.id).max().unwrap_or(0) + 1
    }
}

/// Bir olayın koşullarla karşılaştırılan özeti.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts<'a> {
    pub trigger: Trigger,
    pub channel: Option<u64>,
    pub content: &'a str,
    /// Rol tetikleyicilerinde verilen/alınan rol.
    pub role: Option<u64>,
    /// `reactionroles::emoji_key` biçiminde.
    pub emoji: Option<&'a str>,
    pub member_roles: &'a [RoleId],
    pub streamer: Option<&'a str>,
    pub platform: Option<&'a str>,
}

/// Büyük/küçük harf ve Türkçe noktalı-noktasız i farkını yok sayan karşılaştırma biçimi:
/// "NASIL", "nasıl" ve "nasil" aynı sayılır (İngilizce klavyeyle yazanlar da eşleşsin).
pub(crate) fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| *c != '\u{0307}')
        .map(|c| if c == 'ı' { 'i' } else { c })
        .collect()
}

/// Otomasyon bu olayda çalışmalı mı?
pub fn matches(automation: &Automation, facts: &Facts<'_>) -> bool {
    if !automation.enabled || automation.trigger != facts.trigger {
        return false;
    }
    if automation.channel.is_some() && automation.channel != facts.channel {
        return false;
    }
    let role_ok = match (automation.role, facts.trigger) {
        (None, _) => true,
        (Some(role), Trigger::RoleAdded | Trigger::RoleRemoved) => facts.role == Some(role),
        (Some(role), _) => facts.member_roles.iter().any(|r| r.get() == role),
    };
    if !role_ok {
        return false;
    }
    match facts.trigger {
        Trigger::Message => automation
            .keyword
            .as_deref()
            .is_none_or(|keyword| fold(facts.content).contains(&fold(keyword))),
        Trigger::Reaction => {
            let wanted = automation.emoji.as_deref().and_then(crate::reaction_roles::emoji_key);
            automation.emoji.is_none() || (wanted.is_some() && wanted.as_deref() == facts.emoji)
        }
        Trigger::StreamWentLive | Trigger::StreamWentOffline => {
            if let Some(wanted) = automation.streamer.as_deref().filter(|s| !s.trim().is_empty()) {
                let ws = fold(wanted.trim());
                let streamer_match = facts.streamer.map(|s| fold(s) == ws).unwrap_or(false);
                let platform_match = facts.platform.map(|p| fold(p) == ws).unwrap_or(false);
                streamer_match || platform_match
            } else {
                true
            }
        }
        _ => true,
    }
}

/// Şablon: `{kullanıcı}` (etiket), `{isim}`, `{sunucu}`, `{kanal}`, `{rol}`, `{mesaj}`, `{yayinci}`, `{baslik}`.
pub fn render(template: &str, user: &User, guild: &str, channel: Option<u64>, role: Option<u64>, content: &str) -> String {
    let name = user.global_name.clone().unwrap_or_else(|| user.name.clone());
    let quoted: String = content.chars().take(300).collect();
    template
        .replace("{kullanıcı}", &user.id.mention().to_string())
        .replace("{kullanici}", &user.id.mention().to_string())
        .replace("{isim}", &name)
        .replace("{sunucu}", guild)
        .replace("{kanal}", &channel.map(|c| format!("<#{c}>")).unwrap_or_default())
        .replace("{rol}", &role.map(|r| format!("<@&{r}>")).unwrap_or_default())
        .replace("{mesaj}", &quoted)
        .replace("{yayinci}", &quoted)
        .replace("{yayıncı}", &quoted)
        .replace("{baslik}", &quoted)
        .replace("{başlık}", &quoted)
}

/// Yayın bildirimleri için şablon: `{sunucu}`, `{kanal}`, `{yayinci}`, `{platform}`, `{baslik}`.
pub fn render_stream(template: &str, guild: &str, channel: Option<u64>, streamer: &str, platform: &str, title: &str) -> String {
    template
        .replace("{sunucu}", guild)
        .replace("{kanal}", &channel.map(|c| format!("<#{c}>")).unwrap_or_default())
        .replace("{yayinci}", streamer)
        .replace("{yayıncı}", streamer)
        .replace("{platform}", platform)
        .replace("{baslik}", title)
        .replace("{başlık}", title)
        .replace("{mesaj}", title)
}

fn is_private_or_restricted_ip(ip: IpAddr, allow_localhost: bool) -> Result<(), &'static str> {
    match ip {
        IpAddr::V4(ipv4) => {
            if ipv4.is_loopback() {
                if allow_localhost {
                    return Ok(());
                }
                return Err("Yerel loopback (127.0.0.1) adresi için yerel izin verilmelidir");
            }
            if ipv4.is_unspecified() || ipv4.is_broadcast() {
                return Err("Geçersiz veya broadcast IP adresi");
            }
            let octets = ipv4.octets();
            // 10.0.0.0/8
            if octets[0] == 10 {
                return Err("Özel IP adresi (RFC 1918 10.0.0.0/8) engellendi");
            }
            // 172.16.0.0/12 -> 172.16.0.0 - 172.31.255.255
            if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                return Err("Özel IP adresi (RFC 1918 172.16.0.0/12) engellendi");
            }
            // 192.168.0.0/16
            if octets[0] == 192 && octets[1] == 168 {
                return Err("Özel IP adresi (RFC 1918 192.168.0.0/16) engellendi");
            }
            // 169.254.0.0/16 (link-local, cloud metadata 169.254.169.254)
            if octets[0] == 169 && octets[1] == 254 {
                return Err("Link-local/metadata IP adresi engellendi");
            }
            // 100.64.0.0/10 (CGNAT)
            if octets[0] == 100 && (64..=127).contains(&octets[1]) {
                return Err("CGNAT IP adresi engellendi");
            }
            // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24 (TEST-NET)
            if (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
                || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
            {
                return Err("Test/dokümantasyon IP adresi engellendi");
            }
            Ok(())
        }
        IpAddr::V6(ipv6) => {
            if ipv6.is_loopback() {
                if allow_localhost {
                    return Ok(());
                }
                return Err("Yerel loopback (::1) adresi için yerel izin verilmelidir");
            }
            if ipv6.is_unspecified() {
                return Err("Geçersiz IPv6 adresi");
            }
            let segments = ipv6.segments();
            // fc00::/7 (ULA)
            if (segments[0] & 0xfe00) == 0xfc00 {
                return Err("Özel IPv6 adresi engellendi");
            }
            // fe80::/10 (Link-local)
            if (segments[0] & 0xffc0) == 0xfe80 {
                return Err("Link-local IPv6 adresi engellendi");
            }
            Ok(())
        }
    }
}

pub fn validate_webhook_url(raw_url: &str, allow_localhost: bool) -> Result<url::Url, &'static str> {
    let parsed = url::Url::parse(raw_url).map_err(|_| "Geçersiz URL biçimi")?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err("Yalnızca http veya https şeması desteklenir");
    }
    let host = parsed.host().ok_or("URL bir ana makine (host) içermelidir")?;
    match host {
        url::Host::Ipv4(ipv4) => {
            is_private_or_restricted_ip(IpAddr::V4(ipv4), allow_localhost)?;
        }
        url::Host::Ipv6(ipv6) => {
            is_private_or_restricted_ip(IpAddr::V6(ipv6), allow_localhost)?;
        }
        url::Host::Domain(domain) => {
            let d = domain.to_lowercase();
            if (d == "localhost" || d.ends_with(".localhost")) && !allow_localhost {
                return Err("localhost hedefi için 'Yerel hedeflere izin ver' seçeneği işaretlenmelidir");
            }
        }
    }
    Ok(parsed)
}

pub async fn send_webhook(
    client: &reqwest::Client,
    automation: &Automation,
    default_payload: &serde_json::Value,
) {
    let Some(raw_url) = automation.webhook_url.as_deref().filter(|u| !u.trim().is_empty()) else {
        return;
    };
    let target_url = match validate_webhook_url(raw_url, automation.allow_localhost) {
        Ok(u) => u,
        Err(err) => {
            warn!(automation_id = automation.id, url = raw_url, error = err, "Otomasyon webhook URL doğrulaması başarısız");
            return;
        }
    };

    let mut req = client
        .post(target_url.as_str())
        .timeout(Duration::from_secs(5));

    if let Some(secret) = automation.webhook_secret.as_deref().filter(|s| !s.trim().is_empty()) {
        req = req.header("x-webhook-secret", secret)
            .header("Authorization", format!("Bearer {secret}"));
    }

    let req = if let Some(custom) = automation.webhook_payload.as_deref().filter(|p| !p.trim().is_empty()) {
        req.header("content-type", "application/json").body(custom.to_owned())
    } else {
        req.json(default_payload)
    };

    match req.send().await {
        Ok(resp) => {
            debug!(automation_id = automation.id, status = %resp.status(), "Otomasyon webhook başarıyla gönderildi");
        }
        Err(err) => {
            warn!(automation_id = automation.id, error = %err, "Otomasyon webhook gönderilemedi");
        }
    }
}

pub struct StreamEventContext<'a> {
    pub guild_id: GuildId,
    pub platform: &'a str,
    pub streamer: &'a str,
    pub live: bool,
    pub title: Option<&'a str>,
    pub game: Option<&'a str>,
    pub viewers: Option<u64>,
}

pub async fn run_stream(
    http: &serenity::all::Http,
    cache: &serenity::all::Cache,
    db: &PgPool,
    settings_cache: &crate::settings::PluginSettings,
    http_client: &reqwest::Client,
    stream_ctx: StreamEventContext<'_>,
) {
    let settings: Arc<AutomationSettings> = match settings_cache.get(db, stream_ctx.guild_id).await {
        Ok(s) => s,
        Err(e) => {
            warn!(guild = %stream_ctx.guild_id, error = ?e, "Yayın otomasyon ayarları okunamadı");
            return;
        }
    };

    let facts = Facts {
        trigger: if stream_ctx.live {
            Trigger::StreamWentLive
        } else {
            Trigger::StreamWentOffline
        },
        channel: None,
        content: stream_ctx.title.unwrap_or_default(),
        role: None,
        emoji: None,
        member_roles: &[],
        streamer: Some(stream_ctx.streamer),
        platform: Some(stream_ctx.platform),
    };

    let guild_name = cache.guild(stream_ctx.guild_id).map(|g| g.name.clone()).unwrap_or_default();

    for automation in settings.automations.iter().filter(|a| matches(a, &facts)) {
        // 1. Mesaj gönderimi
        if let Some(template) = automation.message.as_deref().filter(|m| !m.trim().is_empty()) {
            if let Some(channel_id) = automation.message_channel.filter(|c| *c != 0) {
                let text = render_stream(
                    template,
                    &guild_name,
                    Some(channel_id),
                    stream_ctx.streamer,
                    stream_ctx.platform,
                    stream_ctx.title.unwrap_or_default(),
                );
                let mentions = CreateAllowedMentions::new().all_roles(automation.ping_roles);
                let msg = CreateMessage::new().content(text).allowed_mentions(mentions);
                if let Err(e) = ChannelId::new(channel_id).send_message(http, msg).await {
                    warn!(guild = %stream_ctx.guild_id, automation = automation.id, error = %e, "Yayın otomasyon mesajı gönderilemedi");
                }
            }
        }

        // 2. Webhook tetikleyici
        if automation.webhook_url.is_some() {
            let default_payload = serde_json::json!({
                "event": if stream_ctx.live { "stream_went_live" } else { "stream_went_offline" },
                "trigger": if stream_ctx.live { "stream_went_live" } else { "stream_went_offline" },
                "guild_id": stream_ctx.guild_id.get(),
                "platform": stream_ctx.platform,
                "streamer": stream_ctx.streamer,
                "live": stream_ctx.live,
                "title": stream_ctx.title,
                "game": stream_ctx.game,
                "viewers": stream_ctx.viewers,
            });
            send_webhook(http_client, automation, &default_payload).await;
        }
    }
}

static RECENT: LazyLock<DashMap<(GuildId, UserId), (Instant, u8)>> = LazyLock::new(DashMap::new);

/// Üyenin bu pencerede çalıştırma hakkı varsa bir hak düşer ve `true` döner.
fn claim_run(guild_id: GuildId, user: UserId) -> bool {
    if RECENT.len() > 5_000 {
        RECENT.retain(|_, (at, _)| at.elapsed() < BURST_WINDOW);
    }
    let mut entry = RECENT.entry((guild_id, user)).or_insert((Instant::now(), 0));
    if entry.0.elapsed() >= BURST_WINDOW {
        *entry = (Instant::now(), 0);
    }
    if entry.1 >= BURST_LIMIT {
        return false;
    }
    entry.1 += 1;
    true
}

/// Bir olay: koşullar için `facts`, eylemler için ilgili nesneler.
pub struct Event<'a> {
    pub guild_id: GuildId,
    pub user: &'a User,
    pub facts: Facts<'a>,
    /// Tetikleyen mesaj (mesaj tetikleyicisinde); tepki ekleme, silme ve alt başlık bunu gerektirir.
    pub message: Option<&'a Message>,
}

/// Olaya uyan otomasyonları çalıştırır.
pub async fn run(ctx: &serenity::Context, data: &Data, event: Event<'_>) {
    run_with_target(ctx, data, event, None).await;
}

/// `target`: kuralda kanal seçilmemişse mesajın gideceği kanal (alt başlıkta: alt başlığın kendisi).
pub async fn run_with_target(ctx: &serenity::Context, data: &Data, event: Event<'_>, target: Option<ChannelId>) {
    if event.user.bot {
        return;
    }
    let settings: Arc<AutomationSettings> = match data.settings.get(&data.db, event.guild_id).await {
        Ok(settings) => settings,
        Err(e) => {
            warn!(guild = %event.guild_id, error = ?e, "Otomasyonlar okunamadı");
            return;
        }
    };
    for automation in settings.automations.iter().filter(|a| matches(a, &event.facts)) {
        if !claim_run(event.guild_id, event.user.id) {
            debug!(guild = %event.guild_id, user = %event.user.id, "Otomasyon sınırı aşıldı, atlandı");
            return;
        }
        execute(ctx, data, &event, automation, target).await;
    }
}

async fn execute(
    ctx: &serenity::Context,
    data: &Data,
    event: &Event<'_>,
    automation: &Automation,
    target: Option<ChannelId>,
) {
    let guild = ctx.cache.guild(event.guild_id).map(|g| g.name.clone()).unwrap_or_default();
    let fill = |template: &str| render(template, event.user, &guild, event.facts.channel, event.facts.role, event.facts.content);
    let failed = |what: &str, e: serenity::Error| {
        debug!(guild = %event.guild_id, automation = automation.id, error = %e, "Otomasyon eylemi başarısız: {what}");
    };

    if let Some(template) = automation.message.as_deref().filter(|m| !m.trim().is_empty()) {
        if let Some(channel) = automation.message_channel.or(target.map(ChannelId::get)).or(event.facts.channel).filter(|c| *c != 0) {
            // Üye her zaman etiketlenebilir; rol etiketleri yalnızca kural izin veriyorsa bildirim gönderir.
            let mentions = CreateAllowedMentions::new().users(vec![event.user.id]).all_roles(automation.ping_roles);
            let message = CreateMessage::new().content(fill(template)).allowed_mentions(mentions);
            if let Err(e) = ChannelId::new(channel).send_message(&ctx.http, message).await {
                failed("mesaj", e);
            }
        }
    }
    if let Some(template) = automation.dm.as_deref().filter(|m| !m.trim().is_empty()) {
        if let Err(e) = event.user.direct_message(ctx, CreateMessage::new().content(fill(template))).await {
            failed("özel mesaj", e);
        }
    }
    for (list, add) in [(&automation.add_roles, true), (&automation.remove_roles, false)] {
        for role_id in list.iter().filter(|id| **id != 0).map(|id| RoleId::new(*id)) {
            let role = ctx.cache.guild(event.guild_id).and_then(|g| g.roles.get(&role_id).cloned());
            let Some(role) = role else { continue };
            if reaction_roles::check_role(ctx, event.guild_id, &role).is_err() {
                continue;
            }
            let result = if add {
                ctx.http.add_member_role(event.guild_id, event.user.id, role_id, Some("Otomasyon")).await
            } else {
                ctx.http.remove_member_role(event.guild_id, event.user.id, role_id, Some("Otomasyon")).await
            };
            if let Err(e) = result {
                failed("rol", e);
            }
        }
    }
    if automation.webhook_url.is_some() {
        let default_payload = serde_json::json!({
            "event": automation.trigger.label(),
            "trigger": automation.trigger,
            "guild_id": event.guild_id.get(),
            "user_id": event.user.id.get(),
            "user_name": event.user.name,
            "channel_id": event.facts.channel,
            "content": event.facts.content,
            "role_id": event.facts.role,
        });
        send_webhook(&data.http_client, automation, &default_payload).await;
    }
    let Some(message) = event.message else { return };
    if let Some(emoji) = automation.react.as_deref().and_then(|raw| ReactionType::try_from(raw.trim()).ok()) {
        if let Err(e) = message.react(&ctx.http, emoji).await {
            failed("tepki", e);
        }
    }
    if let Some(name) = automation.thread_name.as_deref().filter(|n| !n.trim().is_empty()) {
        let name: String = fill(name).chars().take(100).collect();
        if let Err(e) = message.channel_id.create_thread_from_message(&ctx.http, message.id, CreateThread::new(name)).await {
            failed("alt başlık", e);
        }
    }
    if automation.delete_message {
        if let Err(e) = message.delete(&ctx.http).await {
            failed("mesaj silme", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(trigger: Trigger) -> Automation {
        Automation { id: 1, name: "t".to_owned(), enabled: true, trigger, ..Automation::default() }
    }

    #[test]
    fn message_conditions() {
        let roles = [RoleId::new(5)];
        let facts = Facts { trigger: Trigger::Message, channel: Some(10), content: "Sunucuya NASIL katılırım?", member_roles: &roles, ..Facts::default() };
        assert!(matches(&rule(Trigger::Message), &facts));
        assert!(matches(&Automation { keyword: Some("nasıl".to_owned()), ..rule(Trigger::Message) }, &facts));
        assert!(!matches(&Automation { keyword: Some("fiyat".to_owned()), ..rule(Trigger::Message) }, &facts));
        // Türkçe i/ı ve İ/I: hangi klavyeyle yazılırsa yazılsın eşleşir.
        for keyword in ["NASIL", "nasil", "Nasıl", "KATILIRIM", "katilirim"] {
            assert!(matches(&Automation { keyword: Some(keyword.to_owned()), ..rule(Trigger::Message) }, &facts), "{keyword}");
        }
        assert_eq!(fold("İSTANBUL ISPARTA"), "istanbul isparta");
        assert!(!matches(&Automation { channel: Some(11), ..rule(Trigger::Message) }, &facts));
        assert!(matches(&Automation { role: Some(5), ..rule(Trigger::Message) }, &facts));
        assert!(!matches(&Automation { role: Some(6), ..rule(Trigger::Message) }, &facts));
        assert!(!matches(&Automation { enabled: false, ..rule(Trigger::Message) }, &facts));
        assert!(!matches(&rule(Trigger::MemberJoin), &facts));
    }

    #[test]
    fn role_and_reaction_conditions() {
        let added = Facts { trigger: Trigger::RoleAdded, role: Some(7), ..Facts::default() };
        assert!(matches(&rule(Trigger::RoleAdded), &added));
        assert!(matches(&Automation { role: Some(7), ..rule(Trigger::RoleAdded) }, &added));
        assert!(!matches(&Automation { role: Some(8), ..rule(Trigger::RoleAdded) }, &added));

        let key = crate::reaction_roles::emoji_key("⭐");
        let reaction = Facts { trigger: Trigger::Reaction, emoji: key.as_deref(), ..Facts::default() };
        assert!(matches(&rule(Trigger::Reaction), &reaction));
        assert!(matches(&Automation { emoji: Some("⭐\u{FE0F}".to_owned()), ..rule(Trigger::Reaction) }, &reaction));
        assert!(!matches(&Automation { emoji: Some("🎮".to_owned()), ..rule(Trigger::Reaction) }, &reaction));
    }

    #[test]
    fn burst_limit_stops_loops() {
        let (guild, user) = (GuildId::new(424_242), UserId::new(1));
        let allowed = (0..20).filter(|_| claim_run(guild, user)).count();
        assert_eq!(allowed, usize::from(BURST_LIMIT));
        // Başka bir üye etkilenmez.
        assert!(claim_run(guild, UserId::new(2)));
    }

    #[test]
    fn renders_and_loads_old_records() -> anyhow::Result<()> {
        let mut user = User::default();
        user.name = "ali".to_owned();
        let text = render("{isim} {kanal} {rol} {mesaj}", &user, "S", Some(3), Some(4), "merhaba");
        assert_eq!(text, "ali <#3> <@&4> merhaba");
        let settings: AutomationSettings = serde_json::from_str("{\"automations\":[{\"id\":2,\"trigger\":\"member_join\"}]}")?;
        assert_eq!(settings.automations[0].trigger, Trigger::MemberJoin);
        assert_eq!(settings.next_id(), 3);
        assert_eq!(settings.automations[0].streamer, None);
        assert_eq!(settings.automations[0].webhook_url, None);
        assert!(!settings.automations[0].allow_localhost);
        for (trigger, code, _) in Trigger::ALL {
            assert_eq!(Trigger::from_code(code), Some(trigger));
        }
        Ok(())
    }

    #[test]
    fn webhook_ssrf_validation() {
        // Geçersiz şema
        assert!(validate_webhook_url("ftp://example.com", false).is_err());
        assert!(validate_webhook_url("javascript:alert(1)", false).is_err());

        // Kamu / genel internet hedefleri izinli
        assert!(validate_webhook_url("https://hooks.slack.com/services/xxx", false).is_ok());
        assert!(validate_webhook_url("http://api.example.com:8080/webhook", false).is_ok());

        // Localhost ve loopback koruması
        assert!(validate_webhook_url("http://localhost:5000/hook", false).is_err());
        assert!(validate_webhook_url("http://127.0.0.1:8080/hook", false).is_err());
        assert!(validate_webhook_url("http://[::1]:8080/hook", false).is_err());

        // allow_localhost = true iken loopback serbest
        assert!(validate_webhook_url("http://localhost:5000/hook", true).is_ok());
        assert!(validate_webhook_url("http://127.0.0.1:8080/hook", true).is_ok());
        assert!(validate_webhook_url("http://[::1]:8080/hook", true).is_ok());

        // Özel ve bulut metadata IP'leri her zaman engellenir
        assert!(validate_webhook_url("http://10.0.0.5/api", true).is_err());
        assert!(validate_webhook_url("http://192.168.1.1/admin", true).is_err());
        assert!(validate_webhook_url("http://172.16.0.1/status", true).is_err());
        assert!(validate_webhook_url("http://169.254.169.254/latest/meta-data", true).is_err());
        assert!(validate_webhook_url("http://0.0.0.0:80/hook", true).is_err());
    }

    #[test]
    fn stream_trigger_conditions() {
        let live_facts = Facts {
            trigger: Trigger::StreamWentLive,
            content: "Yeni yayın başladı!",
            streamer: Some("elraenn"),
            platform: Some("twitch"),
            ..Facts::default()
        };

        // Koşulsuz yayın kuralı her yayında çalışır
        assert!(matches(&rule(Trigger::StreamWentLive), &live_facts));

        // Yayıncı adına göre filtre
        let specific_streamer = Automation {
            streamer: Some("Elraenn".to_owned()),
            ..rule(Trigger::StreamWentLive)
        };
        assert!(matches(&specific_streamer, &live_facts));

        // Farklı yayıncı eşleşmez
        let other_streamer = Automation {
            streamer: Some("wtcn".to_owned()),
            ..rule(Trigger::StreamWentLive)
        };
        assert!(!matches(&other_streamer, &live_facts));

        // Platform adına göre filtre
        let twitch_rule = Automation {
            streamer: Some("twitch".to_owned()),
            ..rule(Trigger::StreamWentLive)
        };
        assert!(matches(&twitch_rule, &live_facts));

        let kick_rule = Automation {
            streamer: Some("kick".to_owned()),
            ..rule(Trigger::StreamWentLive)
        };
        assert!(!matches(&kick_rule, &live_facts));

        // Offline tetikleyicisi
        let offline_facts = Facts {
            trigger: Trigger::StreamWentOffline,
            streamer: Some("elraenn"),
            platform: Some("twitch"),
            ..Facts::default()
        };
        assert!(matches(&rule(Trigger::StreamWentOffline), &offline_facts));
        assert!(!matches(&rule(Trigger::StreamWentLive), &offline_facts));
    }

    #[test]
    fn stream_template_rendering() {
        let template = "{sunucu} | {yayinci} ({platform}) yayında: {baslik}! Mesaj: {mesaj}";
        let rendered = render_stream(template, "Oyun Sunucusu", Some(12345), "Elraenn", "twitch", "Valorant Yayını");
        assert_eq!(rendered, "Oyun Sunucusu | Elraenn (twitch) yayında: Valorant Yayını! Mesaj: Valorant Yayını");

        let tr_template = "🔴 {yayıncı} açtı! Kanal: {kanal}, Başlık: {başlık}";
        let tr_rendered = render_stream(tr_template, "Oyun Sunucusu", Some(12345), "wtcN", "kick", "Dereceli Maç");
        assert_eq!(tr_rendered, "🔴 wtcN açtı! Kanal: <#12345>, Başlık: Dereceli Maç");
    }

    #[tokio::test]
    async fn webhook_live_dispatch_and_isolation() {
        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            return;
        };
        let Ok(addr) = listener.local_addr() else {
            return;
        };
        let url = format!("http://127.0.0.1:{}/test-webhook", addr.port());

        let client = reqwest::Client::new();
        let automation = Automation {
            id: 42,
            name: "Local Webhook".to_owned(),
            enabled: true,
            trigger: Trigger::StreamWentLive,
            webhook_url: Some(url),
            webhook_secret: Some("secret-token-xyz".to_owned()),
            allow_localhost: true,
            ..Automation::default()
        };

        let server_task = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK";
                let _ = stream.write_all(response.as_bytes()).await;
                req_str.to_string()
            } else {
                String::new()
            }
        });

        let payload = serde_json::json!({
            "event": "stream_went_live",
            "streamer": "elraenn",
            "platform": "twitch",
            "live": true
        });

        send_webhook(&client, &automation, &payload).await;
        let received_req = server_task.await.unwrap_or_default();
        assert!(received_req.contains("POST /test-webhook"));
        assert!(received_req.to_lowercase().contains("authorization: bearer secret-token-xyz"));
        assert!(received_req.to_lowercase().contains("x-webhook-secret: secret-token-xyz"));
        assert!(received_req.contains("\"streamer\":\"elraenn\""));

        // Hata yalıtımı: kapalı porta gönderildiğinde çökmez, güvenle döner
        let closed_automation = Automation {
            id: 43,
            webhook_url: Some("http://127.0.0.1:59998/closed".to_owned()),
            allow_localhost: true,
            ..Automation::default()
        };
        send_webhook(&client, &closed_automation, &payload).await;
    }

    #[tokio::test]
    async fn dry_run_stream_trigger_match_and_mismatch() {
        let automation = Automation {
            id: 10,
            name: "Yayın Bildirimi".to_owned(),
            enabled: true,
            trigger: Trigger::StreamWentLive,
            streamer: Some("elraenn".to_owned()),
            message: Some("🔴 {yayinci} {platform}'da yayında: {baslik}".to_owned()),
            message_channel: Some(999),
            ..Automation::default()
        };

        // 1. Eşleşen yayın senaryosu
        let matching_input = DryRunInput {
            trigger: Trigger::StreamWentLive,
            platform: Some("twitch".to_owned()),
            streamer: Some("Elraenn".to_owned()),
            title: Some("Büyük Turnuva".to_owned()),
            guild_name: Some("Test Sunucu".to_owned()),
            ..DryRunInput::default()
        };
        let report = automation.evaluate_dry_run(&matching_input, None).await;
        assert!(report.matched);
        assert_eq!(
            report.rendered_message.as_deref(),
            Some("🔴 Elraenn twitch'da yayında: Büyük Turnuva")
        );
        assert_eq!(report.planned_actions.len(), 1);
        assert_eq!(report.planned_actions[0].action_type, "Mesaj Gönder");
        assert_eq!(report.planned_actions[0].target, "<#999>");

        // 2. Farklı yayıncı senaryosu (koşul reddi)
        let mismatch_input = DryRunInput {
            trigger: Trigger::StreamWentLive,
            platform: Some("twitch".to_owned()),
            streamer: Some("wtcn".to_owned()),
            ..DryRunInput::default()
        };
        let mismatch_report = automation.evaluate_dry_run(&mismatch_input, None).await;
        assert!(!mismatch_report.matched);
        let streamer_check = mismatch_report
            .condition_results
            .iter()
            .find(|c| c.name == "Yayıncı / Platform");
        assert!(streamer_check.is_some_and(|c| !c.passed));

        // 3. Farklı tetikleyici türü (örn. çevrimdışı)
        let offline_input = DryRunInput {
            trigger: Trigger::StreamWentOffline,
            streamer: Some("elraenn".to_owned()),
            ..DryRunInput::default()
        };
        let offline_report = automation.evaluate_dry_run(&offline_input, None).await;
        assert!(!offline_report.matched);
        let trigger_check = offline_report
            .condition_results
            .iter()
            .find(|c| c.name == "Tetikleyici Türü");
        assert!(trigger_check.is_some_and(|c| !c.passed));
    }

    #[tokio::test]
    async fn dry_run_message_keyword_and_roles() {
        let automation = Automation {
            id: 20,
            name: "Soru Cevaplayıcı".to_owned(),
            enabled: true,
            trigger: Trigger::Message,
            keyword: Some("destek".to_owned()),
            channel: Some(101),
            role: Some(501),
            message: Some("Merhaba {kullanıcı}, sorunu aldık.".to_owned()),
            dm: Some("Özel bildirim: {mesaj}".to_owned()),
            add_roles: vec![502],
            react: Some("👍".to_owned()),
            ..Automation::default()
        };

        let valid_input = DryRunInput {
            trigger: Trigger::Message,
            channel_id: Some(101),
            role_id: Some(501),
            member_roles: vec![501],
            user_id: Some(777),
            user_name: Some("Ahmet".to_owned()),
            content: Some("Acil DESTEK talebi".to_owned()),
            guild_name: Some("Topluluk".to_owned()),
            ..DryRunInput::default()
        };

        let report = automation.evaluate_dry_run(&valid_input, None).await;
        assert!(report.matched);
        assert_eq!(
            report.rendered_message.as_deref(),
            Some("Merhaba <@777>, sorunu aldık.")
        );
        assert_eq!(
            report.rendered_dm.as_deref(),
            Some("Özel bildirim: Acil DESTEK talebi")
        );
        assert_eq!(report.planned_actions.len(), 4); // Mesaj, DM, Rol Ver, Tepki
    }

    #[tokio::test]
    async fn dry_run_ssrf_and_webhook_probe() {
        // 1. allow_localhost = false iken yerel adres reddedilmeli
        let ssrf_blocked = Automation {
            id: 30,
            name: "SSRF Test".to_owned(),
            enabled: true,
            trigger: Trigger::MemberJoin,
            webhook_url: Some("http://127.0.0.1:9090/hook".to_owned()),
            allow_localhost: false,
            ..Automation::default()
        };
        let input = DryRunInput {
            trigger: Trigger::MemberJoin,
            probe_webhook: false,
            ..DryRunInput::default()
        };
        let report = ssrf_blocked.evaluate_dry_run(&input, None).await;
        let check = report.webhook_check.as_ref();
        assert!(check.is_some_and(|c| !c.url_valid && c.ssrf_status.contains("SSRF Engeli")));

        // 2. allow_localhost = true ve probe_webhook = true iken yerel sunucuya probe gitmeli
        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            return;
        };
        let Ok(addr) = listener.local_addr() else {
            return;
        };
        let url = format!("http://127.0.0.1:{}/probe", addr.port());

        let server_task = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let response = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(response.as_bytes()).await;
                req_str.to_string()
            } else {
                String::new()
            }
        });

        let allowed = Automation {
            id: 31,
            name: "Probe Test".to_owned(),
            enabled: true,
            trigger: Trigger::MemberJoin,
            webhook_url: Some(url),
            webhook_secret: Some("probe-secret".to_owned()),
            allow_localhost: true,
            ..Automation::default()
        };

        let client = reqwest::Client::new();
        let probe_input = DryRunInput {
            trigger: Trigger::MemberJoin,
            probe_webhook: true,
            ..DryRunInput::default()
        };
        let probe_report = allowed.evaluate_dry_run(&probe_input, Some(&client)).await;
        let req_header = server_task.await.unwrap_or_default();
        assert!(req_header.to_lowercase().contains("x-axbot-dry-run: true"));

        let check = probe_report.webhook_check.as_ref();
        assert!(check.is_some_and(|c| c.url_valid && c.probe_status_code == Some(200)));
    }
}
