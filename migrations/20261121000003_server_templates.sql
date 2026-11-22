-- Xenon tarzı sunucu şablonları ve HTML bilet transkriptleri
ALTER TABLE tickets ADD COLUMN IF NOT EXISTS html_transcript TEXT;

CREATE TABLE IF NOT EXISTS server_templates (
    id              TEXT PRIMARY KEY,
    guild_id        BIGINT,
    creator_id      BIGINT,
    name            TEXT NOT NULL,
    description     TEXT,
    category        TEXT NOT NULL DEFAULT 'genel',
    data            JSONB NOT NULL,
    uses_count      INTEGER NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_server_templates_category ON server_templates (category);
CREATE INDEX IF NOT EXISTS idx_server_templates_guild ON server_templates (guild_id);
