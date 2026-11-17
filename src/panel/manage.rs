//! Daha önce yalnızca komutla yönetilen özelliklerin panel sayfaları: etiketler, ekonomi (mağaza),
//! sayaç kanalları, geçici odalar, anket, çekiliş, başvuru formları; ayrıca ModMail ve yıkım koruması.

use std::fmt::Write as _;
use std::time::Duration;

use poise::serenity_prelude::{ChannelId, ChannelType, CreateAllowedMentions, CreateMessage, CreatePoll, CreatePollAnswer, GuildId, RoleId};
use tracing::warn;

use super::plugins::{checkbox, checklist, field, radios, section, Input, Options};
use crate::antinuke::{self, AntiNukeSettings, Punishment};
use crate::guild_config::Setting;
use crate::modmail::{self, ModmailSettings};
use crate::statchannels::{self, StatKind};
use crate::web::{escape, WebState};
use crate::{applications, economy, giveaway, tags};

fn text_input(name: &str, value: &str, placeholder: &str, max: usize) -> String {
    format!(
        "<input type=\"text\" class=\"wide\" name=\"{name}\" value=\"{}\" maxlength=\"{max}\" placeholder=\"{}\">",
        escape(value),
        escape(placeholder)
    )
}

fn big_textarea(name: &str, value: &str, max: usize, rows: u8, placeholder: &str) -> String {
    format!("<textarea name=\"{name}\" rows=\"{rows}\" maxlength=\"{max}\" placeholder=\"{}\">{}</textarea>", escape(placeholder), escape(value))
}

fn number_input(name: &str, value: u32, min: u32, max: u32) -> String {
    format!("<input type=\"number\" name=\"{name}\" value=\"{value}\" min=\"{min}\" max=\"{max}\">")
}

fn preview(text: &str, max: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= max { flat } else { flat.chars().take(max).collect::<String>() + "…" }
}

fn channel_opt(options: &Options, name: &str, selected: Option<u64>, none: &str) -> String {
    Options::select(name, &options.channels, "# ", selected, none)
}

