//! Yapay zekâ (kanallar + karakterler), bot profili, bot durumu ve sunucu istatistikleri sayfaları.

use std::fmt::Write as _;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use poise::serenity_prelude::{ChannelId, ChannelType, GuildId};
use tracing::{info, warn};

use super::pages::{forbidden, layout, GuildNav, Nav, Page};
use super::plugins::{field, section, Input, Options};
use super::{auth, can_manage};
use crate::ai::characters::{self, AiSettings, Character};
use crate::presence::{self, BotProfile, Presence, ProfileError};
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

/// Boşsa `None`; `max` karakterden uzunsa hata.
fn optional(input: &Input<'_>, key: &str, max: usize) -> Result<Option<String>, &'static str> {
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

// ---------------------------------------------------------------- Yapay zekâ

pub async fn ai_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<AiSettings>(&state.db, guild_id).await?;
    let channels = state.ai.channels(&state.db, guild_id).await?;
    let selected: Vec<u64> = channels.iter().map(|c| c.get()).collect();

    let provider = match &state.panel.ai {
        Some(description) => format!("<p class=\"note\">Sağlayıcı: {}</p>", escape(&description.replace('`', ""))),
        None => "<p class=\"note\">⚠️ Yapay zekâ anahtarı tanımlı değil: <code>.env</code> dosyasına <code>NVIDIA_API_KEY</code> ya da \
                 <code>GEMINI_API_KEY</code> yazılıp bot yeniden başlatılmalı. Ayarlar yine de kaydedilir.</p>"
            .to_owned(),
    };
    let mut out = section(
        "Sohbet kanalları",
        provider
            + &field(
                "Botun her mesaja cevap vereceği kanallar",
                &super::plugins::checklist("ai_channel", &options.channels, "# ", &selected),
                "Bu kanallara yazılan her mesaja yapay zekâ cevap verir. Kanaldan çıkarılan kanalın sohbet geçmişi silinir.",
            ),
    );

    out.push_str(&section(
        "Sunucu kişiliği",
        field(
            "Arka plan hikâyesi",
            &big_textarea("persona", settings.persona.as_deref().unwrap_or_default(), characters::MAX_PROMPT_LEN, 4, "Örn: Sen bu sunucunun neşeli maskotu Ax'sın; kısa ve esprili konuşursun."),
            "Boşsa bot yardımsever, sade bir asistandır. Kanala karakter seçilmişse o kanalda karakter geçerli olur. \
             Dil, biçim ve güvenlik kuralları her durumda korunur.",
        ),
    ));

    out.push_str(&section(
        "Bilgi bankası",
        field(
            "Sunucu hakkında bilgiler",
            &big_textarea(
                "knowledge",
                &settings.knowledge,
                characters::MAX_KNOWLEDGE,
                10,
                "Her maddeyi boş bir satırla ayır. Örn:\n\nKurallar: küfür ve reklam yasak; ilk uyarıdan sonra susturma.\n\nYetkili başvurusu her ayın 1'inde #başvuru kanalında açılır.",
            ),
            "Yapay zekâ sunucuyla ilgili sorularda önce buraya bakar ve burada olmayan sunucu bilgisini uydurmaz. Bilet asistanı da SSS ile \
             birlikte bunu kullanır. Uzun bankada soruyla ilgili maddeler seçilir (en fazla ~3000 karakter). Gizli bilgi yazma: üyeler soru sorarak görebilir.",
        ),
    ));

    out.push_str(&section(
        "Yapay zekâ moderasyonu",
        super::plugins::checkbox("moderation", settings.moderation, "Gizlenmiş küfürleri yakala (ör. “a.q”, “4mk”, “s*k”)")
            + "<p class=\"note\">Kelime listesinin kaçırdığı, harflerin arasına nokta / yıldız / rakam konmuş yazımlar yapay zekâya sorulur; açıkça küfürse mesaj silinir \
               ve olay kayıt kanalına yazılır. Yalnızca şüpheli görünen mesajlar sorulur (dakikada en fazla 15), emin olunamayan mesaj silinmez. \
               Otomatik moderasyondan muaf roller burada da muaftır.</p>",
    ));

    let choices = settings.choices();
    let mut per_channel = String::new();
    for (id, name) in options.channels.iter().filter(|(id, _)| selected.contains(id)) {
        let current = settings.key_of(*id).unwrap_or_default();
        let mut select = format!("<select name=\"char_{id}\"><option value=\"\">— Sunucu kişiliği —</option>");
        for (key, label) in &choices {
            let _ = write!(select, "<option value=\"{}\"{}>{}</option>", escape(key), if key == current { " selected" } else { "" }, escape(label));
        }
        select.push_str("</select>");
        per_channel.push_str(&field(&format!("# {name}"), &select, ""));
    }
    if per_channel.is_empty() {
        per_channel.push_str("<p class=\"note\">Önce yukarıdan bir sohbet kanalı seçip kaydet; sonra burada o kanalın karakterini seçebilirsin.</p>");
    }
    out.push_str(&section("Kanal karakterleri", per_channel + "<p class=\"note\">Karakter değişince o kanalın sohbet geçmişi sıfırlanır. Discord'dan: <code>/ai karakter</code>.</p>"));

    let mut list = String::new();
    if settings.characters.is_empty() {
        list.push_str("<p class=\"note\">Henüz özel karakter yok. Hazır karakterler her zaman seçilebilir.</p>");
    } else {
        list.push_str("<table><tr><th>Ad</th><th>Tanım</th><th>Sil</th></tr>");
        for character in &settings.characters {
            let short: String = character.prompt.chars().take(110).collect();
            let _ = write!(
                list,
                "<tr><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"char_del\" value=\"{}\"></td></tr>",
                escape(&character.name),
                escape(&short),
                character.id
            );
        }
        list.push_str("</table>");
    }
    out.push_str(&section(
        "Özel karakterler",
        list + &field("Yeni karakterin adı", &text_input("new_name", "", "Kaptan Zeki", characters::MAX_NAME_LEN), "Boş bırakırsan yeni karakter eklenmez.")
            + &field(
                "Karakterin tanımı",
                &big_textarea("new_prompt", "", characters::MAX_PROMPT_LEN, 4, "Kim olduğunu, nasıl konuştuğunu ve nelere dikkat ettiğini yaz."),
                "",
            ),
    ));
    Ok((out, !selected.is_empty()))
}

pub async fn ai_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "yapay zekâ ayarı");
    let wanted = input.ids("ai_channel", &input.options.channels, "kanal")?;
    let persona = optional(input, "persona", characters::MAX_PROMPT_LEN)?;
    let removed: Vec<u32> = input.all("char_del").into_iter().map(|raw| raw.parse().map_err(|_| "hata")).collect::<Result<_, _>>()?;
    let new_name = optional(input, "new_name", characters::MAX_NAME_LEN)?;
    let new_prompt = optional(input, "new_prompt", characters::MAX_PROMPT_LEN)?;

    let current = state.settings.get::<AiSettings>(&state.db, guild_id).await.map_err(&failed)?;
    let mut settings = (*current).clone();
    settings.persona = persona;
    settings.moderation = input.checked("moderation");
    let knowledge = input.raw("knowledge").replace("\r\n", "\n");
    if knowledge.chars().count() > characters::MAX_KNOWLEDGE {
        return Err("metin");
    }
    settings.knowledge = knowledge.trim().to_owned();
    settings.characters.retain(|c| !removed.contains(&c.id));
    match (new_name, new_prompt) {
        (Some(name), Some(prompt)) => {
            if settings.characters.len() >= characters::MAX_CHARACTERS {
                return Err("limit");
            }
            if settings.find_by_name(&name).is_some() {
                return Err("tekrar");
            }
            let id = current.next_id().max(settings.next_id());
            settings.characters.push(Character { id, name, prompt });
        }
        (None, None) => {}
        _ => return Err("bos"),
    }

    // Kanal karakterleri: yalnızca seçili sohbet kanalları için; anahtar var olan bir karakteri göstermeli.
    let mut reset: Vec<u64> = Vec::new();
    for channel in &wanted {
        let key = input.raw(&format!("char_{channel}"));
        let key = if key.is_empty() {
            None
        } else if settings.resolve(key).is_some() {
            Some(key.to_owned())
        } else if removed.iter().any(|id| key == format!("ozel:{id}")) {
            // Aynı kayıtta silinen karakter seçili kalmış: varsayılana döner.
            None
        } else {
            return Err("hata");
        };
        if settings.assign(*channel, key) {
            reset.push(*channel);
        }
    }
    settings.assignments.retain(|a| wanted.contains(&a.channel));
    settings.prune();

    let before = state.ai.channels(&state.db, guild_id).await.map_err(&failed)?;
    for channel in &wanted {
        if !before.contains(&ChannelId::new(*channel)) {
            state.ai.add_channel(&state.db, guild_id, ChannelId::new(*channel)).await.map_err(&failed)?;
        }
    }
    for channel in before.iter().filter(|c| !wanted.contains(&c.get())) {
        state.ai.remove_channel(&state.db, guild_id, *channel).await.map_err(&failed)?;
        reset.push(channel.get());
    }
    // Sunucu kişiliği değiştiyse karakteri olmayan kanallar da yeni kişilikle başlar.
    if current.persona != settings.persona {
        reset.extend(wanted.iter().copied().filter(|c| settings.key_of(*c).is_none()));
    }
    state.settings.set(&state.db, guild_id, settings).await.map_err(&failed)?;
    for channel in reset {
        if let Err(e) = crate::ai::clear_history(&state.redis, ChannelId::new(channel.max(1))).await {
            warn!(channel, error = %e, "Panel: sohbet geçmişi silinemedi");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- Bot profili

pub async fn profile_fields(state: &WebState, guild_id: GuildId) -> anyhow::Result<(String, bool)> {
    let profile = state.settings.get::<BotProfile>(&state.db, guild_id).await?;
    let out = section(
        "Botun bu sunucudaki görünümü",
        field(
            "Takma ad",
            &text_input("nick", profile.nick.as_deref().unwrap_or_default(), "Boşsa botun kendi adı", presence::MAX_NICK_LEN),
            "",
        ) + &field(
            "Hakkında",
            &big_textarea("bio", profile.bio.as_deref().unwrap_or_default(), presence::MAX_BIO_LEN, 3, "Botun bu sunucudaki profil yazısı"),
            "",
        ) + &field(
            "Avatar resmi (https adresi)",
            &text_input("avatar_url", profile.avatar_url.as_deref().unwrap_or_default(), "https://…/avatar.png", 400),
            "PNG, JPEG, GIF ya da WebP; en fazla 6 MB. Boş bırakırsan botun asıl avatarı kullanılır.",
        ) + &field(
            "Afiş resmi (https adresi)",
            &text_input("banner_url", profile.banner_url.as_deref().unwrap_or_default(), "https://…/afis.png", 400),
            "Önerilen boyut 680×240.",
        ) + "<p class=\"note\">Bu ayarlar yalnızca bu sunucuda geçerlidir. Botun çevrimiçi durumu ve etkinlik yazısı bütün \
             sunucularda aynıdır; bot sahibi <a href=\"/panel/sistem\">Sistem</a> sayfasından ayarlar. Discord avatar ve afişi \
             sık değiştirmeye izin vermez; art arda denersen bir süre reddedebilir.</p>",
    );
    Ok((out, profile.is_custom()))
}

fn image_url(input: &Input<'_>, key: &str) -> Result<Option<String>, &'static str> {
    let raw = input.raw(key);
    if raw.is_empty() {
        return Ok(None);
    }
    let ok = raw.starts_with("https://") && raw.chars().count() <= 400 && url::Url::parse(raw).is_ok_and(|u| u.host_str().is_some());
    ok.then(|| Some(raw.to_owned())).ok_or("adres")
}

pub async fn profile_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "bot profili");
    let new = BotProfile {
        nick: optional(input, "nick", presence::MAX_NICK_LEN)?,
        bio: optional(input, "bio", presence::MAX_BIO_LEN)?,
        avatar_url: image_url(input, "avatar_url")?,
        banner_url: image_url(input, "banner_url")?,
    };
    let old = state.settings.get::<BotProfile>(&state.db, guild_id).await.map_err(&failed)?;
    if *old == new {
        return Ok(());
    }
    presence::apply_profile(&state.http, guild_id, &old, &new).await.map_err(|e| match e {
        ProfileError::Image => "adres",
        ProfileError::Discord => "discord",
    })?;
    state.settings.set(&state.db, guild_id, new).await.map_err(&failed)
}

// ---------------------------------------------------------------- Bot durumu (bot sahibi)

/// Sistem sayfasındaki durum formu.
pub async fn presence_form(state: &WebState, csrf: &str, flash: Option<&str>) -> String {
    let current = presence::load(&state.redis).await.unwrap_or_default();
    let select = |name: &str, items: &[(&str, &str)], selected: &str| {
        let mut out = format!("<select name=\"{name}\">");
        for (key, label) in items {
            let _ = write!(out, "<option value=\"{key}\"{}>{label}</option>", if *key == selected { " selected" } else { "" });
        }
        out + "</select>"
    };
    let message = match flash {
        Some("kaydedildi") => "<div class=\"flash ok\">✅ Durum güncellendi.</div>",
        Some("gecersiz") => "<div class=\"flash err\">❌ Geçersiz değer: etkinlik seçtiysen metin yazmalısın (en fazla 100 karakter).</div>",
        Some("hata") => "<div class=\"flash err\">❌ Kaydedilemedi; biraz sonra tekrar dene.</div>",
        _ => "",
    };
    format!(
        "{message}<form method=\"post\" action=\"/panel/sistem/durum\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">{status}{activity}{text}\
         <div class=\"savebar\"><button>Durumu kaydet</button></div></form>\
         <p class=\"note\">Bütün sunucularda aynı görünür ve bot yeniden başlasa da korunur.</p>",
        csrf = escape(csrf),
        status = field("Durum", &select("status", presence::STATUSES, &current.status), ""),
        activity = field("Etkinlik türü", &select("activity", presence::ACTIVITIES, &current.activity), ""),
        text = field("Etkinlik metni", &text_input("text", &current.text, "/yardım · axbot", presence::MAX_ACTIVITY_LEN), ""),
    )
}

pub async fn presence_save(State(state): State<WebState>, headers: HeaderMap, Form(form): Form<Vec<(String, String)>>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let get = |key: &str| form.iter().find(|(k, _)| k == key).map_or("", |(_, v)| v.as_str());
    if !state.panel.is_owner(session.user) || get("csrf") != session.csrf {
        return forbidden();
    }
    let back = |code: &str| Redirect::to(&format!("/panel/sistem?durum={code}")).into_response();
    let Some(new) = Presence::parse(get("status"), get("activity"), get("text")) else {
        return back("gecersiz");
    };
    if let Err(e) = presence::save(&state.redis, &new).await {
        warn!(error = ?e, "Panel: bot durumu kaydedilemedi");
        return back("hata");
    }
    if let Some(shards) = state.panel.shards.get() {
        presence::apply(shards, &new).await;
    }
    info!(user = %session.user, status = %new.status, activity = %new.activity, "Panelden bot durumu değiştirildi");
    back("kaydedildi")
}

// ---------------------------------------------------------------- İstatistikler

/// Sunucuya ait kayıt sayıları. Sorgular sabittir; yalnızca sunucu kimliği bağlanır.
const COUNTS: &[(&str, &str)] = &[
    ("Moderasyon vakası", "SELECT COUNT(*) FROM mod_cases WHERE guild_id = $1"),
    ("Açılan bilet", "SELECT COUNT(*) FROM tickets WHERE guild_id = $1"),
    ("XP kazanan üye", "SELECT COUNT(*) FROM member_xp WHERE guild_id = $1"),
    ("Kazanılan başarım", "SELECT COUNT(*) FROM member_achievements WHERE guild_id = $1"),
    ("Çekiliş", "SELECT COUNT(*) FROM giveaways WHERE guild_id = $1"),
    ("Öneri", "SELECT COUNT(*) FROM suggestions WHERE guild_id = $1"),
    ("Starboard gönderisi", "SELECT COUNT(*) FROM starboard_posts WHERE guild_id = $1"),
    ("Davetle katılım", "SELECT COUNT(*) FROM invite_joins WHERE guild_id = $1"),
    ("Ekonomi hesabı", "SELECT COUNT(*) FROM economy_accounts WHERE guild_id = $1"),
    ("Kayıtlı doğum günü", "SELECT COUNT(*) FROM birthdays WHERE guild_id = $1"),
    ("Etiket", "SELECT COUNT(*) FROM tags WHERE guild_id = $1"),
    ("Yakalanan hayvan", "SELECT COUNT(*) FROM animal_catches WHERE guild_id = $1"),
];

fn stat(label: &str, value: &str) -> String {
    format!("<div class=\"pcard\"><div class=\"note\">{}</div><div class=\"ptitle\" style=\"font-size:22px\">{}</div></div>", escape(label), escape(value))
}

pub async fn stats(State(state): State<WebState>, headers: HeaderMap, Path(guild): Path<u64>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await {
        return forbidden();
    }
    // Önbellekten okunanlar (kilit hemen bırakılır).
    let Some((name, cards)) = state.cache.guild(guild_id).map(|g| {
        let bots = g.members.values().filter(|m| m.user.bot).count() as u64;
        let kind = |k: ChannelType| g.channels.values().filter(|c| c.kind == k).count();
        let in_voice = g.voice_states.values().filter(|v| v.channel_id.is_some()).count();
        let cards = [
            ("Üye", g.member_count.to_string()),
            ("İnsan", g.member_count.saturating_sub(bots).to_string()),
            ("Bot", bots.to_string()),
            ("Seste", in_voice.to_string()),
            ("Metin kanalı", (kind(ChannelType::Text) + kind(ChannelType::News)).to_string()),
            ("Ses kanalı", (kind(ChannelType::Voice) + kind(ChannelType::Stage)).to_string()),
            ("Rol", g.roles.len().saturating_sub(1).to_string()),
            ("Emoji", g.emojis.len().to_string()),
            ("Takviye", g.premium_subscription_count.unwrap_or(0).to_string()),
        ]
        .iter()
        .map(|(label, value)| stat(label, value))
        .collect::<String>();
        (g.name.clone(), cards)
    }) else {
        return forbidden();
    };

    let mut records = String::new();
    let Ok(id) = i64::try_from(guild_id.get()) else {
        return forbidden();
    };
    for (label, sql) in COUNTS {
        let value = match sqlx::query_scalar::<_, i64>(*sql).bind(id).fetch_one(&state.db).await {
            Ok(count) => count.to_string(),
            Err(e) => {
                warn!(guild = %guild_id, error = %e, "Panel: istatistik okunamadı");
                "—".to_owned()
            }
        };
        records.push_str(&stat(label, &value));
    }
    let total_xp: Option<i64> = sqlx::query_scalar("SELECT COALESCE(SUM(xp), 0)::BIGINT FROM member_xp WHERE guild_id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await
        .ok();
    if let Some(total) = total_xp {
        records.push_str(&stat("Toplam XP", &total.to_string()));
    }

    // Son 14 günün mesaj etkinliği (Redis sayaçları).
    let (daily, top_channels) = crate::community::activity(&state.redis, guild_id, 14).await;
    let peak = daily.iter().map(|(_, n)| *n).max().unwrap_or(0).max(1);
    let mut chart = String::from("<div class=\"chart\">");
    for (day, count) in &daily {
        // Gün damgası → "gg.aa" (UTC).
        let date = chrono::DateTime::from_timestamp(day * 86_400, 0).map_or_else(String::new, |d| d.format("%d.%m").to_string());
        let _ = write!(
            chart,
            "<div class=\"col\"><b>{count}</b><div class=\"bar\" style=\"height:{}%\"></div><small>{date}</small></div>",
            (count * 100 / peak).min(100)
        );
    }
    chart.push_str("</div>");
    let mut channels = String::new();
    if top_channels.is_empty() {
        channels.push_str("<p class=\"note\">Henüz kayıt yok; sayım bu sürümle birlikte başladı.</p>");
    } else {
        // Kanal adları önbellekten; kilit hemen bırakılır.
        let names: Vec<String> = {
            let guild = state.cache.guild(guild_id);
            top_channels
                .iter()
                .map(|(id, _)| guild.as_ref().and_then(|g| g.channels.get(&ChannelId::new((*id).max(1))).map(|c| c.name.clone())).unwrap_or_else(|| "silinmiş kanal".to_owned()))
                .collect()
        };
        channels.push_str("<table><tr><th>Kanal</th><th>Mesaj (14 gün)</th></tr>");
        for ((_, count), name) in top_channels.iter().zip(names) {
            let _ = write!(channels, "<tr><td># {}</td><td>{count}</td></tr>", escape(&name));
        }
        channels.push_str("</table>");
    }

    // Üye giriş / çıkışı (son 14 gün).
    let flow = crate::community::flow(&state.redis, guild_id, 14).await;
    let flow_peak = flow.iter().map(|(_, joined, left)| (*joined).max(*left)).max().unwrap_or(0).max(1);
    let (total_in, total_out) = flow.iter().fold((0, 0), |(a, b), (_, joined, left)| (a + joined, b + left));
    let mut members_chart = String::from("<div class=\"chart\">");
    for (day, joined, left) in &flow {
        let date = chrono::DateTime::from_timestamp(day * 86_400, 0).map_or_else(String::new, |d| d.format("%d.%m").to_string());
        let _ = write!(
            members_chart,
            "<div class=\"col\"><b>+{joined} / −{left}</b><div style=\"display:flex;gap:2px;align-items:flex-end;height:100%;width:100%;max-width:38px\">\
             <div class=\"bar\" style=\"height:{}%;background:var(--ok)\"></div><div class=\"bar\" style=\"height:{}%;background:var(--err)\"></div></div><small>{date}</small></div>",
            (joined * 100 / flow_peak).min(100),
            (left * 100 / flow_peak).min(100)
        );
    }
    let _ = write!(members_chart, "</div><p class=\"note\">Yeşil: katılan, kırmızı: ayrılan. 14 günde <b>+{total_in}</b> / <b>−{total_out}</b> (net {}).</p>", total_in as i64 - total_out as i64);

    // Kalıcılık: 7-14 gün önce katılanların kaçı hâlâ sunucuda.
    let (stayed, joined_then) = crate::community::retention(&state.redis, guild_id).await;
    let _ = write!(
        members_chart,
        "<p class=\"note\">Kalıcılık: 7-14 gün önce katılan <b>{joined_then}</b> üyeden <b>{stayed}</b> tanesi hâlâ sunucuda{}.</p>",
        (stayed * 100).checked_div(joined_then).map_or_else(|| " (henüz yeterli veri yok)".to_owned(), |percent| format!(" (%{percent})"))
    );

    // En aktif saatler (haftanın günü × saat, Türkiye saati).
    let grid = crate::community::heatmap(&state.redis, guild_id).await;
    let hottest = grid.iter().flatten().copied().max().unwrap_or(0).max(1);
    let mut heat = String::from("<div class=\"heat\"><div></div>");
    for hour in 0..24 {
        let _ = write!(heat, "<small>{hour:02}</small>");
    }
    for (name, row) in ["Pzt", "Sal", "Çar", "Per", "Cum", "Cmt", "Paz"].iter().zip(grid.iter()) {
        let _ = write!(heat, "<small>{name}</small>");
        for count in row {
            let _ = write!(heat, "<i title=\"{count} mesaj\" style=\"opacity:{:.2}\"></i>", 0.08 + 0.92 * (*count as f64 / hottest as f64));
        }
    }
    heat.push_str("</div><p class=\"note\">Koyu kareler en çok mesaj yazılan saatlerdir (Türkiye saati). Sayım bu sürümle başladı; zamanla dolar.</p>");

    let body = format!(
        "<p><a href=\"/panel/sunucu/{guild_id}\">← {name}</a></p><h1>📊 İstatistikler</h1>\
         <p class=\"sub\">Sunucunun şu anki durumu ve botun bu sunucuda tuttuğu kayıtlar</p>\
         <h2>Sunucu</h2><div class=\"pcards\">{cards}</div><h2>Günlük mesaj sayısı (son 14 gün)</h2>{chart}\
         <h2>En hareketli kanallar</h2>{channels}<h2>Üye giriş / çıkışı (son 14 gün)</h2>{members_chart}\
         <h2>En aktif saatler</h2>{heat}<h2>Bot kayıtları</h2><div class=\"pcards\">{records}</div>\
         <p class=\"note\">Sıralama için: <a href=\"/panel/sunucu/{guild_id}/siralama\">XP sıralaması</a>.</p>",
        name = escape(&name),
    );
    let nav = GuildNav { id: guild_id, name: &name, active: "istatistik" };
    Html(layout(&state, &session, Nav::Home, Some(nav), Page { title: "İstatistikler", body }).await).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_queries_are_scoped_to_one_guild() {
        for (label, sql) in COUNTS {
            assert!(sql.starts_with("SELECT COUNT(*) FROM ") && sql.ends_with(" WHERE guild_id = $1"), "{label}");
        }
    }
}
