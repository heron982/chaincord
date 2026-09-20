import type { CallLink } from "./call";

export type SignalGrade = "good" | "ok" | "bad" | "wait" | "off";

export function gradeLink(link: CallLink | null): SignalGrade {
  if (!link || link.phase === "connecting") return "wait";
  if (link.phase === "failed") return "off";
  const rtt = link.rttMs ?? 0;
  const loss = link.lossPct ?? 0;
  if (link.phase === "unstable" || rtt > 250 || loss >= 8) return "bad";
  if (rtt > 120 || loss >= 3 || (link.peers > 0 && link.live < link.peers)) return "ok";
  return "good";
}

export function seedPercent(sent: number, total: number): number {
  const max = Math.max(total, 1);
  return Math.max(0, Math.min(100, Math.round((sent / max) * 100)));
}

export function seedingCardVisible(seedActive: boolean): boolean {
  return seedActive;
}

export type ArchiveStatusKind = "live" | "pendingK" | "lost" | string;

export function archiveBannerVisible(status: ArchiveStatusKind | undefined): boolean {
  return status === "pendingK" || status === "lost";
}

export function archiveStatusTitle(status: ArchiveStatusKind | undefined): string {
  switch (status) {
    case "pendingK":
      return "Archive pending";
    case "lost":
      return "Archive unavailable";
    default:
      return "";
  }
}

export function archiveStatusBody(status: ArchiveStatusKind | undefined): string {
  switch (status) {
    case "pendingK":
      return "History shards are incomplete or waiting for members to come online. Chat still works.";
    case "lost":
      return "Not enough history shards are known. This part of the past may not come back.";
    default:
      return "";
  }
}

export function isChatLine(kind: string): boolean {
  return kind === "chat";
}

export function trackLooksLive(track: {
  readyState: string;
  enabled: boolean;
  muted: boolean;
} | null | undefined): boolean {
  return Boolean(track && track.readyState === "live" && track.enabled && !track.muted);
}

export function remoteVideoVisible(media: {
  frames?: boolean;
  stream?: {
    getVideoTracks(): Array<{ readyState: string; enabled: boolean; muted: boolean }>;
  } | null;
} | null | undefined): boolean {
  if (!media) return false;
  if (media.frames) return true;
  return Boolean(
    media.stream
      ?.getVideoTracks()
      .some((track) => trackLooksLive(track)),
  );
}

export function remoteScreenVisible(media: {
  frames?: boolean;
  stream?: {
    getVideoTracks(): Array<{ readyState: string; enabled: boolean; muted: boolean }>;
  } | null;
} | null | undefined): boolean {
  return remoteScreenPresent(media);
}

export function remoteScreenPresent(media: {
  frames?: boolean;
  stream?: {
    getVideoTracks(): Array<{ readyState: string; enabled: boolean; muted: boolean }>;
  } | null;
} | null | undefined): boolean {
  if (!media) return false;
  if (media.frames) return true;
  return Boolean(media.stream?.getVideoTracks().some((track) => trackLooksLive(track)));
}

export function callTrackGone(track: {
  readyState: string;
  enabled: boolean;
} | null | undefined): boolean {
  return !track || track.readyState === "ended" || !track.enabled;
}

export function remoteMediaLive(media: {
  frames?: boolean;
  stream?: {
    getTracks(): Array<{ readyState: string; enabled: boolean; muted: boolean }>;
  } | null;
} | null | undefined): boolean {
  if (!media) return false;
  if (media.frames) return true;
  return Boolean(media.stream?.getTracks().some((track) => trackLooksLive(track)));
}

export function isWebKitEngine(ua = typeof navigator === "undefined" ? "" : navigator.userAgent): boolean {
  return /AppleWebKit/i.test(ua) && !/Chrome|Chromium|Edg\//i.test(ua);
}

export function webkitSafeIceUrl(url: string): boolean {
  if (!url.startsWith("stun:") && !url.startsWith("turn:")) return false;
  if (url.startsWith("turns:")) return false;
  return !url.includes("?");
}

