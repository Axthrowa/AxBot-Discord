#!/usr/bin/env bash
# AxBot kurulum / güncelleme betiği (paketin içinde gelir).
#
#   tar -xzf AxBot-sunucu.tar.gz && cd AxBot-sunucu && sudo bash kur.sh
#
# - Bot /opt/axbot klasörüne kurulur ve kendi Docker kutularında çalışır. Dışarıya port açmaz;
#   sunucudaki diğer programlara / portlara dokunmaz.
# - İlk kurulumda paketteki veritabanı yedeği yüklenir. Sonraki çalıştırmalarda (güncelleme)
#   sunucudaki veriler korunur, paketteki yedek yüklenmez.
# - Sunucu yeniden başlarsa bot kendiliğinden açılır; her gece 03:30'da veritabanı yedeği alınır.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DIR=/opt/axbot
DC="docker compose -f docker-compose.yml -f deploy/docker-compose.sunucu.yml"
# youtube-test.sh "yalnızca IPv6 çalışıyor" derse: touch /opt/axbot/ipv6-acik ve kur.sh yeniden.
[ -f "$DIR/ipv6-acik" ] && DC="$DC -f deploy/docker-compose.ipv6.yml"

say() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }
die() { printf '\n\033[1;31mHATA: %s\033[0m\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "root olarak çalıştırın:  sudo bash kur.sh"
[ -f "$HERE/.env" ] || die ".env bulunamadı; paketi eksiksiz açtığınızdan emin olun."

# ---------- Docker ----------
say "Docker kontrol ediliyor"
if ! command -v docker >/dev/null 2>&1 || ! docker compose version >/dev/null 2>&1; then
    . /etc/os-release
    case "${ID}" in
        ubuntu|debian)
            echo "Docker kurulu değil, kuruluyor..."
            apt-get update -y
            apt-get install -y ca-certificates curl
            install -m 0755 -d /etc/apt/keyrings
            curl -fsSL "https://download.docker.com/linux/${ID}/gpg" -o /etc/apt/keyrings/docker.asc
            chmod a+r /etc/apt/keyrings/docker.asc
            echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/${ID} ${VERSION_CODENAME} stable" \
                > /etc/apt/sources.list.d/docker.list
            apt-get update -y
            apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
            ;;
        *) die "Docker kurulu değil. Bu sunucu (${PRETTY_NAME}) için kurulum: https://docs.docker.com/engine/install/" ;;
    esac
fi
systemctl enable --now docker >/dev/null 2>&1 || true
ver="$(docker compose version --short | sed 's/^v//')"
if [ "$(printf '%s\n' 2.24 "$ver" | sort -V | head -n1)" != "2.24" ]; then
    die "Docker Compose $ver eski; en az 2.24 gerekli. Güncelleme: apt-get install --only-upgrade docker-compose-plugin"
fi
echo "$(docker --version) · Compose $ver"

# ---------- Dosyalar ----------
say "Dosyalar $DIR klasörüne kopyalanıyor"
mkdir -p "$DIR/yedekler" "$DIR/yt-cerez"
if [ "$HERE" != "$DIR" ]; then
    cp -a "$HERE"/. "$DIR"/
fi
cd "$DIR"
# Windows'ta paketlenen dosyalar herkes yazabilir izinle gelir. Paylaşılan sunucuda başka bir
# kullanıcı ayarları/betikleri değiştiremesin: sadece root yazar, klasöre sadece root girer.
chown -R root:root "$DIR"
chmod -R u=rwX,go=rX "$DIR"
chmod 700 "$DIR"
chmod 600 .env
# İsteğe bağlı YouTube çerezi: bot kapsayıcısı (nonroot) okuyabilmeli; üst klasör 700 olduğu için sunucudaki
# başka kullanıcılar yine erişemez.
chmod 755 yt-cerez
[ -f yt-cerez/cookies.txt ] && chmod 644 yt-cerez/cookies.txt

