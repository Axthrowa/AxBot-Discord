-- İsteğe bağlı doğum yılı (sunucu izin veriyorsa yaşla kutlama için).
ALTER TABLE birthdays ADD COLUMN IF NOT EXISTS year SMALLINT;
