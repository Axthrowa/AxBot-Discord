-- Moderatör notları (/üye-notu): üyeye bildirilmez, ceza değildir.
CREATE TABLE IF NOT EXISTS mod_notes (
    id         BIGSERIAL PRIMARY KEY,
    guild_id   BIGINT NOT NULL,
    user_id    BIGINT NOT NULL,
    author_id  BIGINT NOT NULL,
    note       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS mod_notes_member ON mod_notes (guild_id, user_id);
