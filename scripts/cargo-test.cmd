@echo off
setlocal
if exist D:\ (
  if not exist D:\Temp mkdir D:\Temp
  set "TEMP=D:\Temp"
  set "TMP=D:\Temp"
  if exist D:\cargo-home set "CARGO_HOME=D:\cargo-home"
  set "CARGO_TARGET_DIR=D:\cargo-target\chaincord"
)
if exist "D:\Tools\VSBuildTools\VC\Auxiliary\Build\vcvars64.bat" (
  call "D:\Tools\VSBuildTools\VC\Auxiliary\Build\vcvars64.bat"
)
cd /d "%~dp0.."
cargo test --manifest-path src-tauri\Cargo.toml --lib -- --test-threads=1 %*
