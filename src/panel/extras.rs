//! Panel: liste biçimindeki eklentiler — rol menüleri, özel komutlar, tekrarlı mesajlar ve gömülü
//! mesajlar. Her sayfa aynı düzendedir: mevcut kayıtlar (silme kutusuyla) + yeni kayıt formu;
//! tek "Kaydet" hem silmeleri hem yeni kaydı uygular.

use std::fmt::Write as _;

use poise::serenity_prelude::{self as serenity, ChannelId, CreateMessage, EditMessage, GuildId, MessageId};
use tracing::warn;

use super::plugins::{checkbox, checklist, field, section, textarea, Input, Options};
use crate::customcmd::{self, Command, CustomCommandSettings};
use crate::embeds::{self, EmbedSettings, SavedEmbed};
use crate::reaction_roles::{self, Menu, Mode, Pair, ReactionRoleSettings};
use crate::scheduler::recurring::{self, Timer, TimerSettings};
use crate::web::{escape, WebState};

fn text_input(name: &str, value: &str, placeholder: &str, max: usize) -> String {
    format!(
        "<input type=\"text\" class=\"wide\" name=\"{name}\" value=\"{}\" maxlength=\"{max}\" placeholder=\"{}\">",
        escape(value),
        escape(placeholder)
    )
}

fn big_textarea(name: &str, value: &str, max: usize, rows: u8, placeholder: &str) -> String {
    format!(
        "<textarea name=\"{name}\" rows=\"{rows}\" maxlength=\"{max}\" placeholder=\"{}\">{}</textarea>",
        escape(placeholder),
        escape(value)
    )
}

fn channel_name(options: &Options, id: u64) -> String {
    options
        .channels
        .iter()
        .find(|(known, _)| *known == id)
        .map_or_else(|| "(silinmiş kanal)".to_owned(), |(_, name)| format!("# {name}"))
}

fn preview(text: &str, max: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= max {
        flat
    } else {
        flat.chars().take(max).collect::<String>() + "…"
    }
}

