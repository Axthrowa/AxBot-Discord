-- Karantina: üyenin karantinadan önceki rolleri (geri verilmek üzere) ve sebebi.
CREATE TABLE IF NOT EXISTS quarantines (
    guild_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL,
    roles BIGINT[] NOT NULL,
    reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_id)
);

-- Uyarı puanı: her uyarı varsayılan 1 puan; otomatik cezalar puan toplamına bakar.
ALTER TABLE mod_cases ADD COLUMN IF NOT EXISTS points INTEGER NOT NULL DEFAULT 1;
