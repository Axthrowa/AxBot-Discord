-- ===================== Sunucu ayarları genişletmesi =====================
ALTER TABLE guild_settings
    ADD COLUMN IF NOT EXISTS event_log_channel_id  BIGINT,                      -- mesaj/üye/ses olay logları
    ADD COLUMN IF NOT EXISTS welcome_channel_id    BIGINT,
    ADD COLUMN IF NOT EXISTS welcome_message       TEXT,
    ADD COLUMN IF NOT EXISTS goodbye_channel_id    BIGINT,
    ADD COLUMN IF NOT EXISTS goodbye_message       TEXT,
    ADD COLUMN IF NOT EXISTS autorole_id           BIGINT,
    ADD COLUMN IF NOT EXISTS min_account_age_days  INTEGER NOT NULL DEFAULT 0,  -- 0 = kapalı
    ADD COLUMN IF NOT EXISTS raid_join_threshold   INTEGER NOT NULL DEFAULT 0;  -- 10 sn'de N giriş; 0 = kapalı

-- ===================== Uyarı sayısına göre otomatik ceza =====================
CREATE TABLE IF NOT EXISTS warn_punishments (
    guild_id       BIGINT  NOT NULL,
    warn_count     INTEGER NOT NULL,
    action         TEXT    NOT NULL,   -- timeout, kick, ban
    duration_secs  BIGINT,             -- timeout süresi veya süreli ban
    PRIMARY KEY (guild_id, warn_count)
);

-- ===================== Zamanlanmış görevler =====================
-- Çekiliş bitişi, hatırlatma, süreli ban kaldırma. Bot kapalıyken gelen görevler açılışta çalışır.
CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id          BIGSERIAL   PRIMARY KEY,
    run_at      TIMESTAMPTZ NOT NULL,
    payload     JSONB       NOT NULL,
    attempts    INTEGER     NOT NULL DEFAULT 0,   -- başarısız denemeler (5'te vazgeçilir)
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS scheduled_tasks_run_at_idx ON scheduled_tasks (run_at);

-- ===================== Çekilişler =====================
CREATE TABLE IF NOT EXISTS giveaways (
    id            BIGSERIAL   PRIMARY KEY,
    guild_id      BIGINT      NOT NULL,
    channel_id    BIGINT      NOT NULL,
    message_id    BIGINT,
    host_id       BIGINT      NOT NULL,
    prize         TEXT        NOT NULL,
    winner_count  INTEGER     NOT NULL,
    ends_at       TIMESTAMPTZ NOT NULL,
    ended         BOOLEAN     NOT NULL DEFAULT FALSE,
    winners       BIGINT[]    NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS giveaway_entries (
    giveaway_id  BIGINT NOT NULL REFERENCES giveaways (id) ON DELETE CASCADE,
    user_id      BIGINT NOT NULL,
    PRIMARY KEY (giveaway_id, user_id)
);
