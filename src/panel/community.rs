//! Topluluk araçları, panel ayarları (bot yöneticisi rolleri), yayın / YouTube duyuruları,
//! otomatik cevaplar ve davet sıralaması sayfaları.

use std::fmt::Write as _;

use poise::serenity_prelude::{ChannelId, GuildId, RoleId};
use tracing::warn;

use super::plugins::{checkbox, checklist, field, section, Input, Options};
use crate::autoresponder::{self, MatchMode};
use crate::community::{self, CommunitySettings, Sticky};
use crate::streams::{self, Platform};
use crate::web::{escape, WebState};
use crate::youtube;

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

fn channel_name(options: &Options, id: u64) -> String {
    options.channels.iter().find(|(known, _)| *known == id).map_or_else(|| "(silinmiş kanal)".to_owned(), |(_, name)| format!("# {name}"))
}

fn preview(text: &str, max: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= max { flat } else { flat.chars().take(max).collect::<String>() + "…" }
}

fn failed(guild_id: GuildId, what: &'static str) -> impl Fn(anyhow::Error) -> &'static str {
    move |e| {
        warn!(guild = %guild_id, error = ?e, "Panel: {what} kaydedilemedi");
        "hata"
    }
}

fn channel_opt(options: &Options, name: &str, selected: Option<u64>) -> String {
    Options::select(name, &options.channels, "# ", selected, "— Kapalı —")
}

// ---------------------------------------------------------------- Topluluk araçları

pub async fn community_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<CommunitySettings>(&state.db, guild_id).await?;

    let mut list = String::new();
    if settings.sticky.is_empty() {
        list.push_str("<p class=\"note\">Henüz sabit mesaj yok.</p>");
    } else {
        list.push_str("<table><tr><th>Kanal</th><th>Mesaj</th><th>Sil</th></tr>");
        for sticky in &settings.sticky {
            let _ = write!(
                list,
                "<tr><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"sticky_del\" value=\"{}\"></td></tr>",
                escape(&channel_name(options, sticky.channel)),
                escape(&preview(&sticky.text, 90)),
                sticky.channel
            );
        }
        list.push_str("</table>");
    }
    let mut out = section(
        "Sabit mesajlar",
        list + &field("Kanal", &channel_opt(options, "sticky_channel", None), "Boş bırakırsan yeni sabit mesaj eklenmez. Kanalda zaten varsa metni değişir.")
            + &field(
                "Mesaj",
                &big_textarea("sticky_text", "", community::MAX_STICKY_LEN, 3, "Kuralları okumadan yazmayın!"),
                "Kanala her birkaç mesajda bir yeniden gönderilir, eskisi silinir; böylece hep en altta kalır. Kimseyi etiketlemez.",
            ),
    );
    out.push_str(&section(
        "Sayma oyunu",
        field(
            "Sayma kanalı",
            &channel_opt(options, "counting_channel", settings.counting_channel),
            "Üyeler sırayla 1, 2, 3… yazar. Yanlış sayı ya da aynı kişinin art arda yazması sayımı sıfırlar; rekor tutulur.",
        ),
    ));
    out.push_str(&section(
        "Kelime zinciri",
        field(
            "Kelime zinciri kanalı",
            &channel_opt(options, "word_chain_channel", settings.word_chain_channel),
            "Her üye, bir öncekinin son harfiyle başlayan tek bir kelime yazar. Aynı kişi art arda yazamaz, kullanılmış kelime tekrar yazılamaz, \
             ğ ile biten kelime kabul edilmez. Kanalı değiştirirsen zincir sıfırlanır. Kelimelerin sözlükte olup olmadığı denetlenmez.",
        ),
    ));
    out.push_str(&section(
        "İtiraflar",
        field("İtiraf kanalı", &channel_opt(options, "confession_channel", settings.confession_channel), "Üyeler <code>/itiraf</code> ile adları görünmeden yazar.")
            + &field(
                "Yazarın bildirileceği yetkili kanalı",
                &channel_opt(options, "confession_log_channel", settings.confession_log_channel),
                "Seçersen her itirafın kimden geldiği bu kanala yazılır (kötüye kullanıma karşı); üyeye de yetkililerin görebildiği söylenir. \
                 Boşsa yazar hiçbir yere kaydedilmez.",
            ),
    ));
    out.push_str(&section(
        "Üyeler",
        checkbox("persist_roles", settings.persist_roles, "Kalıcı roller: sunucudan ayrılıp 30 gün içinde dönen üyeye eski rollerini geri ver")
            + "<p class=\"note\">Yetki içeren, bota ait ya da botun rolünün üstündeki roller geri verilmez.</p>"
            + &field(
                "Ses kanalında dakika başına XP",
                &format!("<input type=\"number\" name=\"voice_xp\" value=\"{}\" min=\"0\" max=\"{}\">", settings.voice_xp_per_min, community::MAX_VOICE_XP),
                "0 = kapalı. XP, üye ses kanalından çıkınca toplu eklenir (tek seferde en fazla 10 saatlik).",
            )
            + "<p class=\"note\">AFK: üyeler <code>/afk</code> yazar; ayar gerekmez.</p>",
    ));
    out.push_str(&section(
        "Klip kanalı (AxT Capture)",
        field(
            "Klip kanalı",
            &channel_opt(options, "clip_channel", settings.clip_channel),
            "Bu kanala gönderilen her video klip sayılır ve 🎬 ile işaretlenir; sıralama: <code>/eğlence klipler</code>.",
        ) + "<p class=\"note\"><b>AxT Capture'dan tek tıkla göndermek için:</b> Discord'da klip kanalı → Kanalı Düzenle → Entegrasyonlar → Webhook'lar → \
             Yeni Webhook; adını kendi adın yap (sıralamada bu ad görünür), adresini kopyala ve AxT Capture → Ayarlar → Discord webhook alanına yapıştır. \
             Her üye kendi adıyla ayrı bir webhook oluşturursa sıralamada ayrı sayılır. Discord'a elle yüklenen videolar da üyenin kendi adına sayılır.</p>",
    ));
    out.push_str(&section(
        "Sezon ödülleri",
        field(
            "Kazananların duyurulacağı kanal",
            &channel_opt(options, "season_channel", settings.season_channel),
            "Her Pazartesi haftanın, her ayın 1'inde ayın en çok XP kazanan ilk üç üyesi burada duyurulur. Boşsa duyuru yapılmaz.",
        ) + &field(
            "Birincinin ödülü (🪙)",
            &format!("<input type=\"number\" name=\"season_reward\" value=\"{}\" min=\"0\" max=\"{}\">", settings.season_reward, community::MAX_SEASON_REWARD),
            "İkinci yarısını, üçüncü çeyreğini alır. 0 = yalnızca duyuru.",
        ),
    ));
    Ok((out, settings.is_active()))
}

