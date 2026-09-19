# Chaincord — architecture notebook

Living product and architecture notes.

Project vocabulary: **community**, **signed log**, **channel**, **node**, **client**, **E2EE**. Do not use ledger analogies or ledger implementations as design tools.

- **Definitions (Felipe)** — locked decisions. Change only if redefined.
- **Open** — hypotheses and disputed points.
- **Technical review** — architect analysis; does not become definition by itself.

When something is decided in conversation, it goes into Definitions and into the history at the end.

### Shipped today vs target (verify against the binary)

| Area | In the alpha binary now | Target (this notebook) |
|---|---|---|
| Chat transport | Creator-hosted MQTT hub in the app; invite carries `ws://` | Same idea; optional member `--node` later |
| Channel crypto | Live key **in the invite** (not MLS) | OpenMLS + history keys |
| History | Local SQLite cache; no erasure pool | Erasure among members; pending/lost UI states |
| Call 2 people | Direct WebRTC (+ public STUN/TURN OpenRelay) | Same + community coturn |
| Call 3+ | Elected **HUB:N** WebRTC peer (forwards media); prefer **non-owner**, then stable sorted key | Full desktop SFU (`str0m`) with capability tests |
| Platform broker | None | None (def. 24) |

README / SECURITY describe the binary. This notebook is mostly **target design**. Sections below that say “definition” are product intent; they are not a claim that MLS/erasure/SFU already ship.

---

## Definitions (Felipe)

1. **The network must be decentralized.** Communication infrastructure does not depend on a central operator as owner of the network. Users (and nodes they control) are the basis of availability.
2. The product is a **live chat** with a Discord-like mental model: communities, channels, roles, permissions.
3. Communication must be **E2EE**. Intermediate peers see ciphertext, not content.
4. **Messages live in the channel log**, not in the community authority log.
5. **Do not assume** that: every user must host data; all communication must be direct P2P; no server may exist; tokens/rewards are required; a DAO is required; every community must be public.
6. Communities may be **public or private**. Private ones need not be enumerable. Users may be **pseudonymous**. Members should not be globally trackable by default.
7. Private data stays on community nodes and clients, encrypted. There is no public membership registry or public graph of user relationships.
8. **Community authority ≠ infrastructure that delivers bytes.** A node can host without owning the community.
9. Platform-owned infrastructure, if any, is for **coordination and fallback**, not the primary path by design.
10. Tokenomics / rewards are **not a requirement** unless they solve a concrete technical problem.
11. What you “own” is the **community** (owner, roles, permissions). A message is not that state.
12. Each community has its **signed log** (ops from a genesis). That is the source of truth for authority.
13. **Out of scope:** any ledger model, analogy, or implementation. From here on the notebook and conversation do not use those terms as design tools.
14. **A new member sees channel history.** Discord-like: joining means seeing `#general`’s past, not starting from zero.
15. **The community node does not hold history keys and does not read the channel.** If no prior member is online at join, the new client **cannot decrypt the past** until an older member appears and shares keys. The node may already have ciphertext; without keys it is unreadable.
16. **No commercial storage plans.** Whoever needs durable history runs their own node (PC/VPS) and owns the disk. Charging for GB/retention as a platform product is out of scope.
17. **Community history is split among that community’s members** (sharding / erasure coding). There is no single disk that owns the archive, and no swarm across other communities. Each storage member holds **encrypted shards**; any *k* of *n* reconstructs. Without channel keys, a shard stays unreadable (definition 15).
18. **Live chat does not depend on history.** The product accepts that the past may be **pending** (no key, or fewer than *k* shards online). The UI says so instead of faking Discord when the guild is empty. Offline ≠ loss if someone’s disk still has the shard; real loss only if shards are gone and *n* no longer covers *k*.
19. **Call mode by client count.** **2 clients → 1:1** (WebRTC between them + TURN if NAT; **no hub/SFU**), even if both are phones. **3 or more → elect one media hub** automatically in the call roster. The user does not configure a server. Do not slice the stream across multiple hubs. If the elected peer drops, re-elect. If 3+ and nobody is eligible, group camera/screen is unavailable (2-person 1:1 does not depend on this).

If a third person joins a call that was already 1:1, the app **migrates** to hub mode (if someone is eligible) or refuses the third person’s video / greys the icon. If the room returns to 2, it may return to 1:1.

