@echo off
rem Runs the automatic offline test (fake sites + fake Serper on localhost).
cd /d "%~dp0.."
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0run-lab.ps1" %*
echo.
pause