pub async fn community_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "topluluk ayarı");
    let removed = input.all("sticky_del").into_iter().map(|raw| raw.parse::<u64>().map_err(|_| "hata")).collect::<Result<Vec<_>, _>>()?;
    let new_sticky = match input.channel("sticky_channel")? {
        Some(channel) => {
            let text = input.raw("sticky_text");
            if text.is_empty() {
                return Err("bos");
            }
            if text.chars().count() > community::MAX_STICKY_LEN {
                return Err("metin");
            }
            Some(Sticky { channel: channel.get(), text: text.to_owned() })
        }
        None => None,
    };
    let counting_channel = input.channel("counting_channel")?.map(ChannelId::get);
    let word_chain_channel = input.channel("word_chain_channel")?.map(ChannelId::get);
    let confession_channel = input.channel("confession_channel")?.map(ChannelId::get);
    let confession_log_channel = input.channel("confession_log_channel")?.map(ChannelId::get);
    let voice_xp_per_min = input.number("voice_xp", 0, community::MAX_VOICE_XP)?;
    let season_channel = input.channel("season_channel")?.map(ChannelId::get);
    let clip_channel = input.channel("clip_channel")?.map(ChannelId::get);
    let season_reward = if input.raw("season_reward").is_empty() { 0 } else { input.number("season_reward", 0, community::MAX_SEASON_REWARD)? };

    let current = state.settings.get::<CommunitySettings>(&state.db, guild_id).await.map_err(&failed)?;
    let mut settings = (*current).clone();
    settings.sticky.retain(|s| !removed.contains(&s.channel));
    if let Some(sticky) = new_sticky {
        settings.sticky.retain(|s| s.channel != sticky.channel);
        if settings.sticky.len() >= community::MAX_STICKY {
            return Err("limit");
        }
        settings.sticky.push(sticky);
    }
    settings.counting_channel = counting_channel;
    if settings.word_chain_channel != word_chain_channel {
        community::reset_word_chain(&state.redis, guild_id).await;
    }
    settings.word_chain_channel = word_chain_channel;
    settings.confession_channel = confession_channel;
    settings.confession_log_channel = confession_log_channel;
    settings.persist_roles = input.checked("persist_roles");
    settings.voice_xp_per_min = voice_xp_per_min;
    settings.season_channel = season_channel;
    settings.clip_channel = clip_channel;
    settings.season_reward = season_reward;
    state.settings.set(&state.db, guild_id, settings).await.map_err(&failed)
}

