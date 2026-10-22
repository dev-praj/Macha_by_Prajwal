@echo off
setlocal
powershell.exe -NoProfile -File "%~dp0uninstall-macha.ps1"
echo.
pause
