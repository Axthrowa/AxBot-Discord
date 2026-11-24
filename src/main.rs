//! Botun giriş noktası: ortamı yükler, loglamayı başlatır, framework'ü kurar
//! ve Discord gateway'ine bağlanır.

mod achievements;
mod analytics;
mod privacy;
mod ai;
mod alerts;
mod antinuke;
mod appeals;
mod applications;
mod birthdays;
mod automations;
mod automod;
mod backup;
mod autoresponder;
mod cache;
mod cards;
mod cleanup;
mod commands;
mod community;
mod config;
mod customcmd;
mod data;
mod db;
mod economy;
mod error;
mod eventlog;
mod events;
mod modules;
mod perms;
mod timings;
mod ui;
mod feeds;
mod catalog;
mod feelm;
mod games;
mod giveaway;
mod greetings;
mod guild_config;
mod help;
pub mod i18n;
mod highlights;
mod lockdown;
mod integrations;
mod invites;
mod leveling;
mod members;
mod modals;
mod moderation;
mod logbuffer;
mod modlog;
mod modmail;
mod music;
mod net;
mod news;
mod newsprefs;
mod nativeautomod;
mod nickguard;
mod panel;
mod presence;
mod quarantine;
mod profile;
mod rand_util;
mod rankcard;
mod reaction_roles;
mod rsvp;
mod scheduler;
mod settings;
mod setup_report;
mod supervise;
mod starboard;
mod statchannels;
mod streams;
mod suggestions;
mod tags;
mod tempvoice;
mod embeds;
mod telemetry;
mod tickets;
mod verification;
mod web;
mod youtube;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use poise::serenity_prelude as serenity;
use songbird::SerenityInit as _;
use tracing::{debug, error, info, warn};

use crate::ai::Ai;
use crate::automod::AutoMod;
use crate::config::Config;
use crate::data::Data;
use crate::guild_config::GuildConfigs;
use crate::modlog::ModLog;
use crate::music::Music;
use crate::scheduler::{Executor, Scheduler};
use crate::streams::Streams;

/// Silinen/düzenlenen mesaj logları için kanal başına önbellekte tutulan mesaj sayısı.
const MESSAGE_CACHE_PER_CHANNEL: usize = 200;
/// Bu süreden uzun süren komutlar uyarı olarak loglanır (yapay zekâ / müzik gibi doğası yavaş
/// olanlar da dahil; hangi komutların yavaşladığını görmek için).
const SLOW_COMMAND_MS: u128 = 400;

