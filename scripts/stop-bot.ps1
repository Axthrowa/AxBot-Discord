# Arka planda (-Arkaplan ile) çalışan botu durdurur.
# Pencereli çalışıyorsa pencereyi kapatmak yeterli ve tercih edilir (tam düzgün kapanış).
#
# Not: Burada süreç doğrudan sonlandırılır. Veri kaybı olmaz: XP Redis'te kalıcıdır ve
# bot bir sonraki açılışta 30 saniye içinde PostgreSQL'e yazar.
#
# Veritabanı servisleri çalışmaya devam eder; onları da durdurmak için: docker compose down

$running = Get-Process -Name 'discord-bot' -ErrorAction SilentlyContinue
if (-not $running) {
    Write-Host 'Bot çalışmıyor.'
    exit 0
}

# Bekçiye (run-bot.cmd) "bilerek durduruldu, yeniden açma" işareti.
$flag = Join-Path (Split-Path -Parent $PSScriptRoot) 'logs\.durdur'
New-Item -ItemType Directory -Force (Split-Path $flag) | Out-Null
Set-Content -Path $flag -Value 'durdur' -Encoding ASCII
$running | Stop-Process -Force
Write-Host "Bot durduruldu (PID $($running.Id -join ', '))." -ForegroundColor Green
