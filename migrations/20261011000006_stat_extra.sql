-- Sayaç kanalları: kripto para sayacının izlediği paranın kimliği (CoinGecko).
ALTER TABLE stat_channels ADD COLUMN IF NOT EXISTS extra TEXT;
