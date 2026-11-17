//! Panel: sunucu eklentileri. Her eklentinin kendi sayfası ve ayar formu vardır; kaydedilen ayar
//! botla paylaşılan önbelleğe (`GuildConfigs`) yazıldığı için anında geçerli olur.
//!
//! Güvenlik: her istek oturum + 'Sunucuyu Yönet' yetkisi + CSRF anahtarı ile doğrulanır. Formdan
//! gelen kanal ve rol kimlikleri, o sunucunun seçilebilir kanal/rol listesinde yoksa reddedilir
//! (başka sunucunun kanalı ya da yetkili bir rol gönderilemez).

use std::fmt::Write as _;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use poise::serenity_prelude::{ChannelId, ChannelType, GuildId, RoleId};
use serde::Deserialize;
use tracing::{info, warn};

use super::auth;
use super::can_manage;
use super::pages::{forbidden, layout, GuildNav, Nav, Page};
use crate::guild_config::{GuildConfig, Setting};
use crate::web::{escape, WebState};

const MESSAGE_MAX: usize = 1500;

pub struct Plugin {
    pub slug: &'static str,
    pub icon: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub group: &'static str,
}

/// Menüde ve sunucu sayfasındaki kartlarda bu sırayla görünür.
pub const PLUGINS: &[Plugin] = &[
    Plugin {
        slug: "karsilama",
        icon: "👋",
        name: "Hoş geldin & Veda",
        about: "Karşılama ve veda mesajları, resimli karşılama kartı, özelden karşılama, otomatik roller.",
        group: "Temel",
    },
    Plugin {
        slug: "dogrulama",
        icon: "✅",
        name: "Doğrulama",
        about: "Yeni üyeler butona basıp (istersen captcha çözerek) doğrulanır ve rol alır.",
        group: "Temel",
    },
    Plugin {
        slug: "seviye",
        icon: "🏆",
        name: "Seviye",
        about: "Mesaj yazan üyeler XP kazanır, seviye atlar ve ödül rolleri alır.",
        group: "Temel",
    },
    Plugin {
        slug: "moderasyon",
        icon: "🔨",
        name: "Moderasyon",
        about: "Otomatik moderasyon kuralları, kısıtlı kanallar, muaf roller ve otomatik cezalar.",
        group: "Temel",
    },
    Plugin {
        slug: "koruma",
        icon: "🛡️",
        name: "Koruma",
        about: "Çok yeni hesapları ve toplu girişleri (baskın) durdur.",
        group: "Temel",
    },
    Plugin {
        slug: "kayitlar",
        icon: "📋",
        name: "Olay kayıtları",
        about: "Mesaj, üye, rol, ses, kanal ve sunucu olayları ile moderasyon vakaları seçtiğin kanala yazılır.",
        group: "Temel",
    },
    Plugin {
        slug: "starboard",
        icon: "⭐",
        name: "Starboard",
        about: "Yeterince ⭐ alan mesajlar ayrı bir kanalda sergilenir.",
        group: "Temel",
    },
    Plugin {
        slug: "ek-starboard",
        icon: "🌟",
        name: "Ek starboard'lar",
        about: "⭐ dışındaki emojiler için ayrı panolar (ör. 😂 → komik anlar).",
        group: "Temel",
    },
    Plugin {
        slug: "basarimlar",
        icon: "🏅",
        name: "Başarımlar",
        about: "Üyeler mesaj, tepki, ses ve alt başlık sayılarına göre rozet kazanır.",
        group: "Temel",
    },
    Plugin {
        slug: "discord-automod",
        icon: "🧱",
        name: "Discord AutoMod",
        about: "Discord'un kendi filtreleri: mesaj gönderilmeden engellenir, bot kapalıyken de korur.",
        group: "Temel",
    },
    Plugin {
        slug: "yikim-korumasi",
        icon: "🚨",
        name: "Yıkım koruması",
        about: "Kısa sürede çok kanal / rol silen ya da üye yasaklayan yetkiliyi durdurur.",
        group: "Temel",
    },
    Plugin {
        slug: "biletler",
        icon: "🎫",
        name: "Destek biletleri",
        about: "Üyeler butonla ya da konu seçerek özel destek kanalı açar; kayıt tutulur.",
        group: "Sunucu yönetimi",
    },
    Plugin {
        slug: "otomasyon",
        icon: "⚙️",
        name: "Otomasyon",
        about: "\"Şu olunca şunu yap\": mesaj, giriş, rol ve tepki olaylarına otomatik karşılık.",
        group: "Sunucu yönetimi",
    },
    Plugin {
        slug: "tepki-rol",
        icon: "🎭",
        name: "Rol menüleri",
        about: "Üyeler emoji tepkisi, buton ya da açılır menüyle kendi rollerini seçer.",
        group: "Sunucu yönetimi",
    },
    Plugin {
        slug: "ozel-komutlar",
        icon: "⌨️",
        name: "Özel komutlar",
        about: "Kendi !komutlarını oluştur: cevap yazsın, rol versin, belirli rollere özel olsun.",
        group: "Sunucu yönetimi",
    },
    Plugin {
        slug: "modmail",
        icon: "📬",
        name: "ModMail",
        about: "Üyeler bota özelden yazar; mesaj yetkili kanalında alt başlığa düşer.",
        group: "Sunucu yönetimi",
    },
    Plugin {
        slug: "basvurular",
        icon: "📝",
        name: "Başvuru formları",
        about: "Sorulu başvuru formları; yetkili kanalında Kabul / Ret.",
        group: "Sunucu yönetimi",
    },
    Plugin {
        slug: "etiketler",
        icon: "🏷️",
        name: "Etiketler",
        about: "Sohbette !ad yazınca gönderilen hazır metinler.",
        group: "Sunucu yönetimi",
    },
    Plugin {
        slug: "davetler",
        icon: "📨",
        name: "Davet takibi",
        about: "Kim kimi davet etti: davet sıralaması ve sahte davet ayıklama.",
        group: "Sunucu yönetimi",
    },
    Plugin {
        slug: "otocevap",
        icon: "💬",
        name: "Otomatik cevaplar",
        about: "Belirli kelimelere botun hazır cevap vermesi.",
        group: "Sunucu yönetimi",
    },
    Plugin {
        slug: "anket",
        icon: "📊",
        name: "Anket",
        about: "Seçtiğin kanala Discord anketi gönder.",
        group: "Araçlar",
    },
    Plugin {
        slug: "sayaclar",
        icon: "🔢",
        name: "Sayaç kanalları",
        about: "Üye, bot, kanal, rol sayısı ya da kripto fiyatını gösteren kanallar.",
        group: "Araçlar",
    },
    Plugin {
        slug: "gecici-odalar",
        icon: "🎙️",
        name: "Geçici odalar",
        about: "Bir kanala girene kendi ses odasını açar; boşalınca siler.",
        group: "Araçlar",
    },
    Plugin {
        slug: "zamanlayici",
        icon: "⏰",
        name: "Tekrarlı mesajlar",
        about: "Seçtiğin kanala belirli aralıklarla aynı mesajı gönderir.",
        group: "Araçlar",
    },
    Plugin {
        slug: "gomulu-mesaj",
        icon: "🖼️",
        name: "Gömülü mesajlar",
        about: "Kurallar ve duyurular için renkli, resimli gömülü mesajlar hazırla ve gönder.",
        group: "Araçlar",
    },
    Plugin {
        slug: "yayinlar",
        icon: "📺",
        name: "Yayın ve video duyuruları",
        about: "YouTube'da yeni video, Twitch ve Kick'te canlı yayın duyuruları.",
        group: "Sosyal",
    },
    Plugin {
        slug: "sosyal",
        icon: "📡",
        name: "Sosyal akışlar",
        about: "Reddit, Bluesky, RSS ve podcast paylaşımlarını seçtiğin kanala duyurur.",
        group: "Sosyal",
    },
    Plugin {
        slug: "haberler",
        icon: "📰",
        name: "Haberler",
        about: "Teknoloji, oyun, sinema ve dizi haberleri otomatik olarak kanala; her hafta özet.",
        group: "Sosyal",
    },
    Plugin {
        slug: "topluluk",
        icon: "🧰",
        name: "Topluluk araçları",
        about: "Sabit mesajlar, sayma oyunu, itiraflar, kalıcı roller ve ses XP'si.",
        group: "Topluluk",
    },
    Plugin {
        slug: "oneriler",
        icon: "💡",
        name: "Öneriler",
        about: "Üyeler öneri gönderir, herkes butonla oylar.",
        group: "Topluluk",
    },
    Plugin {
        slug: "dogum-gunu",
        icon: "🎂",
        name: "Doğum günü",
        about: "Doğum günü olanlar kutlanır ve bir günlüğüne özel rol alır.",
        group: "Topluluk",
    },
    Plugin {
        slug: "muzik",
        icon: "🎵",
        name: "Müzik",
        about: "7/24 modu, otomatik çalma ve şu an çalan şarkı.",
        group: "Eğlence",
    },
    Plugin {
        slug: "cekilis",
        icon: "🎉",
        name: "Çekiliş",
        about: "Butonla katılınan, süresi dolunca kazananı seçilen çekilişler.",
        group: "Eğlence",
    },
    Plugin {
        slug: "ekonomi",
        icon: "🪙",
        name: "Ekonomi",
        about: "Sunucu parası, mağaza ürünleri ve en zenginler.",
        group: "Eğlence",
    },
    Plugin {
        slug: "hayvanlar",
        icon: "🐾",
        name: "Hayvan yakalama",
        about: "Seçilen kanalda ara sıra bir hayvan belirir; ilk basan yakalar.",
        group: "Eğlence",
    },
    Plugin {
        slug: "yz",
        icon: "🤖",
        name: "Yapay zekâ",
        about: "Sohbet kanalları, sunucu kişiliği ve kanal başına karakterler.",
        group: "Bot",
    },
    Plugin {
        slug: "bot",
        icon: "🪪",
        name: "Bot profili",
        about: "Botun bu sunucudaki takma adı, avatarı, afişi ve hakkında yazısı.",
        group: "Bot",
    },
    Plugin {
        slug: "ayarlar",
        icon: "⚙️",
        name: "Panel ayarları",
        about: "Paneli kullanabilen bot yöneticisi rolleri.",
        group: "Bot",
    },
];

