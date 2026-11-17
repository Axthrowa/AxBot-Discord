//! Özel komutlar: panelden tanımlanan `!komut`lar. Etiketlerden (`/etiket`) farkı: cevap gömülü
//! mesaj olabilir, rol verip alabilir, belirli rollere/kanallara kısıtlanabilir, bekleme süresi
//! olabilir, tetikleyen mesajı silebilir ya da cevabı özelden gönderebilir.
//!
//! Komut listesi bellekte durur (`PluginSettings`); mesaj başına veritabanına gidilmez.

use std::sync::Arc;

use poise::serenity_prelude::{self as serenity, CreateAllowedMentions, CreateEmbed, CreateMessage, GuildId, Mentionable, Message, RoleId};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::data::Data;
use crate::settings::PluginConfig;
use crate::{rolepanel, tags};

pub const MAX_COMMANDS: usize = 100;
pub const MAX_RESPONSE: usize = 2000;
pub const MAX_COOLDOWN_SECS: u32 = 3600;
pub const MAX_PATTERN: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Command {
    /// Küçük harfli, öneksiz ad (`kurallar`).
    pub name: String,
    pub response: String,
    /// Cevap gömülü mesaj (embed) olarak gönderilir.
    pub embed: bool,
    /// Cevap kanala değil, komutu yazana özelden gider.
    pub dm: bool,
    /// Komutu tetikleyen mesaj silinir.
    pub delete_trigger: bool,
    /// Aynı komut kanalda bu kadar saniyede bir çalışır (0 = sınırsız).
    pub cooldown_secs: u32,
    pub add_roles: Vec<u64>,
    pub remove_roles: Vec<u64>,
    /// Doluysa yalnızca bu rollerden birine sahip üyeler kullanabilir.
    pub required_roles: Vec<u64>,
    /// Doluysa yalnızca bu kanallarda çalışır.
    pub channels: Vec<u64>,
    /// Doluysa komut `!ad` yerine bu düzenli ifadeye (regex, büyük/küçük harf önemsiz) uyan HER mesajda çalışır.
    pub pattern: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomCommandSettings {
    pub commands: Vec<Command>,
}

impl PluginConfig for CustomCommandSettings {
    const KEY: &'static str = "custom_commands";
}

/// Üye bu komutu bu kanalda kullanabilir mi?
pub fn allowed(command: &Command, channel: u64, roles: &[RoleId]) -> bool {
    let channel_ok = command.channels.is_empty() || command.channels.contains(&channel);
    let role_ok = command.required_roles.is_empty() || roles.iter().any(|r| command.required_roles.contains(&r.get()));
    channel_ok && role_ok
}

/// Şablon değişkenlerinin değerleri.
#[derive(Debug, Default)]
pub struct Vars {
    pub mention: String,
    pub name: String,
    pub user_id: String,
    pub guild: String,
    pub channel: String,
    pub members: u64,
    /// `!komut` için komuttan sonraki kelimeler, regex için yakalanan gruplar.
    pub args: Vec<String>,
}

/// Cevap şablonu, tek geçişte (üyenin yazdığı metin yeniden işlenmez):
/// `{kullanıcı}` `{isim}` `{kimlik}` `{sunucu}` `{kanal}` `{üye_sayısı}` `{argümanlar}` `{1}`…`{9}` `{rastgele:a|b|c}`.
/// Bilinmeyen `{…}` olduğu gibi kalır.
pub fn render(template: &str, vars: &Vars, pick: impl Fn(usize) -> usize) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find('}') else {
            out.push_str(tail);
            return out;
        };
        let key = &tail[1..end];
        let value = match key {
            "kullanıcı" | "kullanici" => Some(vars.mention.clone()),
            "isim" => Some(vars.name.clone()),
            "kimlik" => Some(vars.user_id.clone()),
            "sunucu" => Some(vars.guild.clone()),
            "kanal" => Some(vars.channel.clone()),
            "üye_sayısı" | "uye_sayisi" => Some(vars.members.to_string()),
            "argümanlar" | "argumanlar" => Some(vars.args.join(" ")),
            _ => {
                if let Some(choices) = key.strip_prefix("rastgele:") {
                    let choices: Vec<&str> = choices.split('|').collect();
                    choices.get(pick(choices.len())).map(|c| (*c).to_owned())
                } else if let Ok(n @ 1..=9) = key.parse::<usize>() {
                    Some(vars.args.get(n - 1).cloned().unwrap_or_default())
                } else {
                    None
                }
            }
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Derlenmiş regex önbelleği (geçersiz ifade `None`); mesaj başına derleme yapılmaz.
static PATTERNS: std::sync::LazyLock<dashmap::DashMap<String, Option<Arc<regex::Regex>>>> = std::sync::LazyLock::new(dashmap::DashMap::new);

/// Panelde ve çalışırken aynı kurallarla derler (regex kütüphanesi doğrusal zamanlıdır; boyut sınırlı).
pub fn compile(pattern: &str) -> Option<Arc<regex::Regex>> {
    if let Some(found) = PATTERNS.get(pattern) {
        return found.clone();
    }
    let built = (pattern.chars().count() <= MAX_PATTERN)
        .then(|| regex::RegexBuilder::new(pattern).case_insensitive(true).size_limit(1 << 20).build().ok().map(Arc::new))
        .flatten();
    if PATTERNS.len() > 2000 {
        PATTERNS.clear();
    }
    PATTERNS.insert(pattern.to_owned(), built.clone());
    built
}

/// Mesaja uyan komut ve argümanları: önce `!ad`, yoksa sıradaki ilk regex.
fn find<'a>(commands: &'a [Command], content: &str) -> Option<(&'a Command, Vec<String>)> {
    if let Some(name) = tags::trigger(content) {
        if let Some(command) = commands.iter().find(|c| c.pattern.is_empty() && c.name == name) {
            let args = content.split_whitespace().skip(1).map(str::to_owned).collect();
            return Some((command, args));
        }
    }
    commands.iter().filter(|c| !c.pattern.is_empty()).find_map(|command| {
        let caps = compile(&command.pattern)?.captures(content)?;
        let args = caps.iter().skip(1).take(9).map(|m| m.map_or_else(String::new, |m| m.as_str().to_owned())).collect();
        Some((command, args))
    })
}

/// Mesaj bir özel komutsa çalıştırır; çalıştırdıysa (ya da izin vermediyse) `true`.
pub async fn handle(ctx: &serenity::Context, data: &Data, guild_id: GuildId, msg: &Message, roles: &[RoleId]) -> bool {
    // Ayarlar bellekte önbellekli; regex komutu olmayan sunucuda `!` ile başlamayan mesaj hemen geçer.
    let settings: Arc<CustomCommandSettings> = match data.settings.get(&data.db, guild_id).await {
        Ok(settings) => settings,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Özel komutlar okunamadı");
            return false;
        }
    };
    let Some((command, args)) = find(&settings.commands, &msg.content) else {
        return false;
    };
    if !allowed(command, msg.channel_id.get(), roles) {
        // Komut var ama bu üyeye/kanala kapalı: sessizce yutulur (etiket olarak da aranmaz).
        return true;
    }
    if command.cooldown_secs > 0 && !claim_cooldown(data, guild_id, msg, command).await {
        return true;
    }

    let (guild, members) = ctx.cache.guild(guild_id).map(|g| (g.name.clone(), g.member_count)).unwrap_or_default();
    let vars = Vars {
        mention: msg.author.id.mention().to_string(),
        name: msg.author.global_name.clone().unwrap_or_else(|| msg.author.name.clone()),
        user_id: msg.author.id.to_string(),
        guild,
        channel: msg.channel_id.mention().to_string(),
        members,
        args,
    };
    let text = render(&command.response, &vars, |n| usize::try_from(crate::rand_util::below(n as u64)).unwrap_or(0));
    // Cevap yöneticinin yazdığı metindir; yine de yalnızca komutu yazan kişi etiketlenir.
    let mut reply = CreateMessage::new().allowed_mentions(CreateAllowedMentions::new().users(vec![msg.author.id]));
    reply = if command.embed {
        reply.embed(CreateEmbed::new().description(text).colour(serenity::Colour::BLURPLE))
    } else {
        reply.content(text)
    };
    let sent = if command.dm {
        msg.author.direct_message(ctx, reply).await.map(|_| ())
    } else {
        msg.channel_id.send_message(&ctx.http, reply).await.map(|_| ())
    };
    if let Err(e) = sent {
        debug!(channel = %msg.channel_id, command = command.name, error = %e, "Özel komut cevabı gönderilemedi");
    }

    for (list, add) in [(&command.add_roles, true), (&command.remove_roles, false)] {
        for role in list.iter().filter(|id| **id != 0).map(|id| RoleId::new(*id)) {
            change_role(ctx, guild_id, msg, role, add).await;
        }
    }
    if command.delete_trigger {
        if let Err(e) = msg.delete(&ctx.http).await {
            debug!(channel = %msg.channel_id, error = %e, "Özel komut mesajı silinemedi");
        }
    }
    true
}