/// Formdaki kısa metin: boşsa `None`, uzunsa hata.
fn short(input: &Input<'_>, key: &str, max: usize) -> Result<Option<String>, &'static str> {
    let raw = input.raw(key);
    if raw.chars().count() > max {
        return Err("metin");
    }
    Ok((!raw.is_empty()).then(|| raw.to_owned()))
}

fn failed(guild_id: GuildId, what: &'static str) -> impl Fn(anyhow::Error) -> &'static str {
    move |e| {
        warn!(guild = %guild_id, error = ?e, "Panel: {what} kaydedilemedi");
        "hata"
    }
}

/// Mesajı siler; zaten silinmişse sorun değildir.
async fn delete_message(state: &WebState, channel: u64, message: u64) {
    if channel == 0 || message == 0 {
        return;
    }
    if let Err(e) = ChannelId::new(channel).delete_message(&state.http, MessageId::new(message)).await {
        warn!(channel, message, error = %e, "Panel: mesaj silinemedi (zaten silinmiş olabilir)");
    }
}

fn ids_to_delete(input: &Input<'_>, key: &str) -> Result<Vec<u64>, &'static str> {
    input.all(key).into_iter().map(|raw| raw.parse().map_err(|_| "hata")).collect()
}

// ---------------------------------------------------------------- Rol menüleri

pub async fn role_menu_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<ReactionRoleSettings>(&state.db, guild_id).await?;
    let mut list = String::new();
    if settings.menus.is_empty() {
        list.push_str("<p class=\"note\">Henüz rol menüsü yok.</p>");
    } else {
        list.push_str("<table><tr><th>Başlık</th><th>Kanal</th><th>Tür</th><th>Rol</th><th>Sil</th></tr>");
        for menu in &settings.menus {
            let behavior = if menu.behavior == reaction_roles::Behavior::Normal { String::new() } else { format!(" · {}", menu.behavior.label()) };
            let _ = write!(
                list,
                "<tr><td><a href=\"https://discord.com/channels/{guild_id}/{}/{}\" target=\"_blank\" rel=\"noopener\">{}</a></td>\
                 <td>{}</td><td>{}{}{}</td><td>{}</td><td><input type=\"checkbox\" name=\"menu_del\" value=\"{}\"></td></tr>",
                menu.channel_id,
                menu.message_id,
                escape(&menu.title),
                escape(&channel_name(options, menu.channel_id)),
                menu.mode.label(),
                if menu.unique { " · tek seçim" } else { "" },
                behavior,
                menu.pairs.len(),
                menu.message_id
            );
        }
        list.push_str("</table><p class=\"note\">Silinen menünün Discord'daki mesajı da silinir.</p>");
    }
    let mut out = section("Rol menüleri", list);

    let mut modes = String::from("<select name=\"mode\">");
    for (_, code, label) in Mode::ALL {
        let _ = write!(modes, "<option value=\"{code}\">{label}</option>");
    }
    modes.push_str("</select>");
    let mut behaviors = String::from("<select name=\"behavior\">");
    for (_, code, label) in reaction_roles::Behavior::ALL {
        let _ = write!(behaviors, "<option value=\"{code}\">{label}</option>");
    }
    behaviors.push_str("</select>");
    let mut rows = String::from("<table><tr><th style=\"width:150px\">Emoji</th><th>Rol</th><th>Etiket (buton / menü yazısı)</th></tr>");
    for i in 0..reaction_roles::MAX_PAIRS {
        let _ = write!(
            rows,
            "<tr><td><input type=\"text\" name=\"pair_emoji_{i}\" maxlength=\"64\" placeholder=\"🎮\"></td><td>{}</td>\
             <td><input type=\"text\" class=\"wide\" name=\"pair_label_{i}\" maxlength=\"80\" placeholder=\"Boşsa rolün adı\"></td></tr>",
            options.role_select(&format!("pair_role_{i}"), None)
        );
    }
    rows.push_str("</table>");
    out.push_str(&section(
        "Yeni rol menüsü",
        field("Kanal", &options.channel_select("channel", None), "Menü mesajı bu kanala gönderilir. Boş bırakırsan yeni menü oluşturulmaz.")
            + &field("Başlık", &text_input("title", "", "Rollerini seç", 100), "")
            + &field("Açıklama", &big_textarea("description", "", 500, 3, "İsteğe bağlı"), "")
            + &field("Tür", &modes, "Emoji tepkileri: üyeler mesajın altındaki emojilere tıklar. Butonlar ve açılır menüde emoji isteğe bağlıdır.")
            + &field("Davranış", &behaviors, "Doğrulama: rol bırakılamaz. Ters: tepki verince rol alınır. Bağlayıcı: menüden bir kez tek rol seçilir. Geçici: rol aşağıdaki süre sonunda alınır.")
            + &field("Geçici rol süresi (dakika)", "<input type=\"number\" name=\"temp_minutes\" value=\"60\" min=\"1\" max=\"43200\">", "Yalnızca Geçici davranışta kullanılır (en fazla 30 gün).")
            + &checkbox("unique", false, "Tek seçim: üye bu menüden aynı anda yalnızca bir rol taşıyabilir (renk rolleri gibi)")
            + "<div style=\"height:10px\"></div>"
            + &rows
            + "<p class=\"note\">Sunucu emojisi için <code>&lt;:ad:kimlik&gt;</code> biçimini kullan (Discord'da emojinin önüne \\ yazıp gönderince çıkar). \
               Listede sadece botun verebileceği, yönetim yetkisi içermeyen roller görünür.</p>",
    ));
    Ok((out, !settings.menus.is_empty()))
}

/// Formdaki yeni menüyü doğrular (kanal seçilmemişse `None`).
fn parse_menu(input: &Input<'_>) -> Result<Option<Menu>, &'static str> {
    let Some(channel) = input.channel("channel")? else {
        return Ok(None);
    };
    let title = short(input, "title", 100)?.ok_or("bos")?;
    let mode = Mode::from_code(input.raw("mode")).ok_or("hata")?;
    // Eski formlar davranış göndermez: normal sayılır.
    let behavior = match input.raw("behavior") {
        "" => reaction_roles::Behavior::Normal,
        code => reaction_roles::Behavior::from_code(code).ok_or("hata")?,
    };
    if behavior == reaction_roles::Behavior::Reverse && mode != Mode::Reactions {
        return Err("ters");
    }
    let temp_minutes = if behavior == reaction_roles::Behavior::Temporary { input.number("temp_minutes", 1, 43_200)? } else { 0 };
    let mut pairs: Vec<Pair> = Vec::new();
    let mut seen_emojis = Vec::new();
    for i in 0..reaction_roles::MAX_PAIRS {
        let Some(role) = input.role(&format!("pair_role_{i}"))? else {
            continue;
        };
        if pairs.iter().any(|p| p.role == role.get()) {
            return Err("tekrar");
        }
        let emoji = short(input, &format!("pair_emoji_{i}"), 64)?;
        match emoji.as_deref().map(reaction_roles::emoji_key) {
            Some(None) => return Err("emoji"),
            Some(Some(key)) => {
                if seen_emojis.contains(&key) {
                    return Err("tekrar");
                }
                seen_emojis.push(key);
            }
            None if mode == Mode::Reactions => return Err("emoji"),
            None => {}
        }
        let role_name = input.options.roles.iter().find(|(id, _)| *id == role.get()).map(|(_, name)| name.clone()).unwrap_or_default();
        let label = short(input, &format!("pair_label_{i}"), 80)?.unwrap_or(role_name);
        pairs.push(Pair { emoji, role: role.get(), label });
    }
    if pairs.is_empty() {
        return Err("bos");
    }
    Ok(Some(Menu {
        message_id: 0,
        channel_id: channel.get(),
        mode,
        title,
        description: short(input, "description", 500)?,
        unique: input.checked("unique"),
        pairs,
        behavior,
        temp_minutes,
    }))
}

pub async fn role_menu_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "rol menüsü");
    let removed = ids_to_delete(input, "menu_del")?;
    let new_menu = parse_menu(input)?;
    let current = state.settings.get::<ReactionRoleSettings>(&state.db, guild_id).await.map_err(&failed)?;
    let mut settings = (*current).clone();

    let (gone, kept): (Vec<Menu>, Vec<Menu>) = settings.menus.into_iter().partition(|m| removed.contains(&m.message_id));
    settings.menus = kept;
    if new_menu.is_some() && settings.menus.len() >= reaction_roles::MAX_MENUS {
        return Err("limit");
    }
    if let Some(mut menu) = new_menu {
        reaction_roles::publish(&state.http, &mut menu).await.map_err(|e| {
            warn!(guild = %guild_id, error = ?e, "Panel: rol menüsü gönderilemedi");
            "gonder"
        })?;
        settings.menus.push(menu);
    }
    state.settings.set(&state.db, guild_id, settings).await.map_err(&failed)?;
    for menu in gone {
        delete_message(state, menu.channel_id, menu.message_id).await;
    }
    Ok(())
}

// ---------------------------------------------------------------- Özel komutlar

pub async fn command_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<CustomCommandSettings>(&state.db, guild_id).await?;
    let mut list = String::new();
    if settings.commands.is_empty() {
        list.push_str("<p class=\"note\">Henüz özel komut yok.</p>");
    } else {
        list.push_str("<table><tr><th>Komut</th><th>Cevap</th><th>Özellikler</th><th>Sil</th></tr>");
        for command in &settings.commands {
            let mut traits = Vec::new();
            for (on, label) in [
                (command.embed, "gömülü"),
                (command.dm, "özelden"),
                (command.delete_trigger, "mesajı siler"),
                (!command.add_roles.is_empty(), "rol verir"),
                (!command.remove_roles.is_empty(), "rol alır"),
                (!command.required_roles.is_empty(), "role özel"),
                (!command.channels.is_empty(), "kanala özel"),
            ] {
                if on {
                    traits.push(label.to_owned());
                }
            }
            if command.cooldown_secs > 0 {
                traits.push(format!("{} sn bekleme", command.cooldown_secs));
            }
            let trigger = if command.pattern.is_empty() { format!("!{}", command.name) } else { format!("{} → /{}/", command.name, command.pattern) };
            let _ = write!(
                list,
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"cmd_del\" value=\"{}\"></td></tr>",
                escape(&trigger),
                escape(&preview(&command.response, 80)),
                escape(&traits.join(", ")),
                escape(&command.name)
            );
        }
        list.push_str("</table>");
    }
    let mut out = section("Özel komutlar", list);
    out.push_str(&section(
        "Yeni komut (aynı adla kaydedersen eskisinin yerine geçer)",
        field("Komut adı", &text_input("cmd_name", "", "kurallar", 32), "Sohbette <code>!kurallar</code> yazınca çalışır. Harf, rakam, <code>-</code> ve <code>_</code>; boşluk olmaz.")
            + &field(
                "Cevap",
                &big_textarea("cmd_response", "", customcmd::MAX_RESPONSE, 5, "Komut yazılınca gönderilecek metin"),
                "Değişkenler: <code>{kullanıcı}</code> (etiket), <code>{isim}</code>, <code>{kimlik}</code>, <code>{sunucu}</code>, <code>{kanal}</code>, \
                 <code>{üye_sayısı}</code>, <code>{argümanlar}</code> (komuttan sonra yazılanlar), <code>{1}</code>…<code>{9}</code> (tek tek kelimeler ya da \
                 regex grupları), <code>{rastgele:a|b|c}</code> (biri rastgele seçilir).",
            )
            + &field(
                "Düzenli ifade (regex, isteğe bağlı)",
                &text_input("cmd_pattern", "", "^merhaba (\\w+)", customcmd::MAX_PATTERN),
                "Doluysa komut <code>!ad</code> ile değil, bu kalıba uyan HER mesajda çalışır (büyük/küçük harf önemsiz). Parantezle yakalanan \
                 parçalar <code>{1}</code>, <code>{2}</code>… olur. Sık eşleşen kalıba bekleme süresi ver.",
            )
            + &checkbox("cmd_embed", false, "Cevabı gömülü mesaj olarak gönder")
            + &checkbox("cmd_dm", false, "Cevabı kanala değil, komutu yazana özelden gönder")
            + &checkbox("cmd_delete", false, "Komutu yazan mesajı sil")
            + &field(
                "Bekleme süresi (saniye)",
                &format!("<input type=\"number\" name=\"cmd_cooldown\" value=\"0\" min=\"0\" max=\"{}\">", customcmd::MAX_COOLDOWN_SECS),
                "Aynı kanalda komut bu süre dolmadan tekrar çalışmaz. 0 = sınırsız.",
            )
            + "<h4>Verilecek roller</h4>"
            + &checklist("cmd_add_role", &options.roles, "@ ", &[])
            + "<h4>Alınacak roller</h4>"
            + &checklist("cmd_remove_role", &options.roles, "@ ", &[])
            + "<h4>Sadece bu roller kullanabilsin (boşsa herkes)</h4>"
            + &checklist("cmd_required_role", &options.all_roles, "@ ", &[])
            + "<h4>Sadece bu kanallarda çalışsın (boşsa her yerde)</h4>"
            + &checklist("cmd_channel", &options.channels, "# ", &[]),
    ));
    Ok((out, !settings.commands.is_empty()))
}

fn parse_command(input: &Input<'_>) -> Result<Option<Command>, &'static str> {
    let raw_name = input.raw("cmd_name");
    if raw_name.is_empty() {
        // Cevap ya da regex yazılıp ad unutulduysa sessizce kaydetmemek yerine uyarılır.
        let filled = !input.raw("cmd_response").is_empty() || !input.raw("cmd_pattern").is_empty();
        return if filled { Err("ad_bos") } else { Ok(None) };
    }
    let name = crate::tags::normalize_name(raw_name.trim_start_matches(crate::tags::PREFIX)).map_err(|_| "ad")?;
    let response = short(input, "cmd_response", customcmd::MAX_RESPONSE)?.ok_or("bos")?;
    Ok(Some(Command {
        name,
        response,
        embed: input.checked("cmd_embed"),
        dm: input.checked("cmd_dm"),
        delete_trigger: input.checked("cmd_delete"),
        cooldown_secs: input.number("cmd_cooldown", 0, customcmd::MAX_COOLDOWN_SECS)?,
        add_roles: input.ids("cmd_add_role", &input.options.roles, "rol")?,
        remove_roles: input.ids("cmd_remove_role", &input.options.roles, "rol")?,
        required_roles: input.ids("cmd_required_role", &input.options.all_roles, "rol")?,
        channels: input.ids("cmd_channel", &input.options.channels, "kanal")?,
        pattern: match input.raw("cmd_pattern").trim() {
            "" => String::new(),
            p => customcmd::compile(p).map(|_| p.to_owned()).ok_or("regex")?,
        },
    }))
}

pub async fn command_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "özel komut");
    let removed = input.all("cmd_del");
    let new_command = parse_command(input)?;
    let current = state.settings.get::<CustomCommandSettings>(&state.db, guild_id).await.map_err(&failed)?;
    let mut settings = (*current).clone();
    settings.commands.retain(|c| !removed.contains(&c.name.as_str()));
    if let Some(command) = new_command {
        settings.commands.retain(|c| c.name != command.name);
        if settings.commands.len() >= customcmd::MAX_COMMANDS {
            return Err("limit");
        }
        settings.commands.push(command);
        settings.commands.sort_by(|a, b| a.name.cmp(&b.name));
    }
    state.settings.set(&state.db, guild_id, settings).await.map_err(&failed)
}

