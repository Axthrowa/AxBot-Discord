-- Kişisel rütbe kartı ayarları (/kartım).
CREATE TABLE IF NOT EXISTS rank_cards (
    guild_id   BIGINT NOT NULL,
    user_id    BIGINT NOT NULL,
    accent     TEXT,
    background TEXT,
    font       TEXT,
    PRIMARY KEY (guild_id, user_id)
);
