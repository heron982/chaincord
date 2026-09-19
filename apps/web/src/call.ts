import { listen } from "@tauri-apps/api/event";
import {
  backendAppendLog,
  backendRtcPushFrame,
  backendRtcShareScreen,
  backendRtcSignal,
  backendRtcStart,
  backendRtcStop,
  backendRtcSync,
  rtcPeerConnectionMissing,
} from "./backend";
import {
  callPathMode,
  callTrackIsScreen,
  callForwardPeers,
  callPrimaryCount,
  callSlotPeer,
  callSlotScreen,
  callWantedMLines,
  callWantedPeers,
  answerMatchesLocalOffer,
  canPublishLocalSdp,
  shouldResendCallAnswer,
  createRtcPeerConnection,
  holdCallHub,
  iceTraceLevel,
  iceTraceText,
  ignoreStaleCallBye,
  ignoreStaleCallSess,
  isWebKitEngine,
  pickCallTransceivers,
  rtcPeerConfigs,
  rtcPolite,
  pcTraceText,
  shouldHealSend,
  trackLooksLive,
  type CallPathMode,
} from "./logic";

export type RtcFrame = {
  type: "rtc";
  room: string;
  from: string;
  to: string;
  kind: "offer" | "answer" | "ice" | "bye";
  sdp?: string;
  candidate?: RTCIceCandidateInit | null;
  ts?: number;
  sess?: string;
};

export type RemoteMedia = {
  peer: string;
  stream: MediaStream;
  screen: boolean;
  frames?: boolean;
};

export type CallLink = {
  phase: "connecting" | "connected" | "unstable" | "failed";
  rttMs: number | null;
  lossPct: number | null;
  jitterMs: number | null;
  peers: number;
  live: number;
  mode: CallPathMode;
};

export type CallTrace = {
  id: number;
  t: number;
  mode: CallPathMode;
  peer: string | null;
  event: string;
  level: "info" | "ok" | "warn" | "err";
};

const nativeFrameUrls = new Map<string, string>();

export function lastNativeFrame(id: string): string {
  return nativeFrameUrls.get(id) ?? "";
}

function jpegToUrl(jpeg: string): string {
  const bin = atob(jpeg);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i += 1) bytes[i] = bin.charCodeAt(i);
  return URL.createObjectURL(new Blob([bytes], { type: "image/jpeg" }));
}

const iceServers: RTCIceServer[] = [
  { urls: ["stun:stun.l.google.com:19302", "stun:stun1.l.google.com:19302"] },
  { urls: "stun:stun.cloudflare.com:3478" },
  {
    urls: [
      "turn:openrelay.metered.ca:80",
      "turn:openrelay.metered.ca:443",
      "turn:openrelay.metered.ca:80?transport=tcp",
      "turn:openrelay.metered.ca:443?transport=tcp",
      "turns:openrelay.metered.ca:443?transport=tcp",
    ],
    username: "openrelayproject",
    credential: "openrelayproject",
  },
];

function sleep(ms: number) {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, ms);
  });
}

export class CallNet {
  private pcs = new Map<string, RTCPeerConnection>();
  private camSenders = new Map<string, RTCRtpSender>();
  private screenSenders = new Map<string, RTCRtpSender>();
  private audioSenders = new Map<string, RTCRtpSender>();
  private screenXcvr = new Map<string, RTCRtpTransceiver>();
  private remoteCam = new Map<string, MediaStream>();
  private remoteScr = new Map<string, MediaStream>();
  private pendingIce = new Map<string, RTCIceCandidateInit[]>();
  private makingOffer = new Set<string>();
  private ignoreOffer = new Set<string>();
  private jobs = new Map<string, Promise<void>>();
  private cam: MediaStream | null = null;
  private screen: MediaStream | null = null;
  private stopped = false;
  private timer: number | null = null;
  private rtpPrev = { lost: 0, recv: 0 };
  private oneWayTicks = new Map<string, number>();
  private reconnectAt = new Map<string, number>();
  private iceSeen = new Map<string, string>();
  private pcSeen = new Map<string, string>();
  private heard = new Set<string>();
  private pathMode: CallPathMode = "1:1";
  private hubHoldUntil = 0;
  private rtcFailed: string | null = null;
  private traceSeq = 0;
  private native = rtcPeerConnectionMissing();
  private nativeUnsub: Array<() => void> = [];
  private nativeReady: Promise<void> = Promise.resolve();
  private nativePump: number | null = null;
  private nativeGrab = {
    cam: null as HTMLVideoElement | null,
    screen: null as HTMLVideoElement | null,
    canvas: null as HTMLCanvasElement | null,
  };
  private nativeSent = { cam: false, screen: false };
  private nativeSeen = new Set<string>();
  private remoteSinks = new Map<string, HTMLVideoElement>();
  private seenScreen = new Set<string>();
  private roster: string[] = [];
  private hub: string | null = null;
  private offerRetry = new Map<string, number>();
  private peerDropTimer = new Map<string, number>();
  private wantPeers = new Set<string>();
  private iceStuckTimer = new Map<string, number>();
  private peerSigAt = new Map<string, number>();
  private peerSess = new Map<string, string>();
  private readonly sess = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
  private fwdSenders = new Map<string, RTCRtpSender>();
  private inbound = new Map<string, { audio: MediaStreamTrack | null; cam: MediaStreamTrack | null; screen: MediaStreamTrack | null }>();

  constructor(
    private readonly me: string,
    private readonly room: string,
    private readonly send: (frame: RtcFrame) => void,
    private readonly onRemote: (media: RemoteMedia) => void,
    private readonly onGone: (peer: string) => void,
    private readonly onLink: (link: CallLink) => void,
    private readonly onLog: (line: CallTrace) => void,
  ) {
    this.trace("call started", "info");
    if (this.native) {
      this.nativeReady = this.bootNative();
      return;
    }
    this.timer = window.setInterval(() => void this.emitLink(), 1000);
    void this.emitLink();
  }

  private async bootNative() {
    try {
      this.nativeUnsub.push(
        await listen<CallLink>("ui-call-link", (e) => this.onLink(e.payload)),
      );
      this.nativeUnsub.push(
        await listen<{ peer: string; screen: boolean; jpeg: string }>("ui-call-video", (e) => {
          this.ingestNativeFrame(e.payload.peer, e.payload.screen, e.payload.jpeg);
        }),
      );
      await backendRtcStart(this.room, this.me);
      if (this.stopped) {
        await backendRtcStop();
      }
    } catch (err) {
      const why = err instanceof Error ? err.message : String(err);
      this.trace(`native app: ${why}`, "err");
    }
  }

  private currentMode(): CallPathMode {
    const others = Math.max(0, this.roster.length - 1);
    return callPathMode(others, Boolean(this.hub && others >= 2));
  }

