-- ===================== Sunucu ayarları: starboard ve öneriler =====================
ALTER TABLE guild_settings
    ADD COLUMN IF NOT EXISTS starboard_channel_id   BIGINT,
    ADD COLUMN IF NOT EXISTS starboard_threshold    INTEGER NOT NULL DEFAULT 3,
    ADD COLUMN IF NOT EXISTS suggestion_channel_id  BIGINT;

-- ===================== Starboard =====================
-- Orijinal mesaj → starboard kanalındaki kopyası.
CREATE TABLE IF NOT EXISTS starboard_posts (
    message_id       BIGINT  PRIMARY KEY,
    guild_id         BIGINT  NOT NULL,
    channel_id       BIGINT  NOT NULL,
    star_message_id  BIGINT  NOT NULL,
    stars            INTEGER NOT NULL
);

-- ===================== Etiketler (özel komutlar) =====================
CREATE TABLE IF NOT EXISTS tags (
    guild_id    BIGINT      NOT NULL,
    name        TEXT        NOT NULL,          -- küçük harf, boşluksuz
    content     TEXT        NOT NULL,
    created_by  BIGINT      NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, name)
);

-- ===================== Öneriler =====================
CREATE TABLE IF NOT EXISTS suggestions (
    id           BIGSERIAL   PRIMARY KEY,
    guild_id     BIGINT      NOT NULL,
    number       INTEGER     NOT NULL,
    channel_id   BIGINT      NOT NULL,
    message_id   BIGINT,
    author_id    BIGINT      NOT NULL,
    content      TEXT        NOT NULL,
    status       TEXT        NOT NULL DEFAULT 'open',   -- open, approved, denied, considering, implemented
    reviewer_id  BIGINT,
    review_note  TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (guild_id, number)
);

CREATE TABLE IF NOT EXISTS suggestion_votes (
    suggestion_id  BIGINT   NOT NULL REFERENCES suggestions (id) ON DELETE CASCADE,
    user_id        BIGINT   NOT NULL,
    vote           SMALLINT NOT NULL,   -- 1 = evet, -1 = hayır
    PRIMARY KEY (suggestion_id, user_id)
);

-- ===================== Biletler: üstlenme =====================
ALTER TABLE tickets ADD COLUMN IF NOT EXISTS claimed_by BIGINT;

-- ===================== XP kazanılmayan kanal/roller =====================
CREATE TABLE IF NOT EXISTS xp_exclusions (
    guild_id   BIGINT NOT NULL,
    target_id  BIGINT NOT NULL,   -- kanal veya rol ID'si
    kind       TEXT   NOT NULL,   -- channel, role
    PRIMARY KEY (guild_id, target_id)
);
