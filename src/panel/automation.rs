//! Panel: Otomasyon ve Başarımlar eklentilerinin formları.

use std::fmt::Write as _;

use poise::serenity_prelude::{ChannelId, GuildId, RoleId};
use tracing::warn;

use super::plugins::{checkbox, checklist, field, section, textarea, Input, Options};
use crate::achievements::{self, AchievementSettings, Definition, Kind, DEFAULTS};
use crate::automations::{self, Automation, AutomationSettings, Trigger};
use crate::reactionroles::emoji_key;
use crate::web::{escape, WebState};

fn text_input(name: &str, placeholder: &str, max: usize) -> String {
    format!("<input type=\"text\" class=\"wide\" name=\"{name}\" maxlength=\"{max}\" placeholder=\"{}\">", escape(placeholder))
}

fn short(input: &Input<'_>, key: &str, max: usize) -> Result<Option<String>, &'static str> {
    let raw = input.raw(key);
    if raw.chars().count() > max {
        return Err("metin");
    }
    Ok((!raw.is_empty()).then(|| raw.to_owned()))
}

/// İsteğe bağlı emoji alanı: doluysa geçerli olmalı.
fn emoji(input: &Input<'_>, key: &str) -> Result<Option<String>, &'static str> {
    match short(input, key, 64)? {
        Some(raw) if emoji_key(&raw).is_none() => Err("emoji"),
        other => Ok(other),
    }
}

/// Kimliği `allowed` listesinde olan isteğe bağlı seçim.
fn pick(input: &Input<'_>, key: &str, allowed: &[(u64, String)], error: &'static str) -> Result<Option<u64>, &'static str> {
    input.id(key, allowed, error)
}

// ---------------------------------------------------------------- Otomasyon

/// Kuralın ne yaptığının kısa özeti (liste için).
fn summary(a: &Automation, options: &Options) -> String {
    let mut parts = Vec::new();
    if a.message.is_some() {
        parts.push("mesaj gönderir".to_owned());
    }
    if a.dm.is_some() {
        parts.push("özelden yazar".to_owned());
    }
    if !a.add_roles.is_empty() {
        parts.push(format!("{} rol verir", a.add_roles.len()));
    }
    if !a.remove_roles.is_empty() {
        parts.push(format!("{} rol alır", a.remove_roles.len()));
    }
    if a.react.is_some() {
        parts.push("tepki ekler".to_owned());
    }
    if a.thread_name.is_some() {
        parts.push("alt başlık açar".to_owned());
    }
    if a.delete_message {
        parts.push("mesajı siler".to_owned());
    }
    let mut conditions = Vec::new();
    if let Some(keyword) = &a.keyword {
        conditions.push(format!("\"{keyword}\" geçerse"));
    }
    if let Some(channel) = a.channel {
        let name = options.channels.iter().find(|(id, _)| *id == channel).map_or("silinmiş kanal", |(_, name)| name.as_str());
        conditions.push(format!("#{name} kanalında"));
    }
    if let Some(role) = a.role {
        let name = options.all_roles.iter().find(|(id, _)| *id == role).map_or("silinmiş rol", |(_, name)| name.as_str());
        conditions.push(format!("@{name} rolüyle"));
    }
    if let Some(emoji) = &a.emoji {
        conditions.push(format!("{emoji} tepkisiyle"));
    }
    if conditions.is_empty() {
        parts.join(", ")
    } else {
        format!("{} → {}", conditions.join(", "), parts.join(", "))
    }
}

pub async fn automation_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<AutomationSettings>(&state.db, guild_id).await?;
    let mut list = String::new();
    if settings.automations.is_empty() {
        list.push_str("<p class=\"note\">Henüz otomasyon yok.</p>");
    } else {
        list.push_str("<table><tr><th>Açık</th><th>Ad</th><th>Ne zaman</th><th>Ne yapar</th><th>Sil</th></tr>");
        for a in &settings.automations {
            let _ = write!(
                list,
                "<tr><td><input type=\"checkbox\" name=\"auto_on\" value=\"{id}\"{}></td><td>{}</td><td>{}</td><td>{}</td>\
                 <td><input type=\"checkbox\" name=\"auto_del\" value=\"{id}\"></td></tr>",
                if a.enabled { " checked" } else { "" },
                escape(&a.name),
                a.trigger.label(),
                escape(&summary(a, options)),
                id = a.id
            );
        }
        list.push_str("</table>");
    }
    let mut out = section("Otomasyonlar", list);

    let mut triggers = String::from("<select name=\"a_trigger\">");
    for (_, code, label) in Trigger::ALL {
        let _ = write!(triggers, "<option value=\"{code}\">{label}</option>");
    }
    triggers.push_str("</select>");
    out.push_str(&section(
        "Yeni otomasyon",
        field("Ad", &text_input("a_name", "Soru kanalında alt başlık aç", 60), "Boş bırakırsan yeni otomasyon oluşturulmaz.")
            + &field("Ne zaman çalışsın?", &triggers, "")
            + "<h4>Koşullar (boş bıraktığın aranmaz)</h4>"
            + &field("Mesaj şunu içersin", &text_input("a_keyword", "örn. nasıl", 100), "Sadece \"Mesaj gönderildiğinde\" için. Büyük/küçük harf fark etmez.")
            + &field("Sadece bu kanalda", &options.channel_select("a_channel", None), "")
            + &field(
                "Rol",
                &Options::select("a_role", &options.all_roles, "@ ", None, "— Fark etmez —"),
                "Mesaj ve tepkide: üyede bu rol olmalı. Rol verildiğinde/alındığında: verilen ya da alınan rol bu olmalı.",
            )
            + &field("Tepki emojisi", "<input type=\"text\" name=\"a_emoji\" maxlength=\"64\" placeholder=\"⭐\">", "Sadece \"Mesaja tepki eklendiğinde\" için.")
            + "<h4>Eylemler (en az biri)</h4>"
            + &field(
                "Mesaj gönder",
                &textarea("a_message", None, "Hoş geldin {kullanıcı}!"),
                "Değişkenler: <code>{kullanıcı}</code>, <code>{isim}</code>, <code>{sunucu}</code>, <code>{kanal}</code>, <code>{rol}</code>, <code>{mesaj}</code>.",
            )
            + &field(
                "Mesajın gideceği kanal",
                &Options::select("a_message_channel", &options.channels, "# ", None, "— Olayın olduğu kanal —"),
                "Üye girişi/çıkışı ve rol olaylarında bir kanal seçmelisin.",
            )
            + &checkbox("a_ping", false, "Mesajdaki rol etiketleri bildirim göndersin")
            + &field("Üyeye özelden yaz", &textarea("a_dm", None, ""), "")
            + "<h4>Verilecek roller</h4>"
            + &checklist("a_add_role", &options.roles, "@ ", &[])
            + "<h4>Alınacak roller</h4>"
            + &checklist("a_remove_role", &options.roles, "@ ", &[])
            + "<h4>Tetikleyen mesaja (sadece mesaj tetikleyicisinde)</h4>"
            + &field("Tepki ekle", "<input type=\"text\" name=\"a_react\" maxlength=\"64\" placeholder=\"👀\">", "")
            + &field("Alt başlık aç (başlık adı)", &text_input("a_thread", "{isim} sorusu", 100), "")
            + &checkbox("a_delete", false, "Mesajı sil")
            + "<p class=\"note\">Örnekler: <b>Karşılayıcı</b> — \"Üye sunucuya katıldığında\" + mesaj. <b>Alt başlık</b> — \"Mesaj gönderildiğinde\" + kanal + \
               \"Alt başlık aç\". <b>Rol duyurusu</b> — \"Üyeye rol verildiğinde\" + rol + mesaj.</p>",
    ));
    Ok((out, settings.automations.iter().any(|a| a.enabled)))
}

