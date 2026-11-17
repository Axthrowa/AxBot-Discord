//! Panel: Seviye eklentisinin ayar formu ve sıralama sayfaları.

use std::fmt::Write as _;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use poise::serenity_prelude::{ChannelId, GuildId, UserId};
use tracing::warn;

use super::auth;
use super::can_manage;
use super::pages::{forbidden, layout, layout_public, GuildNav, Nav, Page};
use super::plugins::{checkbox, checklist, field, radios, section, textarea, Input, Options};
use crate::leveling::settings::{Announce, LevelSettings, RoleBoost, BOOSTS, DEFAULT_MESSAGE, MAX_ROLE_BOOSTS, RATES};
use crate::leveling::{formula, ExclusionKind};
use crate::web::{escape, WebState};

/// Sıralama sayfasında gösterilen üye sayısı.
const BOARD_SIZE: usize = 100;
const MAX_REWARD_LEVEL: u32 = 500;

fn rate_label(rate: u16) -> String {
    let value = f64::from(rate) / 100.0;
    format!("x{value}")
}

/// Form alanları ve eklentinin açık olup olmadığı.
pub async fn fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<LevelSettings>(&state.db, guild_id).await?;
    let rewards = state.leveling.rewards(&state.db, guild_id).await?;
    let exclusions = state.leveling.exclusions(&state.db, guild_id).await?;
    let listed = |kind: ExclusionKind| -> Vec<u64> { exclusions.iter().filter(|(_, k)| **k == kind).map(|(id, _)| *id).collect() };

    let mut out = section(
        "Genel",
        checkbox("enabled", settings.enabled, "Seviye sistemi açık (kapalıyken kimse XP kazanmaz)")
            + &format!(
                "<p class=\"note\">Her üye dakikada en fazla bir kez, yazdığı ilk mesajla 15-25 XP kazanır. \
                 <a href=\"/panel/sunucu/{guild_id}/siralama\">Sıralamayı gör →</a></p>"
            ),
    );

    // Seviye atlama duyurusu.
    let mut announce = String::from("<select name=\"announce\">");
    for (kind, code, label) in Announce::ALL {
        let _ = write!(announce, "<option value=\"{code}\"{}>{label}</option>", if settings.announce == kind { " selected" } else { "" });
    }
    announce.push_str("</select>");
    out.push_str(&section(
        "Seviye atlama duyurusu",
        field("Duyurunun gideceği yer", &announce, "")
            + &field(
                "Kanal (\"Belirli bir kanal\" seçiliyse)",
                &options.channel_select("announce_channel", settings.announce_channel.filter(|id| *id != 0).map(ChannelId::new)),
                "",
            )
            + &field(
                "Duyuru mesajı",
                &textarea("message", settings.message.as_deref(), DEFAULT_MESSAGE),
                "Değişkenler: <code>{kullanıcı}</code> (etiket), <code>{isim}</code>, <code>{seviye}</code>, <code>{sunucu}</code>. \
                 Boş bırakırsan varsayılan mesaj kullanılır.",
            ),
    ));

    // Ödül rolleri.
    let mut table = String::new();
    if rewards.is_empty() {
        table.push_str("<p class=\"note\">Henüz ödül rolü yok.</p>");
    } else {
        table.push_str("<table><tr><th>Seviye</th><th>Rol</th><th>Sil</th></tr>");
        for reward in rewards.iter() {
            let name = options
                .all_roles
                .iter()
                .find(|(id, _)| *id == reward.role_id.get())
                .map_or_else(|| "(silinmiş rol)".to_owned(), |(_, name)| format!("@ {name}"));
            let _ = write!(
                table,
                "<tr><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"reward_del\" value=\"{}\"></td></tr>",
                reward.level,
                escape(&name),
                reward.level
            );
        }
        table.push_str("</table>");
    }
    out.push_str(&section(
        "Ödül rolleri",
        table
            + "<div style=\"height:14px\"></div>"
            + &field(
                "Yeni ödül: seviye",
                &format!("<input type=\"number\" name=\"reward_level\" min=\"1\" max=\"{MAX_REWARD_LEVEL}\" placeholder=\"örn. 10\">"),
                "",
            )
            + &field(
                "Yeni ödül: rol",
                &options.role_select("reward_role", None),
                "Aynı seviyeye ikinci rol verilirse eskisinin yerine geçer.",
            )
            + &field(
                "Ödül türü",
                &radios(
                    "stack_rewards",
                    &[
                        ("1", "Önceki ödülleri tut (üye kazandığı tüm rolleri taşır)"),
                        ("0", "Önceki ödülleri kaldır (üyede sadece en yüksek ödül kalır)"),
                    ],
                    if settings.stack_rewards { "1" } else { "0" },
                ),
                "",
            )
            + &checkbox("remove_on_loss", settings.remove_on_loss, "Üye XP kaybedip seviyesi düşerse ödül rolünü geri al (/seviye xp al sonrası)"),
    ));

    // TP oranı.
    let mut rates = String::from("<select name=\"xp_rate\">");
    for rate in RATES {
        let _ = write!(rates, "<option value=\"{rate}\"{}>{}</option>", if settings.xp_rate == rate { " selected" } else { "" }, rate_label(rate));
    }
    rates.push_str("</select>");
    out.push_str(&section("TP oranı", field("Kazanılan XP çarpanı", &rates, "Seviye atlamayı zorlaştırmak ya da kolaylaştırmak için.")));

    // Role göre ek XP.
    let mut boosts = String::new();
    if settings.role_boosts.is_empty() {
        boosts.push_str("<p class=\"note\">Henüz rol çarpanı yok.</p>");
    } else {
        boosts.push_str("<table><tr><th>Rol</th><th>Çarpan</th><th>Kaldır</th></tr>");
        for boost in &settings.role_boosts {
            let name = options.all_roles.iter().find(|(id, _)| *id == boost.role).map_or("(silinmiş rol)", |(_, n)| n.as_str());
            let _ = write!(
                boosts,
                "<tr><td>@ {}</td><td>{}</td><td><input type=\"checkbox\" name=\"boost_del\" value=\"{}\"></td></tr>",
                escape(name),
                rate_label(boost.percent),
                boost.role
            );
        }
        boosts.push_str("</table>");
    }
    let mut percents = String::from("<select name=\"boost_percent\">");
    for percent in BOOSTS {
        let _ = write!(percents, "<option value=\"{percent}\"{}>{}</option>", if percent == 200 { " selected" } else { "" }, rate_label(percent));
    }
    percents.push_str("</select>");
    out.push_str(&section(
        "Role göre XP çarpanı",
        boosts
            + "<div style=\"height:14px\"></div>"
            + &field("Yeni çarpan: rol", &Options::select("boost_role", &options.all_roles, "@ ", None, "— Seç —"), "")
            + &field("Çarpan", &percents, "Ör. takviyecilere 2 kat. Üye birden çok roldeyse en yüksek çarpan geçerlidir; genel TP oranıyla çarpılır.")
            + &format!("<p class=\"note\">En fazla {MAX_ROLE_BOOSTS} rol.</p>"),
    ));

    // XP listeleri.
    let mut channel_items = options.channels.clone();
    channel_items.extend(options.categories.iter().map(|(id, name)| (*id, format!("{name} (kategori)"))));
    out.push_str(&section(
        "XP kazanamayan roller",
        radios(
            "roles_mode",
            &[("deny", "Seçilen roller XP kazanamaz"), ("allow", "Sadece seçilen roller XP kazanır")],
            if settings.roles_allow_only { "allow" } else { "deny" },
        ) + &checklist("xp_role", &options.all_roles, "@ ", &listed(ExclusionKind::Role)),
    ));
    out.push_str(&section(
        "XP kazanılmayan kanallar",
        radios(
            "channels_mode",
            &[("deny", "Seçilen kanallarda XP kazanılmaz"), ("allow", "Sadece seçilen kanallarda XP kazanılır")],
            if settings.channels_allow_only { "allow" } else { "deny" },
        ) + &checklist("xp_channel", &channel_items, "# ", &listed(ExclusionKind::Channel))
            + "<p class=\"note\">Kategori seçilirse içindeki tüm kanallar için geçerlidir.</p>",
    ));

    // Sıralama sayfası.
    let public_link = state
        .panel
        .public_url
        .get()
        .map(|base| format!("{base}/siralama/{guild_id}"))
        .map_or_else(String::new, |url| format!("<p class=\"note\">Adres: <a href=\"{0}\" target=\"_blank\" rel=\"noopener\">{0}</a></p>", escape(&url)));
    out.push_str(&section(
        "Sıralama sayfası",
        checkbox("public_leaderboard", settings.public_leaderboard, "Sıralama sayfası herkese açık olsun (bağlantıyı bilen giriş yapmadan görür)")
            + &public_link,
    ));
    Ok((out, settings.enabled))
}

