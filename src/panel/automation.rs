//! Panel: Otomasyon ve Başarımlar eklentilerinin formları.

use std::fmt::Write as _;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use poise::serenity_prelude::{ChannelId, GuildId, RoleId};
use serde::Deserialize;
use tracing::warn;

use super::auth;
use super::can_manage;
use super::plugins::{checkbox, checklist, field, section, textarea, Input, Options};
use crate::achievements::{self, AchievementSettings, Definition, Kind, DEFAULTS};
use crate::automations::{self, Automation, AutomationSettings, Trigger};
use crate::reaction_roles::emoji_key;
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
    if a.webhook_url.is_some() {
        parts.push("webhook tetikler".to_owned());
    }
    let mut conditions = Vec::new();
    if let Some(keyword) = &a.keyword {
        conditions.push(format!("\"{keyword}\" geçerse"));
    }
    if let Some(streamer) = &a.streamer {
        conditions.push(format!("yayıncı/platform: \"{streamer}\""));
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
        list.push_str("<p class=\"note\">Henüz otomasyon kuralı oluşturulmadı. Aşağıdaki formdan yeni bir kural ekleyebilirsin.</p>");
    } else {
        list.push_str("<div class=\"auto-cards\" style=\"display:grid;gap:12px;margin-bottom:18px;\">");
        for a in &settings.automations {
            let status_badge = if a.enabled {
                "<span style=\"display:inline-block;padding:2px 8px;border-radius:10px;font-size:0.75em;background:#23a55a;color:#fff;font-weight:600;\">Açık</span>"
            } else {
                "<span style=\"display:inline-block;padding:2px 8px;border-radius:10px;font-size:0.75em;background:#80848e;color:#fff;font-weight:600;\">Kapalı</span>"
            };
            let _ = write!(
                list,
                "<div class=\"auto-card\" style=\"background:rgba(255,255,255,0.02);border:1px solid rgba(255,255,255,0.08);border-radius:8px;padding:12px 16px;\">\
                   <div style=\"display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;\">\
                     <div style=\"display:flex;align-items:center;gap:8px;\">\
                       {status_badge}\
                       <strong style=\"font-size:1.05em;\">{}</strong>\
                     </div>\
                     <div style=\"display:flex;gap:12px;align-items:center;\">\
                       <button type=\"button\" onclick=\"toggleDryRun({id})\" style=\"background:rgba(88,101,242,0.15);border:1px solid rgba(88,101,242,0.4);color:#5865F2;padding:3px 10px;border-radius:6px;font-size:0.85em;cursor:pointer;font-weight:600;\">🧪 Test Et (Dry-Run)</button>\
                       <label style=\"display:flex;align-items:center;gap:6px;cursor:pointer;font-size:0.9em;\">\
                         <input type=\"checkbox\" name=\"auto_on\" value=\"{id}\"{}>\
                         <span>Aktif</span>\
                       </label>\
                       <label style=\"display:flex;align-items:center;gap:6px;cursor:pointer;color:#ed4245;font-size:0.9em;\">\
                         <input type=\"checkbox\" name=\"auto_del\" value=\"{id}\">\
                         <span>Sil</span>\
                       </label>\
                     </div>\
                   </div>\
                   <div style=\"display:flex;flex-wrap:wrap;gap:6px;font-size:0.85em;color:#b5bac1;\">\
                     <span style=\"background:rgba(88,101,242,0.2);color:#5865F2;padding:2px 8px;border-radius:4px;font-weight:600;\">⚡ {}</span>\
                     <span>➔</span>\
                     <span>{}</span>\
                   </div>\
                   <div id=\"dryrun-box-{id}\" style=\"display:none;margin-top:12px;padding:12px;background:rgba(0,0,0,0.3);border:1px solid rgba(255,255,255,0.08);border-radius:6px;\">\
                     <div style=\"display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;\">\
                       <strong style=\"font-size:0.95em;\">Simülasyon Girdileri (Canlı Yan Etki Yoktur)</strong>\
                       <button type=\"button\" onclick=\"execDryRun({id}, '{trigger_code}', '{guild_id}')\" style=\"background:#5865F2;color:#fff;border:none;padding:4px 12px;border-radius:4px;cursor:pointer;font-size:0.85em;font-weight:600;\">Simülasyonu Çalıştır</button>\
                     </div>\
                     <div style=\"display:grid;grid-template-columns:repeat(auto-fit, minmax(180px, 1fr));gap:8px;font-size:0.85em;\">\
                       <div>\
                         <label style=\"display:block;color:#b5bac1;margin-bottom:2px;\">Mesaj / İsim</label>\
                         <input type=\"text\" id=\"dr-content-{id}\" placeholder=\"Örnek mesaj veya içerik\" style=\"width:100%;box-sizing:border-box;padding:4px 8px;background:rgba(255,255,255,0.05);border:1px solid rgba(255,255,255,0.1);color:#fff;border-radius:4px;\">\
                       </div>\
                       <div>\
                         <label style=\"display:block;color:#b5bac1;margin-bottom:2px;\">Yayıncı / Platform</label>\
                         <input type=\"text\" id=\"dr-streamer-{id}\" placeholder=\"elraenn veya twitch\" style=\"width:100%;box-sizing:border-box;padding:4px 8px;background:rgba(255,255,255,0.05);border:1px solid rgba(255,255,255,0.1);color:#fff;border-radius:4px;\">\
                       </div>\
                       <div>\
                         <label style=\"display:block;color:#b5bac1;margin-bottom:2px;\">Yayın Başlığı</label>\
                         <input type=\"text\" id=\"dr-title-{id}\" placeholder=\"Canlı Yayın Başlığı\" style=\"width:100%;box-sizing:border-box;padding:4px 8px;background:rgba(255,255,255,0.05);border:1px solid rgba(255,255,255,0.1);color:#fff;border-radius:4px;\">\
                       </div>\
                     </div>\
                     <div style=\"margin-top:8px;display:flex;align-items:center;gap:6px;font-size:0.85em;color:#b5bac1;\">\
                       <input type=\"checkbox\" id=\"dr-probe-{id}\">\
                       <label for=\"dr-probe-{id}\" style=\"cursor:pointer;\">Webhook uç noktasını canlı yokla (Probe POST, 4sn timeout)</label>\
                     </div>\
                     <div id=\"dr-res-{id}\" style=\"margin-top:10px;display:none;\"></div>\
                   </div>\
                 </div>",
                escape(&a.name),
                if a.enabled { " checked" } else { "" },
                a.trigger.label(),
                escape(&summary(a, options)),
                id = a.id,
                trigger_code = a.trigger.code(),
                guild_id = guild_id.get()
            );
        }
        list.push_str(
            "<script>\
            function toggleDryRun(id) {\
              var el = document.getElementById('dryrun-box-' + id);\
              if (el) el.style.display = el.style.display === 'none' ? 'block' : 'none';\
            }\
            function execDryRun(id, triggerCode, guildId) {\
              var content = document.getElementById('dr-content-' + id).value;\
              var streamer = document.getElementById('dr-streamer-' + id).value;\
              var title = document.getElementById('dr-title-' + id).value;\
              var probe = document.getElementById('dr-probe-' + id).checked;\
              var resBox = document.getElementById('dr-res-' + id);\
              resBox.style.display = 'block';\
              resBox.innerHTML = '<span style=\"color:#b5bac1;\">Simülasyon çalıştırılıyor...</span>';\
              var payload = {\
                automation_id: id,\
                input: {\
                  trigger: triggerCode,\
                  content: content,\
                  streamer: streamer,\
                  platform: streamer,\
                  title: title,\
                  probe_webhook: probe\
                }\
              };\
              fetch('/panel/sunucu/' + guildId + '/otomasyon/test', {\
                method: 'POST',\
                headers: { 'Content-Type': 'application/json' },\
                body: JSON.stringify(payload)\
              })\
              .then(function(r) { return r.json(); })\
              .then(function(data) {\
                var html = '<div style=\"background:rgba(0,0,0,0.4);border:1px solid rgba(255,255,255,0.1);border-radius:6px;padding:10px;font-size:0.85em;\">';\
                var badge = data.matched\
                  ? '<span style=\"color:#23a55a;font-weight:bold;\">● Koşullar Eşleşti (Eylemler Çalıştırılır)</span>'\
                  : '<span style=\"color:#ed4245;font-weight:bold;\">○ Koşullar Eşleşmedi (Eylemler Çalıştırılmaz)</span>';\
                html += '<div style=\"margin-bottom:8px;\">' + badge + '</div>';\
                html += '<div style=\"margin-bottom:8px;\"><strong>Koşul Kontrolleri:</strong><ul style=\"margin:4px 0 0 16px;padding:0;\">';\
                data.condition_results.forEach(function(c) {\
                  var icon = c.passed ? '<span style=\"color:#23a55a;\">✓</span> ' : '<span style=\"color:#ed4245;\">✗</span> ';\
                  html += '<li style=\"margin-bottom:2px;\">' + icon + '<strong>' + c.name + ':</strong> ' + c.reason + '</li>';\
                });\
                html += '</ul></div>';\
                if (data.rendered_message) {\
                  html += '<div style=\"margin-bottom:6px;\"><strong>Mesaj Önizleme:</strong> <code style=\"background:rgba(255,255,255,0.1);padding:2px 6px;border-radius:3px;\">' + data.rendered_message + '</code></div>';\
                }\
                if (data.planned_actions && data.planned_actions.length > 0) {\
                  html += '<div style=\"margin-bottom:6px;\"><strong>Planlanan Eylemler:</strong><ul style=\"margin:4px 0 0 16px;padding:0;\">';\
                  data.planned_actions.forEach(function(pa) {\
                    html += '<li><strong>' + pa.action_type + '</strong> (' + pa.target + '): ' + pa.preview + '</li>';\
                  });\
                  html += '</ul></div>';\
                }\
                if (data.webhook_check) {\
                  var wc = data.webhook_check;\
                  html += '<div><strong>Webhook Denetimi:</strong> ' + wc.ssrf_status;\
                  if (wc.probe_attempted) {\
                    if (wc.probe_status_code) {\
                      html += ' | <span style=\"color:#23a55a;\">Probe Yanıtı: HTTP ' + wc.probe_status_code + ' (' + wc.probe_latency_ms + 'ms)</span>';\
                    } else if (wc.probe_error) {\
                      html += ' | <span style=\"color:#ed4245;\">Probe Hatası: ' + wc.probe_error + '</span>';\
                    }\
                  }\
                  html += '</div>';\
                }\
                html += '</div>';\
                resBox.innerHTML = html;\
              })\
              .catch(function(err) {\
                resBox.innerHTML = '<span style=\"color:#ed4245;\">Simülasyon isteği başarısız: ' + err + '</span>';\
              });\
            }\
            </script>"
        );
        list.push_str("</div>");
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
            + &field("Yayıncı veya Platform", &text_input("a_streamer", "örn. elraenn veya twitch", 100), "Yayın açıldığında/kapandığında için yayıncı adı veya platform (twitch, kick). Boş bırakılırsa tüm yayınlar.")
            + "<h4>Eylemler (en az biri)</h4>"
            + &field(
                "Mesaj gönder",
                &textarea("a_message", None, "Hoş geldin {kullanıcı}!"),
                "Değişkenler: <code>{kullanıcı}</code>, <code>{isim}</code>, <code>{sunucu}</code>, <code>{kanal}</code>, <code>{rol}</code>, <code>{mesaj}</code>, <code>{yayinci}</code>, <code>{baslik}</code>.",
            )
            + &field(
                "Mesajın gideceği kanal",
                &Options::select("a_message_channel", &options.channels, "# ", None, "— Olayın olduğu kanal —"),
                "Üye girişi/çıkışı, rol ve yayın olaylarında bir kanal seçmelisin.",
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
            + "<h4>Dış Ağ / Yerel Webhook Eylemi</h4>"
            + &field("Webhook Hedef URL", &text_input("a_webhook_url", "https://api.ornek.com/webhook veya http://127.0.0.1:8080/hook", 300), "Tetiklendiğinde HTTP POST isteği gönderilecek hedef uç nokta.")
            + &field("Webhook Gizli Anahtarı / Token", &text_input("a_webhook_secret", "opsiyonel token veya secret", 120), "Authorization: Bearer &lt;secret&gt; ve x-webhook-secret başlıklarına eklenir.")
            + &field("Özel JSON Payload (Opsiyonel)", &textarea("a_webhook_payload", None, ""), "Boş bırakılırsa standart olay özeti JSON olarak gönderilir.")
            + &checkbox("a_allow_localhost", false, "Yerel hedeflere (127.0.0.1 / localhost) izin ver (SSRF koruması istisnası)")
            + "<p class=\"note\">Örnekler: <b>Karşılayıcı</b> — \"Üye sunucuya katıldığında\" + mesaj. <b>Alt başlık</b> — \"Mesaj gönderildiğinde\" + kanal + \
               \"Alt başlık aç\". <b>Rol duyurusu</b> — \"Üyeye rol verildiğinde\" + rol + mesaj. <b>Yayın Bildirimi / Webhook</b> — \"Yayın açıldığında\" + webhook veya mesaj.</p>",
    ));
    Ok((out, settings.automations.iter().any(|a| a.enabled)))
}

