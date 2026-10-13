# Botu bir Linux sunucuya (VPS) gönderir ve orada 7/24 çalıştırır (Docker).
#
# İlk kurulum (veritabanı da taşınır):
#   powershell -ExecutionPolicy Bypass -File deploy\sunucuya-gonder.ps1 -Sunucu root@1.2.3.4 -VeriTasi
# Güncelleme (kod değişince; veriler sunucuda kalır):
#   powershell -ExecutionPolicy Bypass -File deploy\sunucuya-gonder.ps1 -Sunucu root@1.2.3.4
#
# Ne yapar:
#   1. SSH anahtarı yoksa oluşturur ve sunucuya yükler (sunucu şifresi SADECE bir kez, kendi
#      penceresinde sorulur; bir daha sorulmaz).
#   2. Sunucuda Docker yoksa deploy/server-setup.sh'ı çalıştırır (Ubuntu/Debian).
#   3. Projeyi gönderir; x86_64 sunucularda imajı bu bilgisayarda derler (sunucuyu yormaz),
#      ARM sunucularda sunucuda derler.
#   4. Botu başlatır, bu bilgisayardaki botu durdurur ve burada tekrar açılmasını engeller.
#
# Parametreler:
#   -Sunucu        kullanici@adres (zorunlu)
#   -Port          SSH portu (varsayılan 22)
#   -Anahtar       SSH anahtar dosyası (verilmezse %USERPROFILE%\.ssh\axbot_sunucu oluşturulur)
#   -VeriTasi      Bu bilgisayardaki veritabanını sunucuya kopyalar (sunucudakinin ÜSTÜNE yazar)
#   -SunucudaDerle x86_64 sunucuda da imajı sunucuda derle
# Not: .env (token ve şifreler) SSH üzerinden şifreli olarak sunucuya kopyalanır.

param(
    [Parameter(Mandatory = $true)][string]$Sunucu,
    [int]$Port = 22,
    [string]$Anahtar = '',
    [switch]$VeriTasi,
    [switch]$SunucudaDerle,
    [switch]$YerelDerle
)

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$remoteDir = '/opt/discord-bot'
# Sunucuda veritabanı/Redis portları dışarı açılmaz (bkz. deploy/docker-compose.sunucu.yml).
$dc = 'docker compose -f docker-compose.yml -f deploy/docker-compose.sunucu.yml'
$tmp = Join-Path $env:TEMP 'discord-bot-deploy'
New-Item -ItemType Directory -Force $tmp | Out-Null

function Fail([string]$message) { Write-Host ''; Write-Host "HATA: $message" -ForegroundColor Red; exit 1 }
function Step([string]$message) { Write-Host ''; Write-Host "==> $message" -ForegroundColor Cyan }

# Windows'un OpenSSH istemcisi yoksa Git for Windows'un içindeki ssh kullanılır.
if (-not (Get-Command ssh -ErrorAction SilentlyContinue)) {
    $gitBin = Join-Path $env:ProgramFiles 'Git\usr\bin'
    if (Test-Path (Join-Path $gitBin 'ssh.exe')) { $env:Path = "$gitBin;$env:Path" }
}
if (-not (Get-Command ssh -ErrorAction SilentlyContinue)) { Fail 'ssh bulunamadı (Ayarlar → Uygulamalar → İsteğe bağlı özellikler → OpenSSH İstemcisi).' }
if (-not (Test-Path '.env')) { Fail '.env bulunamadı.' }

# ---------- SSH anahtarı ----------
if (-not $Anahtar) {
    $Anahtar = Join-Path $env:USERPROFILE '.ssh\axbot_sunucu'
    if (-not (Test-Path $Anahtar)) {
        Step 'SSH anahtarı oluşturuluyor'
        New-Item -ItemType Directory -Force (Split-Path $Anahtar) | Out-Null
        # PowerShell 5.1 boş argümanı ('') hiç göndermez; '""' boş parola olarak iletilir.
        & ssh-keygen -q -t ed25519 -C axbot -f $Anahtar -N '""'
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path $Anahtar)) { Fail 'SSH anahtarı oluşturulamadı.' }
    }
}
if (-not (Test-Path $Anahtar)) { Fail "SSH anahtarı bulunamadı: $Anahtar" }

