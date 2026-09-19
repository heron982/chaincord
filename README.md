# Chaincord

Decentralized live chat (**alpha**). Communities, channels, E2EE in the design — the binary is still an MVP.

Architecture notebook: [`docs/architecture.md`](docs/architecture.md). License: [Apache-2.0](LICENSE).

This is an **alpha** desktop chat. The live channel key still travels in the invite; do not treat it as production E2EE. See [`SECURITY.md`](SECURITY.md).

## What works today

- Create a community or join with an invite
- Chat in `#general` on the LAN; across networks if the creator’s PC is reachable (their app is the MQTT relay)
- Voice/video call: **1:1** is direct WebRTC; **3+** uses an elected **HUB:N** peer (not a full SFU product yet). CGNAT may fail until you run your own TURN
- Two clients on one PC: open the app twice (the second instance picks another port)

## What it is not yet

MLS, erasure-coded history, a real SFU (`str0m`), community coturn, or a 24h `--node` daemon. The live key still goes in the invite.

There is no Chaincord-operated MQTT broker. The creator hosts the hub in the app; the invite carries the address. Without that machine reachable, only same-network chat works. Public OpenRelay TURN is still a 1:1 call fallback. Details in [`SECURITY.md`](SECURITY.md).

## Development

Needs Node.js, Rust, and (on Windows) WebView2 + Build Tools.

```bash
npm install
npm test
npm run test:core
npm run tauri:dev
```

Installer / exe:

```bash
npm run dist
```

Default Tauri output: `src-tauri/target/release/bundle/` (NSIS + exe). If a `D:\` disk exists with the local build layout, scripts under `scripts/` use it; otherwise they use system Cargo/Node.

## Quick start

1. Open the app
2. **Create** a community or **Join** with an invite
3. Chat in `#general`

Same Wi-Fi: the invite already carries the LAN IP. Different networks: the creator’s app is the relay — both peers reach that machine.
