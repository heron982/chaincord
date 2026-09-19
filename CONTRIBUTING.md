# Contributing to Chaincord

Thanks for helping. Chaincord is an **alpha** desktop chat (Tauri 2 + React + Rust). Read [`README.md`](README.md), [`SECURITY.md`](SECURITY.md), and the [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) before you dig in.

## Prerequisites

- **Node.js** (LTS) and npm
- **Rust** (stable) with Cargo
- **Windows:** WebView2 and C++ build tools (Visual Studio Build Tools) for `tauri:dev` / installers
- Linux / macOS: the UI and Rust unit tests should run; full Tauri packaging is primarily validated on Windows today

## Setup

```bash
npm install
npm test
npm run test:core
npm run tauri:dev
```

Installer / release binary:

```bash
npm run dist
```

Output: `src-tauri/target/release/bundle/` (NSIS + exe on Windows).

## Repository map

| Path | Role |
|---|---|
| `apps/web` | React UI, Vitest |
| `src-tauri` | Rust core (peers, relay, crypto, calls), Tauri shell |
| `docs/architecture.md` | Product **intent** and target design — not a checklist of what already ships |
| `SECURITY.md` | What is true in the alpha binary (invite key, creator-hosted MQTT, etc.) |

## What we welcome

- Bug fixes with a clear repro
- UI polish and accessibility
- Call / invite / Windows build reliability
- Tests for `apps/web/src/logic.ts` and Rust lib tests
- Docs that match the binary

## Discuss first (large / roadmap)

MLS, erasure-coded history, full SFU (`str0m`), platform MQTT broker, Hamachi-style VPN, or a mandatory hosted middlebox. Prefer an issue before a big PR. See `docs/architecture.md`.

## Pull requests

1. Branch from `master`
2. Keep the change focused
3. Run `npm test` and `npm run test:core` locally
4. Describe **what** changed and **how** you tested
5. Do not commit secrets, personal machine paths, or large binaries

Vulnerability reports: do **not** open a public issue — use [SECURITY.md](SECURITY.md).

## Good first issues

Look for GitHub labels `good first issue` and `help wanted`. If none are open yet, small doc fixes, Vitest coverage, and Windows setup clarity are always useful.
