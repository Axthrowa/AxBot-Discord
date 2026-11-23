//! Slash komut kaydı. Yeni bir komut eklemek için modülünü burada tanımlayıp `all()`'a ekleyin.

mod ai;
mod analytics;
mod automod;
mod community;
mod context;
mod economy;
mod en;
mod feeds;
mod engagement;
mod extra;
mod forms;
mod fun;
mod giveaway;
mod heist;
mod highlight;
mod leveling;
mod mod_tools;
mod moderation;
mod music;
mod notes;
mod panel;
mod ping;
mod planning;
mod profile;
mod recommend;
mod roles;
mod search;
mod server;
mod setup;
mod social;
mod stocks;
mod streams;
mod tools;
mod tickets;
mod utility;

use anyhow::Context as _;
use poise::serenity_prelude as serenity;
use tracing::info;

use crate::data::Data;
use crate::error::Error;

pub use fun::{handle_tod, TOD_PREFIX};
pub use recommend::{handle_component as handle_recommend, PREFIX as RECOMMEND_PREFIX};

pub fn all() -> Vec<poise::Command<Data, Error>> {
    let mut commands = every();
    // Kurulum / kullanım bağlamı yalnızca GENEL komutlarda geçerlidir; sunucu kaydında Discord bunları kabul etmez.
    for command in &mut commands {
        command.install_context = None;
        command.interaction_context = None;
    }
    commands
}

/// Bütün komutlar, İngilizce yerelleştirmeleriyle.
fn every() -> Vec<poise::Command<Data, Error>> {
    let mut commands = defined();
    en::apply(&mut commands);
    commands
}

/// Bütün komutlar, makrolarda tanımlandığı hâliyle.
fn defined() -> Vec<poise::Command<Data, Error>> {
    vec![
        ping::ping(),
        setup::kurulum(),
        panel::panel(),
        automod::automod(),
        tickets::ticket(),
        ai::ai(),
        ai::yaz(),
        extra::hayal_et(),
        extra::kartim(),
        notes::uye_notu(),
        planning::etkinlik(),
        planning::yedek(),
        context::cevir_mesaj(),
        context::bildir(),
        context::ozetle(),
        context::cevap_taslagi(),
        context::not_ekle_menu(),
        context::hizli_ceza(),
        context::hizli_ceza_mesaj(),
        social::afk(),
        social::itiraf(),
        social::cevir(),
        social::yardim(),
        streams::yayin(),
        music::cal(),
        music::muzik(),
        forms::dogrulama(),
        forms::basvuru(),
        fun::oyun(),
        fun::eglence(),
        fun::hayvan(),
        tools::duyuru(),
        tools::otocevap(),
        search::ara(),
        search::emoji(),
        mod_tools::sicil(),
        community::karsilama(),
        roles::rol(),
        highlight::vurgu(),
        recommend::tavsiye(),
        community::koruma(),
        giveaway::cekilis(),
        utility::hatirlat(),
        engagement::starboard(),
        engagement::etiket(),
        server::davetler(),
        server::sayac(),
        server::dogum_gunu(),
        server::gecici_oda(),
        server::oda(),
        server::youtube(),
        feeds::akis(),
        economy::ekonomi(),
        moderation::moderasyon(),
        mod_tools::kanal(),
        engagement::oneri_grup(),
        leveling::seviye_grup(),
        utility::bilgi(),
        economy::magaza_yonet(),
        economy::ekonomi_yonet(),
        analytics::istatistik(),
    ]
}