export function rtcIceServersFor(servers: RTCIceServer[], webkit: boolean): RTCIceServer[] {
  if (!webkit) return servers;
  const out: RTCIceServer[] = [];
  for (const server of servers) {
    const urls = (Array.isArray(server.urls) ? server.urls : [server.urls]).filter(webkitSafeIceUrl);
    if (!urls.length) continue;
    out.push({
      ...server,
      urls: urls.length === 1 ? urls[0] : urls,
    });
  }
  return out;
}

export function rtcPeerConfigs(servers: RTCIceServer[], webkit: boolean): RTCConfiguration[] {
  const safe = rtcIceServersFor(servers, webkit);
  const stun = safe.filter((server) => {
    const urls = Array.isArray(server.urls) ? server.urls : [server.urls];
    return urls.every((url) => url.startsWith("stun:"));
  });
  const configs: RTCConfiguration[] = [];
  // One full config first: ICE itself prefers host/srflx (direct) over relay.
  if (!webkit) {
    configs.push({
      iceServers: servers,
      bundlePolicy: "max-bundle",
      iceCandidatePoolSize: 4,
    });
  }
  if (safe.length) {
    configs.push({ iceServers: safe, bundlePolicy: "max-bundle", iceCandidatePoolSize: 4 });
    configs.push({ iceServers: safe });
  }
  if (stun.length) configs.push({ iceServers: stun });
  configs.push({});
  return configs;
}

export function preferH264Codecs<T extends { mimeType: string }>(codecs: T[]): T[] {
  const h264 = codecs.filter((codec) => /h264/i.test(codec.mimeType));
  if (!h264.length) return codecs;
  const rest = codecs.filter((codec) => !/h264/i.test(codec.mimeType));
  return [...h264, ...rest];
}

export function createRtcPeerConnection(
  Ctor: (new (config?: RTCConfiguration) => RTCPeerConnection) | undefined,
  configs: RTCConfiguration[],
): { ok: true; pc: RTCPeerConnection } | { ok: false; error: string } {
  if (!Ctor) return { ok: false, error: "RTCPeerConnection missing from webview" };
  let last = "failed to open link";
  for (const config of configs) {
    try {
      return { ok: true, pc: new Ctor(config) };
    } catch (err) {
      last = err instanceof Error ? err.message : String(err);
    }
  }
  return { ok: false, error: last };
}