  private layoutExtras(remote: string): string[] {
    if (!this.hub || this.roster.length < 3) return [];
    return callForwardPeers(this.hub, remote, this.roster);
  }

  private fwdKey(to: string, from: string, slot: "a" | "c" | "s") {
    return `${to}|${from}|${slot}`;
  }

  private noteMode() {
    const mode = this.currentMode();
    if (mode === this.pathMode) return;
    const prev = this.pathMode;
    this.pathMode = mode;
    this.trace(`mode ${prev} → ${mode}`, "info");
  }

  private trace(event: string, level: CallTrace["level"], peer: string | null = null) {
    this.traceSeq += 1;
    const line: CallTrace = {
      id: this.traceSeq,
      t: Date.now(),
      mode: this.currentMode(),
      peer,
      event,
      level,
    };
    this.onLog(line);
    void backendAppendLog({
      t: line.t,
      mode: line.mode,
      peer: line.peer,
      event: line.event,
      level: line.level,
    }).catch(() => undefined);
  }

  setCamera(stream: MediaStream | null) {
    this.cam = stream;
    if (this.native) {
      this.kickNativePump();
      return;
    }
    void this.pushLocal().then(() => this.kickImpoliteOffers());
  }

  setScreen(stream: MediaStream | null) {
    const track = stream?.getVideoTracks()[0];
    if (track) track.contentHint = "detail";
    this.screen = stream;
    if (this.native) {
      void backendRtcShareScreen(Boolean(stream)).catch((err: unknown) => {
        this.trace(`screen: ${String(err)}`, "err");
      });
      this.kickNativePump();
      return;
    }
    void this.pushLocal().then(() => this.kickImpoliteOffers());
    window.setTimeout(() => void this.pushLocal(), 200);
  }

  async setScreenNative(on: boolean) {
    await backendRtcShareScreen(on);
    this.trace(on ? "native screen on" : "native screen off", on ? "ok" : "info");
  }

  private ingestNativeFrame(peer: string, screen: boolean, jpeg: string) {
    const id = screen ? `screen:${peer}` : `user:${peer}`;
    const prev = nativeFrameUrls.get(id);
    if (prev) URL.revokeObjectURL(prev);
    const url = jpeg ? jpegToUrl(jpeg) : "";
    if (url) nativeFrameUrls.set(id, url);
    else nativeFrameUrls.delete(id);
    window.dispatchEvent(new CustomEvent("chaincord-frame", { detail: { id, url } }));
    const key = `${peer}:${screen ? "s" : "c"}`;
    if (jpeg && !this.nativeSeen.has(key)) {
      this.nativeSeen.add(key);
      this.onRemote({ peer, stream: new MediaStream(), screen, frames: true });
    } else if (!jpeg && this.nativeSeen.has(key)) {
      this.nativeSeen.delete(key);
      this.onRemote({ peer, stream: new MediaStream(), screen, frames: false });
    }
  }

  private kickNativePump() {
    const live = [this.cam, this.screen].some((stream) =>
      Boolean(stream?.getVideoTracks().some((t) => t.readyState === "live")),
    );
    if (!live) {
      void this.pushNativeFrames().finally(() => {
        if (
          ![this.cam, this.screen].some((stream) =>
            Boolean(stream?.getVideoTracks().some((t) => t.readyState === "live")),
          )
        ) {
          this.stopNativePump();
        }
      });
      return;
    }
    if (this.nativePump == null) {
      this.nativePump = window.setInterval(() => void this.pushNativeFrames(), 120);
    }
    void this.pushNativeFrames();
  }

  private stopNativePump() {
    if (this.nativePump != null) {
      window.clearInterval(this.nativePump);
      this.nativePump = null;
    }
    const cam = this.nativeGrab.cam;
    const screen = this.nativeGrab.screen;
    if (cam) {
      cam.srcObject = null;
      cam.remove();
    }
    if (screen) {
      screen.srcObject = null;
      screen.remove();
    }
    this.nativeGrab = { cam: null, screen: null, canvas: null };
    this.nativeSent = { cam: false, screen: false };
  }

  private async pushNativeFrames() {
    if (this.stopped) return;
    await this.grabNative(this.cam, false);
    if (this.screen) await this.grabNative(this.screen, true);
  }

  private async grabNative(stream: MediaStream | null, screen: boolean) {
    const live = stream?.getVideoTracks().some((t) => t.readyState === "live") ?? false;
    const flag = screen ? "screen" : "cam";
    if (!live) {
      if (this.nativeSent[flag]) {
        this.nativeSent[flag] = false;
        await backendRtcPushFrame(screen, "").catch(() => undefined);
        this.ingestNativeFrame(this.me, screen, "");
      }
      return;
    }
    try {
      const jpeg = await this.grabJpeg(stream!, screen);
      if (!jpeg) return;
      this.nativeSent[flag] = true;
      await backendRtcPushFrame(screen, jpeg);
      this.ingestNativeFrame(this.me, screen, jpeg);
    } catch {
      /* webkit grab */
    }
  }

  private async grabJpeg(stream: MediaStream, screen: boolean): Promise<string | null> {
    const key = screen ? "screen" : "cam";
    let video = this.nativeGrab[key];
    if (!video) {
      video = document.createElement("video");
      video.muted = true;
      video.playsInline = true;
      video.autoplay = true;
      video.setAttribute("playsinline", "true");
      video.style.cssText =
        "position:fixed;width:1px;height:1px;opacity:0;pointer-events:none;left:-80px;top:-80px";
      document.body.appendChild(video);
      this.nativeGrab[key] = video;
    }
    if (video.srcObject !== stream) {
      video.srcObject = stream;
      await video.play().catch(() => undefined);
    }
    if (video.readyState < 2 || video.videoWidth < 2 || video.videoHeight < 2) return null;
    const maxW = screen ? 960 : 640;
    const maxH = screen ? 540 : 360;
    let w = video.videoWidth;
    let h = video.videoHeight;
    const scale = Math.min(1, maxW / w, maxH / h);
    w = Math.max(2, Math.floor((w * scale) / 2) * 2);
    h = Math.max(2, Math.floor((h * scale) / 2) * 2);
    let canvas = this.nativeGrab.canvas;
    if (!canvas) {
      canvas = document.createElement("canvas");
      this.nativeGrab.canvas = canvas;
    }
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext("2d");
    if (!ctx) return null;
    ctx.drawImage(video, 0, 0, w, h);
    const blob = await new Promise<Blob | null>((resolve) => {
      canvas.toBlob((b) => resolve(b), "image/jpeg", screen ? 0.42 : 0.48);
    });
    if (!blob) return null;
    const bytes = new Uint8Array(await blob.arrayBuffer());
    let bin = "";
    for (let i = 0; i < bytes.length; i += 0x8000) {
      bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    }
    return btoa(bin);
  }

