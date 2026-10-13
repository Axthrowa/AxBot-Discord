-- İtiraz sunucusunun kimliği: itirazı kabul edilip ana sunucuya dönenler oradan otomatik atılır.
ALTER TABLE guild_settings ADD COLUMN IF NOT EXISTS appeal_guild_id BIGINT;
