-- Web formundan ban itirazı: yasak DM'indeki bağlantı kişiye ve yasağa özel, tek kullanımlık anahtar taşır.
CREATE TABLE IF NOT EXISTS appeal_tokens (
    token       TEXT        PRIMARY KEY,
    guild_id    BIGINT      NOT NULL,
    user_id     BIGINT      NOT NULL,
    used        BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