export function fmtBytes(n: number): string {
  if (n < 1024) return `${Math.max(0, Math.round(n))} B`;
  if (n < 1024 * 1024) {
    const kb = n / 1024;
    return `${kb < 10 ? kb.toFixed(1) : Math.round(kb)} KB`;
  }
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export const AUDIO_PREF = {
  input: "chaincord.audio.input",
  output: "chaincord.audio.output",
} as const;

export type AudioDeviceHint = {
  deviceId: string;
  kind: string;
  label: string;
};

export function readAudioPref(
  kind: "input" | "output",
  storage: { getItem(key: string): string | null } | null,
): string {
  try {
    return storage?.getItem(AUDIO_PREF[kind])?.trim() ?? "";
  } catch {
    return "";
  }
}

export function writeAudioPref(
  kind: "input" | "output",
  value: string,
  storage: { setItem(key: string, value: string): void; removeItem(key: string): void } | null,
): void {
  try {
    if (!storage) return;
    if (value) storage.setItem(AUDIO_PREF[kind], value);
    else storage.removeItem(AUDIO_PREF[kind]);
  } catch {
    /* private mode */
  }
}

export function audioDevicesOfKind(
  devices: AudioDeviceHint[],
  kind: "audioinput" | "audiooutput",
): AudioDeviceHint[] {
  return devices.filter((device) => device.kind === kind && device.deviceId);
}

export function audioDeviceLabel(
  device: AudioDeviceHint,
  index: number,
  kind: "input" | "output",
): string {
  const name = device.label.trim();
  if (name) return name;
  return kind === "input" ? `Microphone ${index + 1}` : `Speaker ${index + 1}`;
}

export function resolveAudioDeviceId(devices: AudioDeviceHint[], preferred: string): string {
  if (!preferred) return "";
  return devices.some((device) => device.deviceId === preferred) ? preferred : "";
}

export function audioInputConstraint(deviceId: string): {
  echoCancellation: boolean;
  noiseSuppression: boolean;
  autoGainControl: boolean;
  deviceId?: { exact: string };
} {
  const constraint = {
    echoCancellation: true,
    noiseSuppression: true,
    autoGainControl: true,
  };
  if (!deviceId) return constraint;
  return { ...constraint, deviceId: { exact: deviceId } };
}

export function audioSinkSupported(proto: { setSinkId?: unknown } | null = null): boolean {
  const fromProto =
    proto ??
    (typeof HTMLMediaElement === "undefined" ? null : HTMLMediaElement.prototype);
  return typeof fromProto?.setSinkId === "function";
}

export function userTileId(pk: string): string {
  return `user:${pk}`;
}

export function screenTileId(pk: string): string {
  return `screen:${pk}`;
}

export function toggleCallFocus(current: string | null, clicked: string): string | null {
  return current === clicked ? null : clicked;
}

export function keepCallFocus(focus: string | null, ids: string[]): string | null {
  if (!focus || !ids.includes(focus)) return null;
  return focus;
}

export function pickCallFocus(
  current: string | null,
  pinned: boolean,
  ids: string[],
  autoIds: string[],
): string | null {
  if (pinned) return keepCallFocus(current, ids);
  const auto = autoIds.find((id) => ids.includes(id));
  if (auto) return auto;
  return keepCallFocus(current, ids);
}

export function autoCallFocusIds(
  tiles: Array<{ id: string; screen: boolean; live: boolean }>,
): string[] {
  const live = tiles.filter((tile) => tile.live);
  const screens = live.filter((tile) => tile.screen).map((tile) => tile.id);
  if (screens.length) return screens;
  if (live.length === 1) return [live[0].id];
  return [];
}

export function wantsCallMedia(hidden: Iterable<string>, tileId: string): boolean {
  return ![...hidden].includes(tileId);
}

export function voiceRoomSharing(
  people: string[],
  profiles: Record<string, { sharingScreen?: boolean } | undefined>,
  selfPk: string,
  selfSharing: boolean,
): boolean {
  return people.some((pk) => (pk === selfPk ? selfSharing : Boolean(profiles[pk]?.sharingScreen)));
}

export function callTileGridCols(count: number): number {
  if (count <= 1) return 1;
  if (count <= 4) return 2;
  if (count <= 9) return 3;
  return 4;
}

export type Presence = "online" | "away" | "busy" | "offline";

export const PRESENCE_GROUPS: Array<{ id: Presence; label: string }> = [
  { id: "online", label: "Online" },
  { id: "away", label: "Away" },
  { id: "busy", label: "Busy" },
  { id: "offline", label: "Offline" },
];

export function normalizePresence(raw: string | undefined): Presence {
  switch (raw) {
    case "away":
    case "ausente":
    case "idle":
      return "away";
    case "busy":
    case "ocupado":
    case "dnd":
      return "busy";
    case "offline":
      return "offline";
    default:
      return "online";
  }
}

export function effectivePresence(
  chosen: Exclude<Presence, "offline">,
  idle: boolean,
): Exclude<Presence, "offline"> {
  if (chosen === "busy") return "busy";
  if (chosen === "away" || idle) return "away";
  return "online";
}

export function presenceLabel(status: Presence, inVoice = false): string {
  if (inVoice && (status === "online" || status === "away")) return "In voice";
  switch (status) {
    case "away":
      return "Away";
    case "busy":
      return "Busy";
    case "offline":
      return "Offline";
    default:
      return "Online";
  }
}

export function rtcPolite(me: string, peer: string): boolean {
  return me > peer;
}

export function shouldHealSend(recvd: number, sent: number, wantSend: boolean): boolean {
  return wantSend && recvd > 1500 && sent < 800;
}

export function canPublishLocalSdp(
  sent: "offer" | "answer",
  descType: string | undefined,
): boolean {
  return descType === sent;
}

export function shouldResendCallAnswer(
  remoteSdp: string | undefined,
  incomingSdp: string,
  localType: string | undefined,
): boolean {
  return Boolean(remoteSdp && incomingSdp && remoteSdp === incomingSdp && localType === "answer");
}

/** Media kinds from SDP `m=` lines, in order (audio/video/…). */
export function sdpMediaLineKinds(sdp: string): string[] {
  const kinds: string[] = [];
  for (const line of sdp.split(/\r?\n/)) {
    if (!line.startsWith("m=")) continue;
    const kind = line.slice(2).trim().split(/\s+/)[0];
    if (kind) kinds.push(kind);
  }
  return kinds;
}

/** True when answer m-line count/order matches the local offer (rejects stale answers). */
export function answerMatchesLocalOffer(
  offerSdp: string | undefined,
  answerSdp: string | undefined,
): boolean {
  if (!offerSdp || !answerSdp) return false;
  const offer = sdpMediaLineKinds(offerSdp);
  const answer = sdpMediaLineKinds(answerSdp);
  if (!offer.length || offer.length !== answer.length) return false;
  return offer.every((kind, i) => kind === answer[i]);
}

export type CallPathMode = "1:1" | "mesh" | "HUB:N";

export function callPathMode(others: number, hasHub = false): CallPathMode {
  if (others <= 1) return "1:1";
  return hasHub ? "HUB:N" : "mesh";
}

export function callPathHint(mode: CallPathMode): string {
  switch (mode) {
    case "1:1":
      return "two clients, direct WebRTC";
    case "HUB:N":
      return "group via elected hub";
    default:
      return "group mesh · HUB:N not elected yet";
  }
}

/** Call hub for 3+ seats. Prefer someone other than the community owner so the
 * creator's laptop is not always the SFU; then pick a stable sorted key. */
export function electCallHub(roster: string[], ownerPk = ""): string | null {
  const people = [...new Set(roster.filter(Boolean))].sort();
  if (people.length < 3) return null;
  const pool = ownerPk ? people.filter((pk) => pk !== ownerPk) : people;
  const pick = (pool.length ? pool : people).slice().sort();
  return pick[0] ?? null;
}

export function pickMyCall(
  voice: Record<string, string[]>,
  me: string,
  preferred = "",
): string | null {
  if (!me) return null;
  const rooms = Object.entries(voice)
    .filter(([, people]) => people.includes(me))
    .map(([room]) => room)
    .sort();
  if (!rooms.length) return null;
  if (preferred && rooms.includes(preferred)) return preferred;
  return rooms[0] ?? null;
}

export function holdCallHub(
  current: string | null,
  elected: string | null,
  rosterSize: number,
  holdUntil: number,
  now: number,
  holdMs = 12000,
): { hub: string | null; holdUntil: number } {
  if (elected && rosterSize >= 3) return { hub: elected, holdUntil: 0 };
  if (rosterSize >= 3 && current) return { hub: current, holdUntil: 0 };
  // Ghost third peer blips 3→2; keep hub briefly so we don't remount mid-ICE.
  if (current && rosterSize === 2) {
    const until = holdUntil > 0 ? holdUntil : now + holdMs;
    if (now < until) return { hub: current, holdUntil: until };
  }
  return { hub: null, holdUntil: 0 };
}

export function ignoreStaleCallBye(byeTs: number | undefined, lastSigTs: number): boolean {
  return Boolean(byeTs && lastSigTs && byeTs < lastSigTs);
}

export function ignoreStaleCallSess(byeSess: string | undefined, liveSess: string | undefined): boolean {
  return Boolean(liveSess && byeSess && byeSess !== liveSess);
}

export function callWantedMLines(extraPeers: number): number {
  return 3 + Math.max(0, extraPeers) * 3;
}

export function callWantedPeers(me: string, roster: string[], hub: string | null): string[] {
  const others = [...new Set(roster.filter((pk) => pk && pk !== me))];
  const people = [...new Set([...roster, me].filter(Boolean))];
  if (!hub || people.length < 3) return others;
  if (me === hub) return others;
  return others.includes(hub) ? [hub] : others;
}

export function callForwardPeers(hub: string, remote: string, roster: string[]): string[] {
  return [...new Set(roster.filter((pk) => pk && pk !== hub && pk !== remote))].sort();
}

export function callSlotPeer(index: number, remote: string, extras: string[]): string {
  if (index < 3) return remote;
  return extras[Math.floor((index - 3) / 3)] ?? remote;
}

export function callSlotScreen(kind: string, index: number): boolean {
  if (kind !== "video" || index < 0) return false;
  return index % 3 === 2;
}

export function callPrimaryCount(extraPeerCount: number, liveCount: number): number {
  if (extraPeerCount > 0) return Math.min(3, liveCount);
  return liveCount;
}

export function formatCallClock(ts: number): string {
  const d = new Date(ts);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

export function iceTraceText(state: string): string | null {
  switch (state) {
    case "connected":
    case "completed":
      return "ICE ok";
    case "disconnected":
      return "ICE disconnected";
    case "failed":
      return "ICE failed";
    case "checking":
      return "ICE negotiating";
    case "closed":
      return "ICE closed";
    default:
      return null;
  }
}

export function iceTraceLevel(state: string): "info" | "ok" | "warn" | "err" | null {
  switch (state) {
    case "connected":
    case "completed":
      return "ok";
    case "checking":
      return "info";
    case "disconnected":
      return "warn";
    case "failed":
    case "closed":
      return "err";
    default:
      return null;
  }
}

export function pcTraceText(state: string): string | null {
  switch (state) {
    case "connecting":
      return "connecting";
    case "connected":
      return "link ok";
    case "disconnected":
      return "link disconnected";
    case "failed":
      return "link failed";
    case "closed":
      return "link closed";
    default:
      return null;
  }
}

export type CallXcvrHint = {
  mid: string | null;
  kind?: string;
};

function byCallMid(
  a: CallXcvrHint & { index: number },
  b: CallXcvrHint & { index: number },
): number {
  if (a.mid == null && b.mid == null) return a.index - b.index;
  if (a.mid == null) return 1;
  if (b.mid == null) return -1;
  return String(a.mid).localeCompare(String(b.mid), undefined, { numeric: true });
}

export function pickCallTransceivers(list: CallXcvrHint[]): {
  audio: number;
  cam: number | null;
  screen: number | null;
} {
  const indexed = list.map((item, index) => ({ ...item, index }));
  const audios = indexed.filter((item) => item.kind === "audio").sort(byCallMid);
  const videos = indexed.filter((item) => item.kind === "video").sort(byCallMid);
  const unknown = indexed
    .filter((item) => item.kind !== "audio" && item.kind !== "video")
    .sort(byCallMid);
  const audio = audios[0]?.index ?? unknown[0]?.index ?? 0;
  const videoSlots = (videos.length ? videos : unknown).filter((item) => item.index !== audio);
  return {
    audio,
    cam: videoSlots[0]?.index ?? null,
    screen: videoSlots[1]?.index ?? null,
  };
}

export function callTrackIsScreen(
  kind: string,
  transceiverIndex: number,
  list: CallXcvrHint[],
  mappedScreenIndex: number | null = null,
): boolean {
  if (kind !== "video" || transceiverIndex < 0) return false;
  if (mappedScreenIndex != null) return transceiverIndex === mappedScreenIndex;
  return pickCallTransceivers(list).screen === transceiverIndex;
}

export function groupByPresence<T extends { status: Presence }>(
  members: T[],
): Record<Presence, T[]> {
  const groups: Record<Presence, T[]> = {
    online: [],
    away: [],
    busy: [],
    offline: [],
  };
  for (const member of members) {
    groups[member.status].push(member);
  }
  return groups;
}

export function extractInvite(raw: string): string {
  const compact = raw.replace(/\s+/g, "");
  const lower = compact.toLowerCase();
  const cc = lower.indexOf("cc/");
  const named = lower.indexOf("chaincord:");
  const start = [cc, named].filter((i) => i >= 0).sort((a, b) => a - b)[0];
  return start == null ? compact : compact.slice(start);
}

export function looksLikeInvite(raw: string): boolean {
  const extracted = extractInvite(raw);
  if (/^(cc\/|chaincord:)/i.test(extracted) && extracted.length >= 20) {
    return true;
  }
  return /^[A-Za-z0-9+/=_-]{40,}$/.test(extracted);
}

export type ReachPathKind = "lan" | "mesh" | "public" | "local";

export type ReachPath = {
  url: string;
  host: string;
  kind: ReachPathKind;
  label: string;
};

function parseWsHost(url: string): string | null {
  const m = url.trim().match(/^wss?:\/\/(\[[^\]]+\]|[^/:]+)(?::\d+)?/i);
  if (!m) return null;
  return m[1].replace(/^\[|\]$/g, "");
}

function isIpv4Private(host: string): boolean {
  const m = host.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/);
  if (!m) return false;
  const a = Number(m[1]);
  const b = Number(m[2]);
  if (a === 10) return true;
  if (a === 192 && b === 168) return true;
  if (a === 172 && b >= 16 && b <= 31) return true;
  return false;
}

