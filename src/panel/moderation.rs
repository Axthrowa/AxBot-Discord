//! Panel: Moderasyon eklentisi (otomatik moderasyon kuralları, eşikler, kelime listeleri, muaf
//! roller, kısıtlı kanallar, otomatik cezalar) ve Olay kayıtları eklentisi (kanallar + olay seçimi).
//!
//! İki sayfa da aynı `ModSettings` kaydını paylaşır; her biri yalnızca kendi alanlarını değiştirir.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::time::Duration;

use poise::serenity_prelude::{ChannelId, GuildId, RoleId};
use tracing::warn;

use super::plugins::{checkbox, checklist, field, section, Input, Options};
use crate::automod::config::{self, WordList, MAX_CUSTOM_WORDS};
use crate::automod::rules::{clean_domains, ModSettings, RestrictKind, Restricted, Rule, RuleAction, LOG_EVENTS};
use crate::guild_config::Setting;
use crate::moderation::{self, duration, Punishment, PunishmentRule};
use crate::web::{escape, WebState};

const MAX_DOMAINS: usize = 100;
const MAX_RESTRICTED: usize = 25;
const MAX_PUNISHMENTS: usize = 10;

fn number_input(name: &str, value: impl std::fmt::Display, min: u32, max: u32) -> String {
    format!("<input type=\"number\" name=\"{name}\" value=\"{value}\" min=\"{min}\" max=\"{max}\">")
}

fn text_input(name: &str, placeholder: &str) -> String {
    format!("<input type=\"text\" name=\"{name}\" maxlength=\"20\" placeholder=\"{}\">", escape(placeholder))
}

fn words_area(name: &str, words: &[String], placeholder: &str) -> String {
    format!(
        "<textarea name=\"{name}\" rows=\"4\" placeholder=\"{}\">{}</textarea>",
        escape(placeholder),
        escape(&words.join("\n"))
    )
}