/// Formdaki yeni otomasyon (ad boşsa `None`).
fn parse_automation(input: &Input<'_>, id: u32) -> Result<Option<Automation>, &'static str> {
    let Some(name) = short(input, "a_name", 60)? else {
        return Ok(None);
    };
    let trigger = Trigger::from_code(input.raw("a_trigger")).ok_or("hata")?;
    let automation = Automation {
        id,
        name,
        enabled: true,
        trigger,
        keyword: short(input, "a_keyword", 100)?,
        channel: input.channel("a_channel")?.map(ChannelId::get),
        role: pick(input, "a_role", &input.options.all_roles, "rol")?,
        emoji: emoji(input, "a_emoji")?,
        message: input.text("a_message")?,
        message_channel: pick(input, "a_message_channel", &input.options.channels, "kanal")?,
        dm: input.text("a_dm")?,
        add_roles: input.ids("a_add_role", &input.options.roles, "rol")?,
        remove_roles: input.ids("a_remove_role", &input.options.roles, "rol")?,
        react: emoji(input, "a_react")?,
        delete_message: input.checked("a_delete"),
        thread_name: short(input, "a_thread", 100)?,
        ping_roles: input.checked("a_ping"),
    };
    let has_action = automation.message.is_some()
        || automation.dm.is_some()
        || !automation.add_roles.is_empty()
        || !automation.remove_roles.is_empty()
        || automation.react.is_some()
        || automation.delete_message
        || automation.thread_name.is_some();
    if !has_action {
        return Err("bos");
    }
    // Kanalsız olaylarda (giriş, çıkış, rol) mesajın gideceği kanal belli olmalı.
    let channelless = matches!(trigger, Trigger::MemberJoin | Trigger::MemberLeave | Trigger::RoleAdded | Trigger::RoleRemoved);
    if channelless && automation.message.is_some() && automation.message_channel.is_none() {
        return Err("kanal");
    }
    Ok(Some(automation))
}

pub async fn automation_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = |e: anyhow::Error| {
        warn!(guild = %guild_id, error = ?e, "Panel: otomasyon kaydedilemedi");
        "hata"
    };
    let numbers = |key: &str| -> Result<Vec<u32>, &'static str> { input.all(key).into_iter().map(|raw| raw.parse().map_err(|_| "hata")).collect() };
    let (removed, on) = (numbers("auto_del")?, numbers("auto_on")?);
    let current = state.settings.get::<AutomationSettings>(&state.db, guild_id).await.map_err(failed)?;
    let mut settings = (*current).clone();
    let new = parse_automation(input, settings.next_id())?;
    settings.automations.retain(|a| !removed.contains(&a.id));
    for automation in &mut settings.automations {
        automation.enabled = on.contains(&automation.id);
    }
    if let Some(automation) = new {
        if settings.automations.len() >= automations::MAX_AUTOMATIONS {
            return Err("limit");
        }
        settings.automations.push(automation);
    }
    state.settings.set(&state.db, guild_id, settings).await.map_err(failed)
}

