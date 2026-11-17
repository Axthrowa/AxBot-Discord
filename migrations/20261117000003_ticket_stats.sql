-- Bilet istatistikleri: yetkilinin ilk cevabı.
ALTER TABLE tickets ADD COLUMN IF NOT EXISTS first_response_at TIMESTAMPTZ;
ALTER TABLE tickets ADD COLUMN IF NOT EXISTS first_responder BIGINT;
CREATE INDEX IF NOT EXISTS tickets_guild_created_idx ON tickets (guild_id, created_at);
