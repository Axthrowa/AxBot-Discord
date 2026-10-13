//! Ek starboard'lar ve müzik sayfaları; rol menüsü ile otomasyon sayfalarındaki hazır şablonlar.

use std::fmt::Write as _;

use poise::serenity_prelude::GuildId;
use tracing::warn;

use super::plugins::{checkbox, field, section, Input, Options};
use crate::music::MusicSettings;
use crate::starboards::{self, Board, StarboardSettings};
use crate::web::{escape, WebState};

fn failed(guild_id: GuildId, what: &'static str) -> impl Fn(anyhow::Error) -> &'static str {
    move |e| {
        warn!(guild = %guild_id, error = ?e, "Panel: {what} kaydedilemedi");
        "hata"
    }
}

fn channel_name(options: &Options, id: u64) -> String {
    options.channels.iter().find(|(known, _)| *known == id).map_or_else(|| "(silinmiş kanal)".to_owned(), |(_, name)| format!("# {name}"))
}

// ---------------------------------------------------------------- Ek starboard'lar

pub async fn starboard_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<StarboardSettings>(&state.db, guild_id).await?;
    let mut list = String::new();
    if settings.boards.is_empty() {
        list.push_str("<p class=\"note\">Henüz ek pano yok.</p>");
    } else {
        list.push_str("<table><tr><th>Emoji</th><th>Pano kanalı</th><th>Gereken tepki</th><th>Sil</th></tr>");
        for (i, board) in settings.boards.iter().enumerate() {
            let _ = write!(
                list,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"board_del\" value=\"{i}\"></td></tr>",
                escape(&board.emoji),
                escape(&channel_name(options, board.channel)),
                board.threshold
            );
        }
        list.push_str("</table>");
    }
    let out = section("Ek panolar", list)
        + &section(
            "Yeni pano",
            field("Emoji", "<input type=\"text\" name=\"board_emoji\" maxlength=\"16\" placeholder=\"😂\">", "Standart bir emoji. Boş bırakırsan yeni pano eklenmez. ⭐ asıl Starboard'a ayrılmıştır.")
                + &field("Pano kanalı", &options.channel_select("board_channel", None), "")
                + &field("Gereken tepki sayısı", "<input type=\"number\" name=\"board_threshold\" value=\"3\" min=\"1\" max=\"50\">", "")
                + "<p class=\"note\">Bir mesaj seçtiğin emojiden yeterince alınca bu kanala kopyalanır; sayı düşerse kopya silinir. Mesaj sahibinin ve botların \
                   tepkisi sayılmaz. Asıl ⭐ panosu “Starboard” sayfasından ayarlanır; onunla birlikte en fazla 10 pano olabilir.</p>",
        );
    Ok((out, !settings.boards.is_empty()))
}

pub async fn starboard_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "ek starboard");
    let removed = input.all("board_del").into_iter().map(|raw| raw.parse::<usize>().map_err(|_| "hata")).collect::<Result<Vec<_>, _>>()?;
    let addition = if input.raw("board_emoji").is_empty() {
        None
    } else {
        if !starboards::valid_emoji(input.raw("board_emoji")) {
            return Err("emoji");
        }
        let channel = input.channel("board_channel")?.ok_or("kanal")?;
        Some(Board { channel: channel.get(), emoji: starboards::normalize_emoji(input.raw("board_emoji")), threshold: input.number("board_threshold", 1, 50)? })
    };
    let current = state.settings.get::<StarboardSettings>(&state.db, guild_id).await.map_err(&failed)?;
    if removed.iter().any(|i| *i >= current.boards.len()) {
        return Err("hata");
    }
    let mut boards: Vec<Board> = current.boards.iter().enumerate().filter(|(i, _)| !removed.contains(i)).map(|(_, b)| b.clone()).collect();
    if let Some(board) = addition {
        if boards.iter().any(|b| b.emoji == board.emoji && b.channel == board.channel) {
            return Err("tekrar");
        }
        if boards.len() >= starboards::MAX_EXTRA {
            return Err("limit");
        }
        boards.push(board);
    }
    state.settings.set(&state.db, guild_id, StarboardSettings { boards }).await.map_err(&failed)
}

// ---------------------------------------------------------------- Müzik

