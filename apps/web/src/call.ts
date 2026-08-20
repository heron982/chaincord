import {
  callPathMode,
  canPublishLocalSdp,
  iceTraceLevel,
  iceTraceText,
  pcTraceText,
  pickCallTransceivers,
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
  private traceSeq = 0;

  constructor(
    private readonly me: string,
    private readonly room: string,
    private readonly send: (frame: RtcFrame) => void,
    private readonly onRemote: (media: RemoteMedia) => void,
    private readonly onGone: (peer: string) => void,
    private readonly onLink: (link: CallLink) => void,
    private readonly onLog: (line: CallTrace) => void,
  ) {
    this.timer = window.setInterval(() => void this.emitLink(), 1000);
    this.trace("call iniciada", "info");
    void this.emitLink();
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
    void this.pushLocal().then(() => this.kickImpoliteOffers());
  }

  setScreen(stream: MediaStream | null) {
    const track = stream?.getVideoTracks()[0];
    if (track) track.contentHint = "detail";
    this.screen = stream;
    void this.pushLocal();
    window.setTimeout(() => void this.pushLocal(), 200);
  }

  sync(peers: string[]) {
    if (this.stopped) return;
    const want = new Set(peers.filter((p) => p && p !== this.me));
    for (const id of [...this.pcs.keys()]) {
      if (!want.has(id)) this.drop(id);
    }
    for (const id of want) {
      if (!this.pcs.has(id)) {
        const polite = rtcPolite(this.me, id);
        this.open(id);
        this.trace("enlace aberto", "info", id);
        if (!polite) this.scheduleOffer(id);
      }
    }
    this.noteMode();
  }

  async handle(frame: RtcFrame) {
    if (this.stopped) return;
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
    if (this.timer != null) {
      window.clearInterval(this.timer);
      this.timer = null;
    }
    for (const id of [...this.pcs.keys()]) this.drop(id);
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
    const pc = new RTCPeerConnection({
      iceServers,
      bundlePolicy: "max-bundle",
    });
    this.pcs.set(peer, pc);

    pc.onicecandidate = (ev) => {
      if (this.stopped) return;
      this.send({
        type: "rtc",
        room: this.room,
        from: this.me,
        to: peer,
        kind: "ice",
        candidate: ev.candidate ? ev.candidate.toJSON() : null,
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
          ev.track.kind === "video" && this.screenXcvr.get(peer) === ev.transceiver;
        if (isScreen && !trackLive(ev.track)) {
          this.remoteScr.delete(peer);
          this.onRemote({ peer, stream: new MediaStream(), screen: true });
          return;
        }
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
  }

  private xcvrKind(t: RTCRtpTransceiver): string | undefined {
    if (t.sender.dtmf) return "audio";
    const kind = t.receiver?.track?.kind || t.sender?.track?.kind;
    return kind === "audio" || kind === "video" ? kind : undefined;
  }

  private bindSenders(pc: RTCPeerConnection, peer: string) {
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
    this.trace(silent ? "par saiu" : "enlace fechado", "warn", peer);
    this.noteMode();
    this.onGone(peer);
  }
}