/// Botun düzgün çalışması için istediği sunucu izinleri (davet bağlantısı ve `/kurulum` kontrolü).
/// Yönetici (Administrator) bilerek istenmez; sadece kullanılan izinler.
pub const BOT_PERMISSIONS: serenity::Permissions = serenity::Permissions::VIEW_CHANNEL
    .union(serenity::Permissions::SEND_MESSAGES)
    .union(serenity::Permissions::SEND_MESSAGES_IN_THREADS)
    .union(serenity::Permissions::CREATE_PUBLIC_THREADS)
    .union(serenity::Permissions::CREATE_PRIVATE_THREADS)
    .union(serenity::Permissions::EMBED_LINKS)
    .union(serenity::Permissions::ATTACH_FILES)
    .union(serenity::Permissions::READ_MESSAGE_HISTORY)
    .union(serenity::Permissions::ADD_REACTIONS)
    .union(serenity::Permissions::USE_EXTERNAL_EMOJIS)
    .union(serenity::Permissions::MANAGE_MESSAGES)
    .union(serenity::Permissions::MANAGE_CHANNELS)
    .union(serenity::Permissions::MANAGE_ROLES)
    .union(serenity::Permissions::MANAGE_GUILD)
    .union(serenity::Permissions::MANAGE_GUILD_EXPRESSIONS)
    .union(serenity::Permissions::CREATE_INSTANT_INVITE)
    .union(serenity::Permissions::VIEW_AUDIT_LOG)
    .union(serenity::Permissions::KICK_MEMBERS)
    .union(serenity::Permissions::BAN_MEMBERS)
    .union(serenity::Permissions::MODERATE_MEMBERS)
    .union(serenity::Permissions::CONNECT)
    .union(serenity::Permissions::SPEAK)
    .union(serenity::Permissions::MOVE_MEMBERS)
    .union(serenity::Permissions::MANAGE_NICKNAMES)
    .union(serenity::Permissions::MANAGE_EVENTS)
    .union(serenity::Permissions::MANAGE_WEBHOOKS);

/// Botu bir sunucuya eklemek için bağlantı (gerekli izinler seçili gelir).
pub fn invite_url(application_id: serenity::UserId) -> String {
    format!(
        "https://discord.com/oauth2/authorize?client_id={application_id}&scope=bot+applications.commands&permissions={}",
        BOT_PERMISSIONS.bits()
    )
}

/// Hesaba kurulabilen (sunucu dışında da çalışan) komutlar. `USER_INSTALL=1` ise bunlar GENEL komut olarak
/// kaydedilir ve sunucu listelerinden çıkarılır (yoksa sunucularda iki kez görünürlerdi).
const USER_COMMANDS: [&str; 2] = ["çevir", "hayal-et"];

fn user_install() -> bool {
    crate::integrations::get().user_install
}

/// Sunuculara kaydedilecek komutlar.
fn guild_commands() -> Vec<poise::Command<Data, Error>> {
    all().into_iter().filter(|c| !user_install() || !USER_COMMANDS.contains(&c.name.as_str())).collect()
}

/// Komutları Discord'a kaydeder. Sunucu komutları anında görünür (global komutlar gecikebilir),
/// bu yüzden her sunucuya ayrı kaydedilir:
/// - `DEV_GUILD_ID` verildiyse sadece o sunucuya (geliştirme/test),
/// - verilmediyse botun bulunduğu bütün sunuculara; yeni eklendiği sunucuya da katıldığı an
///   (`events::handle` → `GuildCreate`).
pub async fn register(
    ctx: &serenity::Context,
    framework: &poise::Framework<Data, Error>,
    dev_guild_id: Option<serenity::GuildId>,
    guilds: &[serenity::GuildId],
) -> Result<(), Error> {
    let commands = &framework.options().commands;

    if let Some(guild_id) = dev_guild_id {
        poise::builtins::register_in_guild(ctx, commands, guild_id)
            .await
            .with_context(|| {
                format!(
                    "komutlar {guild_id} sunucusuna kaydedilemedi                      (bot bu sunucuda mı ve DEV_GUILD_ID bir sunucu ID'si mi?)"
                )
            })?;
        info!(count = commands.len(), guild = %guild_id, "Komutlar sunucuya kaydedildi");
        return Ok(());
    }

    if user_install() {
        // Genel komutların her yerde görünmesi bir saate kadar sürebilir.
        let global: Vec<poise::Command<Data, Error>> = every().into_iter().filter(|c| USER_COMMANDS.contains(&c.name.as_str())).collect();
        match poise::builtins::register_globally(ctx, &global).await {
            Ok(()) => info!(count = global.len(), "Hesaba kurulabilen komutlar genel olarak kaydedildi"),
            Err(e) => tracing::warn!(error = %e, "Genel komutlar kaydedilemedi (Developer Portal → Installation → User Install açık mı?)"),
        }
    }
    for chunk in guilds.chunks(3) {
        let tasks = chunk.iter().map(|&guild_id| register_guild(ctx, commands, guild_id));
        futures_util::future::join_all(tasks).await;
    }
    info!(count = commands.len(), guilds = guilds.len(), "Komutlar sunucuya kaydedildi (tüm sunucular)");
    Ok(())
}

