@echo off
setlocal
cd /d "%~dp0.."
npx tauri build %*
