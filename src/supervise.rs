//! Arka plan döngülerinin (doğum günü, zamanlayıcı, akışlar, XP yazıcısı…) her turunu korur.
//!
//! Bir turda panik olursa ya da tur takılıp kalırsa (yanıt vermeyen bir servis), hata loglanır ve döngü
//! bir sonraki turda devam eder. Koruma olmadan panik o görevi sonsuza dek durdururdu ve özellik bot
//! yeniden başlatılana kadar sessizce çalışmazdı.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures_util::FutureExt as _;
use tracing::error;

/// `fut`'u en fazla `limit` süre çalıştırır; panik ve zaman aşımı loglanır, dışarı taşmaz.
pub async fn round<F: Future<Output = ()>>(task: &'static str, limit: Duration, fut: F) {
    match tokio::time::timeout(limit, AssertUnwindSafe(fut).catch_unwind()).await {
        Ok(Ok(())) => {}
        Ok(Err(panic)) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "bilinmeyen".to_owned());
            error!(task, panic = %message, "Arka plan görevi bu turda çöktü; sonraki turda devam edecek");
        }
        Err(_) => error!(task, limit_secs = limit.as_secs(), "Arka plan görevi bu turda takıldı (zaman aşımı); sonraki turda devam edecek"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn survives_panic_and_hang() {
        let runs = AtomicU32::new(0);
        for _ in 0..3 {
            super::round("deneme", Duration::from_millis(50), async {
                let n = runs.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    panic!("ilk tur çöker");
                }
                if n == 1 {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            })
            .await;
        }
        // Panik ve takılma döngüyü durdurmadı: üç tur da çalıştı.
        assert_eq!(runs.load(Ordering::SeqCst), 3);
    }
}
