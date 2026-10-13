//! Sağlık kontrolü: sunucunun bütün eklenti ayarlarını tarar; silinmiş kanal / rol, eksik izin ve
//! eksik anahtar gibi sorunları tek sayfada listeler.

use std::collections::HashSet;
use std::fmt::Write as _;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use poise::serenity_prelude::GuildId;
use tracing::warn;

use super::pages::{forbidden, layout, GuildNav, Nav, Page};
use super::{auth, can_manage};
use crate::setup_report::{self, State as FeatureState};
use crate::web::{escape, WebState};

/// Bir ayarın işaret ettiği kanal ya da rol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Channel(u64),
    Role(u64),
}

/// Taranan bir bağlantı: hangi eklentide, ne için, neyi gösteriyor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub slug: &'static str,
    pub what: String,
    pub target: Target,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Problem {
    pub slug: &'static str,
    pub text: String,
    pub fix: String,
}

fn link(slug: &'static str, what: impl Into<String>, target: Target) -> Link {
    Link { slug, what: what.into(), target }
}

/// Sunucuda artık bulunmayan kanal / rollere işaret eden bağlantılar (0 kimlikleri yok sayılır).
pub fn dangling(links: &[Link], channels: &HashSet<u64>, roles: &HashSet<u64>) -> Vec<Problem> {
    links
        .iter()
        .filter_map(|l| {
            let (missing, kind) = match l.target {
                Target::Channel(id) => (id != 0 && !channels.contains(&id), "kanal"),
                Target::Role(id) => (id != 0 && !roles.contains(&id), "rol"),
            };
            missing.then(|| Problem {
                slug: l.slug,
                text: format!("{}: ayarlı {kind} silinmiş.", l.what),
                fix: format!("Eklenti sayfasından yeni bir {kind} seç ya da ayarı kapat."),
            })
        })
        .collect()
}

