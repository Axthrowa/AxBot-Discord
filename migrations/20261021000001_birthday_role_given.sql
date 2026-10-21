-- Verilen doğum günü rolünün kimliği: ayar sonradan değişse de ertesi gün doğru rol geri alınır.
ALTER TABLE birthdays ADD COLUMN IF NOT EXISTS role_given_id BIGINT;
