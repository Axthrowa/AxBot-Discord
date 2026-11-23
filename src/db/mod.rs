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
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(10))
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
/// sonsuza kadar meşgul etmez. 500 ms üzeri yavaş sorgular Warn seviyesinde loglanır.
fn connect_options(url: &str) -> anyhow::Result<PgConnectOptions> {
    use sqlx::ConnectOptions;
    let mut options: PgConnectOptions = url.parse().context("DATABASE_URL geçersiz")?;
    options = options.options([("statement_timeout", "15000"), ("lock_timeout", "5000"), ("idle_in_transaction_session_timeout", "60000")]);
    options = options.log_slow_statements(tracing::log::LevelFilter::Warn, Duration::from_millis(500));
    Ok(options)
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

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// Geçici kapsayıcıları test bitince (başarısız olsa da) siler.
    struct Containers(&'static [&'static str]);
    impl Drop for Containers {
        fn drop(&mut self) {
            let _ = Command::new("docker").args(["rm", "-f"]).args(self.0).output();
        }
    }

    fn docker(args: &[&str]) -> anyhow::Result<()> {
        let out = Command::new("docker").args(args).output()?;
        anyhow::ensure!(out.status.success(), "docker {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        Ok(())
    }

    async fn wait_until<F, Fut>(limit: Duration, mut check: F) -> Option<Duration>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let started = Instant::now();
        while started.elapsed() < limit {
            if check().await {
                return Some(started.elapsed());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        None
    }

    /// Veritabanı ve Redis kapanıp geri gelince botun bağlantıları kendiliğinden toparlanıyor mu?
    /// Botun kapsayıcılarına dokunmaz: yerel imajlardan ayrı portlarda geçici kapsayıcı açar.
    #[tokio::test]
    #[ignore = "Docker gerektirir; geçici postgres ve redis kapsayıcısı açıp kapatır"]
    async fn live_connections_recover_after_outage() -> anyhow::Result<()> {
        const PG: &str = "axbot-toparlanma-pg";
        const REDIS: &str = "axbot-toparlanma-redis";
        let _guard = Containers(&[PG, REDIS]);
        docker(&["run", "-d", "--name", PG, "-e", "POSTGRES_PASSWORD=deneme", "-p", "127.0.0.1:55432:5432", "postgres:17-alpine"])?;
        docker(&["run", "-d", "--name", REDIS, "-p", "127.0.0.1:56379:6379", "redis:8-alpine"])?;

        let config = crate::config::DatabaseConfig { url: "postgres://postgres:deneme@127.0.0.1:55432/postgres".into(), max_connections: 5 };
        let started = Instant::now();
        let pool = loop {
            match super::connect(&config).await {
                Ok(pool) => break pool,
                Err(e) if started.elapsed() > Duration::from_secs(60) => return Err(e),
                Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        };
        let redis = crate::cache::connect("redis://127.0.0.1:56379").await?;
        super::ping(&pool).await?;
        crate::cache::ping(&redis).await?;

        docker(&["stop", "-t", "1", PG, REDIS])?;
        anyhow::ensure!(super::ping(&pool).await.is_err(), "veritabanı kapalıyken sorgu başarılı olmamalı");
        anyhow::ensure!(crate::cache::ping(&redis).await.is_err(), "Redis kapalıyken komut başarılı olmamalı");
        // Kesinti bir süre sürsün (bot bu arada denemeye devam eder).
        for _ in 0..5 {
            let _ = super::ping(&pool).await;
            let _ = crate::cache::ping(&redis).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        docker(&["start", PG, REDIS])?;
        let pg = wait_until(Duration::from_secs(60), || async { super::ping(&pool).await.is_ok() }).await;
        let rd = wait_until(Duration::from_secs(60), || async { crate::cache::ping(&redis).await.is_ok() }).await;
        println!("veritabanı toparlandı: {pg:?}, Redis toparlandı: {rd:?}");
        anyhow::ensure!(pg.is_some(), "veritabanı havuzu 60 sn içinde toparlanmadı");
        anyhow::ensure!(rd.is_some(), "Redis bağlantısı 60 sn içinde toparlanmadı");
        Ok(())
    }
}

#[cfg(test)]
mod live_tests {
    #[tokio::test]
    #[ignore = "yerel PostgreSQL'e bağlanır (DATABASE_URL)"]
    async fn live_query_latency() -> anyhow::Result<()> {
        let url = std::env::var("DATABASE_URL")?;
        let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect_with(super::connect_options(&url)?).await?;
        let mut times = Vec::new();
        for _ in 0..30 {
            times.push(super::ping(&pool).await?.as_micros());
        }
        times.sort_unstable();
        println!("SELECT 1 µs: ortanca {} · en yavaş {}", times[times.len() / 2], times[times.len() - 1]);
        Ok(())
    }
}