// ---------------------------------------------------------------- Panel ayarları

pub async fn settings_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let settings = state.settings.get::<CommunitySettings>(&state.db, guild_id).await?;
    let out = section(
        "Bot yöneticisi rolleri",
        field(
            "Paneli kullanabilen ek roller",
            &checklist("manager_role", &options.all_roles, "@ ", &settings.manager_roles),
            "Sunucu sahibi ile 'Yönetici' ya da 'Sunucuyu Yönet' izni olanlar paneli zaten kullanabilir. Buradan seçtiğin rollerdeki üyeler de \
             bu sunucunun bütün panel ayarlarını değiştirebilir; yalnızca güvendiğin rolleri seç.",
        ) + "<p class=\"note\">Değişiklik en geç bir dakika içinde geçerli olur. Slash komutların izinleri Discord'dan ayarlanır: \
             Sunucu Ayarları → Entegrasyonlar → AxBot.</p>",
    );
    Ok((out, !settings.manager_roles.is_empty()))
}

pub async fn settings_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "panel ayarı");
    let manager_roles = input.ids("manager_role", &input.options.all_roles, "rol")?;
    if manager_roles.len() > 15 {
        return Err("limit");
    }
    let current = state.settings.get::<CommunitySettings>(&state.db, guild_id).await.map_err(&failed)?;
    let mut settings = (*current).clone();
    settings.manager_roles = manager_roles;
    state.settings.set(&state.db, guild_id, settings).await.map_err(&failed)
}

// ---------------------------------------------------------------- Yayın ve video duyuruları

pub async fn stream_fields(state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let videos = youtube::list(&state.db, guild_id).await?;
    let lives = streams::list(&state.db, guild_id).await?;

    let mut list = String::new();
    if videos.is_empty() && lives.is_empty() {
        list.push_str("<p class=\"note\">Henüz takip edilen kanal yok.</p>");
    } else {
        list.push_str("<table><tr><th>Platform</th><th>Kanal</th><th>Duyuru kanalı</th><th>Sil</th></tr>");
        for sub in &videos {
            let _ = write!(
                list,
                "<tr><td>YouTube</td><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"sub_del\" value=\"youtube:{}\"></td></tr>",
                escape(&sub.name),
                escape(&channel_name(options, sub.channel_id.get())),
                escape(&sub.yt_channel_id)
            );
        }
        for sub in &lives {
            let _ = write!(
                list,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"sub_del\" value=\"{}:{}\"></td></tr>",
                sub.platform.label(),
                escape(&sub.login),
                escape(&channel_name(options, sub.channel_id.get())),
                sub.platform.as_str(),
                escape(&sub.login)
            );
        }
        list.push_str("</table>");
    }
    let mut out = section("Takip edilen kanallar", list);

    let note = |platform: Platform| if state.streams.is_configured(platform) { "" } else { " (anahtar yok)" };
    let platforms = format!(
        "<select name=\"sub_platform\"><option value=\"youtube\">YouTube — yeni video</option>\
         <option value=\"twitch\">Twitch — canlı yayın{}</option><option value=\"kick\">Kick — canlı yayın{}</option></select>",
        note(Platform::Twitch),
        note(Platform::Kick)
    );
    out.push_str(&section(
        "Yeni kanal",
        field("Platform", &platforms, "")
            + &field(
                "Kanal",
                &text_input("sub_source", "", "@kanal · https://youtube.com/@kanal · twitch kullanıcı adı", 200),
                "Boş bırakırsan yeni kanal eklenmez.",
            )
            + &field("Duyuru kanalı", &channel_opt(options, "sub_channel", None), "")
            + &field("Etiketlenecek rol", &Options::select("sub_role", &options.all_roles, "@ ", None, "— Yok —"), "")
            + "<p class=\"note\">YouTube anahtar gerektirmez (5 dakikada bir kontrol). Twitch ve Kick için bot sahibi <code>.env</code> dosyasına \
               <code>TWITCH_CLIENT_ID</code> / <code>KICK_CLIENT_ID</code> ve gizli anahtarlarını yazmalı. Reddit, Bluesky, RSS ve podcast için \
               “Sosyal akışlar” sayfasını kullan.</p>",
    ));
    Ok((out, !videos.is_empty() || !lives.is_empty()))
}