/// Bütün eklenti ayarlarındaki kanal / rol bağlantılarını toplar. Okunamayan ayar atlanır.
async fn collect_links(state: &WebState, guild_id: GuildId) -> Vec<Link> {
    use Target::{Channel, Role};
    let (db, settings) = (&state.db, &state.settings);
    let mut links = Vec::new();

    if let Ok(cfg) = state.configs.get(db, guild_id).await {
        let channels = [
            ("karsilama", "Karşılama kanalı", cfg.welcome_channel),
            ("karsilama", "Veda kanalı", cfg.goodbye_channel),
            ("kayitlar", "Olay kayıt kanalı", cfg.event_log_channel),
            ("starboard", "Starboard kanalı", cfg.starboard_channel),
            ("oneriler", "Öneri kanalı", cfg.suggestion_channel),
            ("dogum-gunu", "Doğum günü kanalı", cfg.birthday_channel),
            ("gecici-odalar", "Oda oluşturma kanalı", cfg.tempvoice_hub),
            ("hayvanlar", "Hayvan yakalama kanalı", cfg.animal_channel),
        ];
        links.extend(channels.into_iter().filter_map(|(slug, what, id)| Some(link(slug, what, Channel(id?.get())))));
        let roles = [("karsilama", "Otomatik rol", cfg.autorole), ("dogrulama", "Doğrulama rolü", cfg.verify_role), ("dogum-gunu", "Doğum günü rolü", cfg.birthday_role)];
        links.extend(roles.into_iter().filter_map(|(slug, what, id)| Some(link(slug, what, Role(id?.get())))));
    }
    if let Ok(s) = settings.get::<crate::cards::WelcomeSettings>(db, guild_id).await {
        links.extend(s.extra_roles.iter().map(|r| link("karsilama", "Karşılamadaki ek rol", Role(*r))));
    }
    if let Ok(s) = settings.get::<crate::leveling::settings::LevelSettings>(db, guild_id).await {
        links.extend(s.announce_channel.map(|c| link("seviye", "Seviye duyuru kanalı", Channel(c))));
        links.extend(s.role_boosts.iter().map(|b| link("seviye", "XP çarpanı rolü", Role(b.role))));
    }
    if let Ok(s) = settings.get::<crate::automod::rules::ModSettings>(db, guild_id).await {
        links.extend(s.exempt_roles.iter().map(|r| link("moderasyon", "Otomatik moderasyondan muaf rol", Role(*r))));
    }
    if let Ok(s) = settings.get::<crate::reactionroles::ReactionRoleSettings>(db, guild_id).await {
        for menu in &s.menus {
            links.push(link("tepki-rol", format!("Rol menüsü “{}”", menu.title), Channel(menu.channel_id)));
            links.extend(menu.pairs.iter().map(|p| link("tepki-rol", format!("Rol menüsü “{}” içindeki rol", menu.title), Role(p.role))));
        }
    }
    if let Ok(s) = settings.get::<crate::customcmd::CustomCommandSettings>(db, guild_id).await {
        for command in &s.commands {
            let what = format!("Özel komut “!{}”", command.name);
            links.extend(command.add_roles.iter().chain(&command.remove_roles).chain(&command.required_roles).map(|r| link("ozel-komutlar", format!("{what}: rol"), Role(*r))));
            links.extend(command.channels.iter().map(|c| link("ozel-komutlar", format!("{what}: kanal"), Channel(*c))));
        }
    }
    if let Ok(s) = settings.get::<crate::automations::AutomationSettings>(db, guild_id).await {
        for automation in &s.automations {
            let what = format!("Otomasyon “{}”", automation.name);
            links.extend(automation.channel.iter().chain(&automation.message_channel).map(|c| link("otomasyon", format!("{what}: kanal"), Channel(*c))));
            links.extend(automation.role.iter().chain(&automation.add_roles).chain(&automation.remove_roles).map(|r| link("otomasyon", format!("{what}: rol"), Role(*r))));
        }
    }
    if let Ok(s) = settings.get::<crate::timers::TimerSettings>(db, guild_id).await {
        links.extend(s.timers.iter().map(|t| link("zamanlayici", "Tekrarlı mesajın kanalı", Channel(t.channel))));
    }
    if let Ok(s) = settings.get::<crate::feeds::FeedSettings>(db, guild_id).await {
        for feed in &s.feeds {
            links.push(link("sosyal", format!("Akış “{}”: duyuru kanalı", feed.name), Channel(feed.channel)));
            links.extend(feed.role.map(|r| link("sosyal", format!("Akış “{}”: etiketlenen rol", feed.name), Role(r))));
        }
    }
    if let Ok(s) = settings.get::<crate::news::NewsSettings>(db, guild_id).await {
        if s.enabled {
            links.extend(s.channel.map(|c| link("haberler", "Haber kanalı", Channel(c))));
            links.extend(s.channels.iter().map(|(k, c)| link("haberler", format!("Haber kanalı ({k})"), Channel(*c))));
            links.extend(s.role.map(|r| link("haberler", "Haberlerde etiketlenen rol", Role(r))));
            links.extend(s.roles.iter().map(|(k, r)| link("haberler", format!("Haberlerde etiketlenen rol ({k})"), Role(*r))));
        }
    }
    if let Ok(s) = settings.get::<crate::community::CommunitySettings>(db, guild_id).await {
        links.extend(s.sticky.iter().map(|x| link("topluluk", "Sabit mesajın kanalı", Channel(x.channel))));
        links.extend(s.counting_channel.map(|c| link("topluluk", "Sayma kanalı", Channel(c))));
        links.extend(s.confession_channel.map(|c| link("topluluk", "İtiraf kanalı", Channel(c))));
        links.extend(s.word_chain_channel.map(|c| link("topluluk", "Kelime zinciri kanalı", Channel(c))));
        links.extend(s.season_channel.map(|c| link("topluluk", "Sezon duyuru kanalı", Channel(c))));
        links.extend(s.clip_channel.map(|c| link("topluluk", "Klip kanalı", Channel(c))));
        links.extend(s.confession_log_channel.map(|c| link("topluluk", "İtiraf yazarının bildirildiği kanal", Channel(c))));
        links.extend(s.manager_roles.iter().map(|r| link("ayarlar", "Bot yöneticisi rolü", Role(*r))));
        links.extend(s.boost_channel.map(|c| link("topluluk", "Takviye teşekkür kanalı", Channel(c))));
        links.extend(s.boost_role.map(|r| link("topluluk", "Takviye ödül rolü", Role(r))));
    }
    if let Ok(s) = settings.get::<crate::music::playlists::MusicExtras>(db, guild_id).await {
        links.extend(s.dj_role.map(|r| link("muzik", "DJ rolü", Role(r))));
    }
    if let Ok(s) = settings.get::<crate::modmail::ModmailSettings>(db, guild_id).await {
        links.extend(s.channel.map(|c| link("modmail", "ModMail yetkili kanalı", Channel(c))));
    }
    if let Ok(s) = settings.get::<crate::starboards::StarboardSettings>(db, guild_id).await {
        links.extend(s.boards.iter().map(|b| link("ek-starboard", format!("{} panosunun kanalı", b.emoji), Channel(b.channel))));
    }
    if let Ok(s) = settings.get::<crate::antinuke::AntiNukeSettings>(db, guild_id).await {
        links.extend(s.trusted_roles.iter().map(|r| link("yikim-korumasi", "Güvenilen rol", Role(*r))));
    }
    if let Ok(s) = settings.get::<crate::nativeautomod::NativeAutomodSettings>(db, guild_id).await {
        links.extend(s.alert_channel.map(|c| link("discord-automod", "Discord AutoMod bildirim kanalı", Channel(c))));
        links.extend(s.exempt_roles.iter().map(|r| link("discord-automod", "Discord AutoMod muaf rolü", Role(*r))));
    }
    if let Ok(s) = settings.get::<crate::ai::characters::AiSettings>(db, guild_id).await {
        links.extend(s.assignments.iter().map(|a| link("yz", "Karakter atanmış kanal", Channel(a.channel))));
    }
    if let Ok(channels) = state.ai.channels(db, guild_id).await {
        links.extend(channels.iter().map(|c| link("yz", "Yapay zekâ sohbet kanalı", Channel(c.get()))));
    }
    if let Ok(Some(t)) = crate::tickets::load_settings(db, guild_id).await {
        links.extend(t.category_id.map(|c| link("biletler", "Bilet kategorisi", Channel(c.get()))));
        links.extend(t.support_role_id.map(|r| link("biletler", "Destek ekibi rolü", Role(r.get()))));
    }
    if let Ok(s) = settings.get::<crate::tickets::TicketExtras>(db, guild_id).await {
        links.extend(s.closed_category.map(|c| link("biletler", "Kapatılan biletlerin kategorisi", Channel(c))));
    }
    if let Ok(list) = crate::youtube::list(db, guild_id).await {
        for sub in list {
            links.push(link("yayinlar", format!("YouTube “{}”: duyuru kanalı", sub.name), Channel(sub.channel_id.get())));
            links.extend(sub.role_id.map(|r| link("yayinlar", format!("YouTube “{}”: etiketlenen rol", sub.name), Role(r.get()))));
        }
    }
    if let Ok(list) = crate::streams::list(db, guild_id).await {
        for sub in list {
            links.push(link("yayinlar", format!("{} “{}”: duyuru kanalı", sub.platform.label(), sub.login), Channel(sub.channel_id.get())));
            links.extend(sub.role_id.map(|r| link("yayinlar", format!("{} “{}”: etiketlenen rol", sub.platform.label(), sub.login), Role(r.get()))));
        }
    }
    links
}

