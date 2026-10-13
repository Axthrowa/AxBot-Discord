-- Kazanılmış başarımlar. Sayaçlar (mesaj, tepki, ses dakikası, alt başlık) Redis'te tutulur;
-- buraya yalnızca bir başarım kazanıldığında yazılır.
CREATE TABLE IF NOT EXISTS member_achievements (
    guild_id     BIGINT      NOT NULL,
    user_id      BIGINT      NOT NULL,
    key          TEXT        NOT NULL,
    unlocked_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_id, key)
);