fn find(slug: &str) -> Option<&'static Plugin> {
    PLUGINS.iter().find(|p| p.slug == slug)
}

/// Ayarı `GuildConfig` dışında duran eklentilerin açık/kapalı durumu.
#[derive(Clone, Copy, Default)]
pub struct Extra {
    pub levels: bool,
    pub moderation: bool,
    pub role_menus: bool,
    pub commands: bool,
    pub timers: bool,
    pub embeds: bool,
    pub automations: bool,
    pub achievements: bool,
    pub tickets: bool,
    pub feeds: bool,
    pub ai: bool,
    pub profile: bool,
    pub community: bool,
    pub managers: bool,
    pub subscriptions: bool,
    pub replies: bool,
    pub invites: bool,
    pub modmail: bool,
    pub antinuke: bool,
    pub native_automod: bool,
    pub tags: bool,
    pub economy: bool,
    pub stats: bool,
    pub forms: bool,
    pub boards: bool,
    pub music: bool,
    pub news: bool,
}

impl Extra {
    /// Ayarı ayrı depolarda duran eklentilerin durumunu okur (okunamayan kapalı sayılır).
    pub async fn load(state: &WebState, guild_id: GuildId) -> Self {
        let (db, settings) = (&state.db, &state.settings);
        Self {
            levels: settings.get::<crate::leveling::settings::LevelSettings>(db, guild_id).await.is_ok_and(|s| s.enabled),
            moderation: crate::automod::config::load_settings(db, guild_id).await.is_ok_and(|s| s.enabled),
            role_menus: settings.get::<crate::reactionroles::ReactionRoleSettings>(db, guild_id).await.is_ok_and(|s| !s.menus.is_empty()),
            commands: settings.get::<crate::customcmd::CustomCommandSettings>(db, guild_id).await.is_ok_and(|s| !s.commands.is_empty()),
            timers: settings.get::<crate::timers::TimerSettings>(db, guild_id).await.is_ok_and(|s| !s.timers.is_empty()),
            embeds: settings.get::<crate::timers::EmbedSettings>(db, guild_id).await.is_ok_and(|s| !s.embeds.is_empty()),
            automations: settings.get::<crate::automations::AutomationSettings>(db, guild_id).await.is_ok_and(|s| s.automations.iter().any(|a| a.enabled)),
            achievements: settings.get::<crate::achievements::AchievementSettings>(db, guild_id).await.is_ok_and(|s| s.enabled),
            tickets: crate::tickets::load_settings(db, guild_id).await.is_ok_and(|s| s.is_some()),
            feeds: settings.get::<crate::feeds::FeedSettings>(db, guild_id).await.is_ok_and(|s| !s.feeds.is_empty()),
            news: settings.get::<crate::news::NewsSettings>(db, guild_id).await.is_ok_and(|s| s.is_active()),
            ai: state.ai.channels(db, guild_id).await.is_ok_and(|c| !c.is_empty()),
            profile: settings.get::<crate::presence::BotProfile>(db, guild_id).await.is_ok_and(|p| p.is_custom()),
            community: settings.get::<crate::community::CommunitySettings>(db, guild_id).await.is_ok_and(|s| s.is_active()),
            managers: settings.get::<crate::community::CommunitySettings>(db, guild_id).await.is_ok_and(|s| !s.manager_roles.is_empty()),
            subscriptions: crate::youtube::count(db, guild_id).await.is_ok_and(|n| n > 0) || crate::streams::count(db, guild_id).await.is_ok_and(|n| n > 0),
            replies: state.autoresponder.rules(db, guild_id).await.is_ok_and(|r| !r.is_empty()),
            invites: crate::invites::leaderboard(db, guild_id, 1).await.is_ok_and(|top| !top.is_empty()),
            modmail: settings.get::<crate::modmail::ModmailSettings>(db, guild_id).await.is_ok_and(|s| s.channel.is_some()),
            antinuke: settings.get::<crate::antinuke::AntiNukeSettings>(db, guild_id).await.is_ok_and(|s| s.enabled),
            native_automod: settings.get::<crate::nativeautomod::NativeAutomodSettings>(db, guild_id).await.is_ok_and(|s| s.is_active()),
            tags: state.tags.all(db, guild_id).await.is_ok_and(|t| !t.is_empty()),
            economy: crate::economy::shop(db, guild_id).await.is_ok_and(|items| !items.is_empty()),
            stats: crate::statchannels::count(db, guild_id).await.is_ok_and(|n| n > 0),
            forms: crate::applications::forms(db, guild_id).await.is_ok_and(|f| !f.is_empty()),
            boards: settings.get::<crate::starboards::StarboardSettings>(db, guild_id).await.is_ok_and(|s| !s.boards.is_empty()),
            music: {
                let music = state.music.settings(guild_id);
                music.always_on || music.autoplay || settings.get::<crate::music::playlists::MusicExtras>(db, guild_id).await.is_ok_and(|m| m.dj_role.is_some())
            },
        }
    }
}