pub async fn stream_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "yayın duyurusu");
    // 1) Doğrula.
    let mut removals: Vec<(String, String)> = Vec::new();
    for raw in input.all("sub_del") {
        let (platform, id) = raw.split_once(':').ok_or("hata")?;
        if !matches!(platform, "youtube" | "twitch" | "kick") || id.is_empty() || id.len() > 64 {
            return Err("hata");
        }
        removals.push((platform.to_owned(), id.to_owned()));
    }
    let source = input.raw("sub_source");
    let addition = if source.is_empty() {
        None
    } else {
        if source.chars().count() > 200 {
            return Err("metin");
        }
        let platform = input.raw("sub_platform").to_owned();
        if !matches!(platform.as_str(), "youtube" | "twitch" | "kick") {
            return Err("hata");
        }
        let channel = input.channel("sub_channel")?.ok_or("kanal")?;
        let role = input.id("sub_role", &input.options.all_roles, "rol")?.map(RoleId::new);
        Some((platform, channel, role))
    };

    // 2) Uygula.
    for (platform, id) in removals {
        match Platform::from_db(&platform) {
            Some(platform) => streams::remove(&state.db, guild_id, platform, &id).await.map(|_| ()).map_err(&failed)?,
            None => youtube::remove(&state.db, guild_id, &id).await.map(|_| ()).map_err(&failed)?,
        }
    }
    let Some((platform, channel_id, role_id)) = addition else {
        return Ok(());
    };
    match Platform::from_db(&platform) {
        None => {
            if youtube::count(&state.db, guild_id).await.map_err(&failed)? >= youtube::MAX_PER_GUILD {
                return Err("limit");
            }
            let yt_channel_id = youtube::resolve_channel(&state.http_client, source).await.map_err(|_| "kaynak")?;
            let feed = youtube::fetch_feed(&state.http_client, &yt_channel_id).await.map_err(|_| "kaynak")?;
            let sub = youtube::Subscription { yt_channel_id, name: feed.channel_name.chars().take(100).collect(), channel_id, role_id };
            youtube::add(&state.db, guild_id, &sub, feed.videos.first().map(|v| v.id.as_str())).await.map_err(&failed)
        }
        Some(platform) => {
            if !state.streams.is_configured(platform) {
                return Err("anahtar");
            }
            if streams::count(&state.db, guild_id).await.map_err(&failed)? >= streams::MAX_SUBSCRIPTIONS_PER_GUILD {
                return Err("limit");
            }
            let login = streams::normalize_login(source).ok_or("kaynak")?;
            match state.streams.resolve(platform, &login).await {
                Ok(Some(_)) => {}
                _ => return Err("kaynak"),
            }
            streams::upsert(&state.db, guild_id, &streams::Subscription { platform, login, channel_id, role_id }).await.map_err(&failed)
        }
    }
}

// ---------------------------------------------------------------- Otomatik cevaplar

const MODES: [(MatchMode, &str); 3] = [
    (MatchMode::Word, "Kelime olarak geçerse (önerilen)"),
    (MatchMode::Exact, "Mesajın tamamı buysa"),
    (MatchMode::Starts, "Mesaj bununla başlıyorsa"),
];

