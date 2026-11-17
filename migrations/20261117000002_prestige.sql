-- Prestij: en üst seviyeye ulaşan üye XP'sini sıfırlayıp prestij sayısını artırır.
CREATE TABLE IF NOT EXISTS member_prestige (
    guild_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL,
    prestige INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (guild_id, user_id)
);