  sync(peers: string[], hub: string | null = null) {
    if (this.stopped) return;
    const others = peers.filter((p) => p && p !== this.me);
    this.roster = [...new Set([this.me, ...others])];
    const now = Date.now();
    // Elect immediately at 3+; holdCallHub absorbs brief 3→2 roster blips.
    const elected = this.roster.length >= 3 ? hub : null;
    const held = holdCallHub(
      this.hub,
      elected,
      this.roster.length,
      this.hubHoldUntil,
      now,
    );
    this.hubHoldUntil = held.holdUntil;
    const nextHub = held.hub;
    if (this.native) {
      void this.nativeReady
        .then(() => backendRtcSync(others, nextHub))
        .catch((err) => {
          this.trace(`native app: ${String(err)}`, "err");
        });
      this.hub = nextHub;
      this.noteMode();
      return;
    }
    if (this.hub !== nextHub) {
      this.hub = nextHub;
      this.trace(nextHub ? `hub ${nextHub.slice(0, 8)}` : "no hub", "info", nextHub);
    }
    const want = new Set(callWantedPeers(this.me, this.roster, this.hub));
    this.wantPeers = want;
    for (const id of [...this.pcs.keys()]) {
      if (!want.has(id)) this.armPeerDrop(id);
      else this.cancelPeerDrop(id);
    }
    for (const id of want) {
      this.cancelPeerDrop(id);
      if (!this.pcs.has(id)) {
        const polite = rtcPolite(this.me, id);
        try {
          this.open(id);
        } catch (err) {
          const why = err instanceof Error ? err.message : String(err);
          if (this.rtcFailed !== why) {
            this.rtcFailed = why;
            this.trace(`native app: ${why}`, "err");
          }
          continue;
        }
        this.trace("link open", "info", id);
        if (!polite) this.scheduleOffer(id);
      }
    }
    this.noteMode();
    for (const id of want) {
      const pc = this.pcs.get(id);
      if (!pc) continue;
      if (rtcPolite(this.me, id)) {
        this.bindSenders(pc, id);
        this.bindFwd(pc, id);
        continue;
      }
      // Never add m-lines while an offer/answer is in flight — that desyncs SDP and
      // makes the next answer fail with "m-lines order doesn't match".
      if (pc.signalingState !== "stable") continue;
      const before = pc.getTransceivers().filter((t) => t.direction !== "stopped").length;
      this.ensureMLines(pc, id);
      const after = pc.getTransceivers().filter((t) => t.direction !== "stopped").length;
      if (after > before) {
        this.trace("hub layout grew · renegotiating", "info", id);
        void this.enqueue(id, () => this.offerNow(id));
      }
    }
    this.replayInbound();
  }

  async handle(frame: RtcFrame) {
    if (this.stopped) return;
    if (frame.room !== this.room) return;
    if (frame.from === this.me) return;
    if (frame.to !== this.me) return;
    if (
      this.hub &&
      this.roster.length >= 3 &&
      this.me !== this.hub &&
      frame.from !== this.hub
    ) {
      return;
    }
    if (this.native) {
      await this.nativeReady;
      await backendRtcSignal(frame).catch((err) => {
        this.trace(`native app: ${String(err)}`, "err");
      });
      return;
    }
    if (frame.kind === "bye") {
      if (
        ignoreStaleCallSess(frame.sess, this.peerSess.get(frame.from)) ||
        ignoreStaleCallBye(frame.ts, this.peerSigAt.get(frame.from) ?? 0)
      ) {
        this.trace("stale bye", "info", frame.from);
        return;
      }
      // Peer remounted CallNet and spammed bye while still seated — ignore.
      if (this.wantPeers.has(frame.from) || this.roster.includes(frame.from)) {
        this.trace("bye ignored (still in room)", "info", frame.from);
        return;
      }
      this.drop(frame.from, true);
      return;
    }
    if (frame.sess) this.peerSess.set(frame.from, frame.sess);
    if (frame.ts && frame.ts >= (this.peerSigAt.get(frame.from) ?? 0)) {
      this.peerSigAt.set(frame.from, frame.ts);
    }
    await this.enqueue(frame.from, () => this.handleOne(frame));
  }

  stop(quiet = false) {
    if (this.stopped) return;
    this.trace("call ended", "info");
    this.stopped = true;
    for (const peer of [...this.offerRetry.keys()]) this.clearOfferRetry(peer);
    for (const peer of [...this.peerDropTimer.keys()]) this.cancelPeerDrop(peer);
    for (const peer of [...this.iceStuckTimer.keys()]) this.clearIceStuck(peer);
    if (this.native) {
      this.stopNativePump();
      for (const unsub of this.nativeUnsub) unsub();
      this.nativeUnsub = [];
      void backendRtcStop();
      return;
    }
    if (this.timer != null) {
      window.clearInterval(this.timer);
      this.timer = null;
    }
    for (const id of [...this.pcs.keys()]) this.drop(id, quiet);
    this.dropAllSinks();
  }

  private enqueue(peer: string, job: () => Promise<void>) {
    const next = (this.jobs.get(peer) ?? Promise.resolve())
      .catch(() => undefined)
      .then(job);
    this.jobs.set(
      peer,
      next.catch(() => undefined),
    );
    return next;
  }