pub(super) fn is_enabled(slug: &str, cfg: &GuildConfig, extra: Extra) -> bool {
    match slug {
        "seviye" => extra.levels,
        "moderasyon" => extra.moderation,
        "tepki-rol" => extra.role_menus,
        "ozel-komutlar" => extra.commands,
        "zamanlayici" => extra.timers,
        "gomulu-mesaj" => extra.embeds,
        "otomasyon" => extra.automations,
        "basarimlar" => extra.achievements,
        "biletler" => extra.tickets,
        "sosyal" => extra.feeds,
        "haberler" => extra.news,
        "yz" => extra.ai,
        "bot" => extra.profile,
        "topluluk" => extra.community,
        "ayarlar" => extra.managers,
        "yayinlar" => extra.subscriptions,
        "otocevap" => extra.replies,
        "davetler" => extra.invites,
        "modmail" => extra.modmail,
        "yikim-korumasi" => extra.antinuke,
        "discord-automod" => extra.native_automod,
        "etiketler" => extra.tags,
        "ekonomi" => extra.economy,
        "sayaclar" => extra.stats,
        "basvurular" => extra.forms,
        "ek-starboard" => extra.boards,
        "muzik" => extra.music,
        "gecici-odalar" => cfg.tempvoice_hub.is_some(),
        "karsilama" => cfg.welcome_channel.is_some() || cfg.goodbye_channel.is_some() || cfg.autorole.is_some(),
        "dogrulama" => cfg.verify_role.is_some(),
        "koruma" => cfg.min_account_age_days > 0 || cfg.raid_join_threshold > 0,
        "kayitlar" => cfg.event_log_channel.is_some(),
        "starboard" => cfg.starboard_channel.is_some(),
        "oneriler" => cfg.suggestion_channel.is_some(),
        "dogum-gunu" => cfg.birthday_channel.is_some(),
        "hayvanlar" => cfg.animal_channel.is_some(),
        _ => false,
    }
}

fn badge(on: bool) -> &'static str {
    if on {
        "<span class=\"badge b-ok\">AÇIK</span>"
    } else {
        "<span class=\"badge b-off\">KAPALI</span>"
    }
}

/// Sunucu sayfasındaki eklenti kartları (gruplara ayrılmış).
pub fn cards(guild_id: GuildId, cfg: &GuildConfig, extra: Extra) -> String {
    let mut out = String::new();
    let mut group = "";
    for plugin in PLUGINS {
        if plugin.group != group {
            if !group.is_empty() {
                out.push_str("</div>");
            }
            group = plugin.group;
            let _ = write!(out, "<h2 class=\"pgroup\">{}</h2><div class=\"pcards\">", escape(group));
        }
        let _ = write!(
            out,
            "<a class=\"pcard{on}\" data-ara=\"{search}\" href=\"/panel/sunucu/{guild_id}/eklenti/{slug}\"><div class=\"ptop\"><div class=\"pico\">{icon}</div>\
             <span class=\"sw{on}\" title=\"{state}\"></span></div>\
             <div class=\"ptitle\">{name} {badge}</div><div class=\"note\">{about}</div><span class=\"plink\">{action} →</span></a>",
            on = if is_enabled(plugin.slug, cfg, extra) { " on" } else { "" },
            state = if is_enabled(plugin.slug, cfg, extra) { "Açık" } else { "Kapalı" },
            action = if is_enabled(plugin.slug, cfg, extra) { "Ayarla" } else { "Etkinleştir" },
            search = escape(&format!("{} {}", plugin.name, plugin.about).to_lowercase()),
            slug = plugin.slug,
            icon = plugin.icon,
            name = escape(plugin.name),
            badge = badge(is_enabled(plugin.slug, cfg, extra)),
            about = escape(plugin.about),
        );
    }
    if !group.is_empty() {
        out.push_str("</div>");
    }
    out
}