// ---------------------------------------------------------------- Tekrarlı mesajlar

fn interval_label(mins: u32) -> String {
    if mins.is_multiple_of(60) {
        format!("{} saatte bir", mins / 60)
    } else {
        format!("{mins} dakikada bir")
    }
}

pub async fn timer_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<TimerSettings>(&state.db, guild_id).await?;
    let mut list = String::new();
    if settings.timers.is_empty() {
        list.push_str("<p class=\"note\">Henüz tekrarlı mesaj yok.</p>");
    } else {
        list.push_str("<table><tr><th>Kanal</th><th>Sıklık</th><th>Mesaj</th><th>Sil</th></tr>");
        for timer in &settings.timers {
            let _ = write!(
                list,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"timer_del\" value=\"{}\"></td></tr>",
                escape(&channel_name(options, timer.channel)),
                interval_label(timer.interval_mins),
                if timer.purge { format!("🧹 Temizle{}", if timer.message.is_empty() { String::new() } else { format!(" + {}", escape(&preview(&timer.message, 80))) }) } else { escape(&preview(&timer.message, 90)) },
                timer.id
            );
        }
        list.push_str("</table>");
    }
    let mut out = section("Tekrarlı mesajlar", list);
    out.push_str(&section(
        "Yeni tekrarlı mesaj",
        field("Kanal", &options.channel_select("timer_channel", None), "Boş bırakırsan yeni mesaj oluşturulmaz.")
            + &field("Mesaj", &textarea("timer_message", None, "Kuralları okumayı unutmayın!"), "Tekrarlı mesajlar kimseyi etiketlemez.")
            + &field(
                "Sıklık",
                &format!(
                    "<input type=\"number\" name=\"timer_every\" value=\"1\" min=\"1\" max=\"{}\"> \
                     <select name=\"timer_unit\" style=\"max-width:160px\"><option value=\"hours\">saat</option><option value=\"minutes\">dakika</option></select>",
                    recurring::MAX_INTERVAL_MINS
                ),
                "En az 5 dakika, en fazla 7 gün. İlk mesaj bu süre dolunca gönderilir.",
            )
            + &checkbox(
                "timer_purge",
                false,
                "Otomatik temizleme: zamanı gelince kanaldaki mesajları sil (sabitlenenler kalır, 14 günden eskileri Discord silmez); mesaj yazdıysan ardından gönderilir",
            ),
    ));
    Ok((out, !settings.timers.is_empty()))
}

fn parse_timer(input: &Input<'_>, id: u32, now: i64) -> Result<Option<Timer>, &'static str> {
    let Some(channel) = input.channel("timer_channel")? else {
        return Ok(None);
    };
    let purge = input.checked("timer_purge");
    // Temizleme zamanlayıcısında mesaj isteğe bağlıdır (temizlikten sonra gönderilir).
    let message = match input.text("timer_message")? {
        Some(message) => message,
        None if purge => String::new(),
        None => return Err("bos"),
    };
    let every = input.number("timer_every", 1, recurring::MAX_INTERVAL_MINS)?;
    let interval_mins = match input.raw("timer_unit") {
        "minutes" => every,
        "hours" => every.checked_mul(60).ok_or("sayi")?,
        _ => return Err("hata"),
    };
    if !(recurring::MIN_INTERVAL_MINS..=recurring::MAX_INTERVAL_MINS).contains(&interval_mins) {
        return Err("sayi");
    }
    Ok(Some(Timer { id, channel: channel.get(), message, interval_mins, next_unix: now + i64::from(interval_mins) * 60, purge }))
}

pub async fn timer_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "tekrarlı mesaj");
    let removed: Vec<u32> = input.all("timer_del").into_iter().map(|raw| raw.parse().map_err(|_| "hata")).collect::<Result<_, _>>()?;
    let current = state.settings.get::<TimerSettings>(&state.db, guild_id).await.map_err(&failed)?;
    let mut settings = (*current).clone();
    let new_timer = parse_timer(input, settings.next_id(), serenity::Timestamp::now().unix_timestamp())?;
    settings.timers.retain(|t| !removed.contains(&t.id));
    if let Some(timer) = new_timer {
        if settings.timers.len() >= recurring::MAX_TIMERS {
            return Err("limit");
        }
        settings.timers.push(timer);
    }
    let res = state.settings.set(&state.db, guild_id, settings.clone()).await.map_err(&failed);
    if res.is_ok() {
        if settings.timers.is_empty() {
            recurring::mark_inactive(guild_id);
        } else {
            recurring::mark_active(guild_id);
        }
    }
    res
}

// ---------------------------------------------------------------- Gömülü mesajlar

pub async fn embed_fields(state: &WebState, guild_id: GuildId, options: &Options, edit: Option<u64>) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<EmbedSettings>(&state.db, guild_id).await?;
    let mut list = String::new();
    if settings.embeds.is_empty() {
        list.push_str("<p class=\"note\">Henüz gömülü mesaj yok.</p>");
    } else {
        list.push_str("<table><tr><th>Başlık</th><th>Kanal</th><th></th><th>Sil</th></tr>");
        for embed in &settings.embeds {
            let title = if embed.title.is_empty() { preview(&embed.description, 60) } else { embed.title.clone() };
            let _ = write!(
                list,
                "<tr><td><a href=\"https://discord.com/channels/{guild_id}/{}/{}\" target=\"_blank\" rel=\"noopener\">{}</a></td><td>{}</td>\
                 <td><a href=\"/panel/sunucu/{guild_id}/eklenti/gomulu-mesaj?duzenle={}\">Düzenle</a></td>\
                 <td><input type=\"checkbox\" name=\"embed_del\" value=\"{}\"></td></tr>",
                embed.channel_id,
                embed.message_id,
                escape(&title),
                escape(&channel_name(options, embed.channel_id)),
                embed.message_id,
                embed.message_id
            );
        }
        list.push_str("</table><p class=\"note\">Silinen kaydın Discord'daki mesajı da silinir.</p>");
    }
    let mut out = section("Gömülü mesajlar", list);

    let editing = edit.and_then(|id| settings.embeds.iter().find(|e| e.message_id == id));
    let blank = SavedEmbed { colour: "#5865F2".to_owned(), ..SavedEmbed::default() };
    let shown = editing.unwrap_or(&blank);
    let colour = crate::cards::parse_hex(&shown.colour).map_or_else(|| "#5865f2".to_owned(), |[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}"));
    let target = match editing {
        Some(embed) => format!(
            "<input type=\"hidden\" name=\"edit_id\" value=\"{}\"><p class=\"note\">Düzenlenen mesaj: {} · \
             <a href=\"/panel/sunucu/{guild_id}/eklenti/gomulu-mesaj\">vazgeç, yeni mesaj yaz</a></p>",
            embed.message_id,
            escape(&channel_name(options, embed.channel_id))
        ),
        None => field("Kanal", &options.channel_select("embed_channel", None), "Boş bırakırsan yeni mesaj gönderilmez."),
    };

    let mut fields_html = String::from("<h4>Ek Alanlar (Fields)</h4><div style=\"display:grid;gap:8px;margin-bottom:12px;\">");
    for i in 0..3 {
        let f_name = shown.fields.get(i).map(|f| f.name.as_str()).unwrap_or_default();
        let f_val = shown.fields.get(i).map(|f| f.value.as_str()).unwrap_or_default();
        let f_inline = shown.fields.get(i).is_some_and(|f| f.inline);
        let _ = write!(
            fields_html,
            "<div style=\"display:flex;gap:8px;align-items:center;background:rgba(255,255,255,0.03);padding:8px;border-radius:4px;\">\
             <input type=\"text\" name=\"embed_fn_{i}\" value=\"{}\" placeholder=\"Alan Başlığı {}\" style=\"flex:1;\">\
             <input type=\"text\" name=\"embed_fv_{i}\" value=\"{}\" placeholder=\"Alan Değeri {}\" style=\"flex:2;\">\
             <label style=\"display:flex;align-items:center;gap:4px;font-size:0.85em;cursor:pointer;\"><input type=\"checkbox\" name=\"embed_fi_{i}\"{}> Yan yana</label>\
             </div>",
            escape(f_name), i + 1,
            escape(f_val), i + 1,
            if f_inline { " checked" } else { "" }
        );
    }
    fields_html.push_str("</div>");

    let preview_html = format!(
        "<div class=\"embed-preview-wrapper\" style=\"margin-top:20px;\">\
         <h4>Canlı Önizleme (Discord Görünümü)</h4>\
         <div id=\"embed-preview\" style=\"border-left: 4px solid {colour}; background: #2b2d31; padding: 14px 16px; border-radius: 4px; max-width: 520px; font-family: sans-serif;\">\
           <div id=\"prev-author\" style=\"font-size:0.85em; font-weight:600; color:#dbdee1; margin-bottom:4px; display:{author_display};\">{}</div>\
           <div id=\"prev-title\" style=\"font-weight:700; font-size:1.05em; color:#fff; margin-bottom:6px;\">{}</div>\
           <div id=\"prev-desc\" style=\"font-size:0.9em; color:#dbdee1; line-height:1.4; white-space:pre-wrap;\">{}</div>\
           <div id=\"prev-footer\" style=\"font-size:0.75em; color:#949ba4; margin-top:8px; display:{footer_display};\">{}</div>\
         </div>\
         </div>",
        escape(shown.author.as_deref().unwrap_or_default()),
        escape(if shown.title.is_empty() { "Gömülü Mesaj Başlığı" } else { &shown.title }),
        escape(if shown.description.is_empty() { "Mesaj açıklaması burada görünecektir." } else { &shown.description }),
        escape(shown.footer.as_deref().unwrap_or_default()),
        author_display = if shown.author.as_deref().unwrap_or_default().is_empty() { "none" } else { "block" },
        footer_display = if shown.footer.as_deref().unwrap_or_default().is_empty() { "none" } else { "block" }
    );

    out.push_str(&section(
        if editing.is_some() { "Mesajı düzenle" } else { "Yeni gömülü mesaj" },
        target
            + &field("Yazar (Author)", &text_input("embed_author", shown.author.as_deref().unwrap_or_default(), "Yazar adı veya başlığı", 256), "")
            + &field("Başlık (Title)", &text_input("embed_title", &shown.title, "Sunucu kuralları", 256), "")
            + &field("Başlık Bağlantısı (URL)", &text_input("embed_url", shown.url.as_deref().unwrap_or_default(), "https://...", 300), "")
            + &field("Metin (Description)", &big_textarea("embed_description", &shown.description, 4000, 8, "Discord biçimlendirmesi (**kalın**, _eğik_, bağlantılar) kullanılabilir"), "")
            + &field("Renk", &format!("<input type=\"color\" name=\"embed_colour\" value=\"{colour}\">"), "")
            + &field("Büyük resim (https bağlantısı)", &text_input("embed_image", shown.image.as_deref().unwrap_or_default(), "https://…", 300), "")
            + &field("Küçük resim (https bağlantısı)", &text_input("embed_thumbnail", shown.thumbnail.as_deref().unwrap_or_default(), "https://…", 300), "")
            + &fields_html
            + &field("Alt yazı (Footer)", &text_input("embed_footer", shown.footer.as_deref().unwrap_or_default(), "", 200), "")
            + &checkbox("embed_timestamp", shown.timestamp, "Şu anki zaman damgasını ekle (Timestamp)")
            + &preview_html,
    ));
    Ok((out, !settings.embeds.is_empty()))
}