pub async fn music_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.music.settings(guild_id);
    let extras = state.settings.get::<crate::music::playlists::MusicExtras>(&state.db, guild_id).await?;
    let now = match state.music.card_state(guild_id).await {
        Some((track, card)) => format!(
            "<p>▶️ <b>{}</b>{}<br><span class=\"note\">Sırada {} şarkı · ses %{:.0}{}</span></p>{}",
            escape(&track.title),
            if card.paused { " (duraklatıldı)" } else { "" },
            card.upcoming,
            card.volume * 100.0,
            card.next.as_deref().map(|next| format!(" · sıradaki: {}", escape(next))).unwrap_or_default(),
            checkbox("stop", false, "Müziği durdur, sırayı temizle ve ses kanalından ayrıl"),
        ),
        None => "<p class=\"note\">Şu an müzik çalmıyor. Bir ses kanalına girip Discord'da <code>/çal</code> yaz.</p>".to_owned(),
    };
    let out = section("Şu an", now)
        + &section(
            "Ayarlar",
            checkbox("always_on", settings.always_on, "7/24 modu: kanal boşalsa da sıra bitse de ses kanalından ayrılma")
                + &checkbox("autoplay", settings.autoplay, "Otomatik çalma: sıra bitince benzer şarkılarla devam et")
                + &field(
                    "DJ rolü",
                    &Options::select("dj_role", &options.all_roles, "@ ", extras.dj_role, "— Herkes yönetebilir —"),
                    "Seçersen geç / durdur / ses gibi komutları yalnızca bu roldekiler, yöneticiler ve kanalda tek başına olan üye kullanır. Şarkı eklemek herkese açık kalır.",
                )
                + "<p class=\"note\">Komutlar: <code>/çal · geç · durdur · duraklat · devam · sıra · ses · döngü · karıştır · ayrıl</code> ve \
                   <code>/müzik radyo · sözler · sar · sil · taşı · liste-kaydet · liste-çal · listelerim · liste-sil</code>. Müzik yarışması: <code>/oyun müzik-yarışması</code>. \
                   Şarkı kartındaki butonlarla da yönetilir.</p>",
        );
    Ok((out, settings.always_on || settings.autoplay || extras.dj_role.is_some()))
}

pub async fn music_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let dj_role = input.id("dj_role", &input.options.all_roles, "rol")?;
    let settings = MusicSettings { always_on: input.checked("always_on"), autoplay: input.checked("autoplay") };
    state.settings.set(&state.db, guild_id, crate::music::playlists::MusicExtras { dj_role }).await.map_err(failed(guild_id, "DJ rolü"))?;
    state.music.save_settings(&state.db, guild_id, settings).await.map_err(failed(guild_id, "müzik ayarı"))?;
    if input.checked("stop") {
        state.music.stop_and_leave(guild_id).await;
    }
    Ok(())
}

// ---------------------------------------------------------------- Hazır şablonlar

type Template = (&'static str, &'static [(&'static str, &'static str)]);

/// Şablon düğmeleri: tıklanınca sayfadaki "yeni …" formunu doldurur (`panel.js`); hiçbir şey kaydetmez.
fn templates(intro: &str, items: &[Template]) -> String {
    let mut buttons = String::new();
    for (label, fields) in items {
        let map: serde_json::Map<String, serde_json::Value> = fields.iter().map(|(k, v)| ((*k).to_owned(), serde_json::Value::from(*v))).collect();
        let json = serde_json::Value::Object(map).to_string();
        let _ = write!(buttons, "<button type=\"button\" class=\"btn ghost tpl\" data-doldur=\"{}\">{}</button> ", escape(&json), escape(label));
    }
    section("Hazır şablonlar", format!("<p class=\"note\" style=\"margin:0 0 10px\">{intro}</p><div class=\"tpls\">{buttons}</div>"))
}

const AUTOMATION_TEMPLATES: &[Template] = &[
    ("❓ Sorulara alt başlık aç", &[("a_name", "Sorulara alt başlık"), ("a_trigger", "message"), ("a_keyword", "?"), ("a_thread", "{isim} sorusu")]),
    ("👋 Karşılayıcı", &[("a_name", "Karşılayıcı"), ("a_trigger", "member_join"), ("a_message", "Aramıza hoş geldin {kullanıcı}! 🎉 Kurallara göz atmayı unutma.")]),
    ("🚪 Veda mesajı", &[("a_name", "Veda"), ("a_trigger", "member_leave"), ("a_message", "{isim} aramızdan ayrıldı. Yolu açık olsun! 👋")]),
    ("👍 Mesajlara otomatik tepki", &[("a_name", "Otomatik tepki"), ("a_trigger", "message"), ("a_react", "👍")]),
    ("🏅 Rol alana tebrik", &[("a_name", "Rol tebriği"), ("a_trigger", "role_added"), ("a_dm", "Tebrikler {isim}! {sunucu} sunucusunda yeni bir rol kazandın. 🎉")]),
    ("⭐ Tepkiye teşekkür", &[("a_name", "Tepki teşekkürü"), ("a_trigger", "reaction"), ("a_emoji", "⭐"), ("a_message", "{kullanıcı} bir mesajı yıldızladı! ⭐")]),
];

