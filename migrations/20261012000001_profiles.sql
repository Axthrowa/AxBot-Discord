-- Profil sistemi: itibar puanı ve evlilikler.
CREATE TABLE IF NOT EXISTS member_profiles (
    guild_id BIGINT NOT NULL,
    user_id  BIGINT NOT NULL,
    rep      BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (guild_id, user_id)
);

CREATE TABLE IF NOT EXISTS marriages (
    guild_id   BIGINT NOT NULL,
    user_a     BIGINT NOT NULL,
    user_b     BIGINT NOT NULL,
    married_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_a)
);
CREATE UNIQUE INDEX IF NOT EXISTS marriages_b ON marriages (guild_id, user_b);