/// Rol hâlâ güvenle dağıtılabiliyorsa verir / alır (yetkili roller hiçbir zaman dağıtılmaz).
async fn change_role(ctx: &serenity::Context, guild_id: GuildId, msg: &Message, role_id: RoleId, add: bool) {
    let role = ctx.cache.guild(guild_id).and_then(|g| g.roles.get(&role_id).cloned());
    let Some(role) = role else { return };
    if let Err(reason) = rolepanel::check_role(ctx, guild_id, &role) {
        debug!(guild = %guild_id, role = %role_id, reason, "Özel komut: rol değiştirilmedi");
        return;
    }
    let result = if add {
        ctx.http.add_member_role(guild_id, msg.author.id, role_id, Some("Özel komut")).await
    } else {
        ctx.http.remove_member_role(guild_id, msg.author.id, role_id, Some("Özel komut")).await
    };
    if let Err(e) = result {
        warn!(guild = %guild_id, role = %role_id, error = %e, "Özel komut rol değiştiremedi");
    }
}

/// Bekleme süresi dolmuşsa `true` döner ve yeni süreyi başlatır.
async fn claim_cooldown(data: &Data, guild_id: GuildId, msg: &Message, command: &Command) -> bool {
    let key = format!("cc:cd:{guild_id}:{}:{}", msg.channel_id, command.name);
    let mut conn = data.redis.clone();
    let claimed: redis::RedisResult<Option<String>> = redis::cmd("SET")
        .arg(key)
        .arg(1)
        .arg("EX")
        .arg(command.cooldown_secs)
        .arg("NX")
        .query_async(&mut conn)
        .await;
    // Redis geçici olarak yoksa komut çalışmaya devam eder.
    claimed.map_or(true, |c| c.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restrictions() {
        let open = Command::default();
        assert!(allowed(&open, 1, &[]));
        let limited = Command { required_roles: vec![5], channels: vec![10], ..Command::default() };
        assert!(allowed(&limited, 10, &[RoleId::new(5), RoleId::new(6)]));
        assert!(!allowed(&limited, 11, &[RoleId::new(5)]));
        assert!(!allowed(&limited, 10, &[RoleId::new(6)]));
    }

    #[test]
    fn renders_placeholders_and_loads_old_records() -> anyhow::Result<()> {
        let vars = Vars {
            mention: "<@1>".into(),
            name: "Ali".into(),
            guild: "S".into(),
            channel: "<#2>".into(),
            members: 42,
            args: vec!["{isim}".into(), "b".into()],
            ..Vars::default()
        };
        assert_eq!(render("{kullanıcı} {isim} {sunucu} {kanal}", &vars, |_| 0), "<@1> Ali S <#2>");
        // Üyenin yazdığı `{isim}` yeniden işlenmez; eksik argüman boş, bilinmeyen değişken aynen kalır.
        assert_eq!(render("{1}/{2}/{3} {üye_sayısı} {yok} {", &vars, |_| 0), "{isim}/b/ 42 {yok} {");
        assert_eq!(render("{rastgele:a|b|c}", &vars, |_| 2), "c");
        let commands = vec![
            Command { name: "selam".into(), pattern: r"^merhaba (\w+)".into(), response: "x".into(), ..Command::default() },
            Command { name: "kural".into(), response: "y".into(), ..Command::default() },
        ];
        let (c, args) = find(&commands, "MERHABA dünya").ok_or_else(|| anyhow::anyhow!("regex uymadı"))?;
        assert_eq!((c.name.as_str(), args), ("selam", vec!["dünya".to_owned()]));
        let (c, args) = find(&commands, "!kural 3 4").ok_or_else(|| anyhow::anyhow!("komut bulunamadı"))?;
        assert_eq!((c.name.as_str(), args.len()), ("kural", 2));
        assert!(find(&commands, "!selam").is_none(), "regex komutu adıyla çağrılmaz");
        assert!(compile("(").is_none());
        let settings: CustomCommandSettings = serde_json::from_str("{\"commands\":[{\"name\":\"a\",\"response\":\"b\"}]}")?;
        assert_eq!(settings.commands.len(), 1);
        assert!(!settings.commands[0].embed && settings.commands[0].cooldown_secs == 0);
        Ok(())
    }
}