$sshArgs = @('-i', $Anahtar, '-p', "$Port", '-o', 'StrictHostKeyChecking=accept-new', '-o', 'ServerAliveInterval=30')
$scpArgs = @('-i', $Anahtar, '-P', "$Port", '-o', 'StrictHostKeyChecking=accept-new')

function Remote([string]$command) {
    & ssh @sshArgs $Sunucu $command
    if ($LASTEXITCODE -ne 0) { Fail "Sunucu komutu başarısız: $command" }
}
function Upload([string]$local, [string]$remote) {
    & scp @scpArgs $local "${Sunucu}:$remote"
    if ($LASTEXITCODE -ne 0) { Fail "Dosya gönderilemedi: $local" }
}

Step 'Sunucuya bağlanılıyor'
& ssh @sshArgs -o BatchMode=yes -o ConnectTimeout=15 $Sunucu 'true' 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Host 'Anahtar bu sunucuda henüz tanımlı değil; yükleniyor.' -ForegroundColor Yellow
    Write-Host 'Sunucunun ŞİFRESİNİ bir kez gir (yazarken ekranda görünmez, sonra Enter):' -ForegroundColor Yellow
    $pub = (Get-Content "$Anahtar.pub" -Raw).Trim()
    & ssh -p "$Port" -o StrictHostKeyChecking=accept-new $Sunucu "umask 077; mkdir -p ~/.ssh && echo '$pub' >> ~/.ssh/authorized_keys"
    if ($LASTEXITCODE -ne 0) { Fail 'Sunucuya bağlanılamadı (adres, kullanıcı adı, port veya şifre yanlış olabilir).' }
    & ssh @sshArgs -o BatchMode=yes -o ConnectTimeout=15 $Sunucu 'true'
    if ($LASTEXITCODE -ne 0) { Fail 'Anahtar yüklendi ama anahtarla girilemedi (sunucu anahtarla girişi kapatmış olabilir).' }
}
$arch = (& ssh @sshArgs $Sunucu 'uname -m' | Out-String).Trim()
Write-Host "Bağlandı. Sunucu mimarisi: $arch"

# ---------- Sunucu hazırlığı ----------
$ready = (& ssh @sshArgs $Sunucu "command -v docker >/dev/null && docker compose version --short 2>/dev/null && test -x $remoteDir/yedek-al.sh && echo HAZIR" | Out-String)
if ($ready -notmatch 'HAZIR') {
    Step 'Sunucu hazırlanıyor (Docker, swap, güncellemeler, gece yedeği)'
    Upload 'deploy/server-setup.sh' '/tmp/axbot-server-setup.sh'
    # -t: sudo şifre isterse kullanıcı yazabilsin.
    & ssh -t @sshArgs $Sunucu 'bash /tmp/axbot-server-setup.sh; s=$?; rm -f /tmp/axbot-server-setup.sh; exit $s'
    if ($LASTEXITCODE -ne 0) { Fail 'Sunucu hazırlanamadı (yukarıdaki çıktıya bakın).' }
}
$composeVersion = (& ssh @sshArgs $Sunucu 'docker compose version --short' | Out-String).Trim().TrimStart('v')
try {
    if ([version]($composeVersion -replace '[^0-9.].*$', '') -lt [version]'2.24') {
        Fail "Sunucudaki Docker Compose eski ($composeVersion); en az 2.24 gerekli. Sunucuda: sudo apt-get install --only-upgrade docker-compose-plugin"
    }
} catch {
    Write-Host "Docker Compose sürümü okunamadı ($composeVersion), devam ediliyor." -ForegroundColor Yellow
}

# ---------- Proje ----------
Step 'Proje gönderiliyor'
$archive = Join-Path $tmp 'bot.tar.gz'
# Windows'un kendi tar'ı (Git'in tar'ı 'C:' ile başlayan yolları uzak adres sanar).
& (Join-Path $env:SystemRoot 'System32\tar.exe') -czf $archive --exclude=./target --exclude=./logs --exclude=./.git --exclude=./tools --exclude=./.sunucuda-calisiyor -C $root .
if ($LASTEXITCODE -ne 0) { Fail 'Paketleme başarısız.' }
Upload $archive "$remoteDir/bot.tar.gz"
Remote "cd $remoteDir && tar -xzf bot.tar.gz && rm bot.tar.gz && chmod 600 .env"

