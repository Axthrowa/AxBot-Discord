@echo off
rem AxBot bekçisi: botu çalıştırır, beklenmedik şekilde kapanırsa 10 sn sonra yeniden açar.
rem - Pencereyi kapatmak veya "Botu Durdur.cmd" botu kalıcı olarak durdurur (yeniden açılmaz).
rem - Panelden "Yeniden başlat" (çıkış kodu 3) botu hemen yeniden açar.
rem start-bot.ps1 bunu kendisi çalıştırır; elle çalıştırmaya gerek yok.
chcp 65001 >nul
cd /d "%~dp0.."
title AxBot - kapatmak icin pencereyi kapatin
set AXBOT_WATCHDOG=1
if exist "logs\.durdur" del /q "logs\.durdur"

:dongu
"target\release\discord-bot.exe"
set KOD=%ERRORLEVEL%
if exist "logs\.durdur" (
    del /q "logs\.durdur"
    goto son
)
if "%KOD%"=="0" goto son
if "%KOD%"=="3" (
    echo.
    echo Panelden yeniden baslatma istendi, bot yeniden aciliyor...
    goto dongu
)
echo.
echo Bot beklenmedik sekilde kapandi (kod %KOD%). 10 sn sonra yeniden baslatiliyor...
echo Durdurmak icin bu pencereyi kapatin.
timeout /t 10 /nobreak >nul
if exist "logs\.durdur" (
    del /q "logs\.durdur"
    goto son
)
goto dongu

:son
