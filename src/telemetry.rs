//! `tracing` abonesinin kurulumu: konsol + (opsiyonel) günlük dönen log dosyaları.

use std::io::IsTerminal as _;

use anyhow::Context as _;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{fmt, EnvFilter};

/// RUST_LOG tanımsız veya geçersizse kullanılacak filtre.
const DEFAULT_FILTER: &str = "info,serenity=warn,sqlx=warn,songbird=warn";
/// Dosya loglarında tutulacak gün sayısı.
const MAX_LOG_FILES: usize = 7;

/// Program bitene kadar yaşamalı; düşürülünce kuyruktaki loglar dosyaya yazılır.
pub struct LogGuard(#[allow(dead_code)] Option<WorkerGuard>);

/// `LOG_DIR` doluysa loglar ayrıca `LOG_DIR/bot.YYYY-MM-DD.log` dosyalarına yazılır.
pub fn init() -> anyhow::Result<LogGuard> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    let console = fmt::layer()
        .with_target(true)
        // Çıktı bir dosyaya/boruya yönlendirildiyse renk kodları yazılmasın.
        .with_ansi(std::io::stdout().is_terminal())
        .compact();

    let log_dir = std::env::var("LOG_DIR")
        .ok()
        .map(|d| d.trim().to_owned())
        .filter(|d| !d.is_empty());

    let (file, guard) = match log_dir {
        Some(dir) => {
            let appender = RollingFileAppender::builder()
                .rotation(Rotation::DAILY)
                .filename_prefix("bot")
                .filename_suffix("log")
                .max_log_files(MAX_LOG_FILES)
                .build(&dir)
                .with_context(|| format!("log klasörü kullanılamıyor: {dir}"))?;
            // Diske yazma ayrı bir thread'de yapılır; async thread'ler beklemez.
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_target(true)
                .compact();
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(file)
        // Web panelindeki "Loglar" sayfası için son satırlar bellekte tutulur.
        .with(crate::logbuffer::PanelLayer)
        .try_init()
        .map_err(|e| anyhow::anyhow!(e))?;

    Ok(LogGuard(guard))
}