/// Moderasyon sayfasının form alanları ve otomatik moderasyonun açık olup olmadığı.
pub async fn fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let legacy = config::load_from_db(&state.db, guild_id).await?;
    let settings = state.settings.get::<ModSettings>(&state.db, guild_id).await?;
    let punishments = moderation::punishments(&state.db, guild_id).await?;
    let s = &legacy.settings;

    let mut out = section(
        "Genel",
        checkbox("enabled", s.enabled, "Otomatik moderasyon açık")
            + &checkbox("exempt_staff", s.exempt_staff, "Yetkililer muaf (sunucu sahibi, Yönetici ve Mesajları Yönet izni olanlar)")
            + "<p class=\"note\">Botlar otomatik moderasyondan hiçbir zaman etkilenmez.</p>",
    );

    // Kurallar: her biri için eylem seçimi.
    let mut rules = String::from("<table><tr><th>Kural</th><th style=\"width:260px\">Eylem</th></tr>");
    for (rule, key, name, about) in Rule::ALL {
        let current = settings.action(rule, s);
        let mut select = format!("<select name=\"rule_{key}\">");
        for (action, code, label) in RuleAction::ALL {
            let _ = write!(select, "<option value=\"{code}\"{}>{label}</option>", if action == current { " selected" } else { "" });
        }
        select.push_str("</select>");
        let _ = write!(rules, "<tr><td><b>{}</b><div class=\"note\">{}</div></td><td>{select}</td></tr>", escape(name), escape(about));
    }
    rules.push_str("</table><p class=\"note\">\"Üyeyi uyar\" seçilirse bot adına bir uyarı kaydedilir; uyarılar aşağıdaki otomatik cezaları tetikleyebilir.</p>");
    out.push_str(&section("Otomatik moderasyon kuralları", rules));

    out.push_str(&section(
        "Eşikler",
        field("Büyük harf oranı (%)", &number_input("caps_percent", settings.caps_percent, 30, 100), "Mesajdaki harflerin en az bu kadarı büyükse.")
            + &field("Büyük harf: en az harf sayısı", &number_input("caps_min_letters", settings.caps_min_letters, 5, 200), "Daha kısa mesajlar sayılmaz.")
            + &field("Emoji sınırı", &number_input("emoji_limit", settings.emoji_limit, 1, 100), "Tek mesajda bundan fazla emoji olursa.")
            + &field("Spoiler sınırı", &number_input("spoiler_limit", settings.spoiler_limit, 1, 50), "")
            + &field("Bahsetme sınırı", &number_input("mention_limit", settings.mention_limit, 1, 50), "")
            + &field("Tekrar sınırı", &number_input("repeat_limit", settings.repeat_limit, 2, 20), "Aynı mesaj bir dakikada bu kadar kez gönderilirse.")
            + &field("Spam: mesaj sayısı", &number_input("spam_max_messages", s.spam_max_messages, 2, 30), "")
            + &field("Spam: süre (saniye)", &number_input("spam_window_secs", s.spam_window_secs, 2, 60), "Bu sürede yukarıdaki sayıdan fazla mesaj gönderilirse."),
    ));

    out.push_str(&section(
        "Kelime listeleri",
        field(
            "Yasaklı kelimeler (her satıra bir tane)",
            &words_area("blocked_words", &legacy.custom_words, "ornek\nkelime*\n*icinde*"),
            "Hazır Türkçe ve İngilizce küfür listesine ek olarak. <code>kelime</code> tam kelime, <code>kelime*</code> ile başlayan, \
             <code>*kelime*</code> içinde geçen.",
        ) + &field(
            "İstisnalar (her satıra bir tane)",
            &words_area("allowed_words", &legacy.allowed_words, "masum kelime"),
            "Filtreye takılan ama serbest olması gereken kelimeler.",
        ),
    ));

    out.push_str(&section(
        "İzin verilen alan adları",
        field(
            "Harici bağlantı kuralından muaf siteler",
            &words_area("allowed_domains", &settings.allowed_domains, "youtube.com\ntenor.com"),
            "Alt alan adları da serbest olur (youtube.com → m.youtube.com).",
        ),
    ));

    out.push_str(&section(
        "Muaf roller",
        "<p class=\"note\" style=\"margin:0 0 8px\">Bu rollerden birine sahip üyeler otomatik moderasyondan etkilenmez.</p>".to_owned()
            + &checklist("exempt_role", &options.all_roles, "@ ", &settings.exempt_roles),
    ));

    out.push_str(&section(
        "Karantina",
        field(
            "Karantina rolü",
            &options.role_select("quarantine_role", settings.quarantine_role.map(RoleId::new)),
            "/mod karantina bu rolü verir, üyenin diğer rollerini saklayıp alır. Rolün kanalları görmemesini rol izinlerinden ayarla.",
        ),
    ));

    // Kısıtlı kanallar.
    let mut restricted = String::new();
    if settings.restricted.is_empty() {
        restricted.push_str("<p class=\"note\">Henüz kısıtlı kanal yok.</p>");
    } else {
        restricted.push_str("<table><tr><th>Kanal</th><th>İzin verilen</th><th>Kaldır</th></tr>");
        for entry in &settings.restricted {
            let name = options
                .channels
                .iter()
                .find(|(id, _)| *id == entry.channel)
                .map_or_else(|| "(silinmiş kanal)".to_owned(), |(_, name)| format!("# {name}"));
            let _ = write!(
                restricted,
                "<tr><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"restricted_del\" value=\"{}\"></td></tr>",
                escape(&name),
                entry.kind.label(),
                entry.channel
            );
        }
        restricted.push_str("</table>");
    }
    let mut kinds = String::from("<select name=\"restricted_kind\">");
    for (_, code, label) in RestrictKind::ALL {
        let _ = write!(kinds, "<option value=\"{code}\">{label}</option>");
    }
    kinds.push_str("</select>");
    out.push_str(&section(
        "Kısıtlı kanallar",
        restricted
            + "<div style=\"height:14px\"></div>"
            + &field("Yeni kısıtlı kanal", &options.channel_select("restricted_channel", None), "")
            + &field("İzin verilen içerik", &kinds, "Uymayan mesajlar silinir. \"Sadece komutlar\": eğik çizgi komutları ve <code>!etiket</code> mesajları.")
            + &checkbox("restricted_reply", settings.restricted_reply, "Mesaj silinince kanalda kısa bir uyarı göster")
            + &checkbox("restricted_dm", settings.restricted_dm, "Mesaj silinince yazara özelden haber ver"),
    ));
    out.push_str(&section(
        "Takma ad denetimi",
        checkbox("nick_filter", settings.nick_filter, "Yasaklı kelime içeren takma adları değiştir")
            + &checkbox("dehoist", settings.dehoist, "Listede üste çıkmak için başa konan işaretleri (!, ., 0…) temizle")
            + &field(
                "Yerine verilecek ad",
                &format!("<input type=\"text\" name=\"nick_replacement\" value=\"{}\" maxlength=\"32\" placeholder=\"Üye\">", escape(&settings.nick_replacement)),
                "Ad küfürlüyse ya da temizlenince boş kalırsa bu ad verilir. Üye girişinde ve adını değiştirdiğinde denetlenir; muaf roller atlanır.                  Botun <b>Takma Adları Yönet</b> izni olmalı; sunucu sahibinin ve botun rolünden yüksek üyelerin adı değiştirilemez.",
            ),
    ));

    // Otomatik cezalar.
    let mut table = String::new();
    if punishments.is_empty() {
        table.push_str("<p class=\"note\">Henüz otomatik ceza yok.</p>");
    } else {
        table.push_str("<table><tr><th>Uyarı sayısı</th><th>Ceza</th><th>Sayılan süre</th><th>Kaldır</th></tr>");
        for rule in &punishments {
            let _ = write!(
                table,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"punish_del\" value=\"{}\"></td></tr>",
                rule.warn_count,
                escape(&moderation::describe_punishment(rule)),
                rule.window.map_or_else(|| "tüm aktif uyarılar".to_owned(), |w| format!("son {}", duration::format(w))),
                rule.warn_count
            );
        }
        table.push_str("</table>");
    }
    out.push_str(&section(
        "Otomatik cezalar",
        table
            + "<div style=\"height:14px\"></div>"
            + &field("Yeni ceza: uyarı sayısı", "<input type=\"number\" name=\"punish_count\" min=\"1\" max=\"50\" placeholder=\"örn. 3\">", "")
            + &field(
                "Ceza",
                "<select name=\"punish_action\"><option value=\"timeout\">Susturma</option><option value=\"kick\">Sunucudan atma</option>\
                 <option value=\"ban\">Yasaklama</option></select>",
                "",
            )
            + &field("Ceza süresi", &text_input("punish_duration", "örn. 1sa, 1g"), "Susturmada boşsa 1 saat; yasakta boşsa kalıcı; atmada kullanılmaz.")
            + &field("Sayılan süre", &text_input("punish_window", "örn. 1sa, 7g"), "Boşsa üyenin tüm aktif uyarıları sayılır; doluysa sadece bu süre içindekiler."),
    ));
    Ok((out, s.enabled))
}

