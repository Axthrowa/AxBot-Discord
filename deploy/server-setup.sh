#!/usr/bin/env bash
# Linux sunucuyu (Ubuntu veya Debian; herhangi bir VPS) bot için BİR KEZ hazırlar:
#   1. Docker Engine + Compose (zaten kuruluysa dokunulmaz)
#   2. 2 GB swap (RAM'i az sunucularda bellek taşmasına karşı; zaten swap varsa eklenmez)
#   3. Otomatik güvenlik güncellemeleri
#   4. /opt/discord-bot klasörü + her gece veritabanı yedeği (son 7 gün)
#
# deploy/sunucuya-gonder.ps1 Docker'ı bulamazsa bunu kendisi gönderip çalıştırır.
# Elle: bash server-setup.sh
# Dışarıya port açmaya gerek yoktur: bot Discord'a kendisi bağlanır.
set -euo pipefail

# root ile bağlanıldıysa sudo gerekmez (bazı sağlayıcılarda sudo kurulu bile değildir).
if [ "$(id -u)" -eq 0 ]; then SUDO=""; else SUDO="sudo"; fi
. /etc/os-release
case "${ID}" in
    ubuntu|debian) ;;
    *) echo "Bu betik Ubuntu/Debian içindir (bu sunucu: ${PRETTY_NAME}). Docker'ı elle kurun."; exit 1 ;;
esac

if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
    echo "==> Docker zaten kurulu: $(docker --version)"
else
    echo "==> Docker kuruluyor"
    $SUDO apt-get update -y
    $SUDO apt-get install -y ca-certificates curl
    $SUDO install -m 0755 -d /etc/apt/keyrings
    $SUDO curl -fsSL "https://download.docker.com/linux/${ID}/gpg" -o /etc/apt/keyrings/docker.asc
    $SUDO chmod a+r /etc/apt/keyrings/docker.asc
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/${ID} ${VERSION_CODENAME} stable" \
        | $SUDO tee /etc/apt/sources.list.d/docker.list >/dev/null
    $SUDO apt-get update -y
    $SUDO apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
fi
$SUDO systemctl enable --now docker
if [ -n "$SUDO" ]; then
    $SUDO usermod -aG docker "$USER"
fi

echo "==> Swap"
if [ -z "$(swapon --show --noheadings)" ]; then
    $SUDO fallocate -l 2G /swapfile
    $SUDO chmod 600 /swapfile
    $SUDO mkswap /swapfile
    $SUDO swapon /swapfile
    echo '/swapfile none swap sw 0 0' | $SUDO tee -a /etc/fstab >/dev/null
else
    echo "Swap zaten var, eklenmedi."
fi

echo "==> Otomatik güvenlik güncellemeleri"
$SUDO apt-get install -y unattended-upgrades
$SUDO dpkg-reconfigure -f noninteractive unattended-upgrades

echo "==> Bot klasörü"
$SUDO mkdir -p /opt/discord-bot/yedekler
$SUDO chown -R "$(id -un)":"$(id -gn)" /opt/discord-bot

echo "==> Gece yedeği (03:30, son 7 gün /opt/discord-bot/yedekler)"
cat > /opt/discord-bot/yedek-al.sh <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
cd /opt/discord-bot
f="yedekler/bot-$(date +%F).dump"
docker compose exec -T postgres sh -c 'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" -Fc' > "$f.tmp" && mv "$f.tmp" "$f"
find yedekler -name 'bot-*.dump' -mtime +7 -delete
EOF
chmod +x /opt/discord-bot/yedek-al.sh
( crontab -l 2>/dev/null | grep -v yedek-al.sh; echo "30 3 * * * /opt/discord-bot/yedek-al.sh >/dev/null 2>&1" ) | crontab -

echo
echo "Sunucu hazır."