  private async handleOne(frame: RtcFrame) {
    if (this.stopped) return;
    let pc = this.pcs.get(frame.from);
    // Stale answer/ICE after drop+remount must not recreate the PC.
    if (!pc) {
      if (frame.kind !== "offer") return;
      pc = this.open(frame.from);
    }
    const polite = rtcPolite(this.me, frame.from);
    const stillMine = () => !this.stopped && this.pcs.get(frame.from) === pc;
    try {
      if (frame.kind === "offer" && frame.sdp) {
        if (
          shouldResendCallAnswer(
            pc.remoteDescription?.sdp,
            frame.sdp,
            pc.localDescription?.type,
          )
        ) {
          this.emitSig({
            type: "rtc",
            room: this.room,
            from: this.me,
            to: frame.from,
            kind: "answer",
            sdp: pc.localDescription?.sdp,
          });
          this.trace("answer resent", "info", frame.from);
          return;
        }
        const collision = this.makingOffer.has(frame.from) || pc.signalingState !== "stable";
        if (collision) {
          if (!polite) return;
          this.ignoreOffer.add(frame.from);
          if (pc.signalingState !== "stable") {
            await pc.setLocalDescription({ type: "rollback" });
          }
        }
        if (!stillMine()) return;
        await pc.setRemoteDescription({ type: "offer", sdp: frame.sdp });
        this.trace("offer received", "info", frame.from);
        await this.flushIce(frame.from);
        await this.waitForMic(800);
        if (!stillMine()) return;
        this.bindSenders(pc, frame.from);
        this.bindFwd(pc, frame.from);
        this.preferH264IfLinux(pc, frame.sdp);
        await this.pushLocal(frame.from);
        if (!stillMine()) return;
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        await this.pushLocal(frame.from);
        if (!stillMine()) return;
        if (!canPublishLocalSdp("answer", pc.localDescription?.type)) return;
        this.emitSig({
          type: "rtc",
          room: this.room,
          from: this.me,
          to: frame.from,
          kind: "answer",
          sdp: pc.localDescription?.sdp,
        });
        this.trace("answer sent", "info", frame.from);
        this.ignoreOffer.delete(frame.from);
      } else if (frame.kind === "answer" && frame.sdp) {
        if (pc.signalingState !== "have-local-offer") return;
        if (!answerMatchesLocalOffer(pc.localDescription?.sdp, frame.sdp)) {
          this.trace("stale answer (m-lines)", "info", frame.from);
          return;
        }
        await pc.setRemoteDescription({ type: "answer", sdp: frame.sdp });
        if (!stillMine()) return;
        this.trace("answer received", "ok", frame.from);
        this.clearOfferRetry(frame.from);
        await this.flushIce(frame.from);
        this.bindSenders(pc, frame.from);
        this.bindFwd(pc, frame.from);
        await this.pushLocal(frame.from);
      } else if (frame.kind === "ice") {
        if (!pc.remoteDescription) {
          if (frame.candidate) {
            const queued = this.pendingIce.get(frame.from) ?? [];
            queued.push(frame.candidate);
            this.pendingIce.set(frame.from, queued);
          }
          return;
        }
        try {
          await pc.addIceCandidate(frame.candidate ?? null);
        } catch {
          if (!this.ignoreOffer.has(frame.from)) return;
        }
      }
    } catch (err) {
      this.trace(`rtc signal: ${String(err)}`, "err", frame.from);
    }
  }

  private async emitLink() {
    if (this.stopped) return;
    await this.healOneWay();
    this.onLink(await this.snapshot());
  }

  private async healOneWay() {
    for (const [peer, pc] of this.pcs) {
      const ice = pc.iceConnectionState;
      if (ice === "failed") {
        this.maybeReconnect(peer, pc);
        continue;
      }
      if (ice !== "connected" && ice !== "completed") {
        continue;
      }
      let sent = 0;
      let recvd = 0;
      try {
        const stats = await pc.getStats();
        for (const report of stats.values()) {
          if (report.type === "outbound-rtp" && typeof report.bytesSent === "number") {
            sent += report.bytesSent;
          }
          if (report.type === "inbound-rtp" && typeof report.bytesReceived === "number") {
            recvd += report.bytesReceived;
          }
        }
      } catch {
        continue;
      }
      const wantSend = this.wantSend();
      const audioMissing = Boolean(this.localAudio()) && !this.audioSenders.get(peer)?.track;
      if (shouldHealSend(recvd, sent, wantSend) || (wantSend && recvd > 1500 && audioMissing)) {
        const ticks = (this.oneWayTicks.get(peer) ?? 0) + 1;
        this.oneWayTicks.set(peer, ticks);
        if (ticks === 2 || ticks === 4) {
          if (ticks === 2) this.trace("weak send — reattaching mic", "warn", peer);
          await this.pushLocal(peer);
        }
      } else {
        this.oneWayTicks.set(peer, 0);
      }
    }
  }

  private localAudio() {
    return this.cam?.getAudioTracks().find((track) => track.readyState === "live") ?? null;
  }

  private wantSend(): boolean {
    const tracks = [
      this.localAudio(),
      this.cam?.getVideoTracks()[0],
      this.screen?.getVideoTracks()[0],
    ];
    return tracks.some((track) => track && track.readyState === "live" && track.enabled);
  }

  private maybeReconnect(peer: string, pc: RTCPeerConnection) {
    const now = Date.now();
    if (now - (this.reconnectAt.get(peer) ?? 0) < 8000) return;
    this.reconnectAt.set(peer, now);
    if (rtcPolite(this.me, peer)) return;
    this.trace("restarting ICE", "warn", peer);
    try {
      pc.restartIce();
    } catch {
      /* WebView2 */
    }
    void this.enqueue(peer, () => this.offerNow(peer));
  }

  private async snapshot(): Promise<CallLink> {
    const pcs = [...this.pcs.values()];
    if (!pcs.length) {
      return {
        phase: "connected",
        rttMs: null,
        lossPct: null,
        jitterMs: null,
        peers: 0,
        live: 0,
        mode: this.currentMode(),
      };
    }
    let live = 0;
    let failed = 0;
    let connecting = 0;
    const rtts: number[] = [];
    const jitters: number[] = [];
    let lost = 0;
    let recv = 0;
    for (const pc of pcs) {
      // WebView2 sometimes flips connectionState to "failed" while ICE is still
      // connected and media is flowing — trust ICE first for the call grade.
      const ice = pc.iceConnectionState;
      const conn = pc.connectionState;
      if (ice === "connected" || ice === "completed" || conn === "connected") live += 1;
      else if (ice === "failed" || (conn === "failed" && ice !== "checking" && ice !== "connected"))
        failed += 1;
      else connecting += 1;
      try {
        const stats = await pc.getStats();
        for (const report of stats.values()) {
          if (
            report.type === "candidate-pair" &&
            (report.state === "succeeded" || report.nominated) &&
            typeof report.currentRoundTripTime === "number"
          ) {
            rtts.push(report.currentRoundTripTime * 1000);
          }
          if (report.type === "remote-inbound-rtp" && typeof report.roundTripTime === "number") {
            rtts.push(report.roundTripTime * 1000);
          }
          if (report.type === "inbound-rtp") {
            if (typeof report.packetsLost === "number") lost += report.packetsLost;
            if (typeof report.packetsReceived === "number") recv += report.packetsReceived;
            if (typeof report.jitter === "number") jitters.push(report.jitter * 1000);
          }
        }
      } catch {
        /* stats not ready */
      }
    }
    const dLost = Math.max(0, lost - this.rtpPrev.lost);
    const dRecv = Math.max(0, recv - this.rtpPrev.recv);
    this.rtpPrev = { lost, recv };
    const sample = dLost + dRecv;
    let phase: CallLink["phase"] = "connected";
    if (live === 0 && failed > 0) phase = "failed";
    else if (live === 0) phase = "connecting";
    else if (failed > 0 || connecting > 0) phase = "unstable";
    return {
      phase,
      rttMs: rtts.length ? Math.round(Math.max(...rtts)) : null,
      lossPct: sample > 0 ? Math.round((dLost / sample) * 1000) / 10 : 0,
      jitterMs: jitters.length ? Math.round(Math.max(...jitters)) : null,
      peers: pcs.length,
      live,
      mode: this.currentMode(),
    };
  }

