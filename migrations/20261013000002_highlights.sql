-- Vurgu bildirimleri (Carl-bot "highlight"): üyenin seçtiği kelime bir kanalda geçince özelden haber verilir.
CREATE TABLE IF NOT EXISTS highlights (
    guild_id   BIGINT      NOT NULL,
    user_id    BIGINT      NOT NULL,
    word       TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_id, word)
);