/// Formlarda seçilebilecek kanal ve roller (önbellekten; kilit hemen bırakılır).
pub struct Options {
    pub guild_name: String,
    pub channels: Vec<(u64, String)>,
    /// Kategoriler (XP listesi gibi kanal gruplarını hedefleyen ayarlar için).
    pub categories: Vec<(u64, String)>,
    /// Botun verebileceği, yetki içermeyen roller.
    pub roles: Vec<(u64, String)>,
    /// @everyone ve bot rolleri dışındaki tüm roller (sadece okuma amaçlı listeler için).
    pub all_roles: Vec<(u64, String)>,
}

impl Options {
    pub fn load(state: &WebState, guild_id: GuildId) -> Option<Self> {
        let bot = state.cache.current_user().id;
        let guild = state.cache.guild(guild_id)?;
        let top = guild.members.get(&bot).map_or(0, |me| {
            me.roles.iter().filter_map(|r| guild.roles.get(r)).map(|r| r.position).max().unwrap_or(0)
        });

        let mut channels: Vec<(u16, u64, String)> = guild
            .channels
            .values()
            .filter(|c| matches!(c.kind, ChannelType::Text | ChannelType::News) && !crate::backup::is_hidden(c))
            .map(|c| (c.position, c.id.get(), c.name.clone()))
            .collect();
        channels.sort();

        let mut roles: Vec<(u16, u64, String)> = guild
            .roles
            .values()
            .filter(|r| {
                r.id != guild_id.everyone_role()
                    && !r.managed
                    && !r.permissions.intersects(crate::rolepanel::dangerous())
                    && r.position < top
            })
            .map(|r| (r.position, r.id.get(), r.name.clone()))
            .collect();
        // Discord'daki gibi: en üstteki rol en başta.
        roles.sort_by(|a, b| b.cmp(a));

        let mut categories: Vec<(u16, u64, String)> = guild
            .channels
            .values()
            .filter(|c| c.kind == ChannelType::Category)
            .map(|c| (c.position, c.id.get(), c.name.clone()))
            .collect();
        categories.sort();
        let mut all_roles: Vec<(u16, u64, String)> = guild
            .roles
            .values()
            .filter(|r| r.id != guild_id.everyone_role() && !r.managed)
            .map(|r| (r.position, r.id.get(), r.name.clone()))
            .collect();
        all_roles.sort_by(|a, b| b.cmp(a));

        Some(Self {
            guild_name: guild.name.clone(),
            channels: channels.into_iter().map(|(_, id, name)| (id, name)).collect(),
            categories: categories.into_iter().map(|(_, id, name)| (id, name)).collect(),
            roles: roles.into_iter().map(|(_, id, name)| (id, name)).collect(),
            all_roles: all_roles.into_iter().map(|(_, id, name)| (id, name)).collect(),
        })
    }

    pub fn select(name: &str, items: &[(u64, String)], prefix: &str, selected: Option<u64>, none: &str) -> String {
        let mut out = format!("<select name=\"{name}\"><option value=\"\">{}</option>", escape(none));
        for (id, label) in items {
            let _ = write!(
                out,
                "<option value=\"{id}\"{}>{prefix}{}</option>",
                if selected == Some(*id) { " selected" } else { "" },
                escape(label)
            );
        }
        out.push_str("</select>");
        out
    }

    pub fn channel_select(&self, name: &str, selected: Option<ChannelId>) -> String {
        Self::select(name, &self.channels, "# ", selected.map(ChannelId::get), "— Kapalı —")
    }

    pub fn role_select(&self, name: &str, selected: Option<RoleId>) -> String {
        Self::select(name, &self.roles, "@ ", selected.map(RoleId::get), "— Yok —")
    }
}

pub fn section(title: &str, body: String) -> String {
    format!("<div class=\"box sect\"><h3>{}</h3>{body}</div>", escape(title))
}

/// Onay kutusu (`value="1"`).
pub fn checkbox(name: &str, checked: bool, label: &str) -> String {
    format!(
        "<label class=\"check\"><input type=\"checkbox\" name=\"{name}\" value=\"1\"{}> {}</label>",
        if checked { " checked" } else { "" },
        escape(label)
    )
}

/// Çoklu seçim: her öğe aynı adla bir onay kutusu.
pub fn checklist(name: &str, items: &[(u64, String)], prefix: &str, selected: &[u64]) -> String {
    if items.is_empty() {
        return "<p class=\"note\">Seçilebilecek öğe yok.</p>".to_owned();
    }
    let mut out = String::from("<div class=\"checks\">");
    for (id, label) in items {
        let _ = write!(
            out,
            "<label class=\"check\"><input type=\"checkbox\" name=\"{name}\" value=\"{id}\"{}> {prefix}{}</label>",
            if selected.contains(id) { " checked" } else { "" },
            escape(label)
        );
    }
    out.push_str("</div>");
    out
}

/// Tek seçim (radyo düğmeleri): `(değer, etiket)`.
pub fn radios(name: &str, choices: &[(&str, &str)], selected: &str) -> String {
    let mut out = String::from("<div class=\"checks col\">");
    for (value, label) in choices {
        let _ = write!(
            out,
            "<label class=\"check\"><input type=\"radio\" name=\"{name}\" value=\"{value}\"{}> {}</label>",
            if *value == selected { " checked" } else { "" },
            escape(label)
        );
    }
    out.push_str("</div>");
    out
}

