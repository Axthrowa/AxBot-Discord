-- Statbot analitik toplayıcısı ve kalıcı Ses XP motoru tabloları

-- 1. Günlük sunucu genel analitik özeti
CREATE TABLE IF NOT EXISTS server_daily_stats (
    guild_id BIGINT NOT NULL,
    stat_date DATE NOT NULL,
    messages_count BIGINT NOT NULL DEFAULT 0,
    voice_seconds BIGINT NOT NULL DEFAULT 0,
    members_joined INT NOT NULL DEFAULT 0,
    members_left INT NOT NULL DEFAULT 0,
    PRIMARY KEY (guild_id, stat_date)
);

-- 2. Kanal bazlı günlük mesaj ve ses süresi istatistikleri
CREATE TABLE IF NOT EXISTS channel_daily_stats (
    guild_id BIGINT NOT NULL,
    channel_id BIGINT NOT NULL,
    stat_date DATE NOT NULL,
    is_voice BOOLEAN NOT NULL DEFAULT FALSE,
    messages_count BIGINT NOT NULL DEFAULT 0,
    voice_seconds BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (guild_id, channel_id, stat_date)
);

-- 3. Üye bazlı günlük ses ve mesaj istatistikleri
CREATE TABLE IF NOT EXISTS member_daily_stats (
    guild_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL,
    stat_date DATE NOT NULL,
    messages_count INT NOT NULL DEFAULT 0,
    voice_seconds INT NOT NULL DEFAULT 0,
    voice_xp_earned BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (guild_id, user_id, stat_date)
);

-- 4. Üyenin toplam ses istatistikleri ve kalıcı ses seviyesi
CREATE TABLE IF NOT EXISTS member_voice_stats (
    guild_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL,
    total_voice_seconds BIGINT NOT NULL DEFAULT 0,
    total_voice_xp BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_id)
);

-- Performans İndeksleri
CREATE INDEX IF NOT EXISTS idx_server_daily_stats_date ON server_daily_stats (guild_id, stat_date DESC);
CREATE INDEX IF NOT EXISTS idx_channel_daily_stats_date ON channel_daily_stats (guild_id, stat_date DESC);
CREATE INDEX IF NOT EXISTS idx_member_daily_stats_date ON member_daily_stats (guild_id, stat_date DESC);
CREATE INDEX IF NOT EXISTS idx_member_voice_stats_xp ON member_voice_stats (guild_id, total_voice_xp DESC);