fn failed(guild_id: GuildId, what: &'static str) -> impl Fn(anyhow::Error) -> &'static str {
    move |e| {
        warn!(guild = %guild_id, error = ?e, "Panel: {what} kaydedilemedi");
        "hata"
    }
}

/// Formdaki zorunlu kısa metin.
fn required(input: &Input<'_>, key: &str, max: usize) -> Result<String, &'static str> {
    let raw = input.raw(key);
    if raw.is_empty() {
        return Err("bos");
    }
    if raw.chars().count() > max {
        return Err("metin");
    }
    Ok(raw.to_owned())
}

/// Her satırı bir öğe olan liste (boş satırlar atılır).
fn lines(raw: &str, max_items: usize, max_len: usize) -> Result<Vec<String>, &'static str> {
    let items: Vec<String> = raw.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
    if items.len() > max_items {
        return Err("limit");
    }
    if items.iter().any(|item| item.chars().count() > max_len) {
        return Err("metin");
    }
    Ok(items)
}

/// Sunucunun ses kanalları (önbellekten; kilit hemen bırakılır).
fn voice_channels(state: &WebState, guild_id: GuildId) -> Vec<(u64, String)> {
    let mut found: Vec<(u16, u64, String)> = state
        .cache
        .guild(guild_id)
        .map(|g| g.channels.values().filter(|c| c.kind == ChannelType::Voice && !crate::backup::is_hidden(c)).map(|c| (c.position, c.id.get(), c.name.clone())).collect())
        .unwrap_or_default();
    found.sort();
    found.into_iter().map(|(_, id, name)| (id, name)).collect()
}

// ---------------------------------------------------------------- Etiketler

const TAG_MAX: usize = 1500;

pub async fn tag_fields(state: &WebState, guild_id: GuildId) -> anyhow::Result<(String, bool)> {
    let all = state.tags.all(&state.db, guild_id).await?;
    let mut list = String::new();
    if all.is_empty() {
        list.push_str("<p class=\"note\">Henüz etiket yok.</p>");
    } else {
        list.push_str("<table><tr><th>Ad</th><th>İçerik</th><th>Sil</th></tr>");
        for (name, content) in all.iter() {
            let _ = write!(
                list,
                "<tr><td><code>{prefix}{}</code></td><td>{}</td><td><input type=\"checkbox\" name=\"tag_del\" value=\"{}\"></td></tr>",
                escape(name),
                escape(&preview(content, 90)),
                escape(name),
                prefix = tags::PREFIX
            );
        }
        list.push_str("</table>");
    }
    let out = section("Etiketler", list)
        + &section(
            "Yeni etiket",
            field("Ad", &text_input("tag_name", "", "kurallar", tags::MAX_NAME), "Boş bırakırsan yeni etiket eklenmez. Aynı ad varsa içeriği değişir.")
                + &field("İçerik", &big_textarea("tag_content", "", TAG_MAX, 4, "Sohbete !kurallar yazınca gönderilecek metin"), "Etiketler kimseyi etiketlemez."),
        );
    Ok((out, !all.is_empty()))
}

pub async fn tag_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "etiket");
    let all = state.tags.all(&state.db, guild_id).await.map_err(&failed)?;
    let removed: Vec<&str> = input.all("tag_del");
    if removed.iter().any(|name| !all.contains_key(*name)) {
        return Err("hata");
    }
    let addition = if input.raw("tag_name").is_empty() {
        None
    } else {
        let name = tags::normalize_name(input.raw("tag_name")).map_err(|_| "ad")?;
        let content = required(input, "tag_content", TAG_MAX)?;
        let remaining = all.keys().filter(|k| !removed.contains(&k.as_str())).count();
        if !all.contains_key(&name) && remaining >= tags::MAX_TAGS {
            return Err("limit");
        }
        Some((name, content))
    };
    for name in removed {
        state.tags.remove(&state.db, guild_id, name).await.map_err(&failed)?;
    }
    if let Some((name, content)) = addition {
        let author = state.cache.current_user().id;
        state.tags.set(&state.db, guild_id, &name, &content, author).await.map_err(&failed)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- Ekonomi

const ITEM_NAME_MAX: usize = 50;
const ITEM_DESC_MAX: usize = 200;
const ITEM_PRICE_MAX: u32 = 1_000_000_000;

pub async fn economy_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let items = economy::shop(&state.db, guild_id).await?;
    let richest = economy::leaderboard(&state.db, guild_id, 10).await?;

    let mut list = String::new();
    if items.is_empty() {
        list.push_str("<p class=\"note\">Mağazada ürün yok.</p>");
    } else {
        list.push_str("<table><tr><th>Ürün</th><th>Fiyat</th><th>Verdiği rol</th><th>Sil</th></tr>");
        for item in &items {
            let role = item
                .role()
                .map(|id| options.all_roles.iter().find(|(known, _)| *known == id.get()).map_or_else(|| "(silinmiş rol)".to_owned(), |(_, name)| format!("@ {name}")))
                .unwrap_or_else(|| "—".to_owned());
            let _ = write!(
                list,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"item_del\" value=\"{}\"></td></tr>",
                escape(&item.name),
                escape(&economy::money(item.price)),
                escape(&role),
                item.id
            );
        }
        list.push_str("</table>");
    }
    let mut out = section("Mağaza", list);
    out.push_str(&section(
        "Yeni ürün",
        field("Ad", &text_input("item_name", "", "VIP rolü", ITEM_NAME_MAX), "Boş bırakırsan yeni ürün eklenmez.")
            + &field("Fiyat", &number_input("item_price", 1000, 1, ITEM_PRICE_MAX), "")
            + &field("Satın alana verilecek rol", &options.role_select("item_role", None), "Yetki içeren roller satılamaz.")
            + &field("Açıklama", &text_input("item_desc", "", "İsteğe bağlı", ITEM_DESC_MAX), ""),
    ));

    let mut board = String::new();
    if richest.is_empty() {
        board.push_str("<p class=\"note\">Henüz kimse para kazanmadı. Üyeler <code>/günlük</code> ve <code>/çalış</code> ile başlar.</p>");
    } else {
        // Adlar önbellekten; kilit hemen bırakılır.
        let names: Vec<String> = {
            let guild = state.cache.guild(guild_id);
            richest
                .iter()
                .map(|(user, _)| guild.as_ref().and_then(|g| g.members.get(user)).map_or_else(|| format!("Kullanıcı {user}"), |m| m.display_name().to_owned()))
                .collect()
        };
        board.push_str("<table><tr><th>#</th><th>Üye</th><th>Toplam</th></tr>");
        for (i, ((_, total), name)) in richest.iter().zip(names).enumerate() {
            let _ = write!(board, "<tr><td>{}</td><td>{}</td><td>{}</td></tr>", i + 1, escape(&name), escape(&economy::money(*total)));
        }
        board.push_str("</table>");
    }
    out.push_str(&section(
        "En zenginler",
        board + "<p class=\"note\">Üyelere para vermek, almak ya da hesabı sıfırlamak için: <code>/ekonomi-yönet</code>. Oyunlar: <code>/yazı-tura · slot · blackjack</code>.</p>",
    ));
    Ok((out, !items.is_empty() || !richest.is_empty()))
}

