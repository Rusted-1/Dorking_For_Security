@echo off
rem Compares the old release (testlab\old\dorker.exe) with this build on the lab sites.
cd /d "%~dp0.."
if not exist "%~dp0old\dorker.exe" (
  echo Put the current release dorker.exe in testlab\old\ first. See testlab\README.md.
  pause
  exit /b 1
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0compare.ps1" -Old "%~dp0old\dorker.exe" %*
echo.
pause