/// Formdaki yeni otomasyon (ad boşsa `None`).
fn parse_automation(input: &Input<'_>, id: u32) -> Result<Option<Automation>, &'static str> {
    let Some(name) = short(input, "a_name", 60)? else {
        return Ok(None);
    };
    let trigger = Trigger::from_code(input.raw("a_trigger")).ok_or("hata")?;
    let allow_localhost = input.checked("a_allow_localhost");
    let webhook_url = short(input, "a_webhook_url", 300)?;
    if let Some(ref raw_url) = webhook_url {
        automations::validate_webhook_url(raw_url, allow_localhost).map_err(|_| "webhook_url")?;
    }
    let webhook_secret = short(input, "a_webhook_secret", 120)?;
    let webhook_payload = input.text("a_webhook_payload")?;
    let streamer = short(input, "a_streamer", 100)?;

    let automation = Automation {
        id,
        name,
        enabled: true,
        trigger,
        keyword: short(input, "a_keyword", 100)?,
        channel: input.channel("a_channel")?.map(ChannelId::get),
        role: pick(input, "a_role", &input.options.all_roles, "rol")?,
        emoji: emoji(input, "a_emoji")?,
        streamer,
        message: input.text("a_message")?,
        message_channel: pick(input, "a_message_channel", &input.options.channels, "kanal")?,
        dm: input.text("a_dm")?,
        add_roles: input.ids("a_add_role", &input.options.roles, "rol")?,
        remove_roles: input.ids("a_remove_role", &input.options.roles, "rol")?,
        react: emoji(input, "a_react")?,
        delete_message: input.checked("a_delete"),
        thread_name: short(input, "a_thread", 100)?,
        ping_roles: input.checked("a_ping"),
        webhook_url,
        webhook_secret,
        webhook_payload,
        allow_localhost,
    };
    let has_action = automation.message.is_some()
        || automation.dm.is_some()
        || !automation.add_roles.is_empty()
        || !automation.remove_roles.is_empty()
        || automation.react.is_some()
        || automation.delete_message
        || automation.thread_name.is_some()
        || automation.webhook_url.is_some();
    if !has_action {
        return Err("bos");
    }
    // Kanalsız olaylarda (giriş, çıkış, rol, yayın) mesaj gönderilecekse hedef kanal belli olmalı.
    let channelless = matches!(
        trigger,
        Trigger::MemberJoin
            | Trigger::MemberLeave
            | Trigger::RoleAdded
            | Trigger::RoleRemoved
            | Trigger::StreamWentLive
            | Trigger::StreamWentOffline
    );
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

#[derive(Debug, Deserialize)]
pub struct DryRunRequest {
    pub automation_id: Option<u32>,
    pub draft: Option<Automation>,
    #[serde(default)]
    pub input: automations::DryRunInput,
}

pub async fn dry_run_handler(
    State(state): State<WebState>,
    Path(guild_str): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<DryRunRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let Some(session) = auth::session(&state, &headers).await else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let Ok(guild_num) = guild_str.parse::<u64>() else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let guild_id = GuildId::new(guild_num);
    if !can_manage(&state, session.user, guild_id).await {
        return Err(StatusCode::FORBIDDEN);
    }

    let automation = if let Some(draft) = payload.draft {
        draft
    } else if let Some(id) = payload.automation_id {
        let Ok(settings) = state.settings.get::<AutomationSettings>(&state.db, guild_id).await else {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        };
        let Some(found) = settings.automations.iter().find(|a| a.id == id).cloned() else {
            return Err(StatusCode::NOT_FOUND);
        };
        found
    } else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let mut input = payload.input;
    if input.guild_name.is_none() {
        input.guild_name = state.cache.guild(guild_id).map(|g| g.name.clone());
    }

    let report = automation.evaluate_dry_run(&input, Some(&state.http_client)).await;
    Ok(Json(report))
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
            + "<p class=\"note\">Üyeler ilerlemelerini Discord'da <code>/seviye başarımlar</code> ile görür. Sayaçlar başarımlar açıkken işler.</p>",
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

        let auto = Automation {
            id: 1,
            name: "Test".to_owned(),
            enabled: true,
            trigger: Trigger::Message,
            keyword: Some("yardım".to_owned()),
            channel: Some(10),
            role: None,
            emoji: None,
            message: Some("Cevap".to_owned()),
            message_channel: None,
            dm: None,
            add_roles: Vec::new(),
            remove_roles: Vec::new(),
            react: None,
            delete_message: false,
            thread_name: None,
            ping_roles: false,
            ..Automation::default()
        };
        let s = summary(&auto, &options);
        assert!(s.contains("\"yardım\" geçerse"));
        assert!(s.contains("#genel kanalında"));
        assert!(s.contains("mesaj gönderir"));
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
