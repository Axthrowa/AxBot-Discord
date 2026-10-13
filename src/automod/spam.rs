//! Redis tabanlı sabit pencereli mesaj sayacı.
//!
//! Sayaç Redis'te tutulduğu için bot yeniden başlasa veya birden fazla shard/instance
//! çalışsa da tutarlıdır.

use poise::serenity_prelude::{GuildId, UserId};
use redis::aio::ConnectionManager;

/// Kullanıcının mevcut penceredeki mesaj sayısını bir artırıp yeni değeri döner.
/// Pencere ilk mesajla başlar ve `window_secs` sonra kendiliğinden sıfırlanır.
pub async fn register_message(
    redis: &ConnectionManager,
    guild_id: GuildId,
    user_id: UserId,
    window_secs: i32,
) -> redis::RedisResult<i64> {
    let key = format!("automod:spam:{guild_id}:{user_id}");
    let mut conn = redis.clone();

    // SET NX EX + INCR tek atomik pipeline: tek gidiş-dönüş.
    let (count,): (i64,) = redis::pipe()
        .atomic()
        .cmd("SET")
        .arg(&key)
        .arg(0)
        .arg("EX")
        .arg(window_secs)
        .arg("NX")
        .ignore()
        .cmd("INCR")
        .arg(&key)
        .query_async(&mut conn)
        .await?;

    Ok(count)
}