fn https_url(input: &Input<'_>, key: &str) -> Result<Option<String>, &'static str> {
    let url = short(input, key, 300)?;
    match url {
        // Resmi Discord indirir, bot değil; yine de yalnızca düzgün https adresleri kabul edilir.
        Some(url) if !url.starts_with("https://") || url.contains(char::is_whitespace) => Err("adres"),
        other => Ok(other),
    }
}

/// Formdaki içerik (kanal ve mesaj kimliği hariç). Başlık da metin de boşsa `None`.
fn parse_embed(input: &Input<'_>) -> Result<Option<SavedEmbed>, &'static str> {
    let title = short(input, "embed_title", 256)?.unwrap_or_default();
    let description = short(input, "embed_description", 4000)?.unwrap_or_default();
    if title.is_empty() && description.is_empty() {
        return Ok(None);
    }
    let [r, g, b] = crate::cards::parse_hex(input.raw("embed_colour")).ok_or("renk")?;
    let author = short(input, "embed_author", 256)?;
    let url = https_url(input, "embed_url")?;
    let timestamp = input.checked("embed_timestamp");

    let mut fields = Vec::new();
    for i in 0..5 {
        let name = short(input, &format!("embed_fn_{i}"), 256)?.unwrap_or_default();
        let value = short(input, &format!("embed_fv_{i}"), 1024)?.unwrap_or_default();
        let inline = input.checked(&format!("embed_fi_{i}"));
        if !name.is_empty() && !value.is_empty() {
            fields.push(crate::embeds::EmbedField { name, value, inline });
        }
    }

    Ok(Some(SavedEmbed {
        message_id: 0,
        channel_id: 0,
        title,
        description,
        colour: format!("#{r:02X}{g:02X}{b:02X}"),
        image: https_url(input, "embed_image")?,
        thumbnail: https_url(input, "embed_thumbnail")?,
        footer: short(input, "embed_footer", 200)?,
        author,
        url,
        timestamp,
        fields,
    }))
}

pub async fn embed_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "gömülü mesaj");
    let removed = ids_to_delete(input, "embed_del")?;
    let content = parse_embed(input)?;
    let edit_id: Option<u64> = match input.raw("edit_id") {
        "" => None,
        raw => Some(raw.parse().map_err(|_| "hata")?),
    };
    let channel = input.channel("embed_channel")?;
    let current = state.settings.get::<EmbedSettings>(&state.db, guild_id).await.map_err(&failed)?;
    let mut settings = (*current).clone();

    let (gone, kept): (Vec<SavedEmbed>, Vec<SavedEmbed>) = settings.embeds.into_iter().partition(|e| removed.contains(&e.message_id));
    settings.embeds = kept;
    let send_failed = |e: serenity::Error| {
        warn!(guild = %guild_id, error = %e, "Panel: gömülü mesaj gönderilemedi");
        "gonder"
    };
    match (edit_id, content) {
        (Some(id), Some(mut content)) => {
            // Yalnızca bu sunucunun kayıtlı mesajları düzenlenebilir.
            let Some(saved) = settings.embeds.iter_mut().find(|e| e.message_id == id) else {
                return Err("hata");
            };
            content.message_id = saved.message_id;
            content.channel_id = saved.channel_id;
            ChannelId::new(saved.channel_id.max(1))
                .edit_message(&state.http, MessageId::new(saved.message_id.max(1)), EditMessage::new().embed(content.build()))
                .await
                .map_err(send_failed)?;
            *saved = content;
        }
        (Some(_), None) => return Err("bos"),
        (None, Some(mut content)) => {
            let Some(channel) = channel else {
                return Err("kanal");
            };
            if settings.embeds.len() >= embeds::MAX_EMBEDS {
                return Err("limit");
            }
            let sent = channel.send_message(&state.http, CreateMessage::new().embed(content.build())).await.map_err(send_failed)?;
            content.message_id = sent.id.get();
            content.channel_id = channel.get();
            settings.embeds.push(content);
        }
        (None, None) => {}
    }
    state.settings.set(&state.db, guild_id, settings).await.map_err(&failed)?;
    for embed in gone {
        delete_message(state, embed.channel_id, embed.message_id).await;
    }
    Ok(())
}

// ---------------------------------------------------------------- Doğum günü

pub async fn birthday_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    use crate::birthdays::{BirthdaySettings, DEFAULT_AGE_MESSAGE, DEFAULT_MESSAGE};
    let cfg = state.configs.get(&state.db, guild_id).await?;
    let settings = state.settings.get::<BirthdaySettings>(&state.db, guild_id).await?;
    let mut hours = String::from("<select name=\"hour\" style=\"max-width:160px\">");
    for hour in 0..24u8 {
        let _ = write!(hours, "<option value=\"{hour}\"{}>{hour:02}:00</option>", if settings.hour == hour { " selected" } else { "" });
    }
    hours.push_str("</select>");
    let out = section(
        "Doğum günü kutlaması",
        field("Kutlama kanalı", &options.channel_select("channel", cfg.birthday_channel), "")
            + &field("Doğum günü rolü", &options.role_select("role", cfg.birthday_role), "O gün boyunca verilir; isteğe bağlı.")
            + &field("Kutlama saati (Türkiye saati)", &hours, "Mesaj bu saatten sonraki ilk kontrolde gönderilir (kontrol 15 dakikada bir).")
            + &field(
                "Kutlama mesajı",
                &big_textarea("message", settings.message.as_deref().unwrap_or_default(), 1500, 3, DEFAULT_MESSAGE),
                "Değişken: <code>{kullanıcı}</code> (bugün doğanların etiketleri). Boşsa hazır mesaj kullanılır.",
            )
            + &checkbox("allow_year", settings.allow_year, "Üyeler doğum yılını da kaydedebilsin (yaşla kutlama)")
            + &field(
                "Yaşla kutlama mesajı",
                &big_textarea("age_message", settings.age_message.as_deref().unwrap_or_default(), 1500, 2, DEFAULT_AGE_MESSAGE),
                "Yılını yazmış üyeler için. Değişkenler: <code>{kullanıcı}</code>, <code>{yaş}</code>.",
            )
            + "<p class=\"note\">Üyeler doğum günlerini Discord'da <code>/doğum-günü ayarla</code> ile kaydeder.</p>",
    );
    Ok((out, cfg.birthday_channel.is_some() || cfg.birthday_role.is_some()))
}

