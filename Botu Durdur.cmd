@echo off
chcp 65001 >nul
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\stop-bot.ps1"
timeout /t 3 >nul
