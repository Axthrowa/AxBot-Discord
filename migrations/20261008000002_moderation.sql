-- Sunucu başına artan vaka numarası (Vaka #1, #2, ...). Atomik artırma için guild_settings'te tutulur.
ALTER TABLE guild_settings ADD COLUMN IF NOT EXISTS case_counter INTEGER NOT NULL DEFAULT 0;

-- Tüm moderasyon işlemlerinin kalıcı kaydı.
CREATE TABLE IF NOT EXISTS mod_cases (
    id             BIGSERIAL   PRIMARY KEY,
    guild_id       BIGINT      NOT NULL,
    case_number    INTEGER     NOT NULL,
    action         TEXT        NOT NULL,   -- ban, unban, kick, timeout, untimeout, warn
    target_id      BIGINT      NOT NULL,
    moderator_id   BIGINT      NOT NULL,
    reason         TEXT,
    duration_secs  BIGINT,                 -- sadece timeout için
    -- Uyarılar /uyarı-sil ile pasifleştirilir; geçmiş korunur.
    active         BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (guild_id, case_number)
);

-- /uyarılar ve sicil sorguları için.
CREATE INDEX IF NOT EXISTS mod_cases_target_idx ON mod_cases (guild_id, target_id, created_at DESC);
