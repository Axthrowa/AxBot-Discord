-- Kişiye özel kayıtlı çalma listeleri (/müzik liste-kaydet).
CREATE TABLE IF NOT EXISTS user_playlists (
    user_id    BIGINT NOT NULL,
    name       TEXT NOT NULL,
    tracks     TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, name)
);
