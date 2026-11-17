# Veritabanının günlük yedeği (yerel bilgisayar).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\yedek-al.ps1            yedek al
#   ... -Kopya 'D:\AxBot-yedek'                                                        ayrıca başka bir yere kopyala
#   ... -Kur [-Kopya ...]                                                              her gün 04:00'te çalışacak görev ekle
#   ... -Dene                                                                          son yedeği deneme veritabanına yükle, satırları karşılaştır
#
# Yedekler `yedekler\axbot-YYYY-AA-GG.dump` (pg_dump -Fc); 7 günden eskileri silinir.
# Docker / PostgreSQL kapalıysa yedek alınmaz (bot da çalışmıyordur, veri değişmemiştir).
param(
    [string]$Kopya = '',
    [switch]$Kur,
    [switch]$Dene
)

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
$dir = Join-Path $root 'yedekler'
$log = Join-Path $dir 'yedek.log'
New-Item -ItemType Directory -Force $dir | Out-Null
Set-Location $root

function Say([string]$text) {
    $line = "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $text"
    Write-Host $line
    Add-Content -Path $log -Value $line -Encoding UTF8
}

if ($Kur) {
    $taskArgs = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$PSCommandPath`""
    if ($Kopya) { $taskArgs += " -Kopya `"$Kopya`"" }
    $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $taskArgs -WorkingDirectory $root
    $trigger = New-ScheduledTaskTrigger -Daily -At '04:00'
    # Bilgisayar 04:00'te kapalıysa açılınca ilk fırsatta çalışır.
    $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Minutes 30)
    Register-ScheduledTask -TaskName 'AxBot veritabani yedegi' -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null
    Say 'Günlük yedek görevi kuruldu (her gün 04:00).'
    exit 0
}

& docker compose exec -T postgres pg_isready -q 2>$null
if ($LASTEXITCODE -ne 0) {
    Say 'PostgreSQL çalışmıyor; yedek alınmadı.'
    exit 0
}

if ($Dene) {
    $latest = Get-ChildItem $dir -Filter 'axbot-*.dump' | Sort-Object Name | Select-Object -Last 1
    if (-not $latest) { Say 'Denenecek yedek yok.'; exit 1 }
    & docker compose cp $latest.FullName postgres:/tmp/deneme.dump
    # Canlı veritabanına dokunulmaz: ayrı bir deneme veritabanı kurulur, karşılaştırılır, silinir.
    & docker compose exec -T postgres sh -c 'dropdb -U $POSTGRES_USER --if-exists yedek_deneme && createdb -U $POSTGRES_USER yedek_deneme && pg_restore -U $POSTGRES_USER -d yedek_deneme --no-owner /tmp/deneme.dump'
    $restored = $LASTEXITCODE
    $tables = 'SELECT count(*) FROM information_schema.tables WHERE table_schema = chr(112) || chr(117) || chr(98) || chr(108) || chr(105) || chr(99)'
    $live = & docker compose exec -T postgres sh -c "psql -U `$POSTGRES_USER -d `$POSTGRES_DB -tAc '$tables'"
    $copy = & docker compose exec -T postgres sh -c "psql -U `$POSTGRES_USER -d yedek_deneme -tAc '$tables'"
    & docker compose exec -T postgres sh -c 'dropdb -U $POSTGRES_USER --if-exists yedek_deneme; rm -f /tmp/deneme.dump'
    if ($restored -eq 0 -and "$live".Trim() -eq "$copy".Trim()) {
        Say "Geri yükleme denemesi BAŞARILI: $($latest.Name), $("$copy".Trim()) tablo."
        exit 0
    }
    Say "Geri yükleme denemesi BAŞARISIZ: $($latest.Name) (pg_restore çıkış $restored, tablo $("$live".Trim()) / $("$copy".Trim()))."
    exit 1
}

$name = "axbot-$(Get-Date -Format 'yyyy-MM-dd').dump"
$file = Join-Path $dir $name
# PowerShell 5.1 ikili çıktıyı bozar ve harici programlara giden çift tırnağı kaldırır: dosya kapsayıcıda yazılıp kopyalanır.
& docker compose exec -T postgres sh -c 'pg_dump -U $POSTGRES_USER -d $POSTGRES_DB -Fc -f /tmp/yedek.dump'
if ($LASTEXITCODE -ne 0) { Say 'pg_dump başarısız; yedek alınmadı.'; exit 1 }
& docker compose cp postgres:/tmp/yedek.dump "$file.tmp"
$copied = $LASTEXITCODE
& docker compose exec -T postgres rm -f /tmp/yedek.dump
if ($copied -ne 0 -or -not (Test-Path "$file.tmp") -or (Get-Item "$file.tmp").Length -lt 1024) {
    Remove-Item -Force "$file.tmp" -ErrorAction SilentlyContinue
    Say 'Yedek dosyası alınamadı.'
    exit 1
}
Move-Item -Force "$file.tmp" $file
Say "Yedek alındı: $name ($([math]::Round((Get-Item $file).Length / 1KB)) KB)."

Get-ChildItem $dir -Filter 'axbot-*.dump' | Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-7) } | ForEach-Object {
    Remove-Item -Force $_.FullName
    Say "Eski yedek silindi: $($_.Name)."
}

if ($Kopya) {
    try {
        New-Item -ItemType Directory -Force $Kopya -ErrorAction Stop | Out-Null
        Copy-Item -Force $file $Kopya -ErrorAction Stop
        Get-ChildItem $Kopya -Filter 'axbot-*.dump' | Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-7) } | Remove-Item -Force
        Say "Kopya: $Kopya"
    } catch {
        Say "Kopyalanamadı ($Kopya): $($_.Exception.Message)"
        exit 1
    }
}
