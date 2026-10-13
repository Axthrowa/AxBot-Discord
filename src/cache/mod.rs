//! Redis bağlantısı.
//!
//! Redis tek iş parçacıklı olduğu için klasik bir havuz yerine `ConnectionManager`
//! kullanıyoruz: tek bir multiplexed TCP bağlantısı üzerinden tüm görevlerin komutlarını
//! pipeline eder, kopyalaması ucuzdur (içeride `Arc`) ve bağlantı koparsa kendisi yeniden bağlanır.

use std::time::{Duration, Instant};

use anyhow::Context as _;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use tracing::info;

pub async fn connect(url: &str) -> anyhow::Result<ConnectionManager> {
    let client = redis::Client::open(url).context("REDIS_URL geçersiz")?;

    let config = ConnectionManagerConfig::new()
        .set_connection_timeout(Some(Duration::from_secs(5)))
        .set_response_timeout(Some(Duration::from_secs(2)))
        .set_number_of_retries(5);

    let manager = ConnectionManager::new_with_config(client, config)
        .await
        .context("Redis'e bağlanılamadı")?;

    info!("Redis bağlantısı hazır");
    Ok(manager)
}

/// `PING` komutunun gidiş-dönüş süresini ölçer.
pub async fn ping(redis: &ConnectionManager) -> anyhow::Result<Duration> {
    let mut conn = redis.clone();
    let started = Instant::now();
    let _: String = redis::cmd("PING").query_async(&mut conn).await?;
    Ok(started.elapsed())
}