/// Metin kutusundaki kelimeleri doğrular; ilk geçersiz kelimede hata.
fn parse_words(raw: &str, list: WordList) -> Result<BTreeSet<String>, &'static str> {
    let words: BTreeSet<String> = raw
        .split(['\n', ','])
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(|w| config::validate_word(w, list).map_err(|_| "kelime"))
        .collect::<Result<_, _>>()?;
    if i64::try_from(words.len()).unwrap_or(i64::MAX) > MAX_CUSTOM_WORDS {
        return Err("limit");
    }
    Ok(words)
}

fn parse_duration(raw: &str) -> Result<Option<Duration>, &'static str> {
    if raw.is_empty() {
        return Ok(None);
    }
    duration::parse(raw).map(Some).map_err(|_| "sure")
}

/// Formdaki yeni ceza satırı (boşsa `None`).
fn parse_punishment(input: &Input<'_>) -> Result<Option<PunishmentRule>, &'static str> {
    let count = input.raw("punish_count");
    if count.is_empty() {
        return Ok(None);
    }
    let warn_count: i32 = count.parse().ok().filter(|c| (1..=50).contains(c)).ok_or("sayi")?;
    let punishment = Punishment::from_db(input.raw("punish_action")).ok_or("hata")?;
    let mut length = parse_duration(input.raw("punish_duration"))?;
    match punishment {
        Punishment::Timeout => {
            if length.is_some_and(|d| d > duration::MAX_TIMEOUT) {
                return Err("sure");
            }
            length = length.or(Some(Duration::from_secs(3600)));
        }
        Punishment::Kick => length = None,
        Punishment::Ban => {}
    }
    Ok(Some(PunishmentRule { warn_count, punishment, duration: length, window: parse_duration(input.raw("punish_window"))? }))
}

/// Kelime listesini istenen hâle getirir (eksikler eklenir, fazlalar silinir).
async fn sync_words(state: &WebState, guild_id: GuildId, list: WordList, current: &[String], wanted: &BTreeSet<String>) -> anyhow::Result<()> {
    for word in current.iter().filter(|w| !wanted.contains(*w)) {
        config::remove_word(&state.db, guild_id, list, word).await?;
    }
    for word in wanted.iter().filter(|w| !current.contains(w)) {
        config::add_word(&state.db, guild_id, list, word).await?;
    }
    Ok(())
}

pub async fn save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = |e: anyhow::Error| {
        warn!(guild = %guild_id, error = ?e, "Panel: moderasyon ayarı kaydedilemedi");
        "hata"
    };
    let legacy = config::load_from_db(&state.db, guild_id).await.map_err(failed)?;
    let current = state.settings.get::<ModSettings>(&state.db, guild_id).await.map_err(failed)?;

    // 1) Doğrula.
    let mut settings = (*current).clone();
    let mut auto = legacy.settings.clone();
    auto.enabled = input.checked("enabled");
    auto.exempt_staff = input.checked("exempt_staff");
    for (rule, key, _, _) in Rule::ALL {
        let action = RuleAction::from_code(input.raw(&format!("rule_{key}"))).ok_or("hata")?;
        settings.actions.set(rule, action);
        let on = action != RuleAction::Off;
        match rule {
            Rule::BadWords => auto.filter_profanity = on,
            Rule::Invites => auto.block_invites = on,
            Rule::Links => auto.block_links = on,
            Rule::Spam => auto.anti_spam = on,
            _ => debug_assert!(!rule.is_legacy()),
        }
    }
    let small = |key: &str, min: u32, max: u32| input.number(key, min, max).and_then(|v| u16::try_from(v).map_err(|_| "sayi"));
    settings.caps_percent = u8::try_from(input.number("caps_percent", 30, 100)?).map_err(|_| "sayi")?;
    settings.caps_min_letters = small("caps_min_letters", 5, 200)?;
    settings.emoji_limit = small("emoji_limit", 1, 100)?;
    settings.spoiler_limit = small("spoiler_limit", 1, 50)?;
    settings.mention_limit = small("mention_limit", 1, 50)?;
    settings.repeat_limit = small("repeat_limit", 2, 20)?;
    auto.spam_max_messages = i32::try_from(input.number("spam_max_messages", 2, 30)?).map_err(|_| "sayi")?;
    auto.spam_window_secs = i32::try_from(input.number("spam_window_secs", 2, 60)?).map_err(|_| "sayi")?;

    let blocked = parse_words(input.raw("blocked_words"), WordList::Blocked)?;
    let allowed = parse_words(input.raw("allowed_words"), WordList::Allowed)?;
    settings.allowed_domains = clean_domains(input.raw("allowed_domains"));
    if settings.allowed_domains.len() > MAX_DOMAINS {
        return Err("limit");
    }
    settings.exempt_roles = input.ids("exempt_role", &input.options.all_roles, "rol")?;
    settings.quarantine_role = input.role("quarantine_role")?.map(RoleId::get);

    let removed: Vec<u64> = input.all("restricted_del").into_iter().map(|raw| raw.parse().map_err(|_| "kanal")).collect::<Result<_, _>>()?;
    settings.restricted.retain(|r| !removed.contains(&r.channel));
    if let Some(channel) = input.channel("restricted_channel")? {
        let kind = RestrictKind::from_code(input.raw("restricted_kind")).ok_or("hata")?;
        settings.restricted.retain(|r| r.channel != channel.get());
        settings.restricted.push(Restricted { channel: channel.get(), kind });
    }
    if settings.restricted.len() > MAX_RESTRICTED {
        return Err("limit");
    }
    settings.restricted_reply = input.checked("restricted_reply");
    settings.restricted_dm = input.checked("restricted_dm");
    settings.nick_filter = input.checked("nick_filter");
    settings.dehoist = input.checked("dehoist");
    let replacement = input.raw("nick_replacement").trim();
    if replacement.chars().count() > 32 {
        return Err("metin");
    }
    settings.nick_replacement = if replacement.is_empty() { "Üye".to_owned() } else { replacement.to_owned() };

    let punish_removed: Vec<i32> = input.all("punish_del").into_iter().map(|raw| raw.parse().map_err(|_| "sayi")).collect::<Result<_, _>>()?;
    let new_punishment = parse_punishment(input)?;
    let existing = moderation::punishments(&state.db, guild_id).await.map_err(failed)?;
    let remaining = existing.iter().filter(|r| !punish_removed.contains(&r.warn_count)).count();
    let adds_new = new_punishment.as_ref().is_some_and(|new| !existing.iter().any(|r| r.warn_count == new.warn_count && !punish_removed.contains(&r.warn_count)));
    if remaining + usize::from(adds_new) > MAX_PUNISHMENTS {
        return Err("limit");
    }

    // 2) Kaydet.
    config::save_settings(&state.db, guild_id, &auto).await.map_err(failed)?;
    sync_words(state, guild_id, WordList::Blocked, &legacy.custom_words, &blocked).await.map_err(failed)?;
    sync_words(state, guild_id, WordList::Allowed, &legacy.allowed_words, &allowed).await.map_err(failed)?;
    state.automod.refresh(&state.db, &state.redis, guild_id).await.map_err(failed)?;
    state.settings.set(&state.db, guild_id, settings).await.map_err(failed)?;
    for count in punish_removed {
        moderation::remove_punishment(&state.db, guild_id, count).await.map_err(failed)?;
    }
    if let Some(rule) = new_punishment {
        moderation::set_punishment(&state.db, guild_id, &rule).await.map_err(failed)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- Olay kayıtları

/// Olay kayıtları sayfasının alanları ve kaydın açık olup olmadığı.
pub async fn log_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let cfg = state.configs.get(&state.db, guild_id).await?;
    let settings = state.settings.get::<ModSettings>(&state.db, guild_id).await?;
    let mod_channel = state.modlog.channel(&state.db, guild_id).await?;

    let mut out = section(
        "Kayıt kanalları",
        field(
            "Olay kayıt kanalı",
            &options.channel_select("channel", cfg.event_log_channel),
            "Mesaj, üye, rol, ses, kanal ve sunucu olayları. Sadece yetkililerin gördüğü bir kanal seç.",
        ) + &field(
            "Moderasyon kayıt kanalı",
            &options.channel_select("mod_channel", mod_channel),
            "Yasaklama, atma, susturma, uyarı vakaları, otomatik moderasyon ve bilet kayıtları. Aynı kanal olabilir.",
        ),
    );

    let mut events = String::new();
    let mut group = "";
    for (key, group_name, label) in LOG_EVENTS {
        if group_name != group {
            if !group.is_empty() {
                events.push_str("</div>");
            }
            group = group_name;
            let _ = write!(events, "<h4>{}</h4><div class=\"checks\" style=\"max-height:none\">", escape(group));
        }
        let _ = write!(
            events,
            "<label class=\"check\"><input type=\"checkbox\" name=\"event\" value=\"{key}\"{}> {}</label>",
            if settings.log.logs(key) { " checked" } else { "" },
            escape(label)
        );
    }
    if !group.is_empty() {
        events.push_str("</div>");
    }
    out.push_str(&section("Kaydedilecek olaylar", events));

    out.push_str(&section(
        "Yok sayılan kanallar",
        "<p class=\"note\" style=\"margin:0 0 8px\">Bu kanallarda düzenlenen ve silinen mesajlar kaydedilmez.</p>".to_owned()
            + &checklist("ignored_channel", &options.channels, "# ", &settings.log.ignored_channels),
    ));
    out.push_str(&section(
        "Ek ayarlar",
        checkbox("skip_bots", settings.log.skip_bots, "Botların yaptığı işlemleri kaydetme")
            + &checkbox("hide_avatars", settings.log.hide_avatars, "Kayıtlarda profil resmi gösterme"),
    ));
    Ok((out, cfg.event_log_channel.is_some()))
}

pub async fn log_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = |e: anyhow::Error| {
        warn!(guild = %guild_id, error = ?e, "Panel: kayıt ayarı kaydedilemedi");
        "hata"
    };
    let channel = input.channel("channel")?;
    let mod_channel: Option<ChannelId> = input.channel("mod_channel")?;
    let enabled = input.all("event");
    if enabled.iter().any(|key| !LOG_EVENTS.iter().any(|(known, _, _)| known == key)) {
        return Err("hata");
    }
    let ignored = input.ids("ignored_channel", &input.options.channels, "kanal")?;

    let current = state.settings.get::<ModSettings>(&state.db, guild_id).await.map_err(failed)?;
    let mut settings = (*current).clone();
    settings.log.disabled = LOG_EVENTS.iter().map(|(key, _, _)| *key).filter(|key| !enabled.contains(key)).map(str::to_owned).collect();
    settings.log.ignored_channels = ignored;
    settings.log.skip_bots = input.checked("skip_bots");
    settings.log.hide_avatars = input.checked("hide_avatars");

    state.configs.set(&state.db, guild_id, Setting::EventLog(channel)).await.map_err(failed)?;
    state.modlog.set_channel(&state.db, guild_id, mod_channel).await.map_err(failed)?;
    state.settings.set(&state.db, guild_id, settings).await.map_err(failed)?;
    Ok(())
}

