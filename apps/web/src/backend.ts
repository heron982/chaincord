import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type UiState = {
  publicKey: string;
  displayName: string;
  avatar: string;
  communityName: string;
  communityId: string;
  invite: string;
  listenUrl: string;
  peers: string[];
  textChannels: string[];
  callRooms: string[];
  voice: Record<string, string[]>;
  profiles: Record<string, PeerProfile>;
  archiveBytes: number;
  archiveMessages: number;
  seedSent: number;
  seedTotal: number;
  seedActive: boolean;
  seeding: string[];
};

export type PeerProfile = {
  displayName: string;
  avatar: string;
  muted: boolean;
  deafened: boolean;
  status?: string;
};

export type UiMessage = {
  sender: string;
  text: string;
  ts: number;
  channel: string;
  self: boolean;
};

export async function backendGetState(): Promise<UiState> {
  return invoke<UiState>("get_state");
}

export async function backendCreate(name: string): Promise<void> {
  await invoke("create_community", { name });
}

export async function backendJoin(invite: string): Promise<void> {
  await invoke("join_community", { invite });
}

export async function backendChat(text: string, channel: string): Promise<void> {
  await invoke("send_chat", { text, channel });
}

export async function backendLeave(): Promise<void> {
  await invoke("leave_community");
}

export async function backendAddRoom(kind: "text" | "call", name: string): Promise<void> {
  await invoke("add_room", { kind, name });
}

export async function backendJoinCall(room: string): Promise<void> {
  await invoke("join_call", { room });
}

export async function backendLeaveCall(): Promise<void> {
  await invoke("leave_call");
}

export async function backendSaveProfile(displayName: string, avatar: string): Promise<void> {
  await invoke("save_profile", { displayName, avatar });
}

export async function backendPresence(
  muted: boolean,
  deafened: boolean,
  status?: string,
): Promise<void> {
  await invoke("publish_presence", { muted, deafened, status });
}

export async function backendHistory(): Promise<UiMessage[]> {
  return invoke<UiMessage[]>("get_history");
}

export type RtcFrame = {
  type: "rtc";
  room: string;
  from: string;
  to: string;
  kind: "offer" | "answer" | "ice" | "bye";
  sdp?: string;
  candidate?: RTCIceCandidateInit | null;
};

export async function backendSendRtc(frame: RtcFrame): Promise<void> {
  await invoke("send_signal", { frame });
}

export function rtcPeerConnectionMissing(): boolean {
  return typeof RTCPeerConnection === "undefined";
}

export async function backendRtcStart(room: string, me: string): Promise<void> {
  await invoke("rtc_start", { room, me });
}

export async function backendRtcStop(): Promise<void> {
  await invoke("rtc_stop");
}

export async function backendRtcSync(peers: string[]): Promise<void> {
  await invoke("rtc_sync", { peers });
}

export async function backendRtcSignal(frame: RtcFrame): Promise<void> {
  await invoke("rtc_signal", { frame });
}

export async function backendRtcPushFrame(screen: boolean, jpeg: string): Promise<void> {
  await invoke("rtc_push_frame", { screen, jpeg });
}

export function backendSubscribe(handlers: {
  onState: (s: UiState) => void;
  onMessage: (m: UiMessage) => void;
  onInfo: (text: string) => void;
  onError: (text: string) => void;
  onRtc?: (frame: RtcFrame) => void;
}): () => void {
  const unsubs: Array<() => void> = [];
  void (async () => {
    unsubs.push(await listen<UiState>("ui-state", (e) => handlers.onState(e.payload)));
    unsubs.push(await listen<UiMessage>("ui-message", (e) => handlers.onMessage(e.payload)));
    unsubs.push(await listen<string>("ui-info", (e) => handlers.onInfo(e.payload)));
    unsubs.push(await listen<string>("ui-error", (e) => handlers.onError(e.payload)));
    unsubs.push(await listen<RtcFrame>("ui-rtc", (e) => handlers.onRtc?.(e.payload)));
    handlers.onState(await backendGetState());
  })();
  return () => unsubs.forEach((u) => u());
}
