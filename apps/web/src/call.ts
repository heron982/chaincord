import { listen } from "@tauri-apps/api/event";
import {
  backendRtcPushFrame,
  backendRtcSignal,
  backendRtcStart,
  backendRtcStop,
  backendRtcSync,
  rtcPeerConnectionMissing,
} from "./backend";
import {
  callPathMode,
  canPublishLocalSdp,
  createRtcPeerConnection,
  iceTraceLevel,
  iceTraceText,
  isWebKitEngine,
  pcTraceText,
  pickCallTransceivers,
  preferH264Codecs,
  rtcPeerConfigs,
  rtcPolite,
  shouldHealSend,
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

function trackLive(track: MediaStreamTrack | null | undefined) {
  return Boolean(track && track.readyState === "live" && track.enabled && !track.muted);
}

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

  constructor(
    private readonly me: string,
    private readonly room: string,
    private readonly send: (frame: RtcFrame) => void,
    private readonly onRemote: (media: RemoteMedia) => void,
    private readonly onGone: (peer: string) => void,
    private readonly onLink: (link: CallLink) => void,
    private readonly onLog: (line: CallTrace) => void,
  ) {
    this.trace("call iniciada", "info");
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
        await listen<CallTrace>("ui-call-trace", (e) => this.onLog(e.payload)),
      );
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
      this.trace(`neste app: ${why}`, "err");
    }
  }

  private currentMode(): CallPathMode {
    return callPathMode(this.pcs.size);
  }

  private noteMode() {
    const mode = this.currentMode();
    if (mode === this.pathMode) return;
    const prev = this.pathMode;
    this.pathMode = mode;
    this.trace(`modo ${prev} → ${mode}`, "info");
  }

  private trace(event: string, level: CallTrace["level"], peer: string | null = null) {
    this.traceSeq += 1;
    this.onLog({
      id: this.traceSeq,
      t: Date.now(),
      mode: this.currentMode(),
      peer,
      event,
      level,
    });
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
      this.kickNativePump();
      return;
    }
    void this.pushLocal().then(() => this.kickImpoliteOffers());
    window.setTimeout(() => void this.pushLocal(), 200);
  }

  private ingestNativeFrame(peer: string, screen: boolean, jpeg: string) {
    const id = screen ? `screen:${peer}` : `user:${peer}`;
    const url = jpeg ? `data:image/jpeg;base64,${jpeg}` : "";
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
    await this.grabNative(this.screen, true);
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

  sync(peers: string[]) {
    if (this.stopped) return;
    if (this.native) {
      void this.nativeReady
        .then(() => backendRtcSync(peers.filter((p) => p && p !== this.me)))
        .catch((err) => {
          this.trace(`neste app: ${String(err)}`, "err");
        });
      return;
    }
    const want = new Set(peers.filter((p) => p && p !== this.me));
    for (const id of [...this.pcs.keys()]) {
      if (!want.has(id)) this.drop(id);
    }
    for (const id of want) {
      if (!this.pcs.has(id)) {
        const polite = rtcPolite(this.me, id);
        try {
          this.open(id);
        } catch (err) {
          const why = err instanceof Error ? err.message : String(err);
          if (this.rtcFailed !== why) {
            this.rtcFailed = why;
            this.trace(`neste app: ${why}`, "err");
          }
          continue;
        }
        this.trace("enlace aberto", "info", id);
        if (!polite) this.scheduleOffer(id);
      }
    }
    this.noteMode();
  }

  async handle(frame: RtcFrame) {
    if (this.stopped) return;
    if (this.native) {
      await this.nativeReady;
      await backendRtcSignal(frame).catch((err) => {
        this.trace(`neste app: ${String(err)}`, "err");
      });
      return;
    }
    if (frame.room !== this.room) return;
    if (frame.from === this.me) return;
    if (frame.to !== this.me) return;
    if (frame.kind === "bye") {
      this.drop(frame.from, true);
      return;
    }
    await this.enqueue(frame.from, () => this.handleOne(frame));
  }

  stop() {
    this.trace("call encerrada", "info");
    this.stopped = true;
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
    for (const id of [...this.pcs.keys()]) this.drop(id);
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
    const pc = this.pcs.get(frame.from) ?? this.open(frame.from);
    const polite = rtcPolite(this.me, frame.from);
    try {
      if (frame.kind === "offer" && frame.sdp) {
        const collision = this.makingOffer.has(frame.from) || pc.signalingState !== "stable";
        if (collision) {
          if (!polite) return;
          this.ignoreOffer.add(frame.from);
          if (pc.signalingState !== "stable") {
            await pc.setLocalDescription({ type: "rollback" });
          }
        }
        await pc.setRemoteDescription({ type: "offer", sdp: frame.sdp });
        this.trace("oferta recebida", "info", frame.from);
        await this.flushIce(frame.from);
        await this.waitForMic(800);
        this.bindSenders(pc, frame.from);
        await this.pushLocal(frame.from);
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        await this.pushLocal(frame.from);
        await this.waitIce(pc, 400);
        if (this.stopped || this.pcs.get(frame.from) !== pc) return;
        if (!canPublishLocalSdp("answer", pc.localDescription?.type)) return;
        this.send({
          type: "rtc",
          room: this.room,
          from: this.me,
          to: frame.from,
          kind: "answer",
          sdp: pc.localDescription?.sdp,
        });
        this.trace("resposta enviada", "info", frame.from);
        this.ignoreOffer.delete(frame.from);
      } else if (frame.kind === "answer" && frame.sdp) {
        if (pc.signalingState === "have-local-offer") {
          await pc.setRemoteDescription({ type: "answer", sdp: frame.sdp });
          this.trace("resposta recebida", "ok", frame.from);
          await this.flushIce(frame.from);
          this.bindSenders(pc, frame.from);
          await this.pushLocal(frame.from);
        }
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
    } catch {
      /* glare / late ice */
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
          if (ticks === 2) this.trace("envio fraco — reanexando mic", "warn", peer);
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
    this.trace("religando ICE", "warn", peer);
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
      if (pc.connectionState === "connected") live += 1;
      else if (pc.connectionState === "failed") failed += 1;
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
      this.send({
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

    pc.ontrack = (ev) => {
      if (!ev.track) return;
      const publish = () => {
        if (this.stopped) return;
        const isScreen =
          ev.track.kind === "video" &&
          (this.screenXcvr.get(peer) === ev.transceiver || ev.transceiver.mid === "2");
        if (isScreen && ev.track.readyState === "ended") {
          this.remoteScr.delete(peer);
          this.dropSink(peer, true);
          this.seenScreen.delete(peer);
          this.onRemote({ peer, stream: new MediaStream(), screen: true });
          return;
        }
        if (ev.track.kind === "video") this.attachSink(peer, isScreen, ev.track);
        const map = isScreen ? this.remoteScr : this.remoteCam;
        let stream = map.get(peer);
        if (!stream) {
          stream = new MediaStream();
          map.set(peer, stream);
        }
        const sameKind =
          ev.track.kind === "audio" ? stream.getAudioTracks() : stream.getVideoTracks();
        for (const old of sameKind) {
          if (old.id !== ev.track.id) stream.removeTrack(old);
        }
        if (!stream.getTracks().some((t) => t.id === ev.track.id)) stream.addTrack(ev.track);
        if (ev.track.kind === "audio" && !this.heard.has(peer)) {
          this.heard.add(peer);
          this.trace("áudio chegou", "ok", peer);
        }
        if (isScreen && trackLive(ev.track) && !this.seenScreen.has(peer)) {
          this.seenScreen.add(peer);
          this.trace("tela chegou", "ok", peer);
        }
        this.onRemote({ peer, stream: new MediaStream(stream.getTracks()), screen: isScreen });
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
        const text = pcTraceText(state);
        if (text) {
          const level =
            state === "connected" ? "ok" : state === "failed" || state === "closed" ? "err" : state === "disconnected" ? "warn" : "info";
          this.trace(text, level, peer);
        }
      }
      if (pc.connectionState === "closed") this.onGone(peer);
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
        return;
      }
      if (pc.iceConnectionState === "failed") {
        this.maybeReconnect(peer, pc);
        return;
      }
      if (pc.iceConnectionState === "disconnected") {
        window.setTimeout(() => {
          if (this.stopped || this.pcs.get(peer) !== pc) return;
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
      this.send({
        type: "rtc",
        room: this.room,
        from: this.me,
        to: peer,
        kind: "offer",
        sdp: desc.sdp,
      });
    }
  }

  private scheduleOffer(peer: string) {
    void this.enqueue(peer, async () => {
      await this.waitForMic(800);
      await this.offerNow(peer);
    });
    window.setTimeout(() => {
      if (this.stopped) return;
      const pc = this.pcs.get(peer);
      if (!pc) return;
      if (pc.connectionState === "connected") return;
      if (rtcPolite(this.me, peer)) return;
      if (pc.signalingState === "have-local-offer" && pc.localDescription?.type === "offer") {
        this.send({
          type: "rtc",
          room: this.room,
          from: this.me,
          to: peer,
          kind: "offer",
          sdp: pc.localDescription.sdp,
        });
        return;
      }
      if (pc.signalingState === "stable") {
        void this.enqueue(peer, () => this.offerNow(peer));
      }
    }, 2500);
  }

  private kickImpoliteOffers() {
    for (const peer of this.pcs.keys()) {
      if (rtcPolite(this.me, peer)) continue;
      const pc = this.pcs.get(peer);
      if (!pc || pc.signalingState !== "stable") continue;
      if (pc.connectionState === "connected") {
        void this.pushLocal(peer);
        continue;
      }
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
    this.preferH264(pc);
  }

  private preferH264(pc: RTCPeerConnection) {
    const caps = RTCRtpSender.getCapabilities?.("video");
    if (!caps?.codecs.length) return;
    const ranked = preferH264Codecs(caps.codecs);
    for (const xcvr of pc.getTransceivers()) {
      const kind = xcvr.receiver?.track?.kind || xcvr.sender?.track?.kind;
      if (kind !== "video") continue;
      try {
        xcvr.setCodecPreferences(ranked);
      } catch {
        /* older chromium */
      }
    }
  }

  private async tuneVideoSender(sender: RTCRtpSender | undefined, screen: boolean) {
    if (!sender) return;
    try {
      const params = sender.getParameters();
      if (!params.encodings?.length) params.encodings = [{}];
      params.encodings[0].maxBitrate = screen ? 1_800_000 : 800_000;
      if (screen) params.encodings[0].maxFramerate = 15;
      await sender.setParameters(params);
    } catch {
      /* encodings not ready */
    }
  }

  private xcvrKind(t: RTCRtpTransceiver): string | undefined {
    if (t.sender.dtmf) return "audio";
    const kind = t.receiver?.track?.kind || t.sender?.track?.kind;
    return kind === "audio" || kind === "video" ? kind : undefined;
  }

  private bindSenders(pc: RTCPeerConnection, peer: string) {
    this.preferH264(pc);
    const live = [...pc.getTransceivers()].filter((t) => t.direction !== "stopped");
    for (const t of live) {
      if (t.direction === "recvonly" || t.direction === "inactive") {
        try {
          t.direction = "sendrecv";
        } catch {
          /* transceiver already stopped */
        }
      }
    }
    const audio =
      live.find((t) => t.sender.dtmf) ?? live.find((t) => this.xcvrKind(t) === "audio");
    const videos = live
      .filter((t) => t !== audio && !t.sender.dtmf)
      .sort((a, b) =>
        String(a.mid ?? "").localeCompare(String(b.mid ?? ""), undefined, { numeric: true }),
      );
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
        live.map((t) => ({ mid: t.mid, kind: this.xcvrKind(t) })),
      );
      if (!audio && live[pick.audio] && this.xcvrKind(live[pick.audio]) !== "video") {
        this.audioSenders.set(peer, live[pick.audio].sender);
      }
      if (!videos[0] && pick.cam != null && this.xcvrKind(live[pick.cam]) !== "audio") {
        this.camSenders.set(peer, live[pick.cam].sender);
      }
      if (
        !videos[1] &&
        pick.screen != null &&
        this.xcvrKind(live[pick.screen]) !== "audio"
      ) {
        this.screenSenders.set(peer, live[pick.screen].sender);
        this.screenXcvr.set(peer, live[pick.screen]);
      }
    }
    if (!this.screenSenders.get(peer)) {
      const extra = pc.addTransceiver("video", { direction: "sendrecv" });
      this.screenSenders.set(peer, extra.sender);
      this.screenXcvr.set(peer, extra);
      this.preferH264(pc);
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
      await this.attachTrack(this.screenSenders.get(peer), scr);
      await this.tuneVideoSender(this.camSenders.get(peer), false);
      await this.tuneVideoSender(this.screenSenders.get(peer), true);
      if (scr) {
        if (!this.screenSenders.get(peer)) {
          this.trace("tela sem canal de envio", "warn", peer);
        } else if (!this.seenScreen.has(`send:${peer}`)) {
          this.seenScreen.add(`send:${peer}`);
          this.trace("tela anexada", "info", peer);
        }
      } else {
        this.seenScreen.delete(`send:${peer}`);
      }
    }
  }

  private async attachTrack(sender: RTCRtpSender | undefined, track: MediaStreamTrack | null) {
    if (!sender) return;
    if (sender.track?.id === (track?.id ?? null)) return;
    const audioSender = Boolean(sender.dtmf);
    if (track) {
      if (audioSender && track.kind !== "audio") return;
      if (!audioSender && track.kind !== "video") return;
    }
    try {
      await sender.replaceTrack(track);
    } catch {
      /* sender not ready or kind mismatch */
    }
  }

  private async offerNow(peer: string) {
    const pc = this.pcs.get(peer);
    if (!pc || this.stopped) return;
    if (rtcPolite(this.me, peer)) return;
    if (pc.signalingState !== "stable") return;
    if (!pc.getTransceivers().some((t) => t.direction !== "stopped")) {
      this.addMLines(pc, peer);
    }
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
    await this.waitIce(pc, 400);
    if (this.stopped || this.pcs.get(peer) !== pc) return;
    if (!canPublishLocalSdp("offer", pc.localDescription?.type)) return;
    this.send({
      type: "rtc",
      room: this.room,
      from: this.me,
      to: peer,
      kind: "offer",
      sdp: pc.localDescription!.sdp,
    });
    this.trace("oferta enviada", "info", peer);
  }

  private async waitIce(pc: RTCPeerConnection, ms: number) {
    if (pc.iceGatheringState === "complete") return;
    await new Promise<void>((resolve) => {
      let settled = false;
      const done = () => {
        if (settled) return;
        settled = true;
        pc.removeEventListener("icegatheringstatechange", onChange);
        window.clearTimeout(timer);
        resolve();
      };
      const onChange = () => {
        if (pc.iceGatheringState === "complete") done();
      };
      const timer = window.setTimeout(done, ms);
      pc.addEventListener("icegatheringstatechange", onChange);
      onChange();
    });
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
    if (!silent) {
      this.send({ type: "rtc", room: this.room, from: this.me, to: peer, kind: "bye" });
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
    this.dropSinks(peer);
    this.trace(silent ? "par saiu" : "enlace fechado", "warn", peer);
    this.noteMode();
    this.onGone(peer);
  }
}