**Alpha today:** the hub is **HUB:N** (one peer forwards WebRTC media), not a finished SFU product. Election: prefer someone **other than the community owner**, then a stable sorted public key. Capability gates (mobile / cellular / background / bandwidth tests) are still target behavior.

**Must not be hub/SFU (by default, target):**
- phone (iOS/Android);
- device on mobile data only (4G/5G);
- client in background / power-saving;
- who fails the automatic test (insufficient upload, unreachable NAT when TURN-only is not viable for a hub, CPU/thermal).

**May be hub/SFU (target):** desktop (Windows/macOS/Linux) in the call, Wi-Fi/Ethernet, app in foreground, upload above the room floor. A member’s always-on VPS counts as desktop.

**UI:** on a 3+ call, an icon on the participant tile makes **explicit who is the hub**. If re-elected, the icon moves. On 1:1 the icon **does not appear**.

Implication 19: everyone is a client; at most **one** device in the call is hub. “Everyone is SFU” mesh and splitting frames across machines are out. Platform TURN stays invisible to reach the elected hub. Recording the call into erasure: out.

Implication 14+15: community channels have no forward secrecy against future members, but who only stores shards **is not a crypto participant**. UX differs from Discord in an empty corner: joining a dead guild shows no past (or “history pending”) until someone from the community comes online. DMs stay stricter.

Implication 17+22: the pool is **per community**, not all of Chaincord. **Every guild client joins *n*** (encrypted shards); phones get a smaller quota, not zero. Owner / first device is **not** the sole server. Kick/leave: handoff (def. 20) for anyone who leaves, including mobile. Mobile-only communities stay fragile (devices sleep, little disk).

Implication 18: do not promise “joined, read 2019” without keys or *k* storage peers. Promise: live conversation works; the archive returns when the community wakes. UI distinguishes the three states below.

20. **A storage peer does not drop its shard silently.** Whoever is in erasure *n* only leaves the storage role (or the community) after the shard was **re-placed**. Felipe’s preference: on leave, **pick another eligible member** to take over. The app may also **self-repair** in the remaining pool (no forced picker if the pool can absorb it). Destination must be storage-capable (same “can persist” bar — not phones by default) and accept quota. Kick: repair **without** the kicked user choosing. Crash / wipe / disappear: no dialog — definition 18 (pending or lost). With def. 22 there is no “light cache-only” guild client: leaving always goes through the shard rule (auto-repair if the pool can take it).
21. **MVP stack (target):** Rust core (identity, log, MLS, erasure, sync, desktop SFU); desktop UI **Tauri 2 + React/TS**; cache **SQLite**; chat **WebSocket** (QUIC later); 1:1 call **WebRTC**; embedded SFU **`str0m`**; NAT **coturn**; erasure **`reed-solomon-erasure`**; group E2EE **OpenMLS**; identity **ed25519-dalek** + `did:key`. Same Rust binary with `--node` for 24h VPS/PC. Light mobile later (UniFFI). Out of v1: Electron, DHT/libp2p, Matrix fork, LiveKit/mediasoup as product, crypto in the UI.
22. **Every community client is a history node, not live-message-only.** Joining the guild = joining that community’s erasure *n* (encrypted shards). There is no “I only chat, disk is someone else’s problem.” What is **not** fixed: a single device owning the archive (owner / first to open is **not** *the* server). The archive is the set of clients. Per-device quota (phone stores less than desktop); leave handoff still applies (def. 20). Without recoverable *k* shards, history is pending/lost (def. 18). Live chat does not wait on the archive.
23. **P2P between clients stays; the platform does not host a middlebox to hide IPs.** Community is for people you trust — invites are not for anyone. A member (or a leaked invite) can see network addresses and hassle others’ connections; that is the price of not being central Discord. Warn in the UI; do not add a Chaincord gateway. A member’s `--node` is still allowed; Felipe is not obliged to run infra to “fix” P2P.
24. **MQTT relay belongs to the community.** Whoever creates **hosts the hub in their own app** (same port as P2P). The invite carries those `ws://` URLs. No HiveMQ/EMQX/Mosquitto and no URL field. Joiners only paste the invite. Without the creator reachable on the internet (LAN, VPS, opened port), WAN does not come up — LAN/direct P2P still works. `CHAINCORD_RELAY` is local override only. Call TURN is a separate cable.