pub async fn economy_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "mağaza");
    let removed = input.all("item_del").into_iter().map(|raw| raw.parse::<i64>().map_err(|_| "hata")).collect::<Result<Vec<_>, _>>()?;
    let items = economy::shop(&state.db, guild_id).await.map_err(&failed)?;
    let addition = if input.raw("item_name").is_empty() {
        None
    } else {
        let name = required(input, "item_name", ITEM_NAME_MAX)?;
        let price = i64::from(input.number("item_price", 1, ITEM_PRICE_MAX)?);
        let role = input.role("item_role")?;
        let description = input.raw("item_desc");
        if description.chars().count() > ITEM_DESC_MAX {
            return Err("metin");
        }
        if items.iter().any(|i| i.name.to_lowercase() == name.to_lowercase() && !removed.contains(&i.id)) {
            return Err("tekrar");
        }
        let remaining = items.iter().filter(|i| !removed.contains(&i.id)).count();
        if i64::try_from(remaining).map_err(|_| "hata")? >= economy::MAX_SHOP_ITEMS {
            return Err("limit");
        }
        Some((name, price, role, (!description.is_empty()).then(|| description.to_owned())))
    };
    for item in items.iter().filter(|i| removed.contains(&i.id)) {
        economy::remove_item(&state.db, guild_id, &item.name).await.map_err(&failed)?;
    }
    if let Some((name, price, role, description)) = addition {
        economy::add_item(&state.db, guild_id, &name, price, role, description.as_deref()).await.map_err(&failed)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- Sayaç kanalları

const STAT_KINDS: &[(StatKind, &str)] = &[
    (StatKind::Members, "Tüm üyeler"),
    (StatKind::Humans, "İnsanlar (bot hariç)"),
    (StatKind::Bots, "Botlar"),
    (StatKind::Boosts, "Sunucu takviyeleri"),
    (StatKind::Goal, "Üye hedefi (hedef sayısı gerekir)"),
    (StatKind::TextChannels, "Metin kanalları"),
    (StatKind::VoiceChannels, "Ses kanalları"),
    (StatKind::Channels, "Tüm kanallar"),
    (StatKind::Roles, "Rol sayısı"),
    (StatKind::RoleMembers, "Roldeki üyeler (rol gerekir)"),
    (StatKind::Online, "Çevrimiçi üyeler (Presence Intent gerekir)"),
    (StatKind::RecordOnline, "Çevrimiçi rekoru (Presence Intent gerekir)"),
    (StatKind::Crypto, "Kripto para fiyatı (para adı gerekir)"),
];

pub async fn stat_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let current = statchannels::list(&state.db, guild_id).await?;
    let mut list = String::new();
    if current.is_empty() {
        list.push_str("<p class=\"note\">Henüz sayaç kanalı yok.</p>");
    } else {
        // Kanal adları önbellekten; kilit hemen bırakılır.
        let names: Vec<String> = {
            let guild = state.cache.guild(guild_id);
            current.iter().map(|(id, ..)| guild.as_ref().and_then(|g| g.channels.get(id).map(|c| c.name.clone())).unwrap_or_else(|| "(silinmiş kanal)".to_owned())).collect()
        };
        list.push_str("<table><tr><th>Kanal</th><th>Tür</th><th>Sil</th></tr>");
        for ((id, kind, _), name) in current.iter().zip(names) {
            let label = STAT_KINDS.iter().find(|(k, _)| k == kind).map_or("?", |(_, label)| label);
            let _ = write!(list, "<tr><td>🔊 {}</td><td>{}</td><td><input type=\"checkbox\" name=\"stat_del\" value=\"{id}\"></td></tr>", escape(&name), escape(label));
        }
        list.push_str("</table><p class=\"note\">Silince Discord'daki kanal da silinir.</p>");
    }
    let mut kinds = String::from("<select name=\"stat_kind\"><option value=\"\">— Yeni sayaç ekleme —</option>");
    for (kind, label) in STAT_KINDS {
        let _ = write!(kinds, "<option value=\"{}\">{}</option>", kind.db(), escape(label));
    }
    kinds.push_str("</select>");
    let out = section("Sayaç kanalları", list)
        + &section(
            "Yeni sayaç",
            field("Ne sayılsın", &kinds, "Adı 10 dakikada bir güncellenen kilitli bir ses kanalı oluşturulur.")
                + &field("Kategori", &Options::select("stat_category", &options.categories, "", None, "— Kategorisiz —"), "")
                + &field("Üye hedefi", &number_input("stat_goal", 100, 1, 10_000_000), "Yalnızca “Üye hedefi” türünde kullanılır.")
                + &field("Rol", &Options::select("stat_role", &options.all_roles, "@ ", None, "— Yok —"), "Yalnızca “Roldeki üyeler” türünde kullanılır.")
                + &field("Para", &text_input("stat_coin", "", "bitcoin", 50), "Yalnızca “Kripto para fiyatı” türünde; CoinGecko adıyla (bitcoin, ethereum, dogecoin…)."),
        );
    Ok((out, !current.is_empty()))
}

