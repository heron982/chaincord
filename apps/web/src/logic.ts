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

export function remoteScreenVisible(media: {
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
  if (!webkit) configs.push({ iceServers: servers, bundlePolicy: "max-bundle" });
  if (safe.length) {
    configs.push({ iceServers: safe, bundlePolicy: "max-bundle" });
    configs.push({ iceServers: safe });
  }
  if (stun.length) configs.push({ iceServers: stun });
  configs.push({});
  return configs;
}

export function createRtcPeerConnection(
  Ctor: (new (config?: RTCConfiguration) => RTCPeerConnection) | undefined,
  configs: RTCConfiguration[],
): { ok: true; pc: RTCPeerConnection } | { ok: false; error: string } {
  if (!Ctor) return { ok: false, error: "RTCPeerConnection ausente no webview" };
  let last = "falhou ao abrir o enlace";
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

export function callTileGridCols(count: number): number {
  if (count <= 1) return 1;
  if (count <= 4) return 2;
  if (count <= 9) return 3;
  return 4;
}

export type Presence = "online" | "away" | "busy" | "offline";

export const PRESENCE_GROUPS: Array<{ id: Presence; label: string }> = [
  { id: "online", label: "Online" },
  { id: "away", label: "Ausente" },
  { id: "busy", label: "Ocupado" },
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
  if (inVoice && (status === "online" || status === "away")) return "Em voz";
  switch (status) {
    case "away":
      return "Ausente";
    case "busy":
      return "Ocupado";
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

export type CallPathMode = "1:1" | "mesh" | "HUB:N";

export function callPathMode(others: number, hasHub = false): CallPathMode {
  if (others <= 1) return "1:1";
  return hasHub ? "HUB:N" : "mesh";
}

export function callPathHint(mode: CallPathMode): string {
  switch (mode) {
    case "1:1":
      return "dois clientes, WebRTC direto";
    case "HUB:N":
      return "grupo via hub eleito";
    default:
      return "grupo em mesh · HUB:N ainda não elege";
  }
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
      return "ICE caiu";
    case "failed":
      return "ICE falhou";
    case "checking":
      return "ICE negociando";
    case "closed":
      return "ICE fechou";
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
      return "conectando";
    case "connected":
      return "enlace ok";
    case "disconnected":
      return "enlace caiu";
    case "failed":
      return "enlace falhou";
    case "closed":
      return "enlace fechou";
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
