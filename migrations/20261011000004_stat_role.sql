-- "Roldeki üyeler" sayacı için sayılacak rol.
ALTER TABLE stat_channels ADD COLUMN IF NOT EXISTS role_id BIGINT;