// ---------------------------------------------------------------- Discord AutoMod

/// "Discord AutoMod" sayfası: Discord'un kendi kuralları (bot kapalıyken de korur).
pub async fn native_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    use crate::nativeautomod::{self, NativeAutomodSettings};
    let settings = state.settings.get::<NativeAutomodSettings>(&state.db, guild_id).await?;
    let mut out = section(
        "Hazır filtreler (Discord'un kendi listeleri)",
        checkbox("profanity", settings.profanity, "Küfür ve argo")
            + &checkbox("sexual", settings.sexual, "Cinsel içerik")
            + &checkbox("slurs", settings.slurs, "Hakaret ve nefret söylemi")
            + "<p class=\"note\">Discord bu listeleri kendisi günceller (İngilizce ağırlıklıdır). Türkçe küfürler için aşağıdaki kelime listesini ya da "
            + "<b>Moderasyon</b> sayfasındaki botun kendi filtresini kullan.</p>",
    );
    out.push_str(&section(
        "Yasaklı kelimeler",
        field(
            "Kelimeler",
            &words_area("keywords", &settings.keywords, "her satıra bir kelime\n*reklam*  (içinde geçen)\nkumar*  (ile başlayan)"),
            "Satır ya da virgülle ayır; en fazla 1000 kelime, her biri en fazla 60 karakter. Mesaj Discord tarafından gönderilmeden engellenir.",
        ) + &field(
            "Yazanı kaç dakika sustur",
            &number_input("timeout_minutes", settings.timeout_minutes, 0, nativeautomod::MAX_TIMEOUT_MINUTES),
            "0 = susturma, yalnızca engelle. En fazla 4 hafta.",
        ),
    ));
    out.push_str(&section(
        "Toplu etiket ve spam",
        field(
            "Bir mesajda en fazla kaç etiket",
            &number_input("native_mentions", settings.mention_limit, 0, u32::from(nativeautomod::MAX_MENTIONS)),
            "0 = kapalı. Ör. 5: altıncı farklı kişiyi/rolü etiketleyen mesaj engellenir.",
        ) + &checkbox("native_spam", settings.spam, "Discord'un spam sezgisini aç (şüpheli bağlantı ve toplu mesajlar)"),
    ));
    out.push_str(&section(
        "Ortak ayarlar",
        field("Engellenen mesajlar şu kanala bildirilsin", &options.channel_select("alert_channel", settings.alert_channel.filter(|c| *c != 0).map(ChannelId::new)), "Boşsa bildirilmez.")
            + &field(
                "Üyeye gösterilecek açıklama",
                &format!(
                    "<input type=\"text\" name=\"block_message\" value=\"{}\" maxlength=\"{}\" placeholder=\"Bu mesaj sunucu kurallarına aykırı.\">",
                    escape(&settings.block_message),
                    nativeautomod::MAX_BLOCK_MESSAGE
                ),
                "En fazla 150 karakter.",
            )
            + &field("Muaf roller", &checklist("native_exempt", &options.all_roles, "@ ", &settings.exempt_roles), "Bu rollerdekilerin mesajları engellenmez.")
            + "<p class=\"note\">Kaydedince kurallar Discord'da <b>Sunucu Ayarları → AutoMod</b> altında “AxBot · …” adıyla oluşur. Botun "
            + "<b>Sunucuyu Yönet</b> izni olmalı (susturma için <b>Üyelere Zaman Aşımı Uygula</b> da). Discord sunucu başına yalnızca bir hazır filtre "
            + "ve bir spam kuralına izin verir: sunucuda zaten varsa önce onu kapat.</p>",
    ));
    Ok((out, settings.is_active()))
}