/// Formu doğrular ve kaydeder; hata kodu `plugins::flash` anahtarlarından biridir.
pub async fn save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    // 1) Önce her şeyi doğrula (yarım kayıt olmasın).
    let announce = Announce::from_code(input.raw("announce")).ok_or("hata")?;
    let announce_channel = input.channel("announce_channel")?.map(ChannelId::get);
    if announce == Announce::Channel && announce_channel.is_none() {
        return Err("kanal");
    }
    let xp_rate: u16 = input.raw("xp_rate").parse().ok().filter(|rate| RATES.contains(rate)).ok_or("sayi")?;
    let mode = |key: &str| match input.raw(key) {
        "allow" => Ok(true),
        "deny" | "" => Ok(false),
        _ => Err("hata"),
    };
    let current = state.settings.get::<LevelSettings>(&state.db, guild_id).await.map_err(|e| {
        warn!(guild = %guild_id, error = ?e, "Panel: seviye ayarı okunamadı");
        "hata"
    })?;
    let removed: Vec<u64> = input.all("boost_del").into_iter().map(|raw| raw.parse().map_err(|_| "hata")).collect::<Result<_, _>>()?;
    let mut role_boosts: Vec<RoleBoost> = current.role_boosts.iter().copied().filter(|b| !removed.contains(&b.role)).collect();
    if let Some(role) = input.id("boost_role", &input.options.all_roles, "rol")? {
        let percent: u16 = input.raw("boost_percent").parse().ok().filter(|p| BOOSTS.contains(p)).ok_or("sayi")?;
        role_boosts.retain(|b| b.role != role);
        if role_boosts.len() >= MAX_ROLE_BOOSTS {
            return Err("limit");
        }
        role_boosts.push(RoleBoost { role, percent });
    }
    let settings = LevelSettings {
        role_boosts,
        enabled: input.checked("enabled"),
        announce,
        announce_channel,
        message: input.text("message")?,
        stack_rewards: input.raw("stack_rewards") != "0",
        remove_on_loss: input.checked("remove_on_loss"),
        xp_rate,
        roles_allow_only: mode("roles_mode")?,
        channels_allow_only: mode("channels_mode")?,
        public_leaderboard: input.checked("public_leaderboard"),
    };

    let mut channel_items = input.options.channels.clone();
    channel_items.extend(input.options.categories.iter().cloned());
    let xp_channels = input.ids("xp_channel", &channel_items, "kanal")?;
    let xp_roles = input.ids("xp_role", &input.options.all_roles, "rol")?;

    let deletions: Vec<u32> = input.all("reward_del").into_iter().map(|raw| raw.parse().map_err(|_| "sayi")).collect::<Result<_, _>>()?;
    let new_reward = match (input.raw("reward_level"), input.role("reward_role")?) {
        ("", None) => None,
        (level, Some(role)) if !level.is_empty() => {
            let level: u32 = level.parse().ok().filter(|l| (1..=MAX_REWARD_LEVEL).contains(l)).ok_or("sayi")?;
            Some((level, role))
        }
        // Sadece seviye ya da sadece rol girilmiş.
        (_, None) => return Err("rol"),
        (_, Some(_)) => return Err("sayi"),
    };

    // 2) Kaydet.
    let failed = |e: anyhow::Error| {
        warn!(guild = %guild_id, error = ?e, "Panel: seviye ayarı kaydedilemedi");
        "hata"
    };
    state.settings.set(&state.db, guild_id, settings).await.map_err(failed)?;
    state.leveling.set_exclusions(&state.db, guild_id, &xp_channels, &xp_roles).await.map_err(failed)?;
    for level in deletions {
        state.leveling.remove_reward(&state.db, guild_id, level).await.map_err(failed)?;
    }
    if let Some((level, role)) = new_reward {
        state.leveling.set_reward(&state.db, guild_id, level, role).await.map_err(failed)?;
    }
    Ok(())
}

