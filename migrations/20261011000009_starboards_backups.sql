-- Ek starboard panolarındaki kopyalar (mesaj + pano başına bir kayıt).
CREATE TABLE IF NOT EXISTS starboard_extra_posts (
    message_id       BIGINT NOT NULL,
    board_channel_id BIGINT NOT NULL,
    guild_id         BIGINT NOT NULL,
    star_message_id  BIGINT NOT NULL,
    PRIMARY KEY (message_id, board_channel_id)
);

-- Sunucu yapısı yedekleri (/yedek).
CREATE TABLE IF NOT EXISTS guild_backups (
    id         BIGSERIAL PRIMARY KEY,
    guild_id   BIGINT NOT NULL,
    name       TEXT NOT NULL,
    data       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS guild_backups_guild ON guild_backups (guild_id);
