# Contributing to Chaincord

Thanks for helping. Chaincord is an **alpha** desktop chat (Tauri 2 + React + Rust). Read [`README.md`](README.md), [`SECURITY.md`](SECURITY.md), and the [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) before you dig in.

## Prerequisites

- **Node.js** (LTS) and npm
- **Rust** (stable) with Cargo
- **Windows:** WebView2 and C++ build tools (Visual Studio Build Tools) for `tauri:dev` / installers
- Linux / macOS: the UI and Rust unit tests should run; full Tauri packaging is primarily validated on Windows today

## Setup

**External contributors:** fork [heron982/chaincord](https://github.com/heron982/chaincord) on GitHub, then:

```bash
git clone https://github.com/<your-user>/chaincord.git
cd chaincord
git remote add upstream https://github.com/heron982/chaincord.git
npm install
npm test
npm run test:core
npm run tauri:dev
```

Keep your fork current before starting work:

```bash
git fetch upstream
git checkout master
git merge upstream/master
```

**Maintainers** with push access can clone `heron982/chaincord` directly and skip the fork/`upstream` steps.

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

1. Fork the repo (unless you already have push access)
2. Create a branch from up-to-date `master` (`git checkout -b fix/short-name`)
3. Keep the change focused
4. Run `npm test` and `npm run test:core` locally
5. Push to **your fork** and open a PR against `heron982/chaincord` `master`
6. Describe **what** changed and **how** you tested
7. Do not commit secrets, personal machine paths, or large binaries

Vulnerability reports: do **not** open a public issue — use [SECURITY.md](SECURITY.md).

## Releases and auto-update (maintainers)

Windows installers are built by GitHub Actions on version tags (`v0.1.16`) or via **Actions → Release → Run workflow**. Contributors do not need this section to send a PR.

1. Put the updater **private** key in GitHub → Settings → Secrets:
   - `TAURI_SIGNING_PRIVATE_KEY` — full contents of your local `.tauri/chaincord.key` (never commit this file)
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — only if the key has a password
2. Bump `version` in `package.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml`
3. Commit, then `git tag vX.Y.Z && git push origin vX.Y.Z`
4. The workflow uploads the NSIS setup.exe, `.sig`, and `latest.json` to the GitHub Release
5. Installed apps check `…/releases/latest/download/latest.json` and offer **Install and restart** (Settings → About)

The **public** key is already in `src-tauri/tauri.conf.json`. If you regenerate keys, update that pubkey and ship a new build — old installs cannot verify updates signed with a new key.

## Good first issues

Look for GitHub labels `good first issue` and `help wanted`. If none are open yet, small doc fixes, Vitest coverage, and Windows setup clarity are always useful.
