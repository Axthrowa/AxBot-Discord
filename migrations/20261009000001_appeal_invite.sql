-- Ban itirazı: yasaklının botla ortak sunucusu kalmadığı için DM butonu çalışmaz.
-- Ayrı bir "itiraz sunucusu"nun davet linki yasak DM'inde gösterilir.
ALTER TABLE guild_settings ADD COLUMN IF NOT EXISTS appeal_invite TEXT;
