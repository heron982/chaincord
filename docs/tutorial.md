# How to use Chaincord

Chaincord is a small desktop chat for people you trust. There is **no Chaincord cloud server**. Whoever **creates** the community hosts it in their app. Friends reach that machine only on a **shared network**:

- **Same Wi‑Fi** (same house / LAN), or
- **Same mesh VPN** (Hamachi, Radmin VPN, Tailscale, …) — a simulated LAN over the internet

Invites never use a public IP or port forward. If you are not on the same Wi‑Fi or the same VPN, join will not work.

This is **alpha**. Anyone with the invite can read the channel. See [`SECURITY.md`](../SECURITY.md).

---

## 1. Install

1. Download the latest Windows installer from [Releases](https://github.com/heron982/chaincord/releases).
2. Run the setup (`Chaincord_*_x64-setup.exe`).
3. Open **Chaincord**.

Keep the app open while you chat. If the creator closes it, live chat for that community stops until they open it again.

---

## 2. Create a community (host)

1. Open Chaincord → **Create community**.
2. Pick a name → create.
3. Copy the invite (**Copy invite** or **Show invite**).

You are the host. Your PC must stay online for friends to chat.

Share a **fresh** invite after your network is ready (Wi‑Fi connected, or VPN connected). Old invites may still point at the wrong address.

---

## 3. Same Wi‑Fi

Use this when everyone is on the **same home/office Wi‑Fi**.

1. Creator creates the community and copies the invite.
2. Friends open Chaincord → **Join with invite** → paste the code.
3. Chat in `#general`. Use a voice room for calls.

No VPN needed. The invite already carries the LAN address.

**Tips**

- Creator and joiners must be on the same SSID (guest Wi‑Fi often blocks device-to-device traffic).
- Windows Firewall may ask once — allow Chaincord on private networks.
- Two windows on one PC: open the app twice (second instance uses another port).

---

## 4. Different houses — mesh VPN

Use this when friends are on **different networks**.

### Pick a VPN everyone installs

Any mesh VPN that gives you a virtual LAN works. Common choices:

| App | Typical virtual IPs |
|-----|---------------------|
| [Hamachi](https://www.vpn.net/) | `25.x.x.x` |
| [Radmin VPN](https://www.radmin-vpn.com/) | `26.x.x.x` |
| [Tailscale](https://tailscale.com/) | `100.x.x.x` |

Chaincord does not ship a VPN. You install one yourselves and all join the **same** network/room.

### Steps

1. Everyone installs the same mesh VPN and joins the **same** network (same Hamachi network, same Radmin network, same Tailscale tailnet, …).
2. Confirm you can see each other as connected in the VPN app.
3. **Creator** opens Chaincord (VPN already connected).
4. Create the community (or open an existing one).
5. Copy a **fresh** invite — it should prefer the VPN address when one is available.
6. Friends paste the invite in Chaincord → Join.
7. Keep the creator’s Chaincord open while you chat.

**Checklist if join fails**

- [ ] Same VPN network name / room
- [ ] VPN shows the friend as online
- [ ] Creator’s Chaincord is open
- [ ] Invite was copied **after** the VPN was connected
- [ ] Firewall allows Chaincord (and the VPN)

---

## 5. Chat, voice, leave

- **Text:** pick a text channel (e.g. `#general`) and send messages.
- **Voice / video:** join a call room. 1:1 is direct; 3+ uses an elected media hub among you.
- **Leave community:** if others are online, prefer **Transfer and leave** so your history shards move to another peer. **Leave anyway** skips that and may risk the archive.

---

## 6. Mental model (short)

```text
Same Wi‑Fi          →  invite with LAN IP     →  chat
Same mesh VPN       →  invite with VPN IP     →  chat
Different networks  →  no shared LAN/VPN      →  will not connect
  without VPN
```

Creator’s PC = temporary hub for that community. Members share encrypted history pieces so the archive is not only on one disk; see the architecture notes if you want the details.

---

## More

- Security / invite trust: [`SECURITY.md`](../SECURITY.md)
- Design notes: [`docs/architecture.md`](architecture.md)
- Build from source: [`CONTRIBUTING.md`](../CONTRIBUTING.md)