pub async fn stat_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "sayaç kanalı");
    let current = statchannels::list(&state.db, guild_id).await.map_err(&failed)?;
    let removed = input.all("stat_del").into_iter().map(|raw| raw.parse::<u64>().map_err(|_| "hata")).collect::<Result<Vec<_>, _>>()?;
    // Yalnızca bu sunucunun sayaç kanalları silinebilir.
    if removed.iter().any(|id| !current.iter().any(|(known, ..)| known.get() == *id)) {
        return Err("kanal");
    }
    let addition = match input.raw("stat_kind") {
        "" => None,
        raw => {
            let kind = StatKind::from_db(raw).ok_or("hata")?;
            let category = input.id("stat_category", &input.options.categories, "kanal")?.map(ChannelId::new);
            let goal = if kind == StatKind::Goal { Some(input.number("stat_goal", 1, 10_000_000)?) } else { None };
            let role = if kind == StatKind::RoleMembers { Some(input.id("stat_role", &input.options.all_roles, "rol")?.map(RoleId::new).ok_or("rol")?) } else { None };
            if matches!(kind, StatKind::Online | StatKind::RecordOnline) && !statchannels::presence_intent() {
                return Err("presence");
            }
            let coin = if kind == StatKind::Crypto {
                let coin = input.raw("stat_coin").to_lowercase().replace(' ', "-");
                if !statchannels::valid_coin(&coin) {
                    return Err("bos");
                }
                let price = statchannels::coin_price(&state.http_client, &coin).await.map_err(|_| "kaynak")?;
                Some((coin, price))
            } else {
                None
            };
            let remaining = current.iter().filter(|(id, ..)| !removed.contains(&id.get())).count();
            if i64::try_from(remaining).map_err(|_| "hata")? >= statchannels::MAX_PER_GUILD {
                return Err("limit");
            }
            Some((kind, goal, role, category, coin))
        }
    };
    for id in removed {
        let channel = ChannelId::new(id.max(1));
        statchannels::on_channel_deleted(&state.db, channel).await.map_err(&failed)?;
        if let Err(e) = channel.delete(&state.http).await {
            warn!(channel = %channel, error = %e, "Panel: sayaç kanalı Discord'dan silinemedi");
        }
    }
    if let Some((kind, goal, role, category, coin)) = addition {
        let extra = coin.as_ref().map(|(coin, price)| (coin.as_str(), price.as_str()));
        statchannels::create(&state.cache, &state.http, &state.db, guild_id, kind, goal, role, category, extra).await.map_err(|e| {
            warn!(guild = %guild_id, error = ?e, "Panel: sayaç kanalı oluşturulamadı");
            "gonder"
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------- Geçici odalar

pub async fn tempvoice_fields(state: &WebState, guild_id: GuildId) -> anyhow::Result<(String, bool)> {
    let cfg = state.configs.get(&state.db, guild_id).await?;
    let hub = cfg.tempvoice_hub.map(ChannelId::get);
    let out = section(
        "Oda oluşturma kanalı",
        field(
            "“Oda Oluştur” ses kanalı",
            &Options::select("hub", &voice_channels(state, guild_id), "🔊 ", hub, "— Kapalı —"),
            "Bu kanala giren üyeye kendi adına bir ses odası açılır ve üye oraya taşınır; oda boşalınca silinir. Botun 'Kanalları Yönet' ve 'Üyeleri Taşı' izni olmalı.",
        ) + "<p class=\"note\">Oda sahibi komutları: <code>/oda isim · limit · kilitle · aç · gizle · göster · yasakla · yasağı-kaldır · at · devret · sahiplen · sahip</code>. \
             Yöneticiler için: <code>/oda temizle</code>.</p>",
    );
    Ok((out, hub.is_some()))
}

pub async fn tempvoice_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let hub = input.id("hub", &voice_channels(state, guild_id), "kanal")?.map(ChannelId::new);
    state.configs.set(&state.db, guild_id, Setting::TempVoiceHub(hub)).await.map_err(failed(guild_id, "geçici oda"))
}

// ---------------------------------------------------------------- Anket

const POLL_QUESTION_MAX: usize = 300;
const POLL_ANSWER_MAX: usize = 55;
/// Discord anketleri en fazla 32 gün sürer.
const POLL_MAX_HOURS: u32 = 32 * 24;

pub fn poll_fields(options: &Options) -> (String, bool) {
    let out = section(
        "Yeni anket",
        field("Kanal", &channel_opt(options, "poll_channel", None, "— Seç —"), "Kanal seçmezsen anket gönderilmez.")
            + &field("Soru", &text_input("poll_question", "", "Bu hafta hangi oyunu oynayalım?", POLL_QUESTION_MAX), "")
            + &field("Seçenekler (her satıra bir tane)", &big_textarea("poll_answers", "", 700, 5, "Valorant\nMinecraft\nCS2"), "En az 2, en fazla 10 seçenek; her biri en çok 55 karakter.")
            + &field("Süre (saat)", &number_input("poll_hours", 24, 1, POLL_MAX_HOURS), "")
            + &checkbox("poll_multi", false, "Birden fazla seçenek işaretlenebilsin")
            + "<p class=\"note\">Anket, Discord'un kendi anket özelliğiyle gönderilir; sonuçlar mesajın üstünde görünür. \
               Dakikalı ya da butonlu anket için: <code>/eğlence anket</code>.</p>",
    );
    (out, false)
}

pub async fn poll_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let Some(channel) = input.channel("poll_channel")? else {
        return Ok(());
    };
    let question = required(input, "poll_question", POLL_QUESTION_MAX)?;
    let answers = lines(input.raw("poll_answers"), 10, POLL_ANSWER_MAX)?;
    if answers.len() < 2 {
        return Err("bos");
    }
    let hours = input.number("poll_hours", 1, POLL_MAX_HOURS)?;
    let mut poll = CreatePoll::new()
        .question(question)
        .answers(answers.into_iter().map(|a| CreatePollAnswer::new().text(a)).collect())
        .duration(Duration::from_secs(u64::from(hours) * 3600));
    if input.checked("poll_multi") {
        poll = poll.allow_multiselect();
    }
    channel.send_message(&state.http, CreateMessage::new().poll(poll).allowed_mentions(CreateAllowedMentions::new())).await.map(|_| ()).map_err(|e| {
        warn!(guild = %guild_id, error = %e, "Panel: anket gönderilemedi");
        "gonder"
    })
}

// ---------------------------------------------------------------- Çekiliş

const PRIZE_MAX: usize = 200;

pub fn giveaway_fields(options: &Options) -> (String, bool) {
    let out = section(
        "Yeni çekiliş",
        field("Kanal", &channel_opt(options, "gw_channel", None, "— Seç —"), "Kanal seçmezsen çekiliş başlatılmaz.")
            + &field("Ödül", &text_input("gw_prize", "", "1 aylık Nitro", PRIZE_MAX), "")
            + &field("Kazanan sayısı", &number_input("gw_winners", 1, 1, giveaway::MAX_WINNERS), "")
            + &field(
                "Süre",
                &format!(
                    "{} <select name=\"gw_unit\" style=\"max-width:160px\"><option value=\"hours\">saat</option><option value=\"minutes\">dakika</option><option value=\"days\">gün</option></select>",
                    number_input("gw_every", 24, 1, 43_200)
                ),
                "En fazla 30 gün.",
            )
            + &field("Gereken rol", &Options::select("gw_role", &options.all_roles, "@ ", None, "— Herkes katılabilir —"), "")
            + &field("En az seviye", &number_input("gw_level", 0, 0, giveaway::MAX_MIN_LEVEL), "0 = şart yok.")
            + &field("Hesap en az kaç günlük", &number_input("gw_days", 0, 0, giveaway::MAX_ACCOUNT_DAYS), "0 = şart yok. Sahte / yeni hesaplara karşı.")
            + &field(
                "Ek şans rolü",
                &Options::select("gw_bonus", &options.all_roles, "@ ", None, "— Yok —"),
                "Bu roldekilerin katılımı 2 kez sayılır (ör. takviyeciler).",
            )
            + "<p class=\"note\">Üyeler çekiliş mesajındaki butonla katılır; süre dolunca kazananlar kendiliğinden seçilir. \
               Erken bitirmek ya da yeniden çekmek için: <code>/çekiliş bitir · yeniden-çek</code>.</p>",
    );
    (out, false)
}

pub async fn giveaway_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let Some(channel_id) = input.channel("gw_channel")? else {
        return Ok(());
    };
    let prize = required(input, "gw_prize", PRIZE_MAX)?;
    let winners = input.number("gw_winners", 1, giveaway::MAX_WINNERS)?;
    let every = u64::from(input.number("gw_every", 1, 43_200)?);
    let seconds = match input.raw("gw_unit") {
        "minutes" => every * 60,
        "hours" => every * 3600,
        "days" => every * 86_400,
        _ => return Err("hata"),
    };
    let duration = Duration::from_secs(seconds);
    if duration > giveaway::MAX_DURATION {
        return Err("sayi");
    }
    // Çekilişi panelden başlatan kişi yerine bot "düzenleyen" olarak görünür.
    let host = state.cache.current_user().id;
    let requirements = giveaway::Requirements {
        role: input.id("gw_role", &input.options.all_roles, "rol")?.map(RoleId::new),
        min_level: if input.raw("gw_level").is_empty() { 0 } else { input.number("gw_level", 0, giveaway::MAX_MIN_LEVEL)? },
        min_account_days: if input.raw("gw_days").is_empty() { 0 } else { input.number("gw_days", 0, giveaway::MAX_ACCOUNT_DAYS)? },
        bonus_role: input.id("gw_bonus", &input.options.all_roles, "rol")?.map(RoleId::new),
    };
    let new = giveaway::NewGiveaway { guild_id, channel_id, host, prize: &prize, winners, duration, requirements };
    giveaway::create(&state.http, &state.db, &state.scheduler, new).await.map(|_| ()).map_err(|e| {
        warn!(guild = %guild_id, error = ?e, "Panel: çekiliş başlatılamadı");
        "gonder"
    })
}

