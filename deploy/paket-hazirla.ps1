# Arkadaşının sunucusuna yüklenecek tek dosyalık paketi hazırlar: dist\AxBot-sunucu.tar.gz
#
# İçinde: hazır derlenmiş bot imajı (x86_64), veritabanı yedeği, ayarlar (.env), kaynak kod
# (ARM sunucularda orada derlemek için) ve kurulum betiği kur.sh + BENIOKU.txt.
#
# Kullanım: "Sunucu Paketi Hazırla.cmd" (ilk kurulum) / "Sunucu Güncellemesi Hazırla.cmd" (güncelleme)
#   powershell -ExecutionPolicy Bypass -File deploy\paket-hazirla.ps1 [-VeriYok] [-Guncelleme]
#   -VeriYok    : veritabanı yedeği eklenmez (sunucuda sıfırdan başlar)
#   -Guncelleme : AxBot-guncelleme.tar.gz (veritabanı yedeği yok; sunucudaki veriler korunur)

param([switch]$VeriYok, [switch]$Guncelleme)
if ($Guncelleme) { $VeriYok = $true }

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$dist = Join-Path $root 'dist'
$pkg = Join-Path $dist 'AxBot-sunucu'
$archive = Join-Path $dist ($(if ($Guncelleme) { 'AxBot-guncelleme.tar.gz' } else { 'AxBot-sunucu.tar.gz' }))
$tar = Join-Path $env:SystemRoot 'System32\tar.exe'

function Fail([string]$message) { Write-Host ''; Write-Host "HATA: $message" -ForegroundColor Red; exit 1 }
function Step([string]$message) { Write-Host ''; Write-Host "==> $message" -ForegroundColor Cyan }

if (-not (Test-Path '.env')) { Fail '.env bulunamadı.' }
& docker info *> $null
if ($LASTEXITCODE -ne 0) { Fail 'Docker Desktop açık değil. Açıp tekrar deneyin.' }

Remove-Item -Recurse -Force $pkg -ErrorAction SilentlyContinue
Remove-Item -Force $archive -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $pkg | Out-Null

Step 'Bot imajı derleniyor (Linux x86_64; ilk seferde 10-20 dk, sonra hızlı)'
& docker build --platform linux/amd64 -t discord-bot:latest .
if ($LASTEXITCODE -ne 0) { Fail 'İmaj derlenemedi.' }
& docker save discord-bot:latest -o (Join-Path $pkg 'axbot-image.tar')
if ($LASTEXITCODE -ne 0) { Fail 'İmaj kaydedilemedi.' }

if (-not $VeriYok) {
    Step 'Veritabanı yedekleniyor'
    & docker compose up -d --wait postgres
    if ($LASTEXITCODE -ne 0) { Fail 'PostgreSQL başlatılamadı.' }
    # PowerShell 5.1 harici programlara giden argümanlardaki çift tırnağı bozar; tırnaksız yazıldı.
    & docker compose exec -T postgres sh -c 'pg_dump -U $POSTGRES_USER -d $POSTGRES_DB -Fc -f /tmp/bot.dump'
    if ($LASTEXITCODE -ne 0) { Fail 'Veritabanı yedeklenemedi.' }
    & docker compose cp postgres:/tmp/bot.dump (Join-Path $pkg 'bot.dump')
    if ($LASTEXITCODE -ne 0) { Fail 'Yedek dosyası alınamadı.' }
    & docker compose exec -T postgres rm -f /tmp/bot.dump
}

Step 'Dosyalar kopyalanıyor'
foreach ($item in @('.env', '.dockerignore', 'docker-compose.yml', 'Dockerfile', 'Cargo.toml', 'Cargo.lock', 'build.rs', 'src', 'assets', 'migrations', 'searxng')) {
    Copy-Item -Recurse -Force (Join-Path $root $item) $pkg
}
New-Item -ItemType Directory -Force (Join-Path $pkg 'deploy') | Out-Null
Copy-Item (Join-Path $root 'deploy\docker-compose.sunucu.yml') (Join-Path $pkg 'deploy')
Copy-Item (Join-Path $root 'deploy\paket\kur.sh') $pkg
Copy-Item (Join-Path $root 'deploy\paket\BENIOKU.txt') $pkg

Step 'Paketleniyor'
& $tar -czf $archive -C $dist 'AxBot-sunucu'
if ($LASTEXITCODE -ne 0) { Fail 'Paketleme başarısız.' }
Remove-Item -Recurse -Force $pkg

$size = [math]::Round((Get-Item $archive).Length / 1MB, 1)
Write-Host ''
Write-Host "Paket hazır: $archive ($size MB)" -ForegroundColor Green
Write-Host ''
Write-Host 'Arkadaşına gönder; o sunucuda şunları çalıştırsın (BENIOKU.txt içinde de yazıyor):'
Write-Host "    tar -xzf $(Split-Path $archive -Leaf)"
Write-Host '    cd AxBot-sunucu'
Write-Host '    sudo bash kur.sh'
Write-Host ''
Write-Host 'DİKKAT: Pakette botun gizli anahtarları (.env) var. Sadece arkadaşına özelden gönder.' -ForegroundColor Yellow
Write-Host 'Arkadaşın "çalışıyor" deyince bu bilgisayardaki botu kapat (Botu Durdur.cmd),' -ForegroundColor Yellow
Write-Host 'yoksa iki bot aynı anda cevap verir.' -ForegroundColor Yellow