  private async flushIce(peer: string) {
    const pc = this.pcs.get(peer);
    if (!pc?.remoteDescription) return;
    const queued = this.pendingIce.get(peer) ?? [];
    this.pendingIce.set(peer, []);
    for (const candidate of queued) {
      try {
        await pc.addIceCandidate(candidate);
      } catch {
        /* candidate for an old generation */
      }
    }
  }

  private open(peer: string): RTCPeerConnection {
    const existing = this.pcs.get(peer);
    if (existing) return existing;
    const made = createRtcPeerConnection(
      typeof RTCPeerConnection === "undefined" ? undefined : RTCPeerConnection,
      rtcPeerConfigs(iceServers, isWebKitEngine()),
    );
    if (!made.ok) throw new Error(made.error);
    const pc = made.pc;
    this.pcs.set(peer, pc);

    pc.onicecandidate = (ev) => {
      if (this.stopped) return;
      this.emitSig({
        type: "rtc",
        room: this.room,
        from: this.me,
        to: peer,
        kind: "ice",
        candidate: ev.candidate
          ? {
              candidate: ev.candidate.candidate,
              sdpMid: ev.candidate.sdpMid,
              sdpMLineIndex: ev.candidate.sdpMLineIndex,
              usernameFragment: ev.candidate.usernameFragment,
            }
          : null,
      });
    };

    pc.onicegatheringstatechange = () => {
      if (this.stopped || this.pcs.get(peer) !== pc) return;
      if (pc.iceGatheringState !== "complete") return;
      this.resendLocal(peer, pc);
    };

    pc.onnegotiationneeded = () => {
      if (this.stopped || this.pcs.get(peer) !== pc) return;
      if (rtcPolite(this.me, peer)) return;
      if (pc.signalingState !== "stable") return;
      // replaceTrack often fires this; only renegotiate when m-line layout changed.
      if (!this.pcLayoutStale(pc, peer)) return;
      void this.enqueue(peer, () => this.offerNow(peer));
    };

    pc.ontrack = (ev) => {
      if (!ev.track) return;
      const publish = () => {
        if (this.stopped) return;
        const xcvrs = [...pc.getTransceivers()].filter((t) => t.direction !== "stopped");
        const idx = xcvrs.indexOf(ev.transceiver);
        const extras = this.layoutExtras(peer);
        const fromPeer = callSlotPeer(idx, peer, extras);
        const mapped = this.screenXcvr.get(peer);
        const mappedIdx = mapped ? xcvrs.indexOf(mapped) : -1;
        const isScreen =
          extras.length > 0 || this.hub
            ? callSlotScreen(ev.track.kind, idx)
            : callTrackIsScreen(
                ev.track.kind,
                idx,
                xcvrs.map((t) => ({ mid: t.mid, kind: this.xcvrKind(t) })),
                mappedIdx >= 0 ? mappedIdx : null,
              );
        if (isScreen && !trackLooksLive(ev.track)) {
          this.remoteScr.delete(fromPeer);
          this.dropSink(fromPeer, true);
          this.seenScreen.delete(fromPeer);
          this.rememberInbound(fromPeer, true, "video", null);
          this.forwardTrack(fromPeer, "video", true, null);
          this.onRemote({ peer: fromPeer, stream: new MediaStream(), screen: true });
          return;
        }
        if (ev.track.kind === "video") this.attachSink(fromPeer, isScreen, ev.track);
        const map = isScreen ? this.remoteScr : this.remoteCam;
        let stream = map.get(fromPeer);
        if (!stream) {
          stream = new MediaStream();
          map.set(fromPeer, stream);
        }
        const sameKind =
          ev.track.kind === "audio" ? stream.getAudioTracks() : stream.getVideoTracks();
        for (const old of sameKind) {
          if (old.id !== ev.track.id) stream.removeTrack(old);
        }
        if (!stream.getTracks().some((t) => t.id === ev.track.id)) stream.addTrack(ev.track);
        if (ev.track.kind === "audio" && !this.heard.has(fromPeer)) {
          this.heard.add(fromPeer);
          this.trace("audio arrived", "ok", fromPeer);
        }
        if (isScreen && trackLooksLive(ev.track) && !this.seenScreen.has(fromPeer)) {
          this.seenScreen.add(fromPeer);
          this.trace("screen arrived", "ok", fromPeer);
        }
        this.rememberInbound(fromPeer, isScreen, ev.track.kind, ev.track);
        this.forwardTrack(fromPeer, ev.track.kind, isScreen, ev.track);
        this.onRemote({
          peer: fromPeer,
          stream: new MediaStream(stream.getTracks()),
          screen: isScreen,
        });
      };
      ev.track.addEventListener("mute", publish);
      ev.track.addEventListener("unmute", publish);
      ev.track.addEventListener("ended", publish);
      publish();
    };

    pc.onconnectionstatechange = () => {
      void this.emitLink();
      const state = pc.connectionState;
      if (this.pcSeen.get(peer) !== state) {
        this.pcSeen.set(peer, state);
        const ice = pc.iceConnectionState;
        // Ignore spurious "failed" while ICE is healthy (WebView2 quirk).
        if (
          state === "failed" &&
          (ice === "connected" || ice === "completed" || ice === "checking")
        ) {
          this.trace("link failed (ICE still going · keeping)", "warn", peer);
          return;
        }
        const text = pcTraceText(state);
        if (text) {
          const level =
            state === "connected" ? "ok" : state === "failed" || state === "closed" ? "err" : state === "disconnected" ? "warn" : "info";
          this.trace(text, level, peer);
        }
      }
      if (pc.connectionState === "closed") this.onGone(peer);
      if (pc.connectionState === "failed") {
        const ice = pc.iceConnectionState;
        if (ice === "connected" || ice === "completed" || ice === "checking") return;
        this.maybeReconnect(peer, pc);
      }
    };

    pc.oniceconnectionstatechange = () => {
      void this.emitLink();
      const ice = pc.iceConnectionState;
      if (this.iceSeen.get(peer) !== ice) {
        this.iceSeen.set(peer, ice);
        const text = iceTraceText(ice);
        const level = iceTraceLevel(ice);
        if (text && level) this.trace(text, level, peer);
      }
      if (pc.iceConnectionState === "connected" || pc.iceConnectionState === "completed") {
        this.reconnectAt.delete(peer);
        this.clearOfferRetry(peer);
        this.clearIceStuck(peer);
        return;
      }
      if (pc.iceConnectionState === "checking") {
        this.armIceStuck(peer, pc);
        return;
      }
      this.clearIceStuck(peer);
      if (pc.iceConnectionState === "failed") {
        this.maybeReconnect(peer, pc);
        return;
      }
      if (pc.iceConnectionState === "disconnected") {
        window.setTimeout(() => {
          if (this.stopped || this.pcs.get(peer) !== pc) return;
          if (pc.signalingState !== "stable") return;
          if (pc.iceConnectionState === "disconnected" || pc.iceConnectionState === "failed") {
            this.maybeReconnect(peer, pc);
          }
        }, 2500);
      }
    };

    return pc;
  }