struct Row {
    name: String,
    avatar: Option<String>,
    xp: u64,
}

/// En çok XP'si olan üyeler; adlar önbellekten (Discord'a istek atılmaz).
async fn board(state: &WebState, guild_id: GuildId) -> anyhow::Result<Vec<Row>> {
    let top = state.leveling.top(&state.db, &state.redis, guild_id, BOARD_SIZE).await?;
    let guild = state.cache.guild(guild_id);
    Ok(top
        .into_iter()
        .map(|(user, xp)| {
            let member = guild.as_ref().and_then(|g| g.members.get(&user));
            let cached = member.map(|m| (m.display_name().to_owned(), m.face())).or_else(|| cached_user(state, user));
            let (name, avatar) = cached.map_or_else(|| (format!("Üye {}", user.get() % 10_000), None), |(name, avatar)| (name, Some(avatar)));
            Row { name, avatar, xp }
        })
        .collect())
}

fn cached_user(state: &WebState, user: UserId) -> Option<(String, String)> {
    let user = state.cache.user(user)?;
    Some((user.global_name.clone().unwrap_or_else(|| user.name.clone()), user.face()))
}

fn board_html(rows: &[Row]) -> String {
    if rows.is_empty() {
        return "<div class=\"box\">Henüz kimse XP kazanmadı.</div>".to_owned();
    }
    let mut out = String::from("<div class=\"board\">");
    for (index, row) in rows.iter().enumerate() {
        let progress = formula::progress(row.xp);
        let percent = (progress.current * 100).checked_div(progress.needed).unwrap_or(0).min(100);
        let medal = match index {
            0 => "🥇".to_owned(),
            1 => "🥈".to_owned(),
            2 => "🥉".to_owned(),
            n => format!("#{}", n + 1),
        };
        let avatar = row
            .avatar
            .as_deref()
            .map_or_else(|| "<div class=\"bava\"></div>".to_owned(), |url| format!("<img class=\"bava\" src=\"{}\" alt=\"\" loading=\"lazy\">", escape(url)));
        let _ = write!(
            out,
            "<div class=\"brow\"><div class=\"brank\">{medal}</div>{avatar}<div class=\"bmain\"><b>{}</b>\
             <div class=\"bbar\"><span style=\"width:{percent}%\"></span></div>\
             <small>{} / {} XP</small></div><div class=\"blevel\"><small>SEVİYE</small><b>{}</b></div>\
             <div class=\"bxp\"><small>TOPLAM XP</small><b>{}</b></div></div>",
            escape(&row.name),
            progress.current,
            progress.needed,
            progress.level,
            row.xp
        );
    }
    out.push_str("</div>");
    out
}

