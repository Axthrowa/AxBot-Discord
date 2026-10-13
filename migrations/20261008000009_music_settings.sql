-- Müzik: 7/24 modu ve otomatik çalma (sunucu başına, kalıcı).
ALTER TABLE guild_settings
    ADD COLUMN IF NOT EXISTS music_247       BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS music_autoplay  BOOLEAN NOT NULL DEFAULT FALSE;
