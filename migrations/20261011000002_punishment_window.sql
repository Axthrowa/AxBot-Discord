-- Otomatik ceza kuralına isteğe bağlı zaman penceresi: "son 1 saatte 3 uyarı" gibi.
-- Boşsa (eski kurallar) tüm aktif uyarılar sayılır.
ALTER TABLE warn_punishments ADD COLUMN IF NOT EXISTS window_secs BIGINT;