pub async fn birthday_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    use crate::birthdays::BirthdaySettings;
    let failed = failed(guild_id, "doğum günü ayarı");
    let (channel, role) = (input.channel("channel")?, input.role("role")?);
    let settings = BirthdaySettings {
        hour: u8::try_from(input.number("hour", 0, 23)?).map_err(|_| "sayi")?,
        message: short(input, "message", 1500)?,
        age_message: short(input, "age_message", 1500)?,
        allow_year: input.checked("allow_year"),
    };
    state.configs.set(&state.db, guild_id, crate::guild_config::Setting::Birthday(channel, role)).await.map_err(&failed)?;
    state.settings.set(&state.db, guild_id, settings).await.map_err(&failed)
}

// ---------------------------------------------------------------- Biletler

/// Konu listesi: her satır bir konu; boşlar ve yinelenenler atılır.
fn parse_topics(raw: &str) -> Result<Vec<String>, &'static str> {
    let mut topics: Vec<String> = Vec::new();
    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.chars().count() > crate::tickets::MAX_TOPIC_LEN {
            return Err("metin");
        }
        if !topics.iter().any(|t| t == line) {
            topics.push(line.to_owned());
        }
    }
    if topics.len() > crate::tickets::MAX_TOPICS {
        return Err("limit");
    }
    Ok(topics)
}

pub async fn ticket_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    use crate::tickets::{self, TicketExtras};
    let settings = tickets::load_settings(&state.db, guild_id).await?;
    let extras = state.settings.get::<TicketExtras>(&state.db, guild_id).await?;
    let (category, role) = settings.as_ref().map_or((None, None), |s| (s.category_id.map(ChannelId::get), s.support_role_id.map(|r| r.get())));
    let rating_note = match tickets::rating_summary(&state.redis, guild_id).await {
        Some((average, count)) => format!("<p class=\"note\">Ortalama memnuniyet: <b>{average:.1} / 5</b> ⭐ ({count} puan)</p>"),
        None => String::new(),
    };

    let mut out = match tickets::stats(&state.db, guild_id, 30).await {
        Ok(stats) if stats.opened > 0 => {
            let time = |secs: Option<f64>| secs.map_or_else(|| "—".to_owned(), tickets::human_secs);
            let mut body = format!(
                "<p>Açılan: <b>{}</b> · Kapanan: <b>{}</b> · Ortalama ilk yetkili cevabı: <b>{}</b> · Ortalama çözüm süresi: <b>{}</b></p>",
                stats.opened,
                stats.closed,
                time(stats.first_response),
                time(stats.resolution)
            );
            if !stats.staff.is_empty() {
                body.push_str("<table><tr><th>Yetkili</th><th>İlk cevap</th><th>Kapattığı</th><th>Üstlendiği</th></tr>");
                for (who, first, closed, claimed) in &stats.staff {
                    let name = state
                        .cache
                        .guild(guild_id)
                        .and_then(|g| g.members.get(&serenity::UserId::new((*who).max(1))).map(|m| m.display_name().to_owned()))
                        .unwrap_or_else(|| who.to_string());
                    let _ = write!(body, "<tr><td>{}</td><td>{first}</td><td>{closed}</td><td>{claimed}</td></tr>", escape(&name));
                }
                body.push_str("</table>");
            }
            section("İstatistikler (son 30 gün)", body)
        }
        Ok(_) => String::new(),
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Bilet istatistikleri okunamadı");
            String::new()
        }
    };
    out.push_str(&section(
        "Genel",
        field("Biletlerin açılacağı kategori", &Options::select("category", &options.categories, "", category, "— Kategorisiz —"), "")
            + &field(
                "Destek ekibi rolü",
                &Options::select("support_role", &options.all_roles, "@ ", role, "— Yok (sadece yöneticiler görür) —"),
                "Bu roldekiler tüm biletleri görür, üstlenebilir, kapatabilir, yeniden açabilir ve silebilir.",
            )
            + &field(
                "Bir üyenin aynı anda açabileceği bilet sayısı",
                &format!("<input type=\"number\" name=\"max_open\" value=\"{}\" min=\"1\" max=\"5\">", extras.max_open.clamp(1, 5)),
                "",
            ),
    ));
    out.push_str(&section(
        "Bilet içeriği",
        field(
            "Konular (her satıra bir tane)",
            &big_textarea("topics", &extras.topics.join("\n"), 600, 5, "Genel destek\nŞikâyet\nOrtaklık"),
            "Konu yazarsan panelde buton yerine açılır menü çıkar; seçilen konu bilet kanalının adında ve ilk mesajında görünür. En fazla 10 konu.",
        ) + &field(
            "Bilet açılınca gösterilecek mesaj",
            &big_textarea("intro", extras.intro.as_deref().unwrap_or_default(), 1500, 4, "Boşsa hazır karşılama metni kullanılır"),
            "",
        ),
    ));
    out.push_str(&section(
        "Bilet asistanı (yapay zekâ)",
        field(
            "Sık sorulan sorular",
            &big_textarea("faq", extras.faq.as_deref().unwrap_or_default(), crate::ai::assist::MAX_FAQ_LEN, 7, "S: Sunucuya nasıl davet ederim?\nC: #davet kanalındaki bağlantıyı kullan.\n\nS: Rol nasıl alırım?\nC: #roller kanalındaki menüden seç."),
            "Doluysa bot, bileti açan üyenin İLK mesajına bu metinden cevap vermeyi dener; cevap burada yoksa “destek ekibi ilgilenecek” der. \
             Boşsa asistan kapalıdır. Yapay zekâ anahtarı tanımlı olmalı. En fazla 3000 karakter.",
        ),
    ));
    out.push_str(&section(
        "Kapatılan biletler",
        checkbox("keep_closed", extras.keep_closed, "Kapatılan bileti silme, sakla (destek ekibi yeniden açabilir ya da silebilir)")
            + &field(
                "Saklanan biletlerin kategorisi",
                &Options::select("closed_category", &options.categories, "", extras.closed_category, "— Olduğu yerde kalsın —"),
                "Konuşma kaydı her durumda log kanalına ve bileti açana gönderilir.",
            )
            + &field(
                "Hareketsiz bilet kaç saatte kapansın",
                &format!(
                    "<input type=\"number\" name=\"auto_close_hours\" value=\"{}\" min=\"0\" max=\"{}\">",
                    extras.auto_close_hours,
                    crate::tickets::MAX_AUTO_CLOSE_HOURS
                ),
                "Bu kadar saat kimse yazmazsa bilet kendiliğinden kapanır; bir saat önce kanalda uyarılır. 0 = kapalı.",
            )
            + &field(
                "SLA yanıt gecikmesi alarmı (dakika)",
                &format!(
                    "<input type=\"number\" name=\"sla_warn_minutes\" value=\"{}\" min=\"0\" max=\"1440\">",
                    extras.sla_warn_minutes
                ),
                "Bilet açıldıktan sonra bu kadar dakika içinde yetkili yanıt vermezse destek ekibi kanalda uyarılır. 0 = kapalı.",
            )
            + &checkbox("rating", extras.rating, "Kapatılınca bileti açana 1-5 yıldızlı memnuniyet sorusu gönder (puanlar bilet log kanalına düşer)")
            + &checkbox("html_transcript", extras.html_transcript, "Kapatıldığında Discord koyu temalı HTML konuşma kaydı oluştur ve web panelde sakla")
            + &rating_note,
    ));
    out.push_str(&section(
        "Panel mesajı",
        field(
            "Paneli bu kanala gönder",
            &options.channel_select("panel_channel", None),
            "Üyelerin bilet açtığı mesaj. Konuları değiştirdiysen paneli yeniden gönder (eski mesajı elle silebilirsin). Boş bırakırsan gönderilmez.",
        ) + "<p class=\"note\">Komutlar: <code>/bilet kur · üstlen · devret · ekle · çıkar · kapat · yeniden-aç · sil · paneller</code>.</p>",
    ));
    Ok((out, settings.is_some()))
}