// ---------------------------------------------------------------- Başarımlar

fn describe(definition_kind: Kind, threshold: u64) -> String {
    format!("{threshold} {}", definition_kind.label())
}

pub async fn achievement_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<AchievementSettings>(&state.db, guild_id).await?;
    let mut out = section(
        "Genel",
        checkbox("enabled", settings.enabled, "Başarımlar açık")
            + &checkbox("announce", settings.announce, "Kazanılan başarımı kanalda duyur")
            + &field(
                "Duyuru kanalı",
                &Options::select("channel", &options.channels, "# ", settings.channel, "— Başarımın kazanıldığı kanal —"),
                "Kanal seçilmezse seste kazanılan başarımlar duyurulmaz (bir kanalda kazanılmadıkları için).",
            )
            + "<p class=\"note\">Üyeler ilerlemelerini Discord'da <code>/başarımlar</code> ile görür. Sayaçlar başarımlar açıkken işler.</p>",
    );

    let mut defaults = String::from("<div class=\"checks\" style=\"max-height:none\">");
    for (key, name, kind, threshold) in DEFAULTS {
        let _ = write!(
            defaults,
            "<label class=\"check\"><input type=\"checkbox\" name=\"default_on\" value=\"{key}\"{}> <span><b>{}</b> — {}</span></label>",
            if settings.disabled_defaults.iter().any(|d| d == key) { "" } else { " checked" },
            escape(name),
            describe(*kind, *threshold)
        );
    }
    defaults.push_str("</div>");
    out.push_str(&section("Hazır başarımlar", defaults));

    let mut custom = String::new();
    if settings.custom.is_empty() {
        custom.push_str("<p class=\"note\">Henüz özel başarım yok.</p>");
    } else {
        custom.push_str("<table><tr><th>Ad</th><th>Koşul</th><th>Ödül</th><th>Sil</th></tr>");
        for definition in &settings.custom {
            let mut rewards = Vec::new();
            if definition.reward_xp > 0 {
                rewards.push(format!("{} XP", definition.reward_xp));
            }
            if let Some(role) = definition.reward_role {
                let name = options.all_roles.iter().find(|(id, _)| *id == role).map_or("silinmiş rol", |(_, name)| name.as_str());
                rewards.push(format!("@{name}"));
            }
            let _ = write!(
                custom,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"custom_del\" value=\"{}\"></td></tr>",
                escape(&definition.name),
                describe(definition.kind, definition.threshold),
                escape(&if rewards.is_empty() { "—".to_owned() } else { rewards.join(", ") }),
                escape(&definition.key)
            );
        }
        custom.push_str("</table>");
    }
    let mut kinds = String::from("<select name=\"c_kind\">");
    for (_, code, label) in Kind::ALL {
        let _ = write!(kinds, "<option value=\"{code}\">{label}</option>");
    }
    kinds.push_str("</select>");
    out.push_str(&section(
        "Özel başarımlar",
        custom
            + "<div style=\"height:14px\"></div>"
            + &field("Yeni başarım: ad", &text_input("c_name", "Gece Kuşu", 50), "Boş bırakırsan yeni başarım oluşturulmaz.")
            + &field("Sayılan şey", &kinds, "")
            + &field("Eşik", "<input type=\"number\" name=\"c_threshold\" min=\"1\" max=\"10000000\" placeholder=\"örn. 500\">", "")
            + &field("Ödül rolü", &options.role_select("c_role", None), "İsteğe bağlı.")
            + &field(
                "Ödül XP",
                &format!("<input type=\"number\" name=\"c_xp\" value=\"0\" min=\"0\" max=\"{}\">", achievements::MAX_REWARD_XP),
                "",
            ),
    ));
    Ok((out, settings.enabled))
}