pub fn field(label: &str, control: &str, hint: &str) -> String {
    format!(
        "<label class=\"field\"><span>{}</span>{control}{}</label>",
        escape(label),
        if hint.is_empty() { String::new() } else { format!("<small>{hint}</small>") }
    )
}

pub fn textarea(name: &str, value: Option<&str>, placeholder: &str) -> String {
    format!(
        "<textarea name=\"{name}\" rows=\"4\" maxlength=\"{MESSAGE_MAX}\" placeholder=\"{}\">{}</textarea>",
        escape(placeholder),
        escape(value.unwrap_or_default())
    )
}

fn number(name: &str, value: u32, min: u32, max: u32) -> String {
    format!("<input type=\"number\" name=\"{name}\" value=\"{value}\" min=\"{min}\" max=\"{max}\">")
}

pub const TEMPLATE_HINT: &str = "Değişkenler: <code>{kullanıcı}</code> (etiket), <code>{isim}</code>, <code>{sunucu}</code>, \
     <code>{üye}</code> (üye sayısı). Boş bırakırsan her seferinde Star Wars, Yüzüklerin Efendisi veya Harry Potter \
     temalı, GIF'li hazır mesajlardan biri gönderilir. Birden fazla metin için aralarına <code>|</code> koy.";

fn form_fields(slug: &str, cfg: &GuildConfig, options: &Options) -> String {
    match slug {
        "koruma" => section(
            "Giriş koruması",
            field(
                "En az hesap yaşı (gün)",
                &number("min_age", cfg.min_account_age_days, 0, 365),
                "Bundan yeni hesaplar girince atılır. 0 = kapalı.",
            ) + &field(
                "Baskın eşiği (10 saniyedeki giriş sayısı)",
                &number("raid", cfg.raid_join_threshold, 0, 100),
                "Eşik aşılırsa 10 dakika boyunca 7 günden yeni hesaplar atılır. 0 = kapalı.",
            ),
        ),
        "starboard" => section(
            "Starboard",
            field("Kanal", &options.channel_select("channel", cfg.starboard_channel), "")
                + &field("Gereken ⭐ sayısı", &number("threshold", cfg.starboard_threshold.max(1), 1, 50), ""),
        ),
        "oneriler" => section(
            "Öneriler",
            field("Öneri kanalı", &options.channel_select("channel", cfg.suggestion_channel), "Üyeler <code>/öneri gönder</code> ile gönderir."),
        ),
        "hayvanlar" => section(
            "Hayvan yakalama",
            field("Oyun kanalı", &options.channel_select("channel", cfg.animal_channel), "Bu kanalda 10-20 mesajda bir hayvan belirir."),
        ),
        _ => String::new(),
    }
}

#[derive(Deserialize)]
pub struct Flash {
    durum: Option<String>,
    /// Gömülü mesajlar sayfasında düzenlenecek mesajın kimliği.
    duzenle: Option<u64>,
}

fn flash(code: Option<&str>) -> &'static str {
    match code {
        Some("kaydedildi") => "<div class=\"flash ok\">✅ Kaydedildi. Değişiklik hemen geçerli.</div>",
        Some("kanal") => "<div class=\"flash err\">❌ Seçilen kanal bu sunucuda bulunamadı.</div>",
        Some("rol") => "<div class=\"flash err\">❌ Seçilen rol verilemez (silinmiş, yetkili ya da botun rolünün üstünde).</div>",
        Some("metin") => "<div class=\"flash err\">❌ Mesaj en fazla 1500 karakter olabilir.</div>",
        Some("sayi") => "<div class=\"flash err\">❌ Sayı izin verilen aralıkta değil.</div>",
        Some("ters") => "<div class=\"flash err\">❌ Ters davranış yalnızca emoji tepkili menülerde kullanılabilir.</div>",
        Some("kisayol") => "<div class=\"flash err\">❌ Sebep kısayolları \"kısa = tam metin\" biçiminde olmalı (kısa ad en fazla 20, metin 512 karakter, en fazla 25 satır).</div>",
        Some("emoji") => "<div class=\"flash err\">❌ Emoji geçersiz ya da eksik. Tepki türünde her rol için bir emoji gerekir; sunucu emojisi &lt;:ad:kimlik&gt; biçiminde olmalı.</div>",
        Some("tekrar") => "<div class=\"flash err\">❌ Aynı rol ya da aynı emoji bir menüde iki kez kullanılamaz.</div>",
        Some("bos") => "<div class=\"flash err\">❌ Zorunlu bir alan boş (başlık, cevap, mesaj ya da en az bir rol).</div>",
        Some("ad") => "<div class=\"flash err\">❌ Komut adı geçersiz: en fazla 32 karakter; harf, rakam, - ve _ (boşluk olmaz).</div>",
        Some("gonder") => "<div class=\"flash err\">❌ Mesaj Discord'a gönderilemedi. Botun o kanalda 'Mesaj Gönder', 'Bağlantı Yerleştir' ve 'Tepki Ekle' izni var mı?</div>",
        Some("renk") => "<div class=\"flash err\">❌ Renk geçersiz; #RRGGBB biçiminde olmalı.</div>",
        Some("adres") => "<div class=\"flash err\">❌ Resim adresi kabul edilmedi: https ile başlamalı ve herkese açık bir adres olmalı.</div>",
        Some("kelime") => "<div class=\"flash err\">❌ Kelimelerden biri geçersiz: 2-32 karakter olmalı; sadece harf, rakam, boşluk ve * içerebilir.</div>",
        Some("sure") => "<div class=\"flash err\">❌ Süre anlaşılamadı. Örnek: 30dk, 1sa, 7g (susturma en fazla 28 gün).</div>",
        Some("discord") => "<div class=\"flash err\">❌ Discord değişikliği kabul etmedi. Avatar/afiş çok sık değiştirilmiş olabilir ya da botun 'Takma Adı Değiştir' izni yok.</div>",
        Some("presence") => "<div class=\"flash err\">❌ Çevrimiçi sayaçları için Presence Intent açık olmalı: Developer Portal → Bot → Presence Intent, sonra botu yeniden başlat.</div>",
        Some("kaynak") => "<div class=\"flash err\">❌ Kanal bulunamadı. Adı ya da bağlantıyı kontrol et.</div>",
        Some("anahtar") => "<div class=\"flash err\">❌ Bu platformun anahtarları tanımlı değil. Bot sahibi .env dosyasına Twitch / Kick anahtarlarını yazmalı.</div>",
        Some("kopru") => "<div class=\"flash err\">❌ X, Instagram ve TikTok için RSS köprüsü tanımlı değil. Bot sahibi .env dosyasına RSS_BRIDGE_URL yazıp botu yeniden başlatmalı.</div>",
        Some("akis") => "<div class=\"flash err\">❌ Kaynak okunamadı. Ad ya da adres doğru mu, herkese açık bir RSS/Atom akışı mı?</div>",
        Some("automod") => "<div class=\"flash err\">❌ Discord AutoMod kurallarından biri uygulanamadı (ayrıntı Loglar sayfasında). Botun 'Sunucuyu Yönet' izni var mı? Sunucuda zaten Discord'un kendi hazır filtre ya da spam kuralı varsa önce onu kapat.</div>",
        Some("limit") => "<div class=\"flash err\">❌ Liste çok uzun; bazı öğeleri kaldır.</div>",
        Some("oturum") => "<div class=\"flash err\">❌ Oturum doğrulanamadı; sayfayı yenileyip tekrar dene.</div>",
        Some("hata") => "<div class=\"flash err\">❌ Kaydedilemedi; biraz sonra tekrar dene.</div>",
        _ => "",
    }
}

