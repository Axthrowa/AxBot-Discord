-- Sahte borsa: üyelerin elindeki hisseler.
CREATE TABLE IF NOT EXISTS stock_holdings (
    guild_id BIGINT NOT NULL,
    user_id  BIGINT NOT NULL,
    symbol   TEXT NOT NULL,
    shares   BIGINT NOT NULL DEFAULT 0 CHECK (shares >= 0),
    PRIMARY KEY (guild_id, user_id, symbol)
);
