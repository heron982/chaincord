# Security

Chaincord is **alpha**. Do not use it for data you would not put in a group whose invite leaked.

## What holds today

- The live channel key travels **in the invite**. Anyone with the invite can read the chat.
- There is no Chaincord MQTT broker. Whoever **creates** the community hosts the hub in their app; the invite carries the `ws://` address. Joiners do not configure anything. If the creator is unreachable (NAT without a port / no VPS), cross-network chat does not come up. LAN and direct P2P still work. The topic includes `community_id`; the hub sees metadata, not plaintext (when the invite key is correct).
- `CHAINCORD_RELAY=off` disables the MQTT client in this process. `CHAINCORD_RELAY=wss://…` at create time replaces the local hub with an external broker.
- TURN `openrelay.metered.ca` and public STUN exist only so 1:1 calls can traverse NAT. They are not production infrastructure.
- The target design (MLS, erasure, blind node, community SFU) lives in [`docs/architecture.md`](docs/architecture.md) and is **not in the binary** yet. Group calls today use an elected **HUB:N** WebRTC peer, not a full SFU.

## Own relay

There is no relay field in the UI. The desktop that creates the community is already the hub. For internet use, that machine must accept TCP on the app port (LAN, VPS, or opened port).

## Reporting a vulnerability

Do not open a public issue with an exploit, key dump, or PoC.

Preferred: [GitHub Security Advisories](https://github.com/heron982/chaincord/security/advisories/new) (private vulnerability reporting).

Or email: `felipedevlp@gmail.com`, with:

- version (`package.json` / `src-tauri/tauri.conf.json`)
- what happens vs what should happen
- whether a third party can read messages, impersonate identity, or take down the network