/// Kanal / rol dışındaki sorunlar: eksik anahtar, kapalı izin gibi.
async fn other_problems(state: &WebState, guild_id: GuildId) -> Vec<Problem> {
    let db = &state.db;
    let mut problems = Vec::new();
    let mut add = |slug: &'static str, text: &str, fix: &str| problems.push(Problem { slug, text: text.to_owned(), fix: fix.to_owned() });

    if state.panel.ai.is_none() && state.ai.channels(db, guild_id).await.is_ok_and(|c| !c.is_empty()) {
        add("yz", "Yapay zekâ kanalı ayarlı ama yapay zekâ anahtarı yok; bot bu kanallarda cevap vermez.", "Bot sahibi .env dosyasına NVIDIA_API_KEY ya da GEMINI_API_KEY yazıp botu yeniden başlatmalı.");
    }
    if let Ok(list) = crate::streams::list(db, guild_id).await {
        if list.iter().any(|s| !state.streams.is_configured(s.platform)) {
            add("yayinlar", "Twitch / Kick kanalı takip ediliyor ama o platformun anahtarları yok; duyuru gelmez.", "Bot sahibi .env dosyasına platformun Client ID ve Secret değerlerini yazmalı.");
        }
    }
    if let Ok(feeds) = state.settings.get::<crate::feeds::FeedSettings>(db, guild_id).await {
        let needs_bridge = feeds.feeds.iter().any(|f| matches!(f.kind, crate::feeds::Kind::X | crate::feeds::Kind::Instagram | crate::feeds::Kind::Tiktok));
        if needs_bridge && crate::integrations::get().rss_bridge_url.is_none() {
            add("sosyal", "X / Instagram / TikTok akışı var ama RSS köprüsü tanımlı değil; duyuru gelmez.", "Bot sahibi .env dosyasına RSS_BRIDGE_URL yazmalı.");
        }
    }
    if !state.panel.members_intent {
        add("karsilama", "Server Members Intent kapalı: karşılama, otorol, giriş koruması ve üye kayıtları çalışmaz.", "Developer Portal → Bot → Server Members Intent'i aç, botu yeniden başlat.");
    }
    if let Ok(s) = state.settings.get::<crate::antinuke::AntiNukeSettings>(db, guild_id).await {
        if s.enabled && state.configs.get(db, guild_id).await.is_ok_and(|cfg| cfg.event_log_channel.is_none()) {
            add("yikim-korumasi", "Yıkım koruması açık ama olay kayıt kanalı yok; tetiklenince uyarı yazılacak yer bulunamaz.", "“Olay kayıtları” sayfasından bir kayıt kanalı seç.");
        }
    }
    problems
}