#[tokio::main]
async fn main() -> ExitCode {
    // .env'yi loglamadan önce okuyoruz ki RUST_LOG oradan da gelebilsin.
    let dotenv_result = dotenvy::dotenv();

    // main bitene kadar tutulur; bitince kuyruktaki loglar dosyaya yazılır.
    let _log_guard = match telemetry::init() {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("tracing başlatılamadı: {e:#}");
            return ExitCode::FAILURE;
        }
    };

    match dotenv_result {
        Ok(path) => info!(path = %path.display(), ".env yüklendi"),
        Err(e) if e.not_found() => {
            warn!(".env bulunamadı, sistem ortam değişkenleri kullanılacak");
        }
        Err(e) => {
            error!(error = %e, ".env okunamadı");
            return ExitCode::FAILURE;
        }
    }

    match run().await {
        Ok(()) if panel::RESTART_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) => {
            info!("Bot yeniden başlatılmak üzere kapatıldı");
            // Bekçi (scripts/run-bot.cmd) 3 kodunu görünce hemen yeniden açar; Docker her çıkışta açar.
            ExitCode::from(3)
        }
        Ok(()) => {
            info!("Bot düzgün şekilde kapatıldı");
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!(error = ?e, "Bot ölümcül bir hatayla durdu");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let config = Config::from_env().context("yapılandırma yüklenemedi")?;
    integrations::init();
    alerts::install_panic_hook();
    info!(integrations = ?integrations::get(), "İsteğe bağlı servisler");
    let dev_guild_id = config.dev_guild_id;
    let web_config = config.web;
    // Panel adresi: sabit adres > (tünel adresini bulana kadar boş) > bu bilgisayar.
    let initial_url = match (&web_config.public_url, &web_config.tunnel_metrics) {
        (Some(url), _) => Some(url.clone()),
        (None, Some(_)) => None,
        (None, None) => {
            let port = web_config.bind.port();
            Some(format!("http://127.0.0.1:{port}"))
        }
    };
    let public_url = panel::PublicUrl::new(initial_url);
    match config.ai.as_ref() {
        Some(ai) => info!(
            provider = ai.provider.name(),
            model = %ai.model,
            web_search = match (ai.provider, &ai.search) {
                (config::AiProvider::Gemini, _) => "Google",
                (_, Some(backend)) => backend.name(),
                (_, None) => "kapalı",
            },
            "Yapay zekâ etkin"
        ),
        None => info!("Yapay zekâ kapalı (NVIDIA_API_KEY / GEMINI_API_KEY yok)"),
    }

    // Veri katmanı Discord'dan önce ve paralel kurulur; biri hazır değilse bot hiç bağlanmaz.
    let (db, redis) = tokio::try_join!(
        db::connect(&config.database),
        cache::connect(&config.redis_url)
    )?;
    let db_for_shutdown = db.clone();
    let redis_for_shutdown = redis.clone();
    leveling::spawn_flusher(db.clone(), redis.clone());
    analytics::spawn_flusher(db.clone(), redis.clone());
    let automod = Arc::new(AutoMod::new()?);

    // Discord, yapay zekâ ve yayın API'leri aynı HTTP istemcisini (DNS önbelleği + bağlantı havuzu) paylaşır.
    let client = net::http_client()?;
    let ai = Arc::new(Ai::new(config.ai, client.clone()));
    let ai_description = ai.describe();
    let streams = Arc::new(Streams::new(client.clone(), config.twitch, config.kick));
    let stream_poll = std::time::Duration::from_secs(u64::from(config.stream_poll_secs));

    // Ses bağlantıları (müzik). Discord istemcisine de kaydedilir.
    let songbird = songbird::Songbird::serenity();
    let music = Music::with_options(Arc::clone(&songbird), client.clone(), Some(redis.clone()));
    music.load_settings(&db).await?;
    match music.ytdlp_version().await {
        Some(version) => info!(yt_dlp = %version, "Müzik etkin"),
        None => warn!("yt-dlp bulunamadı (tools/yt-dlp.exe veya PATH): müzik komutları çalışmayacak"),
    }

    let http_client = client.clone();
    let tempvoice = tempvoice::TempVoice::default();
    tempvoice.load(&db).await?;

    let http = net::discord_http(&config.discord_token, net::discord_client()?);

    // Server Members Intent (privileged) Developer Portal'da açık değilse istenirse Discord
    // bağlantıyı reddeder; bu yüzden önce uygulama ayarından okunur.
    // Bot sahibi (veya takım üyeleri) web panelinde her şeyi yönetir.
    let mut owners = std::collections::HashSet::new();
    let mut presence_intent = false;
    let members_intent = match http.get_current_application_info().await {
        Ok(app) => {
            let app_presence = app.flags.is_some_and(|f| {
                f.intersects(serenity::ApplicationFlags::GATEWAY_PRESENCE | serenity::ApplicationFlags::GATEWAY_PRESENCE_LIMITED)
            });
            presence_intent = match std::env::var("ENABLE_PRESENCE_INTENT").ok().as_deref() {
                Some("0" | "false" | "hayır" | "no") => false,
                Some("1" | "true" | "evet" | "yes") => app_presence,
                _ => app_presence,
            };
            if let Some(owner) = &app.owner {
                owners.insert(owner.id);
            }
            if let Some(team) = &app.team {
                owners.extend(team.members.iter().map(|m| m.user.id));
            }
            app.flags.is_some_and(|f| {
                f.intersects(
                    serenity::ApplicationFlags::GATEWAY_GUILD_MEMBERS | serenity::ApplicationFlags::GATEWAY_GUILD_MEMBERS_LIMITED,
                )
            })
        }
        Err(e) => {
            warn!(error = %e, "Uygulama bilgisi okunamadı; Server Members Intent kapalı varsayılıyor");
            false
        }
    };
    let alert_owners: Vec<serenity::UserId> = owners.iter().copied().collect();
    if members_intent {
        info!("Server Members Intent açık: giriş/çıkış, karşılama ve koruma özellikleri etkin");
    } else {
        warn!(
            "Server Members Intent kapalı: karşılama, otomatik rol, giriş koruması ve üye logları çalışmaz (Developer Portal → Bot → Server Members Intent)"
        );
    }

    // MESSAGE_CONTENT privileged bir intent'tir; Developer Portal'dan açılmış olmalı.
    let mut intents = serenity::GatewayIntents::non_privileged()
        | serenity::GatewayIntents::GUILD_MESSAGES
        | serenity::GatewayIntents::MESSAGE_CONTENT;
    // Presence Intent yalnızca çevrimiçi sayaçları için; Developer Portal'da açıksa istenir.
    if presence_intent {
        intents |= serenity::GatewayIntents::GUILD_PRESENCES;
        info!("Presence Intent açık: çevrimiçi sayaçları kullanılabilir");
    }
    statchannels::set_presence_intent(presence_intent);
    if members_intent {
        intents |= serenity::GatewayIntents::GUILD_MEMBERS;
    }

    let plugin_settings = settings::PluginSettings::default();
    let modlog = Arc::new(ModLog::new(plugin_settings.clone()));
    let guild_configs = GuildConfigs::default();
    let panel_info = Arc::new(panel::PanelInfo::new(
        owners,
        public_url.clone(),
        web_config.in_docker,
        Arc::clone(&songbird),
        ai_description,
        members_intent,
    ));
    let panel_for_setup = Arc::clone(&panel_info);
    let scheduler = Scheduler::new(db.clone());

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands::all()
                .into_iter()
                .map(|mut c| {
                    perms::take_required(&mut c, serenity::Permissions::empty());
                    c
                })
                .collect(),
            command_check: Some(|ctx| Box::pin(perms::check(ctx))),
            on_error: |err| Box::pin(error::on_error(err)),
            pre_command: |ctx| {
                Box::pin(async move {
                    debug!(
                        command = %ctx.command().qualified_name,
                        user = %ctx.author().id,
                        guild = ?ctx.guild_id(),
                        "Komut çalıştırılıyor"
                    );
                    ctx.set_invocation_data(std::time::Instant::now()).await;
                    // Para birimi bellekte hazır olsun (yalnızca ekonomi/profil/mağaza komutlarında).
                    if let Some(guild_id) = ctx.guild_id() {
                        let cat = ctx.command().category.as_deref();
                        let name = ctx.command().name.as_str();
                        if cat == Some("Ekonomi") || matches!(name, "profil" | "kartım" | "soygun" | "borsa" | "balık-tut" | "avlan") {
                            if let Err(e) = economy::load_settings(&ctx.data().settings, &ctx.data().db, guild_id).await {
                                debug!(guild = %guild_id, error = ?e, "Ekonomi ayarı okunamadı");
                            }
                        }
                    }
                })
            },
            // Yavaş komutlar loglanır (panelin Loglar sayfasında görünür); hız sorunları ölçülebilsin.
            post_command: |ctx| {
                Box::pin(async move {
                    let Some(started) = ctx.invocation_data::<std::time::Instant>().await.map(|s| *s) else {
                        return;
                    };
                    let ms = started.elapsed().as_millis();
                    let name = ctx.command().qualified_name.as_str();
                    let interactive = timings::is_interactive(name);
                    if !interactive {
                        timings::record(name, u32::try_from(ms).unwrap_or(u32::MAX));
                    }
                    if ms >= SLOW_COMMAND_MS && !interactive {
                        warn!(command = %ctx.command().qualified_name, ms, "Yavaş komut");
                    } else {
                        debug!(command = %ctx.command().qualified_name, ms, "Komut tamamlandı");
                    }
                })
            },
            event_handler: |ctx, event, framework, data| {
                Box::pin(events::handle(ctx, event, framework, data))
            },
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            Box::pin(async move {
                // İtiraz sunucularına (yasaklıların bekleme odası) komut kaydedilmez.
                let appeal_guilds: Vec<i64> =
                    sqlx::query_scalar("SELECT appeal_guild_id FROM guild_settings WHERE appeal_guild_id IS NOT NULL")
                        .fetch_all(&db)
                        .await?;
                let guilds: Vec<serenity::GuildId> = ready
                    .guilds
                    .iter()
                    .map(|g| g.id)
                    .filter(|g| !appeal_guilds.iter().any(|a| u64::try_from(*a).ok() == Some(g.get())))
                    .collect();
                commands::register(ctx, framework, dev_guild_id, &guilds, &db, &plugin_settings).await?;
                info!(link = %commands::invite_url(ready.user.id), "Botu sunucuya ekleme bağlantısı");
                // Silinen/düzenlenen mesajları loglayabilmek için kanal başına son mesajlar tutulur.
                ctx.cache.set_max_messages(MESSAGE_CACHE_PER_CHANNEL);
                // Müzik kartı ikonları (botun kendi emojileri); açılışı geciktirmesin diye arka planda.
                {
                    let http = Arc::clone(&ctx.http);
                    tokio::spawn(async move { music::icons::ensure_uploaded(&http).await });
                }
                // Panelden ayarlanan durum / etkinlik yeniden uygulanır.
                if let Some(saved) = presence::load(&redis).await {
                    ctx.set_presence(saved.activity_data(), saved.online_status());
                }
                let (event_bus, _) = tokio::sync::broadcast::channel(256);
                // Açılışta PgPool ve ağ yükünü dağıtmak için arka plan görevleri kademeli (staggered: 5-65 sn) başlar.
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    let streams = Arc::clone(&streams);
                    let settings = plugin_settings.clone();
                    let http_client = http_client.clone();
                    let event_bus = event_bus.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        streams::spawn_poller(ctx, db, streams, settings, http_client, event_bus, stream_poll);
                    }
                });
                tokio::spawn({
                    let http = Arc::clone(&ctx.http);
                    let db = db.clone();
                    let http_client = http_client.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        youtube::spawn_poller(http, db, http_client);
                    }
                });
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(15)).await;
                        statchannels::spawn_updater(ctx, db);
                    }
                });
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    let settings = plugin_settings.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(20)).await;
                        birthdays::spawn_checker(ctx, db, settings);
                    }
                });
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    let settings = plugin_settings.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(25)).await;
                        scheduler::spawn_recurring(ctx, db, settings);
                    }
                });
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    let settings = plugin_settings.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                        feeds::spawn_poller(ctx, db, settings);
                    }
                });
                tokio::spawn({
                    let news_ctx = news::NewsContext {
                        ctx: ctx.clone(),
                        db: db.clone(),
                        redis: redis.clone(),
                        settings: plugin_settings.clone(),
                        ai: Arc::clone(&ai),
                    };
                    async move {
                        tokio::time::sleep(Duration::from_secs(35)).await;
                        news::spawn(news_ctx);
                    }
                });
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    let settings = plugin_settings.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(40)).await;
                        rsvp::spawn(ctx, db, settings);
                    }
                });
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    let settings = plugin_settings.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(45)).await;
                        backup::spawn_auto(ctx, db, settings);
                    }
                });
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    let redis = redis.clone();
                    let settings = plugin_settings.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(50)).await;
                        economy::spawn_interest(ctx, db, redis, settings);
                    }
                });
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    let redis = redis.clone();
                    let settings = plugin_settings.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(55)).await;
                        privacy::spawn_purge(ctx, db, redis, settings);
                    }
                });
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    let redis = redis.clone();
                    let settings = plugin_settings.clone();
                    let modlog = Arc::clone(&modlog);
                    async move {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        tickets::spawn_auto_close(ctx, db, redis, settings, modlog);
                    }
                });
                if let Err(e) = tickets::load_awaiting(&db).await {
                    tracing::warn!(error = ?e, "Cevap bekleyen biletler yüklenemedi");
                }
                alerts::spawn(Arc::clone(&ctx.http), http_client.clone(), alert_owners);
                let leveling = Arc::new(leveling::Leveling::new());
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    let redis = redis.clone();
                    let settings = plugin_settings.clone();
                    let leveling = Arc::clone(&leveling);
                    async move {
                        tokio::time::sleep(Duration::from_secs(65)).await;
                        community::spawn_seasons(ctx, db, redis, settings, leveling);
                    }
                });
                tokio::spawn({
                    let ctx = ctx.clone();
                    let db = db.clone();
                    let redis = redis.clone();
                    let settings = plugin_settings.clone();
                    let leveling = Arc::clone(&leveling);
                    async move {
                        tokio::time::sleep(Duration::from_secs(70)).await;
                        analytics::spawn_voice_tracker(ctx, db, redis, settings, leveling);
                    }
                });
                let autoresponder = Arc::new(autoresponder::AutoResponder::default());
                let tags = Arc::new(tags::Tags::default());
                web::spawn(
                    web_config.bind,
                    web::WebState {
                        http: Arc::clone(&ctx.http),
                        cache: Arc::clone(&ctx.cache),
                        db: db.clone(),
                        redis: redis.clone(),
                        configs: guild_configs.clone(),
                        panel: panel_for_setup,
                        leveling: Arc::clone(&leveling),
                        settings: plugin_settings.clone(),
                        automod: Arc::clone(&automod),
                        modlog: Arc::clone(&modlog),
                        ai: Arc::clone(&ai),
                        streams: Arc::clone(&streams),
                        autoresponder: Arc::clone(&autoresponder),
                        http_client: http_client.clone(),
                        tags: Arc::clone(&tags),
                        scheduler: Arc::clone(&scheduler),
                        music: music.clone(),
                        event_bus: event_bus.clone(),
                    },
                );
                if let Some(metrics) = &web_config.tunnel_metrics {
                    panel::spawn_tunnel_watcher(http_client.clone(), metrics.clone(), public_url.clone());
                }
                scheduler.spawn(Executor {
                    http: Arc::clone(&ctx.http),
                    modlog: Arc::clone(&modlog),
                    bot_id: ready.user.id,
                });
                Ok(Data {
                    db,
                    redis,
                    automod,
                    modlog,
                    guild_configs,
                    leveling,
                    settings: plugin_settings,
                    ai,
                    streams,
                    music,
                    scheduler,
                    tags,
                    invites: invites::Invites::default(),
                    tempvoice,
                    animals: games::animals::Animals::default(),
                    game_channels: dashmap::DashSet::new(),
                    autoresponder,
                    public_url,
                    dev_guild_id,
                    http_client,
                    members_intent,
                    started_at: std::time::Instant::now(),
                    event_bus,
                })
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new_with_http(http, intents)
        .register_songbird_with(songbird)
        .framework(framework)
        .await
        .context("Discord istemcisi oluşturulamadı")?;

    // Ctrl+C (SIGINT) veya `docker stop` (SIGTERM) gelince shard'ları kapat; ardından aşağıda
    // bekleyen XP yazılır ve veritabanı havuzu kapatılır.
    let shard_manager = Arc::clone(&client.shard_manager);
    // Panel: gecikme bilgisi ve yeniden başlatma.
    let _ = panel_info.shards.set(Arc::clone(&client.shard_manager));
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("Kapatma sinyali alındı, shard'lar kapatılıyor...");
        shard_manager.shutdown_all().await;
    });

    info!("Discord gateway'ine bağlanılıyor...");
    let result = client.start_autosharded().await;
    if let Err(serenity::Error::Gateway(serenity::GatewayError::DisallowedGatewayIntents)) = &result {
        error!(
            "Discord izin verilmeyen intent bildirdi: Developer Portal > Bot > \
             'Message Content Intent' seçeneğini açıp kaydedin"
        );
    }
    let result = result.context("gateway bağlantısı başarısız oldu");

    // Bekleyen XP'yi kaybetmemek için son bir kez PostgreSQL'e yaz.
    match leveling::flush(&db_for_shutdown, &redis_for_shutdown).await {
        Ok(n) => info!(records = n, "Bekleyen XP kaydedildi"),
        Err(e) => error!(error = ?e, "Kapanışta XP kaydedilemedi (Redis'te duruyor, sonraki açılışta yazılır)"),
    }

    match analytics::flush(&db_for_shutdown, &redis_for_shutdown).await {
        Ok(n) => info!(records = n, "Bekleyen analitik verileri kaydedildi"),
        Err(e) => error!(error = ?e, "Kapanışta analitik verileri kaydedilemedi (Redis'te duruyor, sonraki açılışta yazılır)"),
    }

    // Hata olsa bile açık PostgreSQL bağlantılarını düzgünce kapat.
    db_for_shutdown.close().await;
    info!("PostgreSQL havuzu kapatıldı");

    result
}

/// Kapatma isteği gelene kadar bekler: Ctrl+C, Unix'te SIGTERM (`docker stop`),
/// Windows'ta konsol penceresinin kapatılması / oturum kapatma / bilgisayarı kapatma.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!(error = %e, "Ctrl+C sinyali dinlenemedi");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(e) => {
                error!(error = %e, "SIGTERM dinlenemedi");
                std::future::pending::<()>().await;
            }
        }
    };
    // Konsol penceresi kapatılınca, oturum kapanınca veya bilgisayar kapanırken.
    // Windows bu durumlarda ~5 sn tanır; kapanış (XP yazma dahil) bundan kısa sürer.
    #[cfg(windows)]
    let terminate = async {
        use tokio::signal::windows::{ctrl_close, ctrl_logoff, ctrl_shutdown};
        match (ctrl_close(), ctrl_shutdown(), ctrl_logoff()) {
            (Ok(mut close), Ok(mut shutdown), Ok(mut logoff)) => {
                tokio::select! {
                    _ = close.recv() => {}
                    _ = shutdown.recv() => {}
                    _ = logoff.recv() => {}
                }
            }
            _ => {
                error!("Windows kapatma sinyalleri dinlenemedi");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(any(unix, windows)))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
