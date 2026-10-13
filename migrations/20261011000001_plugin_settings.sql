-- Eklenti ayarları: her eklentinin sunucuya özel ayarı tek satırda JSON olarak tutulur.
-- Yeni ayar eklemek tablo değişikliği gerektirmez; eksik alanlar kodda varsayılanla doldurulur.
CREATE TABLE IF NOT EXISTS plugin_settings (
    guild_id    BIGINT      NOT NULL,
    plugin      TEXT        NOT NULL,
    data        TEXT        NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, plugin)
);
