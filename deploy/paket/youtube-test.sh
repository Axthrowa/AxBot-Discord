#!/usr/bin/env bash
# YouTube engel testi: bu sunucuda müziğin hangi yöntemle çalıştığını bulur.
#
#   cd /opt/axbot && sudo bash youtube-test.sh
#
# Botu DURDURMAZ ve Discord'a bağlanmaz: yalnızca botun imajındaki yt-dlp'yi çalıştırır.
# Her yöntem için "ÇALIŞIYOR" ya da hata özeti yazar; sonunda ne yapılacağını söyler.
set -uo pipefail

cd /opt/axbot 2>/dev/null || { echo "Önce kurulum yapılmalı (/opt/axbot yok)."; exit 1; }
DC="docker compose -f docker-compose.yml -f deploy/docker-compose.sunucu.yml"
[ -f ipv6-acik ] && DC="$DC -f deploy/docker-compose.ipv6.yml"
VIDEO="https://www.youtube.com/watch?v=dQw4w9WgXcQ"
PLUGINS="--plugin-dirs /usr/local/lib/yt-dlp-plugins"
POT="--extractor-args youtubepot-bgutilhttp:base_url=http://pot:4416"
COOKIE_VOL=()
COOKIE_ARGS=""
if [ -f yt-cerez/cookies.txt ]; then
    # yt-dlp çerezleri geri yazar: yazılabilir geçici bir kopyası bağlanır, sonda silinir.
    TMPC="$(mktemp -d)"; cp yt-cerez/cookies.txt "$TMPC/cookies.txt"; chmod 777 "$TMPC"; chmod 666 "$TMPC/cookies.txt"
    trap 'rm -rf "$TMPC"' EXIT
    COOKIE_VOL=(-v "$TMPC:/run/cerez-test")
    COOKIE_ARGS="--cookies /run/cerez-test/cookies.txt"
fi

$DC --profile docker up -d --no-build pot >/dev/null 2>&1 || true

# Botun ortamında (aynı ağ, aynı salt okunur dosya sistemi) yalnızca yt-dlp çalıştırılır; bot başlamaz.
# (İmajda kabuk yoktur; yt-dlp doğrudan çağrılır.)
run() {
    local label="$1"; shift
    local out
    out="$($DC --profile docker run --rm --no-deps -T "${COOKIE_VOL[@]}" --entrypoint /usr/local/bin/yt-dlp bot         --no-warnings --skip-download -f 'ba[abr>0][vcodec=none]/best' --print '%(title).40s | %(format_id)s' "$@" "$VIDEO" 2>&1 | tail -n 3)"
    if echo "$out" | grep -q "Never Gonna"; then
        printf '  %-38s ÇALIŞIYOR  (%s)
' "$label" "$(echo "$out" | grep "Never Gonna" | head -n1)"
        return 0
    elif echo "$out" | grep -qiE "Sign in to confirm|not a bot"; then
        printf '  %-38s ENGELLİ (bot sanıldı)
' "$label"
        blocked=$((blocked + 1))
    else
        printf '  %-38s HATA: %s
' "$label" "$(echo "$out" | tail -n1 | cut -c1-90)"
    fi
    return 1
}

echo
echo "== Sunucunun ağ bilgisi =="
echo "  IPv4: $(curl -4 -s --max-time 8 https://api.ipify.org || echo yok)"
echo "  IPv6: $(curl -6 -s --max-time 8 https://api64.ipify.org || echo yok)"
echo
echo "== Botun kapsayıcısından denemeler (IPv4) =="
ok=""
blocked=0
run "varsayılan + PO token"            $PLUGINS $POT                                              && ok="${ok}default "
for c in mweb tv_simply web_embedded android_vr; do
    run "istemci $c + PO token"         $PLUGINS $POT --extractor-args youtube:player_client=$c   && ok="${ok}$c "
done
if [ -n "$COOKIE_ARGS" ]; then
    run "çerez + PO token"              $PLUGINS $POT $COOKIE_ARGS                                 && ok="${ok}cerez "
else
    echo "  (yt-cerez/cookies.txt yok; çerez denemesi atlandı)"
fi

echo
echo "== IPv6 denemesi (sunucunun ağıyla, PO token'sız) =="
v6=""
if curl -6 -s --max-time 8 https://api64.ipify.org >/dev/null 2>&1; then
    out="$(docker run --rm --network host --read-only --tmpfs /tmp:size=256m,mode=1777,exec \
        -e XDG_CACHE_HOME=/tmp/cache -e DENO_DIR=/tmp/deno --entrypoint /usr/local/bin/yt-dlp discord-bot:latest \
        --no-warnings --skip-download --force-ipv6 -f 'ba[abr>0][vcodec=none]/best' --print '%(title).40s' "$VIDEO" 2>&1 | tail -n 2)"
    if echo "$out" | grep -q "Never Gonna"; then
        echo "  IPv6                                   ÇALIŞIYOR"; v6="evet"
    elif echo "$out" | grep -qiE "Sign in to confirm|not a bot"; then
        echo "  IPv6                                   ENGELLİ (bot sanıldı)"; blocked=$((blocked + 1))
    else
        echo "  IPv6                                   HATA: $(echo "$out" | tail -n1 | cut -c1-90)"
    fi
else
    echo "  Sunucunun IPv6 bağlantısı yok; atlandı."
fi

echo
echo "== SONUÇ =="
if [ -n "$ok" ]; then
    first="${ok%% *}"
    echo "  Çalışan yöntem(ler): $ok"
    case "$first" in
        default) echo "  Ek ayar gerekmez; bot zaten bu yöntemi kullanıyor. Botu yeniden başlat: docker compose restart bot" ;;
        cerez)   echo "  Çerez dosyası işe yarıyor; bot onu kullanıyor. Botu yeniden başlat: docker compose restart bot" ;;
        *)       echo "  Ek ayar gerekmez: bot engel görünce istemcileri kendisi dener, '$first' istemcisini bulur ve hatırlar." ;;
    esac
elif [ -n "$v6" ]; then
    echo "  Yalnızca IPv6 çalışıyor. Botu IPv6 ile çalıştırmak için:"
    echo "      sudo touch /opt/axbot/ipv6-acik"
    echo "      cd /opt/axbot && sudo $DC -f deploy/docker-compose.ipv6.yml --profile docker up -d --no-build bot"
    echo "  (Bota ayrı bir IPv6 ağı eklenir ve YTDLP_FORCE_IPV6=1 verilir; Docker Engine 27+ gerekir.)"
elif [ "$blocked" -eq 0 ]; then
    echo "  Test yapılamadı: yukarıdaki HATA satırlarına bak (Docker çalışıyor mu, kurulum tamam mı?)."
else
    echo "  Hiçbir yöntem çalışmadı: bu sunucunun IP adresi YouTube tarafından engellenmiş."
    echo "  Çözüm: yedek bir YouTube hesabının çerez dosyası (rehberin 9. bölümü) ya da bir proxy."
fi
