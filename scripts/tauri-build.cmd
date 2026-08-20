@echo off
setlocal
set "TEMP=D:\Temp"
set "TMP=D:\Temp"
set "CARGO_HOME=D:\cargo-home"
set "CARGO_TARGET_DIR=D:\cargo-target\chaincord"
if not exist D:\Temp mkdir D:\Temp
call "D:\Tools\VSBuildTools\VC\Auxiliary\Build\vcvars64.bat"
cd /d "%~dp0.."
npx tauri build %*