function meshVpnLabel(host: string): string | null {
  const m = host.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/);
  if (!m) return null;
  const a = Number(m[1]);
  const b = Number(m[2]);
  if (a === 25) return "Hamachi";
  if (a === 26) return "Radmin VPN";
  if (a === 100 && b >= 64 && b <= 127) return "Tailscale";
  return null;
}

export function classifyListenUrl(url: string): ReachPath | null {
  const host = parseWsHost(url);
  if (!host) return null;
  if (host === "127.0.0.1" || host === "localhost" || host === "::1") {
    return { url, host, kind: "local", label: "This PC only" };
  }
  const mesh = meshVpnLabel(host);
  if (mesh) return { url, host, kind: "mesh", label: mesh };
  if (isIpv4Private(host)) return { url, host, kind: "lan", label: "Local network" };
  return { url, host, kind: "public", label: "Public IP" };
}

export function reachabilityFromListenUrls(urls: string[]): {
  paths: ReachPath[];
  best: ReachPath | null;
  hasMesh: boolean;
  hasLan: boolean;
  summary: string;
} {
  const paths = urls
    .map(classifyListenUrl)
    .filter((p): p is ReachPath => Boolean(p && p.kind !== "local"));
  const hasMesh = paths.some((p) => p.kind === "mesh");
  const hasLan = paths.some((p) => p.kind === "lan");
  const best =
    paths.find((p) => p.kind === "mesh") ||
    paths.find((p) => p.kind === "lan") ||
    paths.find((p) => p.kind === "public") ||
    null;
  let summary = "No shared path detected yet.";
  if (hasMesh && best) {
    summary = `${best.label} detected (${best.host}). Friends on the same VPN can join.`;
  } else if (hasLan && best) {
    summary = `Local network detected (${best.host}). Same Wi‑Fi works; for other networks use a mesh VPN first.`;
  } else if (best?.kind === "public") {
    summary = `No LAN or mesh VPN path yet. Join the same mesh VPN (Hamachi, Radmin, Tailscale…), then copy a fresh invite.`;
  }
  return { paths, best, hasMesh, hasLan, summary };
}

export function inviteShareText(communityName: string, code: string): string {
  const name = communityName.trim() || "community";
  return [
    `Chaincord invite — ${name}`,
    "Open Chaincord → Join with invite and paste this:",
    code.trim(),
    "",
    "If you're not on the same Wi‑Fi: connect to the same mesh VPN (Hamachi, Radmin, Tailscale, …) first, then join.",
  ].join("\n");
}
