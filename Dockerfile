# syntax=docker/dockerfile:1
#
# Çok aşamalı derleme (amd64 ve arm64 — örn. Oracle Cloud Ampere):
#   1. builder : Rust release derlemesi + müzik araçları (yt-dlp, deno) indirme
#   2. runtime : distroless — kabuk, paket yöneticisi yok; root olmayan kullanıcı
#
# TLS için rustls + gömülü kök sertifikalar kullanıldığından OpenSSL gerekmez.
# Builder ve runtime aynı Debian sürümünde (bookworm / glibc 2.36).

FROM rust:1-slim-bookworm AS builder
ARG TARGETARCH
WORKDIR /app

# Paket kaynakları HTTPS: bazı ağlar düz HTTP isteklerini araya girip bozuyor.
# cmake + make + g++: songbird'ün Opus kodlayıcısı (libopus) için. curl/unzip: müzik araçları için.
RUN sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources \
    && apt-get update \
    && apt-get install -y --no-install-recommends cmake make g++ curl unzip \
    && rm -rf /var/lib/apt/lists/*

# Müzik: yt-dlp ve deno (YouTube korumasını çözmek için) — mimariye göre tek dosyalık resmi sürümler.
RUN set -eux; \
    case "${TARGETARCH:-amd64}" in \
        amd64) YTDLP=yt-dlp_linux;         DENO=deno-x86_64-unknown-linux-gnu.zip ;; \
        arm64) YTDLP=yt-dlp_linux_aarch64; DENO=deno-aarch64-unknown-linux-gnu.zip ;; \
        *) echo "desteklenmeyen mimari: ${TARGETARCH}"; exit 1 ;; \
    esac; \
    mkdir -p /tools; \
    curl -fsSL --retry 5 --retry-all-errors --retry-delay 3 -o /tools/yt-dlp "https://github.com/yt-dlp/yt-dlp/releases/latest/download/${YTDLP}"; \
    curl -fsSL --retry 5 --retry-all-errors --retry-delay 3 -o /tmp/deno.zip "https://github.com/denoland/deno/releases/latest/download/${DENO}"; \
    unzip -q /tmp/deno.zip -d /tools; \
    rm /tmp/deno.zip; \
    chmod 0755 /tools/yt-dlp /tools/deno; \
    mkdir -p /tools/yt-dlp-plugins; \
    curl -fsSL --retry 5 --retry-all-errors --retry-delay 3 -o /tools/yt-dlp-plugins/bgutil-ytdlp-pot-provider.zip \
        "https://github.com/Brainicism/bgutil-ytdlp-pot-provider/releases/download/2.0.2/bgutil-ytdlp-pot-provider.zip"; \
    mkdir -p /tools/lib; \
    cp -L "$(dpkg -L zlib1g | grep '/libz\.so\.1$')" /tools/lib/libz.so.1
# (yt-dlp'nin tek dosyalık sürümü zlib ister; distroless imajda yoktur, yukarıda kopyalandı.)

COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY assets ./assets
# sqlx::migrate! migration'ları derleme anında binary'ye gömer.
COPY migrations ./migrations

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked \
    && cp target/release/discord-bot /usr/local/bin/discord-bot

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=builder /usr/local/bin/discord-bot /usr/local/bin/discord-bot
COPY --from=builder /tools/yt-dlp /usr/local/bin/yt-dlp
COPY --from=builder /tools/deno /usr/local/bin/deno
COPY --from=builder /tools/lib /usr/local/lib/tools
# yt-dlp eklentisi: PO token'ı docker-compose'daki `pot` servisinden alır (YouTube'un veri merkezi engeline karşı).
COPY --from=builder /tools/yt-dlp-plugins /usr/local/lib/yt-dlp-plugins
# Dosya sistemi salt okunurdur; yt-dlp ve deno önbelleklerini /tmp'ye (tmpfs) yazar.
ENV XDG_CACHE_HOME=/tmp/cache \
    DENO_DIR=/tmp/deno \
    LD_LIBRARY_PATH=/usr/local/lib/tools \
    YTDLP_PLUGIN_DIRS=/usr/local/lib/yt-dlp-plugins
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/discord-bot"]
