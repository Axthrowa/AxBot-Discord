-- Botun yapay zekâ ile yanıt verdiği kanallar.
CREATE TABLE IF NOT EXISTS ai_channels (
    guild_id    BIGINT      NOT NULL,
    channel_id  BIGINT      NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, channel_id)
);

-- Twitch / Kick yayın duyuru abonelikleri.
CREATE TABLE IF NOT EXISTS stream_subscriptions (
    id                BIGSERIAL   PRIMARY KEY,
    guild_id          BIGINT      NOT NULL,
    platform          TEXT        NOT NULL,    -- twitch, kick
    login             TEXT        NOT NULL,    -- kanal adı (küçük harf)
    channel_id        BIGINT      NOT NULL,    -- duyurunun yapılacağı Discord kanalı
    role_id           BIGINT,                  -- duyuruda etiketlenecek rol
    last_stream_id    TEXT,                    -- aynı yayının iki kez duyurulmaması için
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (guild_id, platform, login)
);