pub async fn ticket_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    use crate::tickets::{self, TicketExtras, TicketSettings};
    let failed = failed(guild_id, "bilet ayarı");
    let category = input.id("category", &input.options.categories, "kanal")?;
    let support_role = input.id("support_role", &input.options.all_roles, "rol")?;
    let extras = TicketExtras {
        topics: parse_topics(input.raw("topics"))?,
        intro: short(input, "intro", 1500)?,
        max_open: u8::try_from(input.number("max_open", 1, 5)?).map_err(|_| "sayi")?,
        keep_closed: input.checked("keep_closed"),
        closed_category: input.id("closed_category", &input.options.categories, "kanal")?,
        faq: {
            let faq = input.raw("faq");
            if faq.chars().count() > crate::ai::assist::MAX_FAQ_LEN {
                return Err("metin");
            }
            (!faq.is_empty()).then(|| faq.to_owned())
        },
        auto_close_hours: if input.raw("auto_close_hours").is_empty() { 0 } else { input.number("auto_close_hours", 0, tickets::MAX_AUTO_CLOSE_HOURS)? },
        rating: input.checked("rating"),
        sla_warn_minutes: if input.raw("sla_warn_minutes").is_empty() { 0 } else { input.number("sla_warn_minutes", 0, 1440)? },
        html_transcript: input.checked("html_transcript"),
    };
    let panel_channel = input.channel("panel_channel")?;

    let settings = TicketSettings {
        category_id: category.map(ChannelId::new),
        support_role_id: support_role.map(serenity::RoleId::new),
    };
    tickets::save_settings(&state.db, guild_id, &settings).await.map_err(&failed)?;
    let message = tickets::panel_message(&extras);
    state.settings.set(&state.db, guild_id, extras).await.map_err(&failed)?;
    if let Some(channel) = panel_channel {
        let sent = channel.send_message(&state.http, message).await.map_err(|e| {
            warn!(guild = %guild_id, error = %e, "Panel: bilet paneli gönderilemedi");
            "gonder"
        })?;
        let panel = tickets::TicketPanel {
            id: 0,
            guild_id,
            channel_id: channel,
            message_id: Some(sent.id),
            title: "Destek Talebi".to_owned(),
            category_id: settings.category_id,
            support_role_id: settings.support_role_id,
        };
        let _ = tickets::save_panel(&state.db, &panel).await;
    }
    Ok(())
}

// ---------------------------------------------------------------- Sosyal akışlar

pub async fn feed_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    use crate::feeds::{self, FeedSettings, Kind};
    let settings = state.settings.get::<FeedSettings>(&state.db, guild_id).await?;
    let mut list = String::new();
    if settings.feeds.is_empty() {
        list.push_str("<p class=\"note\">Henüz takip edilen akış yok.</p>");
    } else {
        list.push_str("<table><tr><th>Tür</th><th>Kaynak</th><th>Kanal</th><th>Sil</th></tr>");
        for feed in &settings.feeds {
            let _ = write!(
                list,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"feed_del\" value=\"{}\"></td></tr>",
                feed.kind.label(),
                escape(&preview(&feed.name, 60)),
                escape(&channel_name(options, feed.channel)),
                feed.id
            );
        }
        list.push_str("</table>");
    }
    let mut out = section("Takip edilen akışlar", list);

    let mut kinds = String::from("<select name=\"feed_kind\">");
    for kind in Kind::ALL {
        let _ = write!(kinds, "<option value=\"{}\">{}</option>", kind.as_str(), kind.label());
    }
    kinds.push_str("</select>");
    out.push_str(&section(
        "Yeni akış",
        field("Tür", &kinds, "")
            + &field(
                "Kaynak",
                &text_input("feed_source", "", "turkey · ad.bsky.social · https://site.com/feed.xml", 400),
                "Reddit için subreddit adı, Bluesky için tam kullanıcı adı, RSS ve podcast için https akış adresi. Boş bırakırsan yeni akış eklenmez.",
            )
            + &field("Duyuru kanalı", &options.channel_select("feed_channel", None), "")
            + &field("Etiketlenecek rol", &Options::select("feed_role", &options.all_roles, "@ ", None, "— Yok —"), "")
            + &field(
                "Duyuru metni",
                &big_textarea("feed_message", "", feeds::MAX_MESSAGE_LEN, 3, "Boşsa hazır metin kullanılır"),
                "Değişkenler: <code>{başlık}</code>, <code>{bağlantı}</code>, <code>{kaynak}</code>.",
            )
            + "<p class=\"note\">Akışlar 5 dakikada bir kontrol edilir; eklediğin andaki eski paylaşımlar duyurulmaz. \
               X, Instagram ve TikTok herkese açık akış sunmaz; bunlar yalnızca bot sahibi <code>.env</code> dosyasına \
               <code>RSS_BRIDGE_URL</code> (RSSHub uyumlu köprü adresi) yazdıysa çalışır. \
               Twitch, Kick ve YouTube için <code>/yayın</code> ve <code>/youtube</code> komutlarını kullan.</p>",
    ));
    Ok((out, !settings.feeds.is_empty()))
}

pub async fn feed_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    use crate::feeds::{self, FeedSettings, Kind};
    let failed = failed(guild_id, "akış");
    let removed: Vec<u32> = input.all("feed_del").into_iter().map(|raw| raw.parse().map_err(|_| "hata")).collect::<Result<_, _>>()?;

    let source = input.raw("feed_source");
    let new_feed = if source.is_empty() {
        None
    } else {
        let kind = Kind::parse(input.raw("feed_kind")).ok_or("hata")?;
        let channel = input.channel("feed_channel")?.ok_or("kanal")?;
        let role = input.id("feed_role", &input.options.all_roles, "rol")?;
        let message = short(input, "feed_message", feeds::MAX_MESSAGE_LEN)?;
        // Biçim hataları ağa çıkmadan yakalanır.
        feeds::resolve_source(kind, source).map_err(|e| if e == feeds::BRIDGE_MISSING { "kopru" } else { "adres" })?;
        Some(feeds::prepare(kind, source, channel.get(), role, message, 0).await.map_err(|_| "akis")?)
    };

    let current = state.settings.get::<FeedSettings>(&state.db, guild_id).await.map_err(&failed)?;
    let mut settings = (*current).clone();
    settings.feeds.retain(|f| !removed.contains(&f.id));
    if let Some(feed) = new_feed {
        if settings.feeds.len() >= feeds::MAX_FEEDS {
            return Err("limit");
        }
        if settings.feeds.iter().any(|f| f.url == feed.url && f.channel == feed.channel) {
            return Err("tekrar");
        }
        let id = settings.next_id();
        settings.feeds.push(feeds::Feed { id, ..feed });
    }
    state.settings.set(&state.db, guild_id, settings).await.map_err(&failed)
}

// ---------------------------------------------------------------- Haberler