// ---------------------------------------------------------------- Başvuru formları

const FORM_NAME_MAX: usize = 45;
const QUESTION_MAX: usize = 45;

pub async fn application_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let forms = applications::forms(&state.db, guild_id).await?;
    let mut list = String::new();
    if forms.is_empty() {
        list.push_str("<p class=\"note\">Henüz başvuru formu yok.</p>");
    } else {
        list.push_str("<table><tr><th>Form</th><th>Sorular</th><th>Paneli gönder</th><th>Sil</th></tr>");
        for form in &forms {
            let _ = write!(
                list,
                "<tr><td>{}</td><td>{}</td><td><input type=\"radio\" name=\"form_send\" value=\"{id}\"></td><td><input type=\"checkbox\" name=\"form_del\" value=\"{id}\"></td></tr>",
                escape(&form.name),
                escape(&preview(&form.questions.join(" · "), 110)),
                id = form.id
            );
        }
        list.push_str("</table>");
        list.push_str(&field(
            "Seçili formun panelini bu kanala gönder",
            &channel_opt(options, "form_send_channel", None, "— Gönderme —"),
            "Üyeler bu mesajdaki butona basıp formu doldurur.",
        ));
    }
    let out = section("Başvuru formları", list)
        + &section(
            "Yeni form",
            field("Formun adı", &text_input("form_name", "", "Moderatör başvurusu", FORM_NAME_MAX), "Boş bırakırsan yeni form eklenmez.")
                + &field("Sorular (her satıra bir tane)", &big_textarea("form_questions", "", 300, 5, "Kaç yaşındasın?\nNeden moderatör olmak istiyorsun?"), "En az 1, en fazla 5 soru; her biri en çok 45 karakter (Discord sınırı).")
                + &field("Başvuruların düşeceği yetkili kanalı", &channel_opt(options, "form_channel", None, "— Seç —"), "")
                + &field("Kabul edilene verilecek rol", &options.role_select("form_role", None), ""),
        );
    Ok((out, !forms.is_empty()))
}

