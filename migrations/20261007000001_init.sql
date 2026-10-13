-- Sunucu bazlı ayarlar. Sonraki adımlar (log kanalı, auto-mod, ticket vb.) bu tabloyu genişletecek.
-- Discord snowflake ID'leri u64'tür; BIGINT (i64) aralığı gerçek ID'ler için yeterlidir.
CREATE TABLE IF NOT EXISTS guild_settings (
    guild_id        BIGINT      PRIMARY KEY,
    log_channel_id  BIGINT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
