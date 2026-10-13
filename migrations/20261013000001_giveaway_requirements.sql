-- Çekiliş katılım şartları ve role göre ek şans (2026-10-13).
ALTER TABLE giveaways ADD COLUMN IF NOT EXISTS required_role BIGINT;
ALTER TABLE giveaways ADD COLUMN IF NOT EXISTS min_level INTEGER NOT NULL DEFAULT 0;
ALTER TABLE giveaways ADD COLUMN IF NOT EXISTS min_account_days INTEGER NOT NULL DEFAULT 0;
ALTER TABLE giveaways ADD COLUMN IF NOT EXISTS bonus_role BIGINT;

-- Katılımın ağırlığı: ek şans rolündekiler 2 kez sayılır.
ALTER TABLE giveaway_entries ADD COLUMN IF NOT EXISTS weight INTEGER NOT NULL DEFAULT 1;