pub async fn news_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    use crate::news::{Delivery, NewsSettings, Style, CATEGORIES, DAYS, LANGUAGES, MAX_PER_HOUR};
    let settings = state.settings.get::<NewsSettings>(&state.db, guild_id).await?;
    let hour_select = |name: &str, selected: u8| {
        let mut out = format!("<select name=\"{name}\" style=\"max-width:140px\">");
        for hour in 0..24u8 {
            let _ = write!(out, "<option value=\"{hour}\"{}>{hour:02}:00</option>", if selected == hour { " selected" } else { "" });
        }
        out + "</select>"
    };
    let option = |value: &str, label: &str, on: bool| format!("<option value=\"{value}\"{}>{}</option>", if on { " selected" } else { "" }, escape(label));

    // Diller
    let mut sources = String::from("<div class=\"checks\">");
    for lang in LANGUAGES {
        let _ = write!(
            sources,
            "<label class=\"check\"><input type=\"checkbox\" name=\"source_lang\" value=\"{}\"{}> {} {}</label>",
            lang.code,
            if settings.source_languages.iter().any(|l| l == lang.code) { " checked" } else { "" },
            lang.flag,
            lang.name
        );
    }
    sources.push_str("</div>");
    let mut output = String::from("<select name=\"output_language\" style=\"max-width:280px\">");
    output.push_str(&option("", "Orijinal (haberin kendi dili)", settings.output_language.is_empty()));
    for lang in LANGUAGES {
        output.push_str(&option(lang.code, &format!("{} {}", lang.flag, lang.name), settings.output_language == lang.code));
    }
    output.push_str("</select>");

    // Kategoriler
    let mut categories = String::new();
    for category in CATEGORIES {
        let on = settings.categories.iter().any(|c| c == category.key);
        let own = settings.channels.get(category.key).copied().map(ChannelId::new);
        let own_role = settings.roles.get(category.key).copied().map(serenity::RoleId::new);
        let names: Vec<String> = category.sources.iter().map(|s| format!("{} ({})", s.name, s.lang)).collect();
        let _ = write!(
            categories,
            "<div class=\"box\" style=\"margin:0 0 10px\">{}{}{}<p class=\"note\" style=\"margin:6px 0 0\">Kaynaklar: {}</p></div>",
            checkbox(&format!("cat_{}", category.key), on, &format!("{} {}", category.emoji, category.name)),
            field("Bu kategoriye özel kanal", &options.channel_select(&format!("channel_{}", category.key), own), "Boşsa yukarıdaki haber kanalı kullanılır."),
            field("Bu kategoride etiketlenecek rol", &options.role_select(&format!("role_{}", category.key), own_role), "Boşsa genel rol (varsa)."),
            escape(&names.join(", ")),
        );
    }
    let mut days = String::from("<select name=\"weekly_day\" style=\"max-width:180px\">");
    for (i, day) in DAYS.iter().enumerate() {
        days.push_str(&option(&i.to_string(), day, usize::from(settings.weekly_day) == i));
    }
    days.push_str("</select>");
    let style = format!(
        "<select name=\"style\" style=\"max-width:280px\">{}{}</select>",
        option("full", "Tam — görsel ve özetle", settings.style == Style::Full),
        option("compact", "Kompakt — başlık ve küçük görsel", settings.style == Style::Compact)
    );
    let delivery = format!(
        "<select name=\"delivery\" style=\"max-width:280px\">{}{}</select>",
        option("instant", "Anlık — haber çıktıkça", settings.delivery == Delivery::Instant),
        option("daily", "Günlük — günde bir kez toplu liste", settings.delivery == Delivery::Daily)
    );

    let mut out = section(
        "Haber kanalı",
        checkbox("enabled", settings.enabled, "Haberler açık")
            + &field("Haber kanalı", &options.channel_select("channel", settings.channel.map(ChannelId::new)), "Yeni haberler buraya otomatik yazılır (10 dakikada bir bakılır).")
            + &field("Haberlerde etiketlenecek rol", &options.role_select("role", settings.role.map(serenity::RoleId::new)), "İsteğe bağlı; örneğin bir \"Haber bildirimi\" rolü.")
            + &field("Görünüm", &style, "")
            + &field("Gönderim", &delivery, "")
            + &field("Günlük listenin saati (Türkiye saati)", &hour_select("digest_hour", settings.digest_hour), "Yalnızca günlük gönderimde.")
            + &field(
                "Kategori başına saatte en fazla",
                &format!("<input type=\"number\" name=\"max_per_hour\" value=\"{}\" min=\"1\" max=\"{MAX_PER_HOUR}\">", settings.max_per_hour),
                "Anlık gönderimde kanal haberle dolmasın diye; fazlası atlanır.",
            ),
    );
    out.push_str(&section(
        "Dil",
        field("Hangi dillerdeki kaynaklar gelsin", &sources, "Hiçbiri işaretli değilse bütün dillerdeki kaynaklar gelir.")
            + &field(
                "Haberler hangi dilde yazılsın",
                &output,
                "Başka dildeki haberin başlığı ve özeti yapay zekâyla bu dile çevrilir. Yapay zekâ yanıt vermezse haber orijinal dilinde gelir.",
            )
            + &checkbox("ai_details", settings.ai_details, "Haberin ayrıntısını yapay zekâyla özetle (tam görünümde)"),
    ));
    out.push_str(&section(
        "Süzgeçler",
        field(
            "Yalnızca bu kelimeler geçen haberler",
            &big_textarea("include_keywords", &settings.include_keywords.join("\n"), 1300, 3, "örnek: iPhone\nPlayStation\nMarvel"),
            "Her satıra bir kelime. Boşsa bütün haberler gelir.",
        ) + &field(
            "Bu kelimeler geçen haberler gelmesin",
            &big_textarea("exclude_keywords", &settings.exclude_keywords.join("\n"), 1300, 3, "örnek: sızıntı\nindirim"),
            "Her satıra bir kelime. Başlık ve özetin içinde aranır; büyük/küçük harf fark etmez.",
        ) + &checkbox("dedupe", settings.dedupe, "Aynı olay birden çok siteden gelirse bir kez paylaş")
            + "<p class=\"note\">Başlığında yetişkin içerik kelimesi geçen haberler her zaman atlanır.</p>",
    ));
    out.push_str(&section(
        "Kategoriler",
        categories + "<p class=\"note\">Hiçbiri işaretli değilse bütün kategoriler gelir.</p>",
    ));
    out.push_str(&section(
        "Haftalık özet",
        checkbox("weekly", settings.weekly, "Her hafta, her kategorinin öne çıkan gelişmelerini özetle")
            + &field("Gün", &days, "")
            + &field("Saat (Türkiye saati)", &hour_select("weekly_hour", settings.weekly_hour), "Özet bu saatteki ilk kontrolde gönderilir.")
            + "<p class=\"note\">Discord'da <code>/akış haber-özeti</code> ile bu haftanın özeti istendiğinde de görülebilir.</p>",
    ));
    let subscribers = crate::newsprefs::subscriber_count(&state.db, guild_id).await.unwrap_or(0);
    out.push_str(&section(
        "Kişisel haberler",
        field(
            "Tercih panelini bu kanala gönder",
            &options.channel_select("personal_panel", None),
            "Üyeler paneldeki <b>⚙️ Tercihlerim</b> ile kendi kategorilerini seçer, isterse haberleri özelden (anlık ya da günlük özet) alır;              <b>📰 Haberlerim</b> son haberleri yalnızca seçtiği kategorilerden, yalnızca ona görünen bir mesajla gösterir. Boş bırakırsan gönderilmez.",
        ) + &format!(
            "<p class=\"note\">Şu an özelden haber alan üye: <b>{subscribers}</b>. Özelden anlık haberde kişi başına saatte en fazla 6 haber gider;              günlük özet yukarıdaki özet saatinde gönderilir. Özel mesajları kapalı olanın tercihi kendiliğinden kapanır.</p>"
        ),
    ));
    Ok((out, settings.is_active()))
}

/// Anahtar kelime listesi: satır ya da virgülle ayrılır; boşlar ve yinelenenler atılır.
fn keyword_list(raw: &str) -> Result<Vec<String>, &'static str> {
    use crate::news::{MAX_KEYWORDS, MAX_KEYWORD_LEN};
    let mut out: Vec<String> = Vec::new();
    for word in raw.split(['\n', ',']).map(str::trim).filter(|w| !w.is_empty()) {
        if word.chars().count() > MAX_KEYWORD_LEN {
            return Err("metin");
        }
        if !out.iter().any(|w| w.to_lowercase() == word.to_lowercase()) {
            out.push(word.to_owned());
        }
    }
    if out.len() > MAX_KEYWORDS {
        return Err("limit");
    }
    Ok(out)
}