/// Tek bir sunucuya kaydeder; hata olursa loglanır (diğer sunucular etkilenmez).
pub async fn register_guild(ctx: &serenity::Context, commands: &[poise::Command<Data, Error>], guild_id: serenity::GuildId) {
    // Hesaba kurulabilen komutlar genel kaydedildiyse sunucu listesinden çıkarılır.
    let filtered = user_install().then(guild_commands);
    let commands = filtered.as_deref().unwrap_or(commands);
    match poise::builtins::register_in_guild(ctx, commands, guild_id).await {
        Ok(()) => info!(guild = %guild_id, "Komutlar bu sunucuya kaydedildi"),
        Err(e) => tracing::warn!(guild = %guild_id, error = %e, "Komutlar bu sunucuya kaydedilemedi"),
    }
}

#[cfg(test)]
mod tree_tests {
    /// Komut ağacını yazdırır (yardım metinleri / belgeler için).
    /// Elle: `cargo test print_command_tree -- --ignored --nocapture`
    #[test]
    #[ignore = "yalnızca listeleme"]
    fn print_command_tree() {
        fn walk(cmd: &poise::Command<crate::data::Data, crate::error::Error>, prefix: &str) {
            let name = format!("{prefix}{}", cmd.name);
            if cmd.subcommands.is_empty() {
                let params: Vec<&str> = cmd.parameters.iter().map(|p| p.name.as_str()).collect();
                println!("/{name} [{}] — {}", params.join(" "), cmd.description.as_deref().unwrap_or(""));
            }
            for sub in &cmd.subcommands {
                walk(sub, &format!("{name} "));
            }
        }
        for cmd in super::every() {
            walk(&cmd, "");
        }
    }

    type Cmd = poise::Command<crate::data::Data, crate::error::Error>;

    /// Discord'un sınırları: en fazla 100 slash üst komutu, komut başına 25 alt komut, en fazla iki kat iç içe.
    #[test]
    fn command_tree_fits_discord_limits() -> Result<(), String> {
        let all = super::every();
        let slash = all.iter().filter(|c| c.slash_action.is_some() || !c.subcommands.is_empty()).count();
        if slash > 100 {
            return Err(format!("{slash} slash üst komutu var (sınır 100)"));
        }
        for top in &all {
            if top.subcommands.len() > 25 {
                return Err(format!("/{} altında {} alt komut var (sınır 25)", top.name, top.subcommands.len()));
            }
            for group in &top.subcommands {
                if group.subcommands.iter().any(|leaf| !leaf.subcommands.is_empty()) {
                    return Err(format!("/{} {} üç kat iç içe", top.name, group.name));
                }
            }
        }
        Ok(())
    }

    /// Discord alt komutlarda `default_member_permissions`'ı yok sayar: alt komutta izin istendiyse
    /// poise'un çalışma anı denetimi (`required_permissions`) de aynı izni istemeli.
    #[test]
    fn subcommand_permissions_are_checked_at_run_time() -> Result<(), String> {
        fn walk(cmd: &Cmd, path: &str, top: bool) -> Result<(), String> {
            let path = format!("{path} {}", cmd.name);
            let wanted = cmd.default_member_permissions;
            if !top && !wanted.is_empty() && !cmd.required_permissions.contains(wanted) {
                return Err(format!("/{} izni yalnızca Discord tarafında denetleniyor", path.trim()));
            }
            cmd.subcommands.iter().try_for_each(|sub| walk(sub, &path, false))
        }
        super::every().iter().try_for_each(|cmd| walk(cmd, "", true))
    }
}