/// Yöneticilerin panel içinden gördüğü sıralama.
pub async fn leaderboard(State(state): State<WebState>, headers: HeaderMap, Path(guild): Path<u64>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await {
        return forbidden();
    }
    let name = state.cache.guild(guild_id).map(|g| g.name.clone()).unwrap_or_default();
    let rows = match board(&state, guild_id).await {
        Ok(rows) => rows,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Panel: sıralama okunamadı");
            return forbidden();
        }
    };
    let body = format!(
        "<h1>🏆 Sıralama</h1><p class=\"sub\">En çok XP'si olan {BOARD_SIZE} üye · \
         <a href=\"/panel/sunucu/{guild_id}/eklenti/seviye\">Seviye ayarları</a></p>{}",
        board_html(&rows)
    );
    let nav = GuildNav { id: guild_id, name: &name, active: "siralama" };
    Html(layout(&state, &session, Nav::Home, Some(nav), Page { title: "Sıralama", body }).await).into_response()
}

/// Herkese açık sıralama: yalnızca sunucu ayarında açıldıysa görünür.
pub async fn public_leaderboard(State(state): State<WebState>, Path(guild): Path<u64>) -> Response {
    let not_found = || {
        (StatusCode::NOT_FOUND, Html(layout_public("Sıralama bulunamadı", "<div class=\"box\">Bu sunucunun sıralaması herkese açık değil.</div>")))
            .into_response()
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return not_found();
    };
    let Some((name, icon)) = state.cache.guild(guild_id).map(|g| (g.name.clone(), g.icon_url())) else {
        return not_found();
    };
    let open = state.settings.get::<LevelSettings>(&state.db, guild_id).await.is_ok_and(|s| s.enabled && s.public_leaderboard);
    if !open {
        return not_found();
    }
    let Ok(rows) = board(&state, guild_id).await else {
        return not_found();
    };
    let icon = icon.map_or_else(String::new, |url| format!("<img src=\"{}\" alt=\"\">", escape(&url)));
    let body = format!(
        "<div class=\"ghead\">{icon}<div><h1>{}</h1><div class=\"sub\" style=\"margin:0\">Seviye sıralaması · ilk {BOARD_SIZE} üye</div></div></div>{}",
        escape(&name),
        board_html(&rows)
    );
    Html(layout_public(&format!("{name} sıralaması"), &body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_escapes_names_and_shows_progress() {
        let rows = [Row { name: "<b>Ali</b>".to_owned(), avatar: None, xp: 150 }];
        let html = board_html(&rows);
        assert!(html.contains("&lt;b&gt;Ali&lt;/b&gt;"));
        assert!(html.contains("🥇"));
        assert!(!html.contains("<b>Ali</b>"));
        assert!(board_html(&[]).contains("Henüz kimse"));
    }

    #[test]
    fn rate_labels() {
        assert_eq!(rate_label(25), "x0.25");
        assert_eq!(rate_label(100), "x1");
        assert_eq!(rate_label(250), "x2.5");
    }
}
