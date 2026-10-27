@echo off
cd /d "%~dp0"
start "" http://localhost:8787
dorker.exe serve --port 8787
