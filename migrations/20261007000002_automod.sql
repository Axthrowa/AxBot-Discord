-- Sunucu bazlı auto-mod ayarları. Satır yoksa koddaki varsayılanlar geçerlidir.
CREATE TABLE IF NOT EXISTS automod_settings (
    guild_id           BIGINT      PRIMARY KEY,
    enabled            BOOLEAN     NOT NULL DEFAULT TRUE,
    block_invites      BOOLEAN     NOT NULL DEFAULT TRUE,
    block_links        BOOLEAN     NOT NULL DEFAULT FALSE,
    filter_profanity   BOOLEAN     NOT NULL DEFAULT TRUE,
    anti_spam          BOOLEAN     NOT NULL DEFAULT TRUE,
    spam_max_messages  INTEGER     NOT NULL DEFAULT 5,
    spam_window_secs   INTEGER     NOT NULL DEFAULT 5,
    exempt_staff       BOOLEAN     NOT NULL DEFAULT TRUE,
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Sunucuya özel yasaklı kelimeler (varsayılan listeye ek). `*` = kelimenin devamı serbest.
CREATE TABLE IF NOT EXISTS automod_words (
    guild_id    BIGINT      NOT NULL,
    word        TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, word)
);
