-- Ticket Tool Pro: Konu, kapanış gerekçesi ve SLA uyarısı
ALTER TABLE tickets ADD COLUMN IF NOT EXISTS topic TEXT;
ALTER TABLE tickets ADD COLUMN IF NOT EXISTS close_reason TEXT;
ALTER TABLE tickets ADD COLUMN IF NOT EXISTS sla_warned_at TIMESTAMPTZ;

-- Çoklu bilet panelleri desteği
CREATE TABLE IF NOT EXISTS ticket_panels (
    id              BIGSERIAL PRIMARY KEY,
    guild_id        BIGINT NOT NULL,
    channel_id      BIGINT NOT NULL,
    message_id      BIGINT,
    title           TEXT NOT NULL DEFAULT 'Destek Talebi',
    category_id     BIGINT,
    support_role_id BIGINT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_ticket_panels_guild ON ticket_panels (guild_id);
