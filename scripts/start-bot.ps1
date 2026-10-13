# Botu normal bir Windows programı olarak başlatır.
#
#   1. Docker Desktop kapalıysa açar ve hazır olmasını bekler
#   2. PostgreSQL, Redis ve SearXNG servislerini başlatır (docker compose)
#   3. Botu release modunda derler (kod değişmediyse saniyeler sürer)
#   4. Botu kendi konsol penceresinde başlatır — pencereyi kapatmak botu düzgünce kapatır
#
# Kullanım:
#   Botu Başlat.cmd                                   (çift tıkla)
#   powershell -ExecutionPolicy Bypass -File scripts\start-bot.ps1             (pencereli)
#   powershell -ExecutionPolicy Bypass -File scripts\start-bot.ps1 -Arkaplan   (pencere yok)

param([switch]$Arkaplan)

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Fail([string]$message) {
    Write-Host ''
    Write-Host "HATA: $message" -ForegroundColor Red
    exit 1
}

function Step([string]$message) {
    Write-Host ''
    Write-Host "==> $message" -ForegroundColor Cyan
}

# Bot 7/24 sunucuya taşındıysa burada da açılırsa aynı token ile iki bot bağlanır
# (her komuta iki cevap, çift XP). deploy\sunucuya-gonder.ps1 bu dosyayı oluşturur.
if ((Test-Path '.sunucuda-calisiyor') -and -not $env:AXBOT_YEREL) {
    $where = (Get-Content '.sunucuda-calisiyor' -Raw).Trim()
    Fail "Bot şu an sunucuda çalışıyor ($where). Bu bilgisayarda da açılırsa iki bot olur.`n       Yine de burada açmak için önce sunucudakini durdurup '.sunucuda-calisiyor' dosyasını silin."
}

if (Get-Process -Name 'discord-bot' -ErrorAction SilentlyContinue) {
    Write-Host 'Bot zaten çalışıyor. Yeniden başlatmak için önce "Botu Durdur.cmd" kullanın.' -ForegroundColor Yellow
    exit 0
}

# ---------------------------------------------------------------- 1. Docker
Step 'Docker kontrol ediliyor'
& docker info --format '{{.ServerVersion}}' 2>$null | Out-Null
if ($LASTEXITCODE -ne 0) {
    $candidates = @(
        "$env:LOCALAPPDATA\Programs\DockerDesktop\Docker Desktop.exe",
        "$env:ProgramFiles\Docker\Docker\Docker Desktop.exe"
    )
    $dockerDesktop = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $dockerDesktop) { Fail 'Docker Desktop bulunamadı. Lütfen kurun veya elle başlatın.' }

    Write-Host 'Docker Desktop başlatılıyor (bu 30-60 saniye sürebilir)...'
    Start-Process -FilePath $dockerDesktop | Out-Null

    $ready = $false
    for ($i = 0; $i -lt 90; $i++) {
        Start-Sleep -Seconds 2
        & docker info --format '{{.ServerVersion}}' 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { $ready = $true; break }
    }
    if (-not $ready) { Fail 'Docker 3 dakika içinde hazır olmadı.' }
}
Write-Host 'Docker hazır.'

# ---------------------------------------------------------------- 2. Servisler
Step 'Veritabanı ve arama servisleri başlatılıyor'
& docker compose up -d --wait
if ($LASTEXITCODE -ne 0) { Fail 'Servisler başlatılamadı (docker compose logs ile bakın).' }

# ---------------------------------------------------------------- 3. Derleme
Step 'Bot derleniyor (release)'
& cargo build --release
if ($LASTEXITCODE -ne 0) { Fail 'Derleme başarısız.' }

$exe = Join-Path $root 'target\release\discord-bot.exe'
if (-not (Test-Path $exe)) { Fail "Derlenmiş dosya bulunamadı: $exe" }

# ---------------------------------------------------------------- 4. Başlat
Step 'Bot başlatılıyor'
if ($Arkaplan) {
    # Pencere açmadan; bu betiği çalıştıran pencere kapansa da bot çalışmaya devam eder.
    $startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ ShowWindow = [uint16]0 }
    $watchdog = Join-Path $root 'scripts\run-bot.cmd'
    $result = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
        CommandLine               = "cmd.exe /c `"`"$watchdog`"`""
        CurrentDirectory          = $root
        ProcessStartupInformation = $startup
    }
    if ($result.ReturnValue -ne 0) { Fail "Bot başlatılamadı (hata kodu $($result.ReturnValue))." }
    Write-Host 'Bot arka planda çalışıyor. Durdurmak için: Botu Durdur.cmd' -ForegroundColor Green
} else {
    # Kendi konsol penceresinde; loglar canlı görünür. Pencereyi kapatmak veya Ctrl+C
    # botu düzgünce kapatır (bekleyen XP kaydedilir). Bekçi (run-bot.cmd) bot çökerse yeniden açar.
    $watchdog = Join-Path $root 'scripts\run-bot.cmd'
    Start-Process -FilePath 'cmd.exe' -WorkingDirectory $root -ArgumentList '/c', "`"$watchdog`""
    Write-Host 'Bot kendi penceresinde açıldı. Kapatmak için o pencereyi kapatın.' -ForegroundColor Green
}
Write-Host "Loglar ayrıca: $root\logs"