fn parse_guild(raw: u64) -> Option<GuildId> {
    (raw != 0).then(|| GuildId::new(raw))
}

pub async fn page(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((guild, slug)): Path<(u64, String)>,
    Query(query): Query<Flash>,
) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let (Some(guild_id), Some(plugin)) = (parse_guild(guild), find(&slug)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await
        || (super::ADMIN_ONLY_PLUGINS.contains(&plugin.slug) && !super::is_guild_admin(&state, session.user, guild_id).await)
    {
        return forbidden();
    }
    let Some(options) = Options::load(&state, guild_id) else {
        return forbidden();
    };
    let cfg = match state.configs.get(&state.db, guild_id).await {
        Ok(cfg) => cfg,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Panel: sunucu ayarları okunamadı");
            return forbidden();
        }
    };
    let custom = match plugin.slug {
        "karsilama" => Some(super::welcome::fields(&state, guild_id, &options).await),
        "dogrulama" => Some(super::welcome::verify_fields(&state, guild_id, &options).await),
        "seviye" => Some(super::levels::fields(&state, guild_id, &options).await),
        "moderasyon" => Some(super::moderation::fields(&state, guild_id, &options).await),
        "kayitlar" => Some(super::moderation::log_fields(&state, guild_id, &options).await),
        "tepki-rol" => Some(super::extras::role_menu_fields(&state, guild_id, &options).await.map(|(html, on)| (super::media::role_menu_templates() + &html, on))),
        "ek-starboard" => Some(super::media::starboard_fields(&state, guild_id, &options).await),
        "muzik" => Some(super::media::music_fields(&state, guild_id, &options).await),
        "ozel-komutlar" => Some(super::extras::command_fields(&state, guild_id, &options).await),
        "zamanlayici" => Some(super::extras::timer_fields(&state, guild_id, &options).await),
        "gomulu-mesaj" => Some(super::extras::embed_fields(&state, guild_id, &options, query.duzenle).await),
        "otomasyon" => Some(super::automation::automation_fields(&state, guild_id, &options).await.map(|(html, on)| (super::media::automation_templates() + &html, on))),
        "dogum-gunu" => Some(super::extras::birthday_fields(&state, guild_id, &options).await),
        "biletler" => Some(super::extras::ticket_fields(&state, guild_id, &options).await),
        "sosyal" => Some(super::extras::feed_fields(&state, guild_id, &options).await),
        "haberler" => Some(super::extras::news_fields(&state, guild_id, &options).await),
        "yz" => Some(super::botpage::ai_fields(&state, guild_id, &options).await),
        "bot" => Some(super::botpage::profile_fields(&state, guild_id).await),
        "topluluk" => Some(super::community::community_fields(&state, guild_id, &options).await),
        "ayarlar" => Some(super::community::settings_fields(&state, guild_id, &options).await),
        "yayinlar" => Some(super::community::stream_fields(&state, guild_id, &options).await),
        "otocevap" => Some(super::community::reply_fields(&state, guild_id).await),
        "davetler" => Some(super::community::invite_fields(&state, guild_id).await),
        "modmail" => Some(super::manage::modmail_fields(&state, guild_id, &options).await),
        "yikim-korumasi" => Some(super::manage::antinuke_fields(&state, guild_id, &options).await),
        "discord-automod" => Some(super::moderation::native_fields(&state, guild_id, &options).await),
        "etiketler" => Some(super::manage::tag_fields(&state, guild_id).await),
        "ekonomi" => Some(super::manage::economy_fields(&state, guild_id, &options).await),
        "sayaclar" => Some(super::manage::stat_fields(&state, guild_id, &options).await),
        "basvurular" => Some(super::manage::application_fields(&state, guild_id, &options).await),
        "gecici-odalar" => Some(super::manage::tempvoice_fields(&state, guild_id).await),
        "anket" => Some(Ok(super::manage::poll_fields(&options))),
        "cekilis" => Some(Ok(super::manage::giveaway_fields(&options))),
        "basarimlar" => Some(super::automation::achievement_fields(&state, guild_id, &options).await),
        _ => None,
    };
    let (fields, enabled) = match custom {
        Some(Ok(found)) => found,
        Some(Err(e)) => {
            warn!(guild = %guild_id, plugin = plugin.slug, error = ?e, "Panel: eklenti ayarları okunamadı");
            return forbidden();
        }
        None => (form_fields(plugin.slug, &cfg, &options), is_enabled(plugin.slug, &cfg, Extra::default())),
    };

    let body = format!(
        "<div class=\"phead\"><div><h1>{icon} {name}</h1><p class=\"sub\">{about}</p></div>{badge}</div>{flash}\
         <form method=\"post\" action=\"/panel/sunucu/{guild_id}/eklenti/{slug}\">\
         <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">{fields}\
         <div class=\"savebar\"><button>Kaydet</button><a class=\"btn ghost\" href=\"/panel/sunucu/{guild_id}\">Vazgeç</a></div></form>\
         <script src=\"/panel/panel.js\"></script>",
        icon = plugin.icon,
        name = escape(plugin.name),
        about = escape(plugin.about),
        badge = badge(enabled),
        flash = flash(query.durum.as_deref()),
        slug = plugin.slug,
        csrf = escape(&session.csrf),
    );
    let nav = GuildNav { id: guild_id, name: &options.guild_name, active: plugin.slug };
    Html(layout(&state, &session, Nav::Home, Some(nav), Page { title: plugin.name, body }).await).into_response()
}

