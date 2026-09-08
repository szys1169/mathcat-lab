@echo off
setlocal
cd /d "%~dp0"
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\start-version.ps1"
if errorlevel 1 (
  echo.
  echo MathCat Lab 2.5.1 failed to start. Please read the message above.
  pause
)
