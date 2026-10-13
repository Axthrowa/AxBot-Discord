-- ===================== Ekonomi: hesaplar =====================
-- Para sunucuya özeldir. Cüzdandaki para soyulabilir, bankadaki soyulamaz.
CREATE TABLE IF NOT EXISTS economy_accounts (
    guild_id      BIGINT      NOT NULL,
    user_id       BIGINT      NOT NULL,
    wallet        BIGINT      NOT NULL DEFAULT 0 CHECK (wallet >= 0),
    bank          BIGINT      NOT NULL DEFAULT 0 CHECK (bank >= 0),
    last_daily    TIMESTAMPTZ,
    daily_streak  INTEGER     NOT NULL DEFAULT 0,
    last_work     TIMESTAMPTZ,
    last_rob      TIMESTAMPTZ,
    PRIMARY KEY (guild_id, user_id)
);
CREATE INDEX IF NOT EXISTS economy_accounts_rich_idx ON economy_accounts (guild_id, (wallet + bank) DESC);

-- ===================== Mağaza =====================
CREATE TABLE IF NOT EXISTS shop_items (
    id           BIGSERIAL   PRIMARY KEY,
    guild_id     BIGINT      NOT NULL,
    name         TEXT        NOT NULL,
    price        BIGINT      NOT NULL CHECK (price > 0),
    role_id      BIGINT,                 -- varsa satın alınca bu rol verilir
    description  TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS shop_items_name_idx ON shop_items (guild_id, lower(name));

CREATE TABLE IF NOT EXISTS economy_inventory (
    guild_id  BIGINT  NOT NULL,
    user_id   BIGINT  NOT NULL,
    item_id   BIGINT  NOT NULL REFERENCES shop_items (id) ON DELETE CASCADE,
    quantity  INTEGER NOT NULL CHECK (quantity > 0),
    PRIMARY KEY (guild_id, user_id, item_id)
);
