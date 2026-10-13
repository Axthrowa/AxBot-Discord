//! PostgreSQL bağlantı havuzu ve migration'lar.

use std::time::{Duration, Instant};

use anyhow::Context as _;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use tracing::info;

use crate::config::DatabaseConfig;

/// Havuzu kurar, bağlantıyı doğrular ve bekleyen migration'ları uygular.
pub async fn connect(config: &DatabaseConfig) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(10 * 60))
        .connect_with(connect_options(&config.url)?)
        .await
        .context("PostgreSQL'e bağlanılamadı")?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("veritabanı migration'ları uygulanamadı")?;

    info!(
        max_connections = config.max_connections,
        "PostgreSQL havuzu hazır"
    );
    Ok(pool)
}

/// Her bağlantıya sorgu süre sınırları: takılan bir sorgu (kilit beklemesi vb.) komutu ve havuzu
/// sonsuza kadar meşgul etmez.
fn connect_options(url: &str) -> anyhow::Result<PgConnectOptions> {
    let options: PgConnectOptions = url.parse().context("DATABASE_URL geçersiz")?;
    Ok(options.options([("statement_timeout", "15000"), ("lock_timeout", "5000"), ("idle_in_transaction_session_timeout", "60000")]))
}

/// Havuzdan bir bağlantı alıp basit bir sorgunun gidiş-dönüş süresini ölçer.
pub async fn ping(pool: &PgPool) -> anyhow::Result<Duration> {
    let started = Instant::now();
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(started.elapsed())
}

/// Discord snowflake'ini (u64) PostgreSQL BIGINT'e (i64) çevirir.
pub fn snowflake(id: u64) -> anyhow::Result<i64> {
    i64::try_from(id).with_context(|| format!("Discord ID'si BIGINT aralığına sığmıyor: {id}"))
}