pub async fn reply_fields(state: &WebState, guild_id: GuildId) -> anyhow::Result<(String, bool)> {
    let rules = state.autoresponder.rules(&state.db, guild_id).await?;
    let mut list = String::new();
    if rules.is_empty() {
        list.push_str("<p class=\"note\">Henüz otomatik cevap yok.</p>");
    } else {
        list.push_str("<table><tr><th>Tetikleyici</th><th>Eşleşme</th><th>Cevap</th><th>Sil</th></tr>");
        for rule in rules.iter() {
            let _ = write!(
                list,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td><input type=\"checkbox\" name=\"reply_del\" value=\"{}\"></td></tr>",
                escape(&rule.trigger),
                rule.mode.label(),
                escape(&preview(&rule.response, 90)),
                rule.id
            );
        }
        list.push_str("</table>");
    }
    let mut modes = String::from("<select name=\"reply_mode\">");
    for (mode, label) in MODES {
        let _ = write!(modes, "<option value=\"{}\">{label}</option>", mode.db());
    }
    modes.push_str("</select>");
    let mut out = section("Otomatik cevaplar", list);
    out.push_str(&section(
        "Yeni otomatik cevap",
        field("Tetikleyici", &text_input("reply_trigger", "", "selam", 100), "Boş bırakırsan yeni cevap eklenmez. Aynı tetikleyici varsa cevabı değişir.")
            + &field("Eşleşme", &modes, "")
            + &field(
                "Cevap",
                &big_textarea("reply_text", "", 1500, 3, "Aleyküm selam {kullanıcı}!"),
                "<code>{kullanıcı}</code> yazanı etiketler. Kanal başına 5 saniyede bir cevap verilir.",
            ),
    ));
    Ok((out, !rules.is_empty()))
}

pub async fn reply_save(state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let failed = failed(guild_id, "otomatik cevap");
    let removed = input.all("reply_del").into_iter().map(|raw| raw.parse::<i64>().map_err(|_| "hata")).collect::<Result<Vec<_>, _>>()?;
    let trigger = autoresponder::normalize(input.raw("reply_trigger"));
    let raw_trigger_given = !input.raw("reply_trigger").is_empty();
    let addition = if raw_trigger_given {
        let response = input.text("reply_text")?.ok_or("bos")?;
        if trigger.is_empty() || trigger.chars().count() > 100 {
            return Err("metin");
        }
        let mode = MODES.iter().find(|(mode, _)| mode.db() == input.raw("reply_mode")).map(|(mode, _)| *mode).ok_or("hata")?;
        Some((response, mode))
    } else {
        None
    };

    let rules = state.autoresponder.rules(&state.db, guild_id).await.map_err(&failed)?;
    let remaining = rules.iter().filter(|r| !removed.contains(&r.id)).count();
    if addition.is_some() && !rules.iter().any(|r| r.trigger == trigger) && remaining >= autoresponder::MAX_RULES {
        return Err("limit");
    }
    for rule in rules.iter().filter(|r| removed.contains(&r.id)) {
        state.autoresponder.remove(&state.db, guild_id, &rule.trigger).await.map_err(&failed)?;
    }
    if let Some((response, mode)) = addition {
        let author = state.cache.current_user().id;
        state.autoresponder.set(&state.db, guild_id, &trigger, &response, mode, author).await.map_err(&failed)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- Davet takibi

pub async fn invite_fields(state: &WebState, guild_id: GuildId) -> anyhow::Result<(String, bool)> {
    let top = crate::invites::leaderboard(&state.db, guild_id, 25).await?;
    let mut list = String::new();
    if top.is_empty() {
        list.push_str("<p class=\"note\">Henüz kayıtlı davet yok. Bot, sunucuya katılan üyenin hangi davetle geldiğini kendiliğinden kaydeder.</p>");
    } else {
        list.push_str("<table><tr><th>#</th><th>Davet eden</th><th>Sunucuda kalan davetli</th></tr>");
        // Adlar önbellekten; kilit hemen bırakılır.
        let names: Vec<String> = {
            let guild = state.cache.guild(guild_id);
            top.iter()
                .map(|(user, _)| {
                    guild.as_ref().and_then(|g| g.members.get(user)).map_or_else(|| format!("Kullanıcı {user}"), |m| m.display_name().to_owned())
                })
                .collect()
        };
        for (i, ((_, count), name)) in top.iter().zip(names).enumerate() {
            let _ = write!(list, "<tr><td>{}</td><td>{}</td><td>{count}</td></tr>", i + 1, escape(&name));
        }
        list.push_str("</table>");
    }
    let out = section("Davet sıralaması", list)
        + &section(
            "Nasıl çalışır",
            "<p class=\"note\">Ayar gerekmez; botun 'Sunucuyu Yönet' izni olmalı. Sunucudan ayrılanlar ve 7 günden yeni hesaplar (sahte) sayılmaz. \
             Komutlar: <code>/davetler</code> (bir üyenin davetleri), <code>/davet-sıralama</code>.</p>"
                .to_owned(),
        );
    Ok((out, !top.is_empty()))
}