  private resendLocal(peer: string, pc: RTCPeerConnection) {
    const desc = pc.localDescription;
    if (!desc) return;
    if (pc.signalingState === "have-local-offer" && desc.type === "offer") {
      this.emitSig({
        type: "rtc",
        room: this.room,
        from: this.me,
        to: peer,
        kind: "offer",
        sdp: desc.sdp,
      });
    }
  }

  private emitSig(frame: Omit<RtcFrame, "ts" | "sess">) {
    this.send({ ...frame, ts: Date.now(), sess: this.sess });
  }

  private clearOfferRetry(peer: string) {
    const id = this.offerRetry.get(peer);
    if (id != null) window.clearInterval(id);
    this.offerRetry.delete(peer);
  }

  private cancelPeerDrop(peer: string) {
    const id = this.peerDropTimer.get(peer);
    if (id != null) window.clearTimeout(id);
    this.peerDropTimer.delete(peer);
  }

  private armPeerDrop(peer: string) {
    if (this.peerDropTimer.has(peer)) return;
    const pc = this.pcs.get(peer);
    const ice = pc?.iceConnectionState;
    // Keep a working ICE link through brief roster blips.
    const grace =
      ice === "connected" || ice === "completed" || ice === "checking" ? 20000 : 5000;
    const id = window.setTimeout(() => {
      this.peerDropTimer.delete(peer);
      if (this.stopped || this.wantPeers.has(peer)) return;
      this.drop(peer);
    }, grace);
    this.peerDropTimer.set(peer, id);
  }

  private clearIceStuck(peer: string) {
    const id = this.iceStuckTimer.get(peer);
    if (id != null) window.clearTimeout(id);
    this.iceStuckTimer.delete(peer);
  }

  private armIceStuck(peer: string, pc: RTCPeerConnection) {
    if (this.iceStuckTimer.has(peer)) return;
    const id = window.setTimeout(() => {
      this.iceStuckTimer.delete(peer);
      if (this.stopped || this.pcs.get(peer) !== pc) return;
      const ice = pc.iceConnectionState;
      if (ice !== "checking" && ice !== "disconnected") return;
      this.trace("ICE stuck · restarting", "warn", peer);
      this.maybeReconnect(peer, pc);
    }, 12000);
    this.iceStuckTimer.set(peer, id);
  }

  private armOfferRetry(peer: string) {
    this.clearOfferRetry(peer);
    const id = window.setInterval(() => {
      if (this.stopped) {
        this.clearOfferRetry(peer);
        return;
      }
      const pc = this.pcs.get(peer);
      if (!pc || rtcPolite(this.me, peer)) {
        this.clearOfferRetry(peer);
        return;
      }
      const ice = pc.iceConnectionState;
      if (ice === "connected" || ice === "completed") {
        this.clearOfferRetry(peer);
        return;
      }
      if (pc.signalingState === "have-local-offer" && pc.localDescription?.type === "offer") {
        this.emitSig({
          type: "rtc",
          room: this.room,
          from: this.me,
          to: peer,
          kind: "offer",
          sdp: pc.localDescription.sdp,
        });
        this.trace("resending offer", "info", peer);
        return;
      }
      // Answer already applied — don't mint a fresh offer while ICE is still checking.
      if (
        pc.remoteDescription &&
        (pc.iceConnectionState === "checking" ||
          pc.iceConnectionState === "connected" ||
          pc.iceConnectionState === "completed")
      ) {
        this.clearOfferRetry(peer);
        return;
      }
      // Only mint a fresh offer if we never got an answer, or layout must grow (hub).
      if (pc.signalingState === "stable" && (!pc.remoteDescription || this.pcLayoutStale(pc, peer))) {
        void this.enqueue(peer, () => this.offerNow(peer));
      }
    }, 2000);
    this.offerRetry.set(peer, id);
  }

  private scheduleOffer(peer: string) {
    void this.enqueue(peer, async () => {
      await this.waitForMic(800);
      await this.offerNow(peer);
    });
  }

  private kickImpoliteOffers() {
    for (const peer of this.pcs.keys()) {
      if (rtcPolite(this.me, peer)) continue;
      const pc = this.pcs.get(peer);
      if (!pc || pc.signalingState !== "stable") continue;
      void this.enqueue(peer, () => this.offerNow(peer));
    }
  }

  private async waitForMic(ms: number) {
    const start = Date.now();
    while (!this.localAudio() && Date.now() - start < ms) {
      await sleep(50);
      if (this.stopped) return;
    }
  }

  private pcLayoutStale(pc: RTCPeerConnection, peer: string): boolean {
    const live = pc.getTransceivers().filter((t) => t.direction !== "stopped").length;
    if (!live) return false;
    // Only undersized layouts are stale. Extra hub forward m-lines after 3→2 are fine.
    return live < callWantedMLines(this.layoutExtras(peer).length);
  }

  private addMLines(pc: RTCPeerConnection, peer: string) {
    const audioTrack = this.localAudio();
    const videoTrack = this.cam?.getVideoTracks()[0] ?? null;
    const screenTrack = this.screen?.getVideoTracks()[0] ?? null;
    const audio = audioTrack
      ? pc.addTransceiver(audioTrack, { direction: "sendrecv" })
      : pc.addTransceiver("audio", { direction: "sendrecv" });
    const cam = videoTrack
      ? pc.addTransceiver(videoTrack, { direction: "sendrecv" })
      : pc.addTransceiver("video", { direction: "sendrecv" });
    const screen = screenTrack
      ? pc.addTransceiver(screenTrack, { direction: "sendrecv" })
      : pc.addTransceiver("video", { direction: "sendrecv" });
    this.audioSenders.set(peer, audio.sender);
    this.camSenders.set(peer, cam.sender);
    this.screenSenders.set(peer, screen.sender);
    this.screenXcvr.set(peer, screen);
    this.addFwdMLines(pc, peer);
  }