# ---------- İmaj ----------
if [ "$(uname -m)" = "x86_64" ] && [ -f axbot-image.tar ]; then
    say "Hazır bot imajı yükleniyor"
    docker load -i axbot-image.tar
else
    say "Bot bu sunucuda derleniyor ($(uname -m); ilk seferde 10-30 dakika sürebilir)"
    $DC --profile docker build bot
fi
rm -f axbot-image.tar

# ---------- Veritabanı ----------
say "Veritabanı başlatılıyor"
$DC up -d --wait postgres redis searxng
tables="$(echo "SELECT count(*) FROM pg_tables WHERE tablename = '_sqlx_migrations';" \
    | $DC exec -T postgres sh -c 'psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tA')"
if [ -f bot.dump ]; then
    if [ "$tables" = "0" ]; then
        say "Veriler yükleniyor (XP, ekonomi, ayarlar...)"
        $DC exec -T postgres sh -c 'pg_restore -U "$POSTGRES_USER" -d "$POSTGRES_DB" --no-owner' < bot.dump \
            || echo "(Bazı uyarılar olabilir; önemli değil.)"
    else
        echo "Sunucuda zaten veri var; korunuyor (paketteki eski yedek yüklenmedi)."
    fi
    rm -f bot.dump
fi

# ---------- Bot ----------
say "Bot başlatılıyor"
$DC --profile docker up -d --no-build --wait --remove-orphans
docker image prune -f >/dev/null 2>&1 || true

# ---------- Gece yedeği ----------
cat > "$DIR/yedek-al.sh" <<'EOF'
#!/usr/bin/env bash
# AxBot veritabanı yedeği (son 7 gün tutulur).
set -euo pipefail
cd /opt/axbot
f="yedekler/bot-$(date +%F).dump"
docker compose exec -T postgres sh -c 'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" -Fc' > "$f.tmp" && mv "$f.tmp" "$f"
find yedekler -name 'bot-*.dump' -mtime +7 -delete
EOF
chmod 700 "$DIR/yedek-al.sh"
chmod 700 "$DIR/youtube-test.sh" 2>/dev/null || true
if command -v crontab >/dev/null 2>&1; then
    ( crontab -l 2>/dev/null | grep -v '/opt/axbot/yedek-al.sh' || true
      echo "30 3 * * * /opt/axbot/yedek-al.sh >/dev/null 2>&1" ) | crontab -
    echo "Gece yedeği ayarlandı (03:30, /opt/axbot/yedekler)."
else
    echo "cron yok; otomatik yedek ayarlanmadı (elle: /opt/axbot/yedek-al.sh)."
fi

# ---------- Sonuç ----------
say "Bot logları (25 sn bekleniyor)"
sleep 25
$DC logs --tail 25 bot
echo
if $DC logs bot 2>/dev/null | grep -q "Discord'a bağlanıldı"; then
    printf '\033[1;32mAxBot çalışıyor ✅  (Sunucu yeniden başlasa da kendiliğinden açılır.)\033[0m\n'
else
    printf '\033[1;33mBağlantı mesajı henüz görünmedi. Birkaç saniye sonra kontrol et:  cd /opt/axbot && docker compose logs -f bot\033[0m\n'
fi
echo "Canlı loglar:  cd /opt/axbot && docker compose logs -f bot"

# Web paneli adresi (Cloudflare tüneli birkaç saniyede adres alır).
url=""
for _ in $(seq 1 20); do
    url="$($DC logs bot 2>/dev/null | grep -o 'https://[a-z0-9-]*\.trycloudflare\.com' | tail -n1 || true)"
    [ -n "$url" ] && break
    sleep 3
done
if [ -n "$url" ]; then
    echo "Web paneli: $url  (giriş: Discord'da /panel yaz)"
else
    echo "Web paneli adresi henüz alınamadı; Discord'da /panel yazınca bot adresi verir."
fi