pub async fn application_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "başvuru formu");
    let forms = applications::forms(&state.db, guild_id).await.map_err(&failed)?;
    let removed = input.all("form_del").into_iter().map(|raw| raw.parse::<i64>().map_err(|_| "hata")).collect::<Result<Vec<_>, _>>()?;
    if removed.iter().any(|id| !forms.iter().any(|f| f.id == *id)) {
        return Err("hata");
    }
    let send = match (input.raw("form_send"), input.channel("form_send_channel")?) {
        ("", _) | (_, None) => None,
        (raw, Some(channel)) => {
            let id: i64 = raw.parse().map_err(|_| "hata")?;
            let form = forms.iter().find(|f| f.id == id && !removed.contains(&id)).ok_or("hata")?;
            Some((form, channel))
        }
    };
    let addition = if input.raw("form_name").is_empty() {
        None
    } else {
        let name = required(input, "form_name", FORM_NAME_MAX)?;
        let questions = lines(input.raw("form_questions"), 5, QUESTION_MAX)?;
        if questions.is_empty() {
            return Err("bos");
        }
        let channel = input.channel("form_channel")?.ok_or("kanal")?;
        let role = input.role("form_role")?;
        if forms.iter().any(|f| f.name.to_lowercase() == name.to_lowercase() && !removed.contains(&f.id)) {
            return Err("tekrar");
        }
        let remaining = forms.iter().filter(|f| !removed.contains(&f.id)).count();
        if i64::try_from(remaining).map_err(|_| "hata")? >= applications::MAX_FORMS {
            return Err("limit");
        }
        Some((name, questions, channel, role))
    };

    if let Some((form, channel)) = send {
        channel.send_message(&state.http, applications::panel_message(form, None)).await.map_err(|e| {
            warn!(guild = %guild_id, error = %e, "Panel: başvuru paneli gönderilemedi");
            "gonder"
        })?;
    }
    for form in forms.iter().filter(|f| removed.contains(&f.id)) {
        applications::delete_form(&state.db, guild_id, &form.name).await.map_err(&failed)?;
    }
    if let Some((name, questions, channel, role)) = addition {
        applications::create_form(&state.db, guild_id, &name, &questions, channel, role).await.map_err(&failed)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- ModMail

pub async fn modmail_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<ModmailSettings>(&state.db, guild_id).await?;
    let out = section(
        "ModMail",
        field(
            "Yetkili kanalı",
            &channel_opt(options, "channel", settings.channel, "— Kapalı —"),
            "Bir üye bota özelden yazınca bu kanalda o üyeye ait bir alt başlık açılır. Kanalı yalnızca yetkililerin görebildiğinden emin ol; \
             botun bu kanalda 'Herkese Açık Alt Başlık Oluştur' ve 'Alt Başlıklarda Mesaj Gönder' izni olmalı.",
        ) + &field(
            "Üyeye ilk mesajında gönderilecek karşılama",
            &big_textarea("greeting", settings.greeting.as_deref().unwrap_or_default(), modmail::MAX_GREETING, 3, "Boşsa hazır metin kullanılır"),
            "",
        ) + "<p class=\"note\">Yetkililer alt başlığa yazarak cevaplar; cevap üyeye “yetkili ekibi” adına gider (kimin yazdığı görünmez). \
             <code>.</code> ile başlayan mesajlar iç nottur, üyeye gitmez. Konuşmayı bitirmek için alt başlığa <code>!kapat</code> yazılır.</p>",
    );
    Ok((out, settings.channel.is_some()))
}