  private addFwdMLines(pc: RTCPeerConnection, peer: string) {
    const extras = this.layoutExtras(peer);
    if (!extras.length) return;
    const direction: RTCRtpTransceiverDirection = this.hub === this.me ? "sendonly" : "recvonly";
    const want = callWantedMLines(extras.length);
    while (pc.getTransceivers().filter((t) => t.direction !== "stopped").length < want) {
      pc.addTransceiver("audio", { direction });
      pc.addTransceiver("video", { direction });
      pc.addTransceiver("video", { direction });
    }
  }

  private ensureMLines(pc: RTCPeerConnection, peer: string) {
    const live = [...pc.getTransceivers()].filter((t) => t.direction !== "stopped");
    if (!live.length) {
      this.addMLines(pc, peer);
    } else {
      this.addFwdMLines(pc, peer);
    }
    this.bindSenders(pc, peer);
    this.bindFwd(pc, peer);
  }

  private bindFwd(pc: RTCPeerConnection, peer: string) {
    const extras = this.layoutExtras(peer);
    const live = [...pc.getTransceivers()].filter((t) => t.direction !== "stopped");
    for (let i = 0; i < extras.length; i += 1) {
      const from = extras[i];
      const base = 3 + i * 3;
      const audio = live[base];
      const cam = live[base + 1];
      const screen = live[base + 2];
      if (audio) this.fwdSenders.set(this.fwdKey(peer, from, "a"), audio.sender);
      if (cam) this.fwdSenders.set(this.fwdKey(peer, from, "c"), cam.sender);
      if (screen) this.fwdSenders.set(this.fwdKey(peer, from, "s"), screen.sender);
    }
  }

  private rememberInbound(
    from: string,
    screen: boolean,
    kind: string,
    track: MediaStreamTrack | null,
  ) {
    const cur = this.inbound.get(from) ?? { audio: null, cam: null, screen: null };
    if (kind === "audio") cur.audio = track;
    else if (screen) cur.screen = track;
    else cur.cam = track;
    this.inbound.set(from, cur);
  }

  private forwardTrack(
    from: string,
    kind: string,
    screen: boolean,
    track: MediaStreamTrack | null,
  ) {
    if (this.hub !== this.me) return;
    const slot: "a" | "c" | "s" = kind === "audio" ? "a" : screen ? "s" : "c";
    for (const to of this.pcs.keys()) {
      if (to === from) continue;
      const sender = this.fwdSenders.get(this.fwdKey(to, from, slot));
      if (!sender) continue;
      void sender.replaceTrack(track).catch(() => undefined);
    }
  }

  private replayInbound() {
    if (this.hub !== this.me) return;
    for (const [from, tracks] of this.inbound) {
      this.forwardTrack(from, "audio", false, tracks.audio);
      this.forwardTrack(from, "video", false, tracks.cam);
      this.forwardTrack(from, "video", true, tracks.screen);
    }
  }

  private preferH264IfLinux(pc: RTCPeerConnection, sdp?: string) {
    if (!sdp || /vp8/i.test(sdp)) return;
    const caps = RTCRtpSender.getCapabilities?.("video");
    if (!caps?.codecs.length) return;
    const h264 = caps.codecs.filter((codec) => /h264/i.test(codec.mimeType));
    if (!h264.length) return;
    const rtx = caps.codecs.filter((codec) => /rtx/i.test(codec.mimeType));
    for (const xcvr of pc.getTransceivers()) {
      if (this.xcvrKind(xcvr) !== "video") continue;
      try {
        xcvr.setCodecPreferences([...h264, ...rtx]);
      } catch {
        /* older chromium */
      }
    }
  }

  private xcvrKind(t: RTCRtpTransceiver): string | undefined {
    if (t.sender.dtmf) return "audio";
    const kind = t.receiver?.track?.kind || t.sender?.track?.kind;
    return kind === "audio" || kind === "video" ? kind : undefined;
  }

  private bindSenders(pc: RTCPeerConnection, peer: string) {
    const extras = this.layoutExtras(peer);
    const live = [...pc.getTransceivers()].filter((t) => t.direction !== "stopped");
    const primaryEnd = callPrimaryCount(extras.length, live.length);
    for (let i = 0; i < primaryEnd; i += 1) {
      const t = live[i];
      if (t.direction === "recvonly" || t.direction === "inactive") {
        try {
          t.direction = "sendrecv";
        } catch {
          /* transceiver already stopped */
        }
      }
    }
    const primary = live.slice(0, primaryEnd);
    const audio =
      primary.find((t) => t.sender.dtmf) ?? primary.find((t) => this.xcvrKind(t) === "audio");
    const videos = primary
      .filter((t) => t !== audio && !t.sender.dtmf)
      .sort((a, b) =>
        String(a.mid ?? "").localeCompare(String(b.mid ?? ""), undefined, { numeric: true }),
      );
    if (extras.length > 0) {
      if (primary[0]) this.audioSenders.set(peer, primary[0].sender);
      if (primary[1]) this.camSenders.set(peer, primary[1].sender);
      if (primary[2]) {
        this.screenSenders.set(peer, primary[2].sender);
        this.screenXcvr.set(peer, primary[2]);
      }
      return;
    }
    if (audio) this.audioSenders.set(peer, audio.sender);
    if (videos[0]) this.camSenders.set(peer, videos[0].sender);
    if (videos[1]) {
      this.screenSenders.set(peer, videos[1].sender);
      this.screenXcvr.set(peer, videos[1]);
    } else if (this.screenXcvr.get(peer) && this.xcvrKind(this.screenXcvr.get(peer)!) === "audio") {
      this.screenXcvr.delete(peer);
    }
    if (!audio || videos.length < 2) {
      const pick = pickCallTransceivers(
        primary.map((t) => ({ mid: t.mid, kind: this.xcvrKind(t) })),
      );
      if (!audio && primary[pick.audio] && this.xcvrKind(primary[pick.audio]) !== "video") {
        this.audioSenders.set(peer, primary[pick.audio].sender);
      }
      if (!videos[0] && pick.cam != null && this.xcvrKind(primary[pick.cam]) !== "audio") {
        this.camSenders.set(peer, primary[pick.cam].sender);
      }
      if (
        !videos[1] &&
        pick.screen != null &&
        this.xcvrKind(primary[pick.screen]) !== "audio"
      ) {
        this.screenSenders.set(peer, primary[pick.screen].sender);
        this.screenXcvr.set(peer, primary[pick.screen]);
      }
    }
  }

