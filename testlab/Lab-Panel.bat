@echo off
rem Opens the control panel wired to the offline test lab.
cd /d "%~dp0.."
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0lab-panel.ps1" %*