if ($VeriTasi) {
    Step 'Veritabanı yedekleniyor (bu bilgisayar)'
    # PowerShell 5.1 harici programlara giden argümanlardaki çift tırnağı bozar; tırnaksız yazıldı.
    & docker compose exec -T postgres sh -c 'pg_dump -U $POSTGRES_USER -d $POSTGRES_DB -Fc -f /tmp/bot.dump'
    if ($LASTEXITCODE -ne 0) { Fail 'Yerel veritabanı yedeklenemedi (Docker Desktop ve servisler açık mı?).' }
    $dump = Join-Path $tmp 'bot.dump'
    & docker compose cp postgres:/tmp/bot.dump $dump
    if ($LASTEXITCODE -ne 0) { Fail 'Yedek dosyası alınamadı.' }

    Step 'Veritabanı sunucuya aktarılıyor'
    Upload $dump "$remoteDir/bot.dump"
    # Sunucuda bot çalışıyorsa önce durdurulur; geri yükleme sırasında veri yazılmasın.
    Remote "cd $remoteDir && ($dc --profile docker stop bot || true) && $dc up -d --wait postgres"
    Remote "cd $remoteDir && $dc cp bot.dump postgres:/tmp/bot.dump && rm -f bot.dump"
    Remote "cd $remoteDir && $dc exec -T postgres sh -c 'pg_restore -U `$POSTGRES_USER -d `$POSTGRES_DB --clean --if-exists --no-owner /tmp/bot.dump || true; rm -f /tmp/bot.dump'"
}

# ---------- İmaj ----------
$localBuild = $YerelDerle -or ($arch -eq 'x86_64' -and -not $SunucudaDerle)
if ($localBuild -and $arch -ne 'x86_64') { Fail "Bu bilgisayarda sadece x86_64 sunucular için derlenebilir (bu sunucu: $arch). -YerelDerle'yi kaldırın." }
if ($localBuild) {
    Step 'İmaj bu bilgisayarda derleniyor (ilk seferde 10-20 dk)'
    & docker build --platform linux/amd64 -t discord-bot:latest .
    if ($LASTEXITCODE -ne 0) { Fail 'İmaj derlenemedi.' }
    $image = Join-Path $tmp 'image.tar'
    & docker save discord-bot:latest -o $image
    if ($LASTEXITCODE -ne 0) { Fail 'İmaj kaydedilemedi.' }
    Step 'İmaj gönderiliyor'
    Upload $image "$remoteDir/image.tar"
    Remote "cd $remoteDir && docker load -i image.tar && rm image.tar && $dc --profile docker up -d --no-build --wait"
} else {
    Step 'İmaj sunucuda derleniyor (ilk seferde 10-30 dakika sürebilir)'
    Remote "cd $remoteDir && $dc --profile docker up -d --build --wait"
}
Remote 'docker image prune -f >/dev/null'

# ---------- Bitiş ----------
Step 'Bot logları'
Remote "cd $remoteDir && sleep 25 && $dc logs --tail 20 bot"

# Aynı token ile iki bot bağlanmasın: buradaki bot durdurulur ve bir daha açılmaz.
$local = Get-Process -Name 'discord-bot' -ErrorAction SilentlyContinue
if ($local) {
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'scripts\stop-bot.ps1') | Out-Null
    Write-Host ''
    Write-Host 'Bu bilgisayardaki bot durduruldu (artık sunucuda çalışıyor).' -ForegroundColor Yellow
}
Set-Content -Path (Join-Path $root '.sunucuda-calisiyor') -Value "$Sunucu (port $Port)" -Encoding UTF8

Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
Write-Host ''
Write-Host 'Bot sunucuda 7/24 çalışıyor (sunucu yeniden başlasa da kendiliğinden açılır).' -ForegroundColor Green
Write-Host 'Canlı loglar:'
Write-Host "  ssh -i $Anahtar -p $Port $Sunucu 'cd $remoteDir && docker compose logs -f bot'"
