-- ===================== Sunucu ayarları: doğum günü ve geçici odalar =====================
ALTER TABLE guild_settings
    ADD COLUMN IF NOT EXISTS birthday_channel_id  BIGINT,
    ADD COLUMN IF NOT EXISTS birthday_role_id     BIGINT,
    ADD COLUMN IF NOT EXISTS tempvoice_hub_id     BIGINT;   -- girilince oda açılan ses kanalı

-- ===================== Davet takibi =====================
-- Üye başına son giriş; tekrar girişte satır güncellenir.
CREATE TABLE IF NOT EXISTS invite_joins (
    guild_id    BIGINT      NOT NULL,
    user_id     BIGINT      NOT NULL,
    inviter_id  BIGINT,                 -- bilinmiyorsa (özel URL, izin yok) NULL
    code        TEXT,
    fake        BOOLEAN     NOT NULL DEFAULT FALSE,   -- 7 günden yeni hesap
    joined_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    left_at     TIMESTAMPTZ,
    PRIMARY KEY (guild_id, user_id)
);
CREATE INDEX IF NOT EXISTS invite_joins_inviter_idx ON invite_joins (guild_id, inviter_id);

-- ===================== İstatistik (sayaç) kanalları =====================
CREATE TABLE IF NOT EXISTS stat_channels (
    channel_id  BIGINT  PRIMARY KEY,
    guild_id    BIGINT  NOT NULL,
    kind        TEXT    NOT NULL,   -- members, humans, bots, boosts, goal
    template    TEXT    NOT NULL,   -- {sayı} ve (hedefte) {hedef} içerir
    goal        INTEGER,
    last_name   TEXT                -- gereksiz yeniden adlandırmayı önler
);

-- ===================== Doğum günleri =====================
CREATE TABLE IF NOT EXISTS birthdays (
    guild_id        BIGINT   NOT NULL,
    user_id         BIGINT   NOT NULL,
    month           SMALLINT NOT NULL,
    day             SMALLINT NOT NULL,
    last_announced  INTEGER,          -- kutlandığı son yıl
    role_given_on   DATE,             -- doğum günü rolü verildiği gün (ertesi gün alınır)
    PRIMARY KEY (guild_id, user_id)
);

-- ===================== Geçici ses odaları =====================
CREATE TABLE IF NOT EXISTS temp_voice_channels (
    channel_id  BIGINT      PRIMARY KEY,
    guild_id    BIGINT      NOT NULL,
    owner_id    BIGINT      NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ===================== YouTube yeni video duyuruları =====================
CREATE TABLE IF NOT EXISTS youtube_subscriptions (
    guild_id        BIGINT NOT NULL,
    yt_channel_id   TEXT   NOT NULL,    -- UC… biçimi
    name            TEXT   NOT NULL,
    channel_id      BIGINT NOT NULL,    -- duyuru kanalı
    role_id         BIGINT,
    last_video_id   TEXT,
    PRIMARY KEY (guild_id, yt_channel_id)
);