pub async fn page(State(state): State<WebState>, headers: HeaderMap, Path(guild): Path<u64>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await {
        return forbidden();
    }
    // Sunucunun kanal ve rolleri (önbellekten; kilit hemen bırakılır). Alt başlıklar da kanal sayılır.
    let Some((name, channels, roles)) = state.cache.guild(guild_id).map(|g| {
        let channels: HashSet<u64> = g.channels.keys().map(|c| c.get()).chain(g.threads.iter().map(|t| t.id.get())).collect();
        let roles: HashSet<u64> = g.roles.keys().map(|r| r.get()).collect();
        (g.name.clone(), channels, roles)
    }) else {
        return forbidden();
    };

    let mut problems = Vec::new();
    match setup_report::build(&state.http, &state.cache, &state.db, &state.configs, guild_id).await {
        Ok(report) => {
            if !report.missing.is_empty() {
                problems.push(Problem {
                    slug: "",
                    text: format!("Botun eksik izinleri: {}.", crate::error::permission_names(report.missing)),
                    fix: "Sunucu Ayarları → Roller → botun rolü → bu izinleri aç. Sayaç kanalları, geçici odalar ve moderasyon bu izinler olmadan sessizce çalışmaz.".to_owned(),
                });
            }
            if !report.role_on_top {
                problems.push(Problem {
                    slug: "",
                    text: "Botun rolü en üstte değil.".to_owned(),
                    fix: "Sunucu Ayarları → Roller → botun rolünü en üste sürükle; yoksa altındaki rolleri veremez, üstündekileri cezalandıramaz.".to_owned(),
                });
            }
            for feature in report.features.iter().filter(|f| matches!(f.state, FeatureState::Broken)) {
                problems.push(Problem { slug: "", text: format!("{}: ayarlı kanal ya da rol silinmiş.", feature.name), fix: format!("Yeniden ayarla: {}", feature.command) });
            }
        }
        Err(e) => warn!(guild = %guild_id, error = ?e, "Sağlık kontrolü: sunucu raporu alınamadı"),
    }
    let links = collect_links(&state, guild_id).await;
    let scanned = links.len();
    problems.extend(dangling(&links, &channels, &roles));
    problems.extend(other_problems(&state, guild_id).await);
    // Aynı sorun iki kaynaktan gelebilir (kurulum raporu + tarama).
    let mut seen = HashSet::new();
    problems.retain(|p| seen.insert(p.text.clone()));

    let mut body = format!(
        "<p><a href=\"/panel/sunucu/{guild_id}\">← {}</a></p><h1>🩺 Sağlık kontrolü</h1>\
         <p class=\"sub\">Eklenti ayarlarındaki {scanned} kanal / rol bağlantısı, botun izinleri ve gerekli anahtarlar tarandı.</p>",
        escape(&name)
    );
    if problems.is_empty() {
        body.push_str("<div class=\"box\">✅ Sorun bulunamadı. Bütün ayarlar var olan kanal ve rollere işaret ediyor, botun izinleri tam.</div>");
    } else {
        let _ = write!(body, "<div class=\"box warn\">⚠️ <b>{}</b> sorun bulundu. Düzelttikten sonra bu sayfayı yenile.</div><div style=\"height:14px\"></div>", problems.len());
        body.push_str("<table><tr><th>Sorun</th><th>Ne yapmalı</th><th></th></tr>");
        for problem in &problems {
            let open = if problem.slug.is_empty() { String::new() } else { format!("<a href=\"/panel/sunucu/{guild_id}/eklenti/{}\">Aç →</a>", problem.slug) };
            let _ = write!(body, "<tr><td>{}</td><td>{}</td><td style=\"white-space:nowrap\">{open}</td></tr>", escape(&problem.text), escape(&problem.fix));
        }
        body.push_str("</table>");
    }
    body.push_str(
        "<p class=\"note\">Bir kanal ya da rol silindiğinde karşılama, otorol, kayıt kanalı gibi temel ayarlar kendiliğinden kapatılır ve olay kayıt kanalına \
         not düşülür; diğer eklentilerdeki kopuk bağlantılar burada görünür.</p>",
    );
    let nav = GuildNav { id: guild_id, name: &name, active: "saglik" };
    Html(layout(&state, &session, Nav::Home, Some(nav), Page { title: "Sağlık kontrolü", body }).await).into_response()
}