### Hypothesis (not locked)

- **Message audit** — put some proof in the authority log? Review recommendation: not the body; per-channel log; Merkle root in the authority log only if it becomes a requirement. See [History and audit](#history-and-audit).

---

## Open

| Topic | Felipe’s position | Review position | Status |
|---|---|---|---|
| Decentralized network | Requirement | Signed log + replaceable nodes | **Defined** |
| Message audit in authority log | Undecided | Body no; hash chain per channel | **Open** |
| P2P as primary delivery path | Yes; no platform server to hide IP | P2P = shortcut; member node ok; trust warning on invite | **Defined (23)** |
| Who operates storage | All clients of that community | Erasure; smaller phone quota | **Defined (22)** |
| Peer roles | Clients = msg + history nodes | Hub/SFU remains desktop-only (def. 19) | **Defined (22)** |
| Sharding / erasure per community | Yes, among guild clients | Pool = members, not Chaincord | **Defined** |
| Phone in erasure *n* | Yes — every client | Small quota; battery/disk cost | **Defined (22)** |
| Leave and storage shard | Leave only after handing off shard (pick someone) | Auto-repair in pool + explicit handoff; kick/crash separate | **Defined (20)** |
| Paid storage plans (GB) | Out of scope | Disk in the member pool | **Closed: no** |
| E2EE vs admin vs history vs search | E2EE; new member sees past; node does not read | Channels: E2EE vs node and strangers; not vs future members. Server-side search still in tension | Partially closed |
| New member sees history? | Yes | Ciphertext on nodes + keys from an online older member | **Defined: yes** |
| History if no older member online | Cannot decrypt until someone connects | No key = pending, not fake empty | **Defined** |
| Live chat vs archive | Live independent; archive may pend | Honest UI; do not mix loss with unavailable | **Defined (18)** |
| Voice/video 2 clients | 1:1 without hub/SFU, including two phones | WebRTC + TURN | **Defined (19)** |
| Voice/video 3+ | Elected hub; grey icon if pool empty | No multi-SFU mesh; no frame slicing | **Defined (19)** |
| Who cannot be hub/SFU | Phone, 4G, background, failed test | Desktop Wi-Fi/Ethernet in foreground | **Defined (19)** |
| UI: who is the hub | Icon on participant (3+) | No icon on 1:1 | **Defined (19)** |
| Stack | Rust + Tauri 2 + React; coturn; OpenMLS | One core, three roles (light, storage, SFU) | **Defined (21)** |
| First device = the server | No | Everyone shares shards; owner ≠ disk | **Defined (22)** |
| Human identity (name) | Pseudonym ok | `did:key` + alias | Open |
| Chaincord middlebox (Discord-style) | Not worth it; breaks the thesis | Closes IP attack; Felipe will not operate that in MVP | **Defined (23): no** |
| Community open to any invite | No — trusted people only | Warning does not stop leaked invite / hostile member | **Defined (23)** |
| MQTT relay | Hub in creator’s app; invite carries `ws://` | No public broker; WAN needs reachable creator | **Defined (24)** |

---

## Current architecture (review — not definition)

```text
Plane 1  Authority         did:key + signed log (genesis → role/channel ops)
Plane 2  Confidentiality   MLS on private channels; Double Ratchet on DMs
Plane 3  Availability      1–3 nodes in the community manifesto
Plane 4  Coordination      STUN/TURN, push, opt-in directory, managed nodes
```

Trust contract:

- Integrity / ownership → user keys.
- Availability / fan-out → community nodes (may be user-owned).
- Delivery metadata → the node sees who / when / size, unless extra protection is added later.

How to decentralize, in order:

1. Open protocol + user keys.
2. Signed authority log — the host does not grant admin.
3. More than one node per community, chosen by the owner.
4. Local-first client — what you already saw lives on the device.
5. Anyone can run the node (if the platform hosts, it is a tenant).
6. Invite carries `community_id` + addresses + capability.

Direct P2P is a **mode** (1:1 shortcut, history sync), not the definition of the network. On Brazilian mobile CGNAT the direct path often fails; user nodes/relays remain decentralization if they are not an operator with protocol power.

---

## History and audit

The product needs **per-channel history**. Without it it becomes IRC: close the app, conversation gone.

Where history lives:

- **Client** — cache of what that device already saw.
- **Community nodes** — ciphertext for offline and for who joins later.
- **Not** in the authority log.

New message: live fan-out to who is connected. Who was offline fetches the missing range from nodes. That can *look* like hash-range sync (I have through 100, send 101–140); the swarm is the community replica set, encrypted, with membership — not a public swarm.

Two logs:

```text
Authority log     (small)
  genesis, owner, roles, channels, replicas
  optional: Merkle checkpoint of channel tips

Message log       (per channel, archivable)
  ciphertext signed by the author
  hash(previous)
  tombstone if deleted
```

| Goal | Body in authority log? | Tool |
|---|---|---|
| Message not tampered | No | Author signature |
| Channel order not rewritten | No | Channel hash chain / DAG |
| Admin did not silent-delete | No | Tombstone or delete op in authority log |
| Replica omitted a message | No | Periodic Merkle root in authority log |
| Third party reads what was said | Yes, and breaks E2EE | Only a declared public channel, still in the channel log |
| Who banned whom | No | Role/kick ops in authority log |

A new member **sees the past** when an older member can share keys (definitions 14 and 15). Flow:

1. Join enters the authority log (now a member).
2. Client downloads channel ciphertext from nodes — still unreadable.
3. First online older member sends history keys (Welcome / history key).
4. Client decrypts the cache and the channel “appears”.

If nobody from the community is around, step 3 does not happen. The product shows pending or empty history; it does not break node E2EE. That corner is more Signal than Discord; day to day, with someone online, it feels like Discord.

Integrity MVP: signed envelope + channel hash chain + tombstone. Authority-log checkpoint only if “prove the host did not omit” becomes a requirement.

---

## Storage (definition 17)

History of **that** community is sliced among its members. A user from another guild stores nothing.

```text
Encrypted blob (media or text batch)
        │
        ▼
erasure encode  →  n shards   (e.g. 10)
        │
        ├── member A (desktop) stores more shards
        ├── member B (PC) stores more shards
        └── phone                 joins n, small quota

Reconstruct: any k shards   (e.g. 4 of 10)
Read content: still need channel keys (older member online)
```

- Text may be fully replicated more widely (cheap); **media** is what gets sliced.
- Join: recent-window cache; the rest builds on demand from shards.
- Member leaves / device gone: see definition 20 (handoff / repair). Without that, *k* vanishes and the archive dies.
- Kick does not wipe what the ex-member already had on disk; they only stop receiving new shards. Ciphertext without keys does not open in the UI. Repair in the remaining pool, without the kicked user choosing a destination.
- Quota still exists: per-member upload cap, or a spammer fills every disk in the pool.
- No paid GB plan (definition 16).

**Leave (definition 20):**

```text
Leave (storage peer)
  1. UI: “who takes your shards?” (eligible members online)
  2. or auto-repair in the rest of the pool, if it fits
  3. transfer / re-encode finishes
  4. only then leave n (and the community, if applicable)
```

If nobody can take over: do not complete leave — warn that the guild archive is at risk (definition 18). Force-leave = accept that loss. Applies to desktop and phone (definition 22).

What this **is not**: a BitTorrent of all Chaincord. The “who has shard X” index lives only among community members.

Review risk (still valid): phones in *n* cost disk, battery, and NAT. Mitigation locked in def. 22: **small quota**, not exclusion. Hub/SFU remains desktop-only (def. 19).

### UI — pending vs lost (definition 18)

Do not conflate in the interface.

| State | What happened | What the UI says |
|---|---|---|
| Live | Someone on the path delivers now | Normal message in the channel |
| Pending — key | Ciphertext (or shards) exist; no older member shared a key | “Encrypted history, waiting for a community member” |
| Pending — *k* | Shards on disks of people who are offline; fewer than *k* online | “Archive unavailable until members return” |
| Lost | Shards below *k* and devices gone | “This part of history could not be recovered” |

Product rules:

- New chat does not wait for the archive to assemble.
- Prefetch / rebuild in background when Wi-Fi and storage peers appear.
- Do not show an empty channel as if there was never a conversation, if the client knows there are pending shards or ciphertext.

---

## Voice, video, and screen (definition 19)

Target SFU = selective forwarder: each participant uploads **one** set of tracks; the hub re-sends. No mosaic mix. Not the chat log. Does not persist the call.

```text
2 clients     A ◄──WebRTC──► B     (+ TURN if NAT)     no hub
3+ clients    all ──► elected hub ──► all
Screen        one heavy track; in groups, usually one at a time
```

**Election (only with 3+; automatic, zero wizard):**

1. Clients announce capability (desktop?, Wi-Fi?, estimated upload, foreground). *(Target; alpha uses a simpler rule.)*
2. Eligible peers enter the pool for **that call**.
3. **Alpha:** prefer a peer who is **not** the community owner, then lowest sorted public key. **Target:** best upload (tie-break: community owner, then who started the room) after capability filters.
4. Host leaves or saturates → re-elect in the pool; the call may flicker; do not ask for an IP.
5. Empty pool → group camera/screen icon **unavailable**. 1:1 / DM call audio continues.

**Hub UI:** badge/icon on the elected participant (3+). Simple tooltip (“this person is forwarding the call”); jargon optional in the label — the icon is the signal. Host change = badge moves. 2 people: no badge.

**Not eligible (target):** mobile, cellular data, background, failed test.  
**Eligible (target):** desktop in the call (or member VPS).  
**Out:** everyone-as-SFU mesh; slicing frames across SFUs; user-configured SFU; recording the call into erasure.

Cascade (2 SFUs + trunk) is for large rooms later — not v1 election.

Call chat follows definition 18: room buffer, gone when the room empties.

---

## Stack (definition 21)

Three roles, one core: light client, desktop storage/SFU, VPS daemon. UI does not implement crypto.

```text
┌─ Desktop app (Tauri 2) ───────────────────┐
│  React/TS UI                              │
│  Rust core: chat · MLS · log · erasure    │
│  optional: SFU (str0m) · storage peer     │
└───────────────────┬───────────────────────┘
                    │ WS / WebRTC
         ┌──────────┼──────────┐
         ▼          ▼          ▼
    other cores   coturn    (later) FCM/APNs
```

| Layer | Choice |
|---|---|
| Core | Rust |
| Desktop UI | Tauri 2 + React/TS |
| Local cache | SQLite (SQLCipher optional) |
| Chat transport | WebSocket now; QUIC (`quinn`) later |
| 1:1 call | WebRTC (`str0m` / webrtc-rs) |
| 3+ SFU | `str0m` in the desktop binary |
| NAT | coturn (platform STUN/TURN, invisible) |
| Push | FCM / APNs on mobile, later |
| Erasure | `reed-solomon-erasure` (desktop/VPS only) |
| Group E2EE | OpenMLS |
| DM | MLS 1:1 or `vodozemac` |
| Identity | ed25519-dalek + `did:key` |
| 24h node | same binary, `--node` |

**Out of v1:** Electron; Flutter as core; Go on the client; LiveKit/mediasoup as product (user-configured server); libp2p/DHT; Matrix/Element fork; keys in the JS renderer.

**Order:** (1) core: keys, genesis, log, WS, live message; (2) SQLite + Tauri UI; (3) MLS + history key; (4) erasure + handoff (def. 20); (5) WebRTC 1:1 + coturn; (6) SFU + election + badge; (7) light mobile.

**Minimal platform:** coturn; small call signaling; HTTPS invites. No GB billing (def. 16).

---

## Original idea (captured, already filtered)

Discord-like live chat, infrastructure by users, E2EE, verifiable community authority (not the platform host).

Conceptual peers: Light, Relay, Storage. Replication for churn. NAT, CGNAT, firewall, mobile, WebRTC, STUN, TURN, DHT, relay/fallback. Platform infra only when necessary.

Community as a verifiable entity:

```text
Community
├── #channels
├── Roles
└── Permissions
```

---

## Technical review (2026-08-18)

Thesis: **portable community, distrustful host, hybrid delivery.**

**Interesting:** authority ≠ infra; identity = key; private communities not enumerable; explicit replicas; P2P when both online; manifesto that survives host swap.

**Necessary:** E2EE against infra; signed log; persistent nodes; STUN/TURN; push; invites; simple RBAC; mailbox / offline history.

**Defer:** global P2P mesh, DAO, token, storage in strangers’ DHT, mesh voice.

**MVP (proposal, not definition):** small communities, text+file, channels, roles, host cannot read or steal owner, self-host and hosted option. Identity, log, node, MLS, local-first client, TURN+push. Out: token, DAO, global DHT, group voice, world directory.

### Condensed answers to the 22 points

1. Viable as signed authority + replaceable nodes; not viable as a world mesh in the core.
2. P2P: 1:1 shortcut and sync among designated replicas.
3. Federated: fan-out, mailbox, presence; platform: TURN, push, tenant.
4. Authority = community signed log.
5. No ledger.
6. Identity: Ed25519 / `did:key`, device linking, seed.
7. E2EE: MLS on private channel; DM with ratchet; large public signed only.
8. Creation: signed genesis + node + invite (`id`, addrs, capability).
9. Roles: ops in the log; node and clients verify back to genesis.
10. Storage: encrypted blobs in the replica set, cache on the client.
11. Replication: manifesto with 1–3 nodes.
12. Offline: mailbox on the node + push; history on the node + local cache.
13. Discovery: invite; mDNS later; global DHT deferred.
14. NAT/CGNAT: ICE; hole punch when it works.
15. TURN: production path — especially mobile in Brazil.
16. Malicious peer: outside the replica set cannot read or forge; at most DoS.
17. Sybil/spam: invite + rate limit; no open directory at the start.
18. Privacy: keys per community; no public membership.
19. No public registry of relationships.
20. Scale: partition by `community_id`.
21. Risks: scope, CGNAT, E2EE vs Discord features, blob liability, mesh voice.
22. MVP: see above.

There is an older review canvas in the Cursor project; **this markdown is the source of definitions.**

---

## History

| Date | What landed |
|---|---|
| 2026-08-18 | First capture of the idea (Discord-like, P2P, E2EE, authority ≠ host) and review of the 22 points. |
| 2026-08-18 | Network **must** be decentralized. Messages in the channel log. No mandatory token. |
| 2026-08-18 | Ledger-analogy exploration **discarded**. Vocabulary and implementation out of scope (definition 13). Notebook rewritten without those terms. |
| 2026-08-18 | Felipe: **new member sees channel history** (definition 14). Implication: no FS against future members; join into empty community needs history keys or delayed history. |
| 2026-08-18 | Felipe: **if no older member is online, the new client has no history** (definition 15). Node does not hold keys and does not read the channel. |
| 2026-08-18 | Felipe: **no storage plans** (definition 16). First sketch was history only on nodes, light clients. |
| 2026-08-18 | Felipe redefined: **split history among community members** via sharding/erasure (definition 17). Pool = that guild, not Chaincord. |
| 2026-08-18 | Felipe: **live chat does not depend on history**; archive may pend; honest UI (definition 18). |
| 2026-08-18 | Felipe: **automatic SFU/hub election** and **who cannot be hub** (definition 19). 1:1 without hub. Group/screen unavailable if pool empty. |
| 2026-08-18 | Felipe: **2 clients in a call = always 1:1**. 3+ elects hub. Third joiner triggers migration. |
| 2026-08-18 | Felipe: **explicit UI icon on who is the hub** (3+ only; gone on 1:1). |
| 2026-08-18 | Felipe: **leaving (storage) requires handing off the shard** (definition 20). |
| 2026-08-18 | Felipe locked **MVP stack** (definition 21): Rust core, Tauri 2 + React, SQLite, OpenMLS, str0m, coturn. |
| 2026-08-18 | Felipe: **storage is not fixed** (definition 22, first version): owner / first device is not the server. |
| 2026-08-18 | Felipe redefined 22: **every client is a history node**. Phone joins erasure with smaller quota. |
| 2026-08-22 | Felipe: **no platform middlebox** to paper over P2P (definition 23). Invite only for trusted people. |
| 2026-09-18 | Felipe: MQTT relay is **the community’s** (definition 24). Hub in creator’s app; invite carries the address. No HiveMQ/EMQX/Mosquitto. |
| 2026-09-18 | Notebook translated to English. Documented alpha vs target: HUB:N (prefer non-owner) ships; MLS/erasure/full SFU do not. |

Next notes: what Felipe marks as “this is how it is” moves into **Definitions**. Questions are not logged unless he asks or closes a decision.