/// Form alanlarını doğrulayan yardımcı; hata kodu `flash`'taki anahtarlardan biridir.
pub struct Input<'a> {
    /// Sıralı alan listesi: onay kutusu listeleri aynı adı birden çok kez gönderir.
    pub form: &'a [(String, String)],
    pub options: &'a Options,
}

impl Input<'_> {
    pub fn raw(&self, key: &str) -> &str {
        self.form.iter().find(|(k, _)| k == key).map_or("", |(_, v)| v.trim())
    }

    /// Aynı adla gönderilen tüm değerler.
    pub fn all(&self, key: &str) -> Vec<&str> {
        self.form.iter().filter(|(k, _)| k == key).map(|(_, v)| v.trim()).collect()
    }

    pub fn checked(&self, key: &str) -> bool {
        self.raw(key) == "1"
    }

    /// Çoklu seçimdeki kimlikler; listede olmayan tek bir değer bile hatadır.
    pub fn ids(&self, key: &str, allowed: &[(u64, String)], error: &'static str) -> Result<Vec<u64>, &'static str> {
        self.all(key)
            .into_iter()
            .map(|raw| {
                let id: u64 = raw.parse().map_err(|_| error)?;
                allowed.iter().any(|(known, _)| *known == id).then_some(id).ok_or(error)
            })
            .collect()
    }

    pub fn id(&self, key: &str, allowed: &[(u64, String)], error: &'static str) -> Result<Option<u64>, &'static str> {
        let raw = self.raw(key);
        if raw.is_empty() {
            return Ok(None);
        }
        let id: u64 = raw.parse().map_err(|_| error)?;
        allowed.iter().any(|(known, _)| *known == id).then_some(Some(id)).ok_or(error)
    }

    pub fn channel(&self, key: &str) -> Result<Option<ChannelId>, &'static str> {
        Ok(self.id(key, &self.options.channels, "kanal")?.map(ChannelId::new))
    }

    pub fn role(&self, key: &str) -> Result<Option<RoleId>, &'static str> {
        Ok(self.id(key, &self.options.roles, "rol")?.map(RoleId::new))
    }

    pub fn text(&self, key: &str) -> Result<Option<String>, &'static str> {
        let raw = self.raw(key);
        if raw.chars().count() > MESSAGE_MAX {
            return Err("metin");
        }
        Ok((!raw.is_empty()).then(|| raw.to_owned()))
    }

    pub fn number(&self, key: &str, min: u32, max: u32) -> Result<u32, &'static str> {
        let value: u32 = self.raw(key).parse().map_err(|_| "sayi")?;
        (min..=max).contains(&value).then_some(value).ok_or("sayi")
    }
}

