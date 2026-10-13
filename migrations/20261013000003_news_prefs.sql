-- Kişisel haber tercihleri: üye kendi kategorilerini seçer, isterse özelden (anlık / günlük) alır.
CREATE TABLE IF NOT EXISTS news_prefs (
    guild_id   BIGINT      NOT NULL,
    user_id    BIGINT      NOT NULL,
    categories TEXT[]      NOT NULL DEFAULT '{}',
    dm         TEXT        NOT NULL DEFAULT 'off',   -- off, instant, daily
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_id)
);
CREATE INDEX IF NOT EXISTS news_prefs_dm_idx ON news_prefs (guild_id) WHERE dm <> 'off';