pub async fn news_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    use crate::news::{Delivery, NewsSettings, Style, CATEGORIES, MAX_PER_HOUR};
    let failed = failed(guild_id, "haber ayarı");
    let mut categories = Vec::new();
    let mut channels = std::collections::BTreeMap::new();
    let mut roles = std::collections::BTreeMap::new();
    for category in CATEGORIES {
        if input.checked(&format!("cat_{}", category.key)) {
            categories.push(category.key.to_owned());
        }
        if let Some(channel) = input.channel(&format!("channel_{}", category.key))? {
            channels.insert(category.key.to_owned(), channel.get());
        }
        if let Some(role) = input.role(&format!("role_{}", category.key))? {
            roles.insert(category.key.to_owned(), role.get());
        }
    }
    let source_languages: Vec<String> = input.all("source_lang").into_iter().map(str::to_owned).collect();
    if source_languages.iter().any(|l| crate::news::language(l).is_none()) {
        return Err("hata");
    }
    let output_language = match input.raw("output_language") {
        "" => String::new(),
        code if crate::news::language(code).is_some() => code.to_owned(),
        _ => return Err("hata"),
    };
    let settings = NewsSettings {
        enabled: input.checked("enabled"),
        channel: input.channel("channel")?.map(ChannelId::get),
        categories,
        channels,
        source_languages,
        output_language,
        ai_details: input.checked("ai_details"),
        style: match input.raw("style") {
            "compact" => Style::Compact,
            "full" | "" => Style::Full,
            _ => return Err("hata"),
        },
        delivery: match input.raw("delivery") {
            "daily" => Delivery::Daily,
            "instant" | "" => Delivery::Instant,
            _ => return Err("hata"),
        },
        digest_hour: u8::try_from(input.number("digest_hour", 0, 23)?).map_err(|_| "sayi")?,
        include_keywords: keyword_list(input.raw("include_keywords"))?,
        exclude_keywords: keyword_list(input.raw("exclude_keywords"))?,
        dedupe: input.checked("dedupe"),
        max_per_hour: input.number("max_per_hour", 1, MAX_PER_HOUR)?,
        weekly: input.checked("weekly"),
        weekly_day: u8::try_from(input.number("weekly_day", 0, 6)?).map_err(|_| "sayi")?,
        weekly_hour: u8::try_from(input.number("weekly_hour", 0, 23)?).map_err(|_| "sayi")?,
        role: input.role("role")?.map(serenity::RoleId::get),
        roles,
    };
    let personal_panel = input.channel("personal_panel")?;
    state.settings.set(&state.db, guild_id, settings).await.map_err(failed)?;
    if let Some(channel) = personal_panel {
        channel.send_message(&state.http, crate::newsprefs::panel_message()).await.map_err(|e| {
            warn!(guild = %guild_id, error = %e, "Panel: kişisel haber paneli gönderilemedi");
            "gonder"
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options {
            guild_name: "T".to_owned(),
            channels: vec![(10, "genel".to_owned())],
            categories: Vec::new(),
            roles: vec![(20, "Oyuncu".to_owned()), (21, "Müzik".to_owned())],
            all_roles: vec![(20, "Oyuncu".to_owned()), (21, "Müzik".to_owned()), (30, "Yönetici".to_owned())],
        }
    }

    fn form(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn topics_are_cleaned_and_limited() {
        assert_eq!(parse_topics(" Genel \n\nGenel\nŞikâyet"), Ok(vec!["Genel".to_owned(), "Şikâyet".to_owned()]));
        assert_eq!(parse_topics(""), Ok(Vec::new()));
        assert_eq!(parse_topics(&"a".repeat(51)), Err("metin"));
        let many: String = (0..11).map(|i| format!("konu {i}\n")).collect();
        assert_eq!(parse_topics(&many), Err("limit"));
    }

    #[test]
    fn role_menu_validation() {
        let options = options();
        let parse = |pairs: &[(&str, &str)]| {
            let form = form(pairs);
            parse_menu(&Input { form: &form, options: &options }).map(|m| m.map(|m| (m.mode, m.pairs.len(), m.pairs.first().map(|p| p.label.clone()))))
        };
        assert_eq!(parse(&[("title", "x")]), Ok(None));
        let base = [("channel", "10"), ("title", "Roller"), ("mode", "reactions")];
        let with = |extra: &[(&'static str, &'static str)]| [&base[..], extra].concat();
        assert_eq!(parse(&with(&[("pair_role_0", "20"), ("pair_emoji_0", "🎮")])), Ok(Some((Mode::Reactions, 1, Some("Oyuncu".to_owned())))));
        // Tepki modunda emoji zorunlu; buton modunda değil.
        assert_eq!(parse(&with(&[("pair_role_0", "20")])), Err("emoji"));
        assert_eq!(parse(&[("channel", "10"), ("title", "R"), ("mode", "buttons"), ("pair_role_0", "20")]), Ok(Some((Mode::Buttons, 1, Some("Oyuncu".to_owned())))));
        assert_eq!(parse(&with(&[("pair_role_0", "20"), ("pair_emoji_0", "yazı")])), Err("emoji"));
        // Yetkili rol (seçilebilir listede yok), aynı rol ya da aynı emoji iki kez olamaz.
        assert_eq!(parse(&with(&[("pair_role_0", "30"), ("pair_emoji_0", "🎮")])), Err("rol"));
        assert_eq!(parse(&with(&[("pair_role_0", "20"), ("pair_emoji_0", "🎮"), ("pair_role_1", "20"), ("pair_emoji_1", "🎵")])), Err("tekrar"));
        assert_eq!(parse(&with(&[("pair_role_0", "20"), ("pair_emoji_0", "🎮"), ("pair_role_1", "21"), ("pair_emoji_1", "🎮")])), Err("tekrar"));
        assert_eq!(parse(&with(&[])), Err("bos"));
        assert_eq!(parse(&[("channel", "10"), ("mode", "buttons"), ("pair_role_0", "20")]), Err("bos"));
    }

    #[test]
    fn command_validation() {
        let options = options();
        let parse = |pairs: &[(&str, &str)]| {
            let form = form(pairs);
            parse_command(&Input { form: &form, options: &options }).map(|c| c.map(|c| (c.name, c.cooldown_secs, c.add_roles)))
        };
        assert_eq!(parse(&[]), Ok(None));
        // Ad unutulup cevap ya da regex yazıldıysa uyarılır (eskiden sessizce kaydedilmiyordu).
        assert_eq!(parse(&[("cmd_response", "x")]), Err("ad_bos"));
        assert_eq!(parse(&[("cmd_pattern", "^selam+$")]), Err("ad_bos"));
        assert_eq!(
            parse(&[("cmd_name", "!KURALLAR"), ("cmd_response", "oku"), ("cmd_cooldown", "30"), ("cmd_add_role", "20")]),
            Ok(Some(("kurallar".to_owned(), 30, vec![20])))
        );
        assert_eq!(parse(&[("cmd_name", "iki kelime"), ("cmd_response", "x"), ("cmd_cooldown", "0")]), Err("ad"));
        assert_eq!(parse(&[("cmd_name", "a"), ("cmd_cooldown", "0")]), Err("bos"));
        assert_eq!(parse(&[("cmd_name", "a"), ("cmd_response", "x"), ("cmd_cooldown", "99999")]), Err("sayi"));
        // Yetkili rol komutla verilemez.
        assert_eq!(parse(&[("cmd_name", "a"), ("cmd_response", "x"), ("cmd_cooldown", "0"), ("cmd_add_role", "30")]), Err("rol"));
    }

    #[test]
    fn timer_validation() {
        let options = options();
        let parse = |pairs: &[(&str, &str)]| {
            let form = form(pairs);
            parse_timer(&Input { form: &form, options: &options }, 7, 1_000).map(|t| t.map(|t| (t.id, t.interval_mins, t.next_unix)))
        };
        assert_eq!(parse(&[("timer_message", "x")]), Ok(None));
        let base = [("timer_channel", "10"), ("timer_message", "merhaba")];
        let with = |extra: &[(&'static str, &'static str)]| [&base[..], extra].concat();
        assert_eq!(parse(&with(&[("timer_every", "2"), ("timer_unit", "hours")])), Ok(Some((7, 120, 1_000 + 7_200))));
        assert_eq!(parse(&with(&[("timer_every", "4"), ("timer_unit", "minutes")])), Err("sayi"));
        assert_eq!(parse(&with(&[("timer_every", "500"), ("timer_unit", "hours")])), Err("sayi"));
        assert_eq!(parse(&with(&[("timer_every", "5"), ("timer_unit", "gün")])), Err("hata"));
        assert_eq!(parse(&[("timer_channel", "10"), ("timer_every", "1"), ("timer_unit", "hours")]), Err("bos"));
        assert_eq!(interval_label(120), "2 saatte bir");
        assert_eq!(interval_label(45), "45 dakikada bir");
    }

    #[test]
    fn embed_validation() {
        let options = options();
        let parse = |pairs: &[(&str, &str)]| {
            let form = form(pairs);
            parse_embed(&Input { form: &form, options: &options }).map(|e| e.map(|e| (e.title, e.colour, e.image)))
        };
        assert_eq!(parse(&[("embed_colour", "#ffffff")]), Ok(None));
        assert_eq!(
            parse(&[("embed_title", "Kurallar"), ("embed_colour", "#ff00aa"), ("embed_image", "https://ornek.com/a.png")]),
            Ok(Some(("Kurallar".to_owned(), "#FF00AA".to_owned(), Some("https://ornek.com/a.png".to_owned()))))
        );
        assert_eq!(parse(&[("embed_title", "K"), ("embed_colour", "mavi")]), Err("renk"));
        assert_eq!(parse(&[("embed_title", "K"), ("embed_colour", "#000000"), ("embed_image", "http://ornek.com/a.png")]), Err("adres"));
        assert_eq!(parse(&[("embed_title", "K"), ("embed_colour", "#000000"), ("embed_thumbnail", "javascript:alert(1)")]), Err("adres"));
    }
}
