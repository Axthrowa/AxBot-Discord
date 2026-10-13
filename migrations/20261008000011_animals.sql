-- Hayvan yakalama oyunu.
ALTER TABLE guild_settings ADD COLUMN IF NOT EXISTS animal_channel_id BIGINT;

CREATE TABLE IF NOT EXISTS animal_catches (
    guild_id  BIGINT  NOT NULL,
    user_id   BIGINT  NOT NULL,
    animal    TEXT    NOT NULL,
    count     INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (guild_id, user_id, animal)
);