const ROLE_MENU_TEMPLATES: &[Template] = &[
    (
        "🔔 Bildirimler",
        &[
            ("title", "Bildirim rolleri"),
            ("description", "Hangi bildirimleri almak istiyorsan onu seç; istediğin zaman bırakabilirsin."),
            ("mode", "buttons"),
            ("unique", "0"),
            ("pair_emoji_0", "📢"),
            ("pair_label_0", "Duyurular"),
            ("pair_emoji_1", "🎉"),
            ("pair_label_1", "Etkinlikler"),
            ("pair_emoji_2", "🎁"),
            ("pair_label_2", "Çekilişler"),
        ],
    ),
    (
        "🎨 Renkler",
        &[
            ("title", "Renk rolleri"),
            ("description", "İsminin rengini seç. Aynı anda tek renk taşıyabilirsin."),
            ("mode", "dropdown"),
            ("unique", "1"),
            ("pair_emoji_0", "🔴"),
            ("pair_label_0", "Kırmızı"),
            ("pair_emoji_1", "🟠"),
            ("pair_label_1", "Turuncu"),
            ("pair_emoji_2", "🟡"),
            ("pair_label_2", "Sarı"),
            ("pair_emoji_3", "🟢"),
            ("pair_label_3", "Yeşil"),
            ("pair_emoji_4", "🔵"),
            ("pair_label_4", "Mavi"),
            ("pair_emoji_5", "🟣"),
            ("pair_label_5", "Mor"),
        ],
    ),
    (
        "✅ Doğrula",
        &[
            ("title", "Kuralları kabul et"),
            ("description", "Kuralları okuduysan aşağıdaki butona basarak sunucuya erişim kazan."),
            ("mode", "buttons"),
            ("unique", "0"),
            ("pair_emoji_0", "✅"),
            ("pair_label_0", "Kuralları okudum, kabul ediyorum"),
        ],
    ),
    (
        "🎮 Oyunlar",
        &[
            ("title", "Oyun rolleri"),
            ("description", "Oynadığın oyunları seç; o oyunun duyurularında etiketlenirsin."),
            ("mode", "reactions"),
            ("unique", "0"),
            ("pair_emoji_0", "🔫"),
            ("pair_label_0", "Valorant"),
            ("pair_emoji_1", "⛏️"),
            ("pair_label_1", "Minecraft"),
            ("pair_emoji_2", "⚽"),
            ("pair_label_2", "FC / FIFA"),
            ("pair_emoji_3", "🏆"),
            ("pair_label_3", "League of Legends"),
        ],
    ),
];

pub fn automation_templates() -> String {
    templates(
        "Bir şablona tıkla: aşağıdaki “yeni otomasyon” formu dolar. Kanal ve rol seçimlerini kendin yapıp Kaydet'e bas; tıklamak hiçbir şeyi kaydetmez.",
        AUTOMATION_TEMPLATES,
    )
}

pub fn role_menu_templates() -> String {
    templates(
        "Bir şablona tıkla: aşağıdaki “yeni rol menüsü” formu dolar. Kanalı ve her satırın rolünü kendin seçip Kaydet'e bas; tıklamak hiçbir şeyi göndermez.",
        ROLE_MENU_TEMPLATES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_use_known_fields_and_codes() {
        for (label, fields) in AUTOMATION_TEMPLATES {
            let get = |key: &str| fields.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
            assert!(get("a_name").is_some(), "{label}");
            assert!(get("a_trigger").and_then(crate::automations::Trigger::from_code).is_some(), "{label}");
            assert!(fields.iter().all(|(k, _)| k.starts_with("a_")), "{label}");
        }
        for (label, fields) in ROLE_MENU_TEMPLATES {
            let get = |key: &str| fields.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
            assert!(get("mode").and_then(crate::reactionroles::Mode::from_code).is_some(), "{label}");
            let pairs = fields.iter().filter(|(k, _)| k.starts_with("pair_emoji_")).count();
            assert!((1..=crate::reactionroles::MAX_PAIRS).contains(&pairs), "{label}");
        }
    }

    #[test]
    fn template_json_is_escaped_for_html() {
        let html = automation_templates();
        assert!(html.contains("data-doldur=\"{&quot;a_"), "{html}");
        assert!(!html.contains("data-doldur=\"{\""));
    }
}