pub async fn native_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    use crate::nativeautomod::{self, NativeAutomodSettings};
    let block_message = input.raw("block_message").trim().to_owned();
    if block_message.chars().count() > nativeautomod::MAX_BLOCK_MESSAGE {
        return Err("metin");
    }
    let current = state.settings.get::<NativeAutomodSettings>(&state.db, guild_id).await.map_err(|e| {
        warn!(guild = %guild_id, error = ?e, "Panel: Discord AutoMod ayarı okunamadı");
        "hata"
    })?;
    let mut settings = NativeAutomodSettings {
        profanity: input.checked("profanity"),
        sexual: input.checked("sexual"),
        slurs: input.checked("slurs"),
        keywords: nativeautomod::parse_keywords(input.raw("keywords"))?,
        timeout_minutes: if input.raw("timeout_minutes").is_empty() { 0 } else { input.number("timeout_minutes", 0, nativeautomod::MAX_TIMEOUT_MINUTES)? },
        mention_limit: if input.raw("native_mentions").is_empty() {
            0
        } else {
            u8::try_from(input.number("native_mentions", 0, u32::from(nativeautomod::MAX_MENTIONS))?).map_err(|_| "sayi")?
        },
        spam: input.checked("native_spam"),
        alert_channel: input.channel("alert_channel")?.map(ChannelId::get),
        block_message,
        exempt_roles: input.ids("native_exempt", &input.options.all_roles, "rol")?,
        rule_ids: current.rule_ids,
    };
    let errors = nativeautomod::apply(&state.http, guild_id, &mut settings).await;
    // Kimlikler her durumda saklanır (bir kısmı oluşmuş olabilir).
    state.settings.set(&state.db, guild_id, settings).await.map_err(|e| {
        warn!(guild = %guild_id, error = ?e, "Panel: Discord AutoMod ayarı kaydedilemedi");
        "hata"
    })?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err("automod")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options { guild_name: "T".to_owned(), channels: vec![(10, "genel".to_owned())], categories: Vec::new(), roles: Vec::new(), all_roles: Vec::new() }
    }

    fn form(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn words_are_validated_and_deduplicated() {
        let words = parse_words("Kelime\nkelime, başka*\n\n", WordList::Blocked);
        assert_eq!(words.map(|w| w.into_iter().collect::<Vec<_>>()), Ok(vec!["başka*".to_owned(), "kelime".to_owned()]));
        assert_eq!(parse_words("a", WordList::Blocked), Err("kelime"));
        assert_eq!(parse_words("<script>", WordList::Blocked), Err("kelime"));
        // İstisnalarda * sadece sonda olabilir.
        assert_eq!(parse_words("*orta*", WordList::Allowed), Err("kelime"));
    }

    #[test]
    fn punishment_row_rules() {
        let options = options();
        let parse = |pairs: &[(&str, &str)]| {
            let form = form(pairs);
            parse_punishment(&Input { form: &form, options: &options }).map(|r| r.map(|r| (r.warn_count, r.punishment, r.duration, r.window)))
        };
        assert_eq!(parse(&[("punish_count", "")]), Ok(None));
        // Süresiz susturma 1 saat olur; atmada süre yok sayılır.
        assert_eq!(parse(&[("punish_count", "3"), ("punish_action", "timeout")]), Ok(Some((3, Punishment::Timeout, Some(Duration::from_secs(3600)), None))));
        assert_eq!(parse(&[("punish_count", "5"), ("punish_action", "kick"), ("punish_duration", "1sa")]), Ok(Some((5, Punishment::Kick, None, None))));
        assert_eq!(parse(&[("punish_count", "0"), ("punish_action", "ban")]), Err("sayi"));
        assert_eq!(parse(&[("punish_count", "2"), ("punish_action", "sil")]), Err("hata"));
        assert_eq!(parse(&[("punish_count", "2"), ("punish_action", "ban"), ("punish_duration", "yarın")]), Err("sure"));
        assert!(matches!(parse(&[("punish_count", "2"), ("punish_action", "ban"), ("punish_window", "7g")]), Ok(Some((2, Punishment::Ban, None, Some(_))))));
    }
}