/// Dışarıdan "bot çalışıyor mu" yoklaması: her şey yolundaysa 200, değilse 503. Gizli bilgi içermez.
pub async fn probe(State(state): State<WebState>) -> Response {
    let discord = match state.panel.shards.get() {
        Some(manager) => {
            let runners = manager.runners.lock().await;
            !runners.is_empty() && runners.values().all(|r| r.stage == poise::serenity_prelude::ConnectionStage::Connected)
        }
        None => false,
    };
    let database = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();
    let mut conn = state.redis.clone();
    let redis = redis::cmd("PING").query_async::<String>(&mut conn).await.is_ok();
    let ok = discord && database && redis;
    let body = serde_json::json!({ "durum": if ok { "calisiyor" } else { "sorun" }, "discord": discord, "veritabani": database, "redis": redis, "calisma_suresi_sn": state.panel.started_at.elapsed().as_secs() });
    let status = if ok { axum::http::StatusCode::OK } else { axum::http::StatusCode::SERVICE_UNAVAILABLE };
    (status, axum::Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_missing_targets() {
        let links = vec![
            link("karsilama", "Karşılama kanalı", Target::Channel(10)),
            link("karsilama", "Otomatik rol", Target::Role(20)),
            link("sosyal", "Akış", Target::Channel(11)),
            link("ayarlar", "Rol", Target::Role(21)),
            // 0 = "ayarlanmamış" artıkları; sorun sayılmaz.
            link("topluluk", "Sabit mesaj", Target::Channel(0)),
        ];
        let channels: HashSet<u64> = [10].into();
        let roles: HashSet<u64> = [21].into();
        let found = dangling(&links, &channels, &roles);
        assert_eq!(found.iter().map(|p| (p.slug, p.text.as_str())).collect::<Vec<_>>(), [("karsilama", "Otomatik rol: ayarlı rol silinmiş."), ("sosyal", "Akış: ayarlı kanal silinmiş.")]);
        assert!(dangling(&[], &channels, &roles).is_empty());
    }
}
