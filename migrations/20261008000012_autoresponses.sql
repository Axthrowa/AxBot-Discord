-- Otomatik cevaplar: belirli bir kelime/cümle yazılınca bot cevap verir.
CREATE TABLE IF NOT EXISTS autoresponses (
    id          BIGSERIAL   PRIMARY KEY,
    guild_id    BIGINT      NOT NULL,
    trigger     TEXT        NOT NULL,          -- küçük harfe çevrilmiş
    response    TEXT        NOT NULL,
    match_mode  TEXT        NOT NULL,          -- word, exact, starts
    created_by  BIGINT      NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS autoresponses_trigger_idx ON autoresponses (guild_id, trigger);