  private async pushLocal(only?: string) {
    const video = this.cam?.getVideoTracks()[0] ?? null;
    const audio = this.localAudio();
    const scr = this.screen?.getVideoTracks()[0] ?? null;
    if (scr) scr.contentHint = "detail";
    const peers = only ? [only] : [...this.pcs.keys()];
    for (const peer of peers) {
      const pc = this.pcs.get(peer);
      if (pc) this.bindSenders(pc, peer);
      await this.attachTrack(this.audioSenders.get(peer), audio);
      await this.attachTrack(this.camSenders.get(peer), video);
      const screenSender = this.screenSenders.get(peer);
      const screenOk = await this.attachTrack(screenSender, scr);
      if (scr && !screenSender && !this.seenScreen.has(`send:${peer}`)) {
        this.seenScreen.add(`send:${peer}`);
        this.trace("screen has no send channel", "warn", peer);
      } else if (scr && screenOk && screenSender?.track?.id === scr.id && !this.seenScreen.has(`send:${peer}`)) {
        this.seenScreen.add(`send:${peer}`);
        this.trace("screen attached", "info", peer);
        window.setTimeout(() => void this.logScreenBytes(peer, screenSender), 1500);
      } else if (scr && !screenOk && !this.seenScreen.has(`fail:${peer}`)) {
        this.seenScreen.add(`fail:${peer}`);
        this.trace("screen attach failed", "err", peer);
      } else if (!scr) {
        this.seenScreen.delete(`send:${peer}`);
        this.seenScreen.delete(`fail:${peer}`);
      }
    }
  }

  private async logScreenBytes(peer: string, sender?: RTCRtpSender) {
    if (!sender || this.stopped || !this.screen?.getVideoTracks().length) return;
    try {
      const stats = await sender.getStats();
      const codecs = new Map<string, string>();
      let bytes = 0;
      let codecId = "";
      for (const report of stats.values()) {
        if (report.type === "codec" && typeof report.mimeType === "string") {
          codecs.set(report.id, report.mimeType);
        }
        if (report.type === "outbound-rtp" && report.kind === "video") {
          bytes += Number(report.bytesSent ?? 0);
          if (typeof report.codecId === "string") codecId = report.codecId;
        }
      }
      const mime = codecs.get(codecId) ?? "";
      this.trace(
        bytes > 0 ? `screen ${bytes} B ${mime}` : `screen 0 B ${mime || "no RTP"}`,
        bytes > 0 ? "ok" : "warn",
        peer,
      );
    } catch {
      /* stats unavailable */
    }
  }

  private async attachTrack(sender: RTCRtpSender | undefined, track: MediaStreamTrack | null) {
    if (!sender) return false;
    if (sender.track?.id === (track?.id ?? null)) return true;
    const audioSender = Boolean(sender.dtmf);
    if (track) {
      if (audioSender && track.kind !== "audio") return false;
      if (!audioSender && track.kind !== "video") return false;
    }
    try {
      await sender.replaceTrack(track);
      return !track || sender.track?.id === track.id;
    } catch (err) {
      this.trace(`replaceTrack: ${String(err)}`, "err");
      return false;
    }
  }

  private async offerNow(peer: string) {
    const pc = this.pcs.get(peer);
    if (!pc || this.stopped) return;
    if (rtcPolite(this.me, peer)) return;
    if (pc.signalingState !== "stable") return;
    if (!pc.getTransceivers().some((t) => t.direction !== "stopped")) {
      this.addMLines(pc, peer);
    } else {
      this.addFwdMLines(pc, peer);
    }
    this.bindSenders(pc, peer);
    this.bindFwd(pc, peer);
    await this.pushLocal(peer);
    if (this.stopped || this.pcs.get(peer) !== pc || pc.signalingState !== "stable") return;
    try {
      this.makingOffer.add(peer);
      const offer = await pc.createOffer();
      if (pc.signalingState !== "stable") return;
      await pc.setLocalDescription(offer);
    } finally {
      this.makingOffer.delete(peer);
    }
    if (this.stopped || this.pcs.get(peer) !== pc) return;
    if (!canPublishLocalSdp("offer", pc.localDescription?.type)) return;
    this.emitSig({
      type: "rtc",
      room: this.room,
      from: this.me,
      to: peer,
      kind: "offer",
      sdp: pc.localDescription!.sdp,
    });
    this.trace("offer sent", "info", peer);
    this.armOfferRetry(peer);
  }

  private sinkKey(peer: string, screen: boolean) {
    return `${peer}:${screen ? "s" : "c"}`;
  }

  private attachSink(peer: string, screen: boolean, track: MediaStreamTrack) {
    const key = this.sinkKey(peer, screen);
    let el = this.remoteSinks.get(key);
    if (!el) {
      el = document.createElement("video");
      el.muted = true;
      el.playsInline = true;
      el.autoplay = true;
      el.setAttribute("playsinline", "true");
      el.style.cssText =
        "position:fixed;width:1px;height:1px;opacity:0;pointer-events:none;left:-80px;top:-80px";
      document.body.appendChild(el);
      this.remoteSinks.set(key, el);
    }
    const stream = new MediaStream([track]);
    el.srcObject = stream;
    void el.play().catch(() => undefined);
  }

  private dropSink(peer: string, screen: boolean) {
    const key = this.sinkKey(peer, screen);
    const el = this.remoteSinks.get(key);
    if (!el) return;
    el.srcObject = null;
    el.remove();
    this.remoteSinks.delete(key);
  }

  private dropSinks(peer: string) {
    this.dropSink(peer, false);
    this.dropSink(peer, true);
  }

  private dropAllSinks() {
    for (const el of this.remoteSinks.values()) {
      el.srcObject = null;
      el.remove();
    }
    this.remoteSinks.clear();
  }

  private drop(peer: string, silent = false) {
    const pc = this.pcs.get(peer);
    if (!pc) return;
    this.clearOfferRetry(peer);
    this.cancelPeerDrop(peer);
    this.clearIceStuck(peer);
    if (!silent) {
      this.emitSig({ type: "rtc", room: this.room, from: this.me, to: peer, kind: "bye" });
    }
    pc.close();
    this.pcs.delete(peer);
    this.jobs.delete(peer);
    this.camSenders.delete(peer);
    this.screenSenders.delete(peer);
    this.audioSenders.delete(peer);
    this.screenXcvr.delete(peer);
    this.remoteCam.delete(peer);
    this.remoteScr.delete(peer);
    this.pendingIce.delete(peer);
    this.oneWayTicks.delete(peer);
    this.reconnectAt.delete(peer);
    this.makingOffer.delete(peer);
    this.ignoreOffer.delete(peer);
    this.iceSeen.delete(peer);
    this.pcSeen.delete(peer);
    this.heard.delete(peer);
    this.seenScreen.delete(peer);
    this.inbound.delete(peer);
    for (const key of [...this.fwdSenders.keys()]) {
      if (key.startsWith(`${peer}|`) || key.includes(`|${peer}|`)) this.fwdSenders.delete(key);
    }
    this.dropSinks(peer);
    this.trace(silent ? "peer left" : "link closed", "warn", peer);
    this.noteMode();
    this.onGone(peer);
  }
}
