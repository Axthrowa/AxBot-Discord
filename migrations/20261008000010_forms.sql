-- ===================== Doğrulama ve ban itirazı ayarları =====================
ALTER TABLE guild_settings
    ADD COLUMN IF NOT EXISTS verify_role_id     BIGINT,
    ADD COLUMN IF NOT EXISTS verify_math        BOOLEAN NOT NULL DEFAULT FALSE,  -- butona ek olarak işlem sorusu
    ADD COLUMN IF NOT EXISTS appeal_channel_id  BIGINT;

-- ===================== Başvuru formları =====================
CREATE TABLE IF NOT EXISTS application_forms (
    id                 BIGSERIAL   PRIMARY KEY,
    guild_id           BIGINT      NOT NULL,
    name               TEXT        NOT NULL,
    questions          TEXT[]      NOT NULL,        -- en fazla 5 (Discord form sınırı)
    review_channel_id  BIGINT      NOT NULL,        -- başvuruların düştüğü yetkili kanalı
    role_id            BIGINT,                      -- kabul edilene verilecek rol
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS application_forms_name_idx ON application_forms (guild_id, lower(name));

CREATE TABLE IF NOT EXISTS applications (
    id           BIGSERIAL   PRIMARY KEY,
    form_id      BIGINT      NOT NULL REFERENCES application_forms (id) ON DELETE CASCADE,
    guild_id     BIGINT      NOT NULL,
    user_id      BIGINT      NOT NULL,
    answers      TEXT[]      NOT NULL,
    status       TEXT        NOT NULL DEFAULT 'pending',   -- pending, accepted, denied
    reviewer_id  BIGINT,
    reason       TEXT,
    channel_id   BIGINT,
    message_id   BIGINT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS applications_pending_idx ON applications (form_id, user_id) WHERE status = 'pending';

-- ===================== Ban itirazları =====================
CREATE TABLE IF NOT EXISTS ban_appeals (
    id           BIGSERIAL   PRIMARY KEY,
    guild_id     BIGINT      NOT NULL,
    user_id      BIGINT      NOT NULL,
    content      TEXT        NOT NULL,
    status       TEXT        NOT NULL DEFAULT 'pending',   -- pending, accepted, denied
    reviewer_id  BIGINT,
    reason       TEXT,
    channel_id   BIGINT,
    message_id   BIGINT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ban_appeals_user_idx ON ban_appeals (guild_id, user_id);