/// Formu doğrular; hepsi geçerliyse yazılacak ayarları döner (yarım kayıt olmasın diye önce doğrulama).
fn settings_from(slug: &str, input: &Input<'_>) -> Result<Vec<Setting>, &'static str> {
    Ok(match slug {
        "koruma" => vec![
            Setting::MinAccountAge(input.number("min_age", 0, 365)?),
            Setting::RaidThreshold(input.number("raid", 0, 100)?),
        ],
        "starboard" => vec![Setting::Starboard(input.channel("channel")?, input.number("threshold", 1, 50)?)],
        "oneriler" => vec![Setting::SuggestionChannel(input.channel("channel")?)],
        "hayvanlar" => vec![Setting::AnimalChannel(input.channel("channel")?)],
        _ => return Err("hata"),
    })
}

pub async fn save(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((guild, slug)): Path<(u64, String)>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let (Some(guild_id), Some(plugin)) = (parse_guild(guild), find(&slug)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await
        || (super::ADMIN_ONLY_PLUGINS.contains(&plugin.slug) && !super::is_guild_admin(&state, session.user, guild_id).await)
    {
        return forbidden();
    }
    let back = |code: &str| Redirect::to(&format!("/panel/sunucu/{guild_id}/eklenti/{}?durum={code}", plugin.slug)).into_response();
    if form.iter().find(|(k, _)| k == "csrf").map(|(_, v)| v.as_str()) != Some(session.csrf.as_str()) {
        return back("oturum");
    }
    let Some(options) = Options::load(&state, guild_id) else {
        return forbidden();
    };
    let input = Input { form: &form, options: &options };
    let custom = match plugin.slug {
        "karsilama" => Some(super::welcome::save(&state, guild_id, &input).await),
        "dogrulama" => Some(super::welcome::verify_save(&state, guild_id, &input).await),
        "seviye" => Some(super::levels::save(&state, guild_id, &input).await),
        "moderasyon" => Some(super::moderation::save(&state, guild_id, &input).await),
        "kayitlar" => Some(super::moderation::log_save(&state, guild_id, &input).await),
        "tepki-rol" => Some(super::extras::role_menu_save(&state, guild_id, &input).await),
        "ek-starboard" => Some(super::media::starboard_save(&state, guild_id, &input).await),
        "muzik" => Some(super::media::music_save(&state, guild_id, &input).await),
        "ozel-komutlar" => Some(super::extras::command_save(&state, guild_id, &input).await),
        "zamanlayici" => Some(super::extras::timer_save(&state, guild_id, &input).await),
        "gomulu-mesaj" => Some(super::extras::embed_save(&state, guild_id, &input).await),
        "otomasyon" => Some(super::automation::automation_save(&state, guild_id, &input).await),
        "dogum-gunu" => Some(super::extras::birthday_save(&state, guild_id, &input).await),
        "biletler" => Some(super::extras::ticket_save(&state, guild_id, &input).await),
        "sosyal" => Some(super::extras::feed_save(&state, guild_id, &input).await),
        "haberler" => Some(super::extras::news_save(&state, guild_id, &input).await),
        "yz" => Some(super::botpage::ai_save(&state, guild_id, &input).await),
        "bot" => Some(super::botpage::profile_save(&state, guild_id, &input).await),
        "topluluk" => Some(super::community::community_save(&state, guild_id, &input).await),
        "ayarlar" => Some(super::community::settings_save(&state, guild_id, &input).await),
        "yayinlar" => Some(super::community::stream_save(&state, guild_id, &input).await),
        "otocevap" => Some(super::community::reply_save(&state, guild_id, &input).await),
        "davetler" => Some(Ok(())),
        "modmail" => Some(super::manage::modmail_save(&state, guild_id, &input).await),
        "yikim-korumasi" => Some(super::manage::antinuke_save(&state, guild_id, &input).await),
        "discord-automod" => Some(super::moderation::native_save(&state, guild_id, &input).await),
        "etiketler" => Some(super::manage::tag_save(&state, guild_id, &input).await),
        "ekonomi" => Some(super::manage::economy_save(&state, guild_id, &input).await),
        "sayaclar" => Some(super::manage::stat_save(&state, guild_id, &input).await),
        "basvurular" => Some(super::manage::application_save(&state, guild_id, &input).await),
        "gecici-odalar" => Some(super::manage::tempvoice_save(&state, guild_id, &input).await),
        "anket" => Some(super::manage::poll_save(&state, guild_id, &input).await),
        "cekilis" => Some(super::manage::giveaway_save(&state, guild_id, &input).await),
        "basarimlar" => Some(super::automation::achievement_save(&state, guild_id, &input).await),
        _ => None,
    };
    if let Some(result) = custom {
        return match result {
            Ok(()) => {
                info!(user = %session.user, guild = %guild_id, plugin = plugin.slug, "Panelden ayar değiştirildi");
                back("kaydedildi")
            }
            Err(code) => back(code),
        };
    }

    let settings = match settings_from(plugin.slug, &input) {
        Ok(settings) => settings,
        Err(code) => return back(code),
    };
    for setting in settings {
        if let Err(e) = state.configs.set(&state.db, guild_id, setting).await {
            warn!(guild = %guild_id, plugin = plugin.slug, error = ?e, "Panel: ayar kaydedilemedi");
            return back("hata");
        }
    }
    info!(user = %session.user, guild = %guild_id, plugin = plugin.slug, "Panelden ayar değiştirildi");
    back("kaydedildi")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options {
            guild_name: "Test".to_owned(),
            channels: vec![(10, "genel".to_owned())],
            categories: vec![(11, "Sohbet".to_owned())],
            roles: vec![(20, "Üye".to_owned())],
            all_roles: vec![(20, "Üye".to_owned()), (21, "Yönetici".to_owned())],
        }
    }

    fn form(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn multi_value_fields_are_all_validated() {
        let options = options();
        let picked = form(&[("xp_role", "20"), ("xp_role", "21"), ("enabled", "1")]);
        let input = Input { form: &picked, options: &options };
        assert_eq!(input.ids("xp_role", &options.all_roles, "rol"), Ok(vec![20, 21]));
        // Listede olmayan tek bir değer bütün seçimi geçersiz kılar.
        assert_eq!(input.ids("xp_role", &options.roles, "rol"), Err("rol"));
        assert!(input.checked("enabled") && !input.checked("yok"));
        assert!(input.ids("bos", &options.roles, "rol").is_ok_and(|ids| ids.is_empty()));
    }

    #[test]
    fn rejects_channels_outside_the_guild() {
        let options = options();
        let foreign = form(&[("channel", "999"), ("threshold", "3")]);
        assert_eq!(settings_from("starboard", &Input { form: &foreign, options: &options }).err(), Some("kanal"));
        let valid = form(&[("channel", "10"), ("threshold", "3")]);
        assert!(matches!(
            settings_from("starboard", &Input { form: &valid, options: &options }).as_deref(),
            Ok([Setting::Starboard(Some(_), 3)])
        ));
    }

    #[test]
    fn validates_numbers_and_message_length() {
        let options = options();
        let too_big = form(&[("min_age", "9999"), ("raid", "0")]);
        assert_eq!(settings_from("koruma", &Input { form: &too_big, options: &options }).err(), Some("sayi"));
        let long = "a".repeat(MESSAGE_MAX + 1);
        let long_message = form(&[("mesaj", long.as_str())]);
        assert_eq!(Input { form: &long_message, options: &options }.text("mesaj"), Err("metin"));
        assert_eq!(settings_from("yok", &Input { form: &[], options: &options }).err(), Some("hata"));
    }

    #[test]
    fn every_plugin_has_fields_and_unique_slug() {
        let options = options();
        let cfg = GuildConfig::default();
        let mut seen = std::collections::HashSet::new();
        for plugin in PLUGINS {
            assert!(seen.insert(plugin.slug));
            assert!(!is_enabled(plugin.slug, &cfg, Extra::default()));
            // Bu eklentilerin formu kendi modülünde (veritabanı okur).
            let own_module = ["karsilama", "dogrulama", "seviye", "moderasyon", "kayitlar", "tepki-rol", "ozel-komutlar", "zamanlayici", "gomulu-mesaj", "otomasyon", "basarimlar", "dogum-gunu", "biletler", "sosyal", "haberler", "yz", "bot", "topluluk", "ayarlar", "yayinlar", "otocevap", "davetler", "modmail", "yikim-korumasi", "discord-automod", "etiketler", "ekonomi", "sayaclar", "basvurular", "gecici-odalar", "anket", "cekilis", "ek-starboard", "muzik"];
            if !own_module.contains(&plugin.slug) {
                assert!(!form_fields(plugin.slug, &cfg, &options).is_empty(), "form yok: {}", plugin.slug);
            }
        }
    }
}
