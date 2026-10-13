-- ===================== Destek biletleri =====================
CREATE TABLE IF NOT EXISTS ticket_settings (
    guild_id          BIGINT      PRIMARY KEY,
    category_id       BIGINT,                 -- biletlerin açılacağı kategori (yoksa kategorisiz)
    support_role_id   BIGINT,                 -- biletleri görebilecek destek ekibi rolü
    counter           INTEGER     NOT NULL DEFAULT 0,
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS tickets (
    id           BIGSERIAL   PRIMARY KEY,
    guild_id     BIGINT      NOT NULL,
    number       INTEGER     NOT NULL,
    channel_id   BIGINT      NOT NULL UNIQUE,
    opener_id    BIGINT      NOT NULL,
    status       TEXT        NOT NULL DEFAULT 'open',   -- open, closed
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at    TIMESTAMPTZ,
    closed_by    BIGINT,
    UNIQUE (guild_id, number)
);

-- Kullanıcı başına tek açık bilet kontrolü için.
CREATE INDEX IF NOT EXISTS tickets_open_idx ON tickets (guild_id, opener_id) WHERE status = 'open';

-- ===================== Seviye / XP =====================
-- Canlı XP Redis'tedir; bu tablo periyodik olarak yazılan kalıcı kopyadır.
CREATE TABLE IF NOT EXISTS member_xp (
    guild_id    BIGINT      NOT NULL,
    user_id     BIGINT      NOT NULL,
    xp          BIGINT      NOT NULL DEFAULT 0,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_id)
);

-- Seviyeye ulaşınca verilen roller.
CREATE TABLE IF NOT EXISTS level_rewards (
    guild_id  BIGINT  NOT NULL,
    level     INTEGER NOT NULL,
    role_id   BIGINT  NOT NULL,
    PRIMARY KEY (guild_id, level)
);