pub async fn modmail_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let greeting = input.raw("greeting");
    if greeting.chars().count() > modmail::MAX_GREETING {
        return Err("metin");
    }
    let settings = ModmailSettings { channel: input.channel("channel")?.map(ChannelId::get), greeting: (!greeting.is_empty()).then(|| greeting.to_owned()) };
    state.settings.set(&state.db, guild_id, settings).await.map_err(failed(guild_id, "ModMail"))
}

// ---------------------------------------------------------------- Yıkım koruması

pub async fn antinuke_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<AntiNukeSettings>(&state.db, guild_id).await?;
    let choices: Vec<(&str, &str)> = Punishment::ALL.iter().map(|(_, key, label)| (*key, *label)).collect();
    let out = section(
        "Yıkım koruması",
        checkbox("enabled", settings.enabled, "Kısa sürede çok sayıda kanal / rol silen ya da açan, üye yasaklayan / atan yetkiliyi durdur")
            + &checkbox("guard_webhooks", settings.guard_webhooks, "Güvenilmeyen birinin açtığı webhook'u anında sil (webhook'la toplu spam / etiketlemeye karşı)")
            + &checkbox("guard_bots", settings.guard_bots, "Güvenilmeyen birinin eklediği botu anında at")
            + &checkbox(
                "guard_permissions",
                settings.guard_permissions,
                "Güvenilmeyen birinin bir role verdiği yönetici / moderasyon yetkisini ve bir üyeye verdiği yetkili rolü geri al",
            )
            + &field("Kaç işlemde tetiklensin", &number_input("limit", settings.limit, antinuke::MIN_LIMIT, antinuke::MAX_LIMIT), "")
            + &field("Kaç saniye içinde", &number_input("window", settings.window_secs, antinuke::MIN_WINDOW, antinuke::MAX_WINDOW), "Örn. 30 saniyede 3 işlem. Kanal açmada sınır iki katıdır (30 saniyede 6 kanal).")
            + &field("Ne yapılsın", &radios("punishment", &choices, settings.punishment.key()), "")
            + &field(
                "Güvenilen roller",
                &checklist("trusted_role", &options.all_roles, "@ ", &settings.trusted_roles),
                "Bu rollerdeki üyeler sayılmaz. Sunucu sahibi ve botun kendisi hiçbir zaman sayılmaz. <b>Yönetici ekibinin rolünü buraya ekleyin</b>; yoksa onların yaptığı meşru yetki değişiklikleri, webhook ve bot eklemeleri de geri alınır.",
            )
            + "<p class=\"note\">Botun 'Denetim Kaydını Görüntüle' izni olmalı ve rolü yetkili rollerinin ÜSTÜNDE durmalı; yoksa kimin yaptığını göremez \
               ya da cezayı uygulayamaz. Tetiklenince olay kayıtları kanalına uyarı yazılır.</p>",
    );
    Ok((out, settings.enabled))
}