/// Formdaki yeni özel başarım (ad boşsa `None`).
fn parse_achievement(input: &Input<'_>, key: String) -> Result<Option<Definition>, &'static str> {
    let Some(name) = short(input, "c_name", 50)? else {
        return Ok(None);
    };
    let kind = Kind::from_code(input.raw("c_kind")).ok_or("hata")?;
    let threshold: u64 = input.raw("c_threshold").parse().ok().filter(|t| (1..=10_000_000).contains(t)).ok_or("sayi")?;
    Ok(Some(Definition {
        key,
        name,
        kind,
        threshold,
        reward_role: input.role("c_role")?.map(RoleId::get),
        reward_xp: input.number("c_xp", 0, achievements::MAX_REWARD_XP)?,
    }))
}

/// Daha önce kullanılmamış özel başarım kimliği (`c1`, `c2`…).
fn next_key(settings: &AchievementSettings) -> String {
    let highest = settings.custom.iter().filter_map(|d| d.key.strip_prefix('c')?.parse::<u32>().ok()).max().unwrap_or(0);
    format!("c{}", highest + 1)
}

pub async fn achievement_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = |e: anyhow::Error| {
        warn!(guild = %guild_id, error = ?e, "Panel: başarım ayarı kaydedilemedi");
        "hata"
    };
    let on = input.all("default_on");
    if on.iter().any(|key| !DEFAULTS.iter().any(|(known, ..)| known == key)) {
        return Err("hata");
    }
    let removed = input.all("custom_del");
    let current = state.settings.get::<AchievementSettings>(&state.db, guild_id).await.map_err(failed)?;
    let mut settings = (*current).clone();
    let new = parse_achievement(input, next_key(&settings))?;

    settings.enabled = input.checked("enabled");
    settings.announce = input.checked("announce");
    settings.channel = pick(input, "channel", &input.options.channels, "kanal")?;
    settings.disabled_defaults = DEFAULTS.iter().map(|(key, ..)| *key).filter(|key| !on.contains(key)).map(str::to_owned).collect();
    settings.custom.retain(|d| !removed.contains(&d.key.as_str()));
    if let Some(definition) = new {
        if settings.custom.len() >= achievements::MAX_CUSTOM {
            return Err("limit");
        }
        settings.custom.push(definition);
    }
    state.settings.set(&state.db, guild_id, settings).await.map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options {
            guild_name: "T".to_owned(),
            channels: vec![(10, "genel".to_owned())],
            categories: Vec::new(),
            roles: vec![(20, "Üye".to_owned())],
            all_roles: vec![(20, "Üye".to_owned()), (30, "Yönetici".to_owned())],
        }
    }

    fn form(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn automation_validation() {
        let options = options();
        let parse = |pairs: &[(&str, &str)]| {
            let form = form(pairs);
            parse_automation(&Input { form: &form, options: &options }, 4).map(|a| a.map(|a| (a.id, a.trigger, a.message_channel, a.add_roles)))
        };
        assert_eq!(parse(&[("a_trigger", "message")]), Ok(None));
        assert_eq!(
            parse(&[("a_name", "x"), ("a_trigger", "member_join"), ("a_message", "selam"), ("a_message_channel", "10"), ("a_add_role", "20")]),
            Ok(Some((4, Trigger::MemberJoin, Some(10), vec![20])))
        );
        // Eylemsiz kural, kanalsız olayda kanalsız mesaj, yetkili rol ve bozuk emoji reddedilir.
        assert_eq!(parse(&[("a_name", "x"), ("a_trigger", "message"), ("a_keyword", "a")]), Err("bos"));
        assert_eq!(parse(&[("a_name", "x"), ("a_trigger", "member_join"), ("a_message", "selam")]), Err("kanal"));
        assert_eq!(parse(&[("a_name", "x"), ("a_trigger", "message"), ("a_add_role", "30")]), Err("rol"));
        assert_eq!(parse(&[("a_name", "x"), ("a_trigger", "message"), ("a_react", "yazı")]), Err("emoji"));
        assert_eq!(parse(&[("a_name", "x"), ("a_trigger", "bilinmeyen"), ("a_delete", "1")]), Err("hata"));
        assert!(parse(&[("a_name", "x"), ("a_trigger", "message"), ("a_react", "👀")]).is_ok_and(|a| a.is_some()));
    }

    #[test]
    fn achievement_validation_and_keys() {
        let options = options();
        let parse = |pairs: &[(&str, &str)]| {
            let form = form(pairs);
            parse_achievement(&Input { form: &form, options: &options }, "c9".to_owned()).map(|d| d.map(|d| (d.key, d.kind, d.threshold, d.reward_role, d.reward_xp)))
        };
        assert_eq!(parse(&[("c_kind", "voice")]), Ok(None));
        assert_eq!(
            parse(&[("c_name", "Gece Kuşu"), ("c_kind", "voice"), ("c_threshold", "300"), ("c_role", "20"), ("c_xp", "50")]),
            Ok(Some(("c9".to_owned(), Kind::Voice, 300, Some(20), 50)))
        );
        assert_eq!(parse(&[("c_name", "x"), ("c_kind", "voice"), ("c_threshold", "0"), ("c_xp", "0")]), Err("sayi"));
        assert_eq!(parse(&[("c_name", "x"), ("c_kind", "uyku"), ("c_threshold", "5"), ("c_xp", "0")]), Err("hata"));
        assert_eq!(parse(&[("c_name", "x"), ("c_kind", "voice"), ("c_threshold", "5"), ("c_role", "30"), ("c_xp", "0")]), Err("rol"));

        let mut settings = AchievementSettings::default();
        assert_eq!(next_key(&settings), "c1");
        settings.custom.push(Definition { key: "c7".to_owned(), name: "a".to_owned(), kind: Kind::Voice, threshold: 1, reward_role: None, reward_xp: 0 });
        assert_eq!(next_key(&settings), "c8");
    }
}
