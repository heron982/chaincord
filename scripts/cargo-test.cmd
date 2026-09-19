@echo off
setlocal
cd /d "%~dp0.."
cargo test --manifest-path src-tauri\Cargo.toml --lib -- --test-threads=1 %*
