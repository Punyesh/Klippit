@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup-ffmpeg.ps1"
set "RC=%ERRORLEVEL%"
if not "%RC%"=="0" echo.
if not "%RC%"=="0" echo Klippit FFmpeg setup failed with exit code %RC%.
exit /b %RC%