pub async fn antinuke_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let settings = AntiNukeSettings {
        enabled: input.checked("enabled"),
        limit: input.number("limit", antinuke::MIN_LIMIT, antinuke::MAX_LIMIT)?,
        window_secs: input.number("window", antinuke::MIN_WINDOW, antinuke::MAX_WINDOW)?,
        punishment: Punishment::parse(input.raw("punishment")).ok_or("hata")?,
        trusted_roles: input.ids("trusted_role", &input.options.all_roles, "rol")?,
        guard_webhooks: input.checked("guard_webhooks"),
        guard_bots: input.checked("guard_bots"),
        guard_permissions: input.checked("guard_permissions"),
    };
    state.settings.set(&state.db, guild_id, settings).await.map_err(failed(guild_id, "yıkım koruması"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_lists_are_cleaned_and_limited() {
        assert_eq!(lines(" a \n\n b ", 5, 10), Ok(vec!["a".to_owned(), "b".to_owned()]));
        assert_eq!(lines("a\nb\nc", 2, 10), Err("limit"));
        assert_eq!(lines("çok uzun bir satır", 5, 5), Err("metin"));
        assert_eq!(lines("", 5, 5), Ok(Vec::new()));
    }

    #[test]
    fn every_stat_kind_is_offered() {
        for (kind, _) in STAT_KINDS {
            assert_eq!(StatKind::from_db(kind.db()), Some(*kind));
        }
    }
}
