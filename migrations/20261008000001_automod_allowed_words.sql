-- Sunucuya özel istisna kelimeler (varsayılan istisna listesine ek).
-- `kelime` tam kelime, `kelime*` önek olarak eşleşir.
CREATE TABLE IF NOT EXISTS automod_allowed_words (
    guild_id    BIGINT      NOT NULL,
    word        TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, word)
);
