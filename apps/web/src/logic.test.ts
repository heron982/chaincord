import { describe, expect, it } from "vitest";
import {
  callTileGridCols,
  effectivePresence,
  fmtBytes,
  gradeLink,
  groupByPresence,
  isChatLine,
  isWebKitEngine,
  looksLikeInvite,
  inviteShareText,
  extractInvite,
  keepCallFocus,
  pickCallFocus,
  autoCallFocusIds,
  audioDeviceLabel,
  audioDevicesOfKind,
  audioInputConstraint,
  audioSinkSupported,
  readAudioPref,
  resolveAudioDeviceId,
  writeAudioPref,
  wantsCallMedia,
  voiceRoomSharing,
  presenceLabel,
  rtcPolite,
  rtcPeerConfigs,
  createRtcPeerConnection,
  rtcIceServersFor,
  webkitSafeIceUrl,
  seedPercent,
  seedingCardVisible,
  shouldHealSend,
  canPublishLocalSdp,
  shouldResendCallAnswer,
  sdpMediaLineKinds,
  answerMatchesLocalOffer,
  callPathMode,
  callPathHint,
  electCallHub,
  pickMyCall,
  holdCallHub,
  ignoreStaleCallBye,
  ignoreStaleCallSess,
  callWantedMLines,
  callWantedPeers,
  callForwardPeers,
  callSlotPeer,
  callSlotScreen,
  callPrimaryCount,
  callTrackGone,
  iceTraceText,
  iceTraceLevel,
  pickCallTransceivers,
  preferH264Codecs,
  toggleCallFocus,
  trackLooksLive,
  callTrackIsScreen,
  remoteMediaLive,
  remoteScreenVisible,
  remoteVideoVisible,
} from "./logic";

describe("seeding card", () => {
  it("only shows while this client is sending", () => {
    expect(seedingCardVisible(true)).toBe(true);
    expect(seedingCardVisible(false)).toBe(false);
  });

  it("does not get stuck at 50% from idle members", () => {
    expect(seedPercent(1, 1)).toBe(100);
    expect(seedPercent(400, 800)).toBe(50);
    expect(seedPercent(0, 0)).toBe(0);
  });
});

describe("channel filter", () => {
  it("keeps system lines out of #general", () => {
    expect(isChatLine("chat")).toBe(true);
    expect(isChatLine("info")).toBe(false);
    expect(isChatLine("error")).toBe(false);
  });
});

describe("call link grade", () => {
  it("maps connecting and failed states", () => {
    expect(gradeLink(null)).toBe("wait");
    expect(
      gradeLink({
        phase: "connecting",
        rttMs: null,
        lossPct: null,
        jitterMs: null,
        peers: 0,
        live: 0,
        mode: "1:1",
      }),
    ).toBe("wait");
    expect(
      gradeLink({
        phase: "failed",
        rttMs: 0,
        lossPct: 0,
        jitterMs: 0,
        peers: 1,
        live: 0,
        mode: "1:1",
      }),
    ).toBe("off");
  });

  it("turns yellow when a peer is missing media", () => {
    expect(
      gradeLink({
        phase: "connected",
        rttMs: 40,
        lossPct: 0,
        jitterMs: 2,
        peers: 1,
        live: 0,
        mode: "1:1",
      }),
    ).toBe("ok");
  });
});

describe("media track liveness", () => {
  it("requires a live unmuted enabled track", () => {
    expect(
      trackLooksLive({ readyState: "live", enabled: true, muted: false }),
    ).toBe(true);
    expect(
      trackLooksLive({ readyState: "ended", enabled: true, muted: false }),
    ).toBe(false);
    expect(
      trackLooksLive({ readyState: "live", enabled: true, muted: true }),
    ).toBe(false);
    expect(trackLooksLive(null)).toBe(false);
  });

  it("hides camera video after replaceTrack(null) mutes the remote track", () => {
    expect(
      remoteVideoVisible({
        stream: {
          getVideoTracks: () => [{ readyState: "live", enabled: true, muted: true }],
        },
      }),
    ).toBe(false);
    expect(
      remoteVideoVisible({
        stream: {
          getVideoTracks: () => [{ readyState: "live", enabled: true, muted: false }],
        },
      }),
    ).toBe(true);
    expect(remoteVideoVisible({ frames: false, stream: { getVideoTracks: () => [] } })).toBe(false);
  });

  it("shows a remote screen tile once frames or unmuted video arrive", () => {
    expect(remoteScreenVisible(null)).toBe(false);
    expect(remoteScreenVisible({ frames: true, stream: { getVideoTracks: () => [] } })).toBe(true);
    expect(
      remoteScreenVisible({
        stream: {
          getVideoTracks: () => [{ readyState: "live", enabled: true, muted: true }],
        },
      }),
    ).toBe(false);
    expect(
      remoteScreenVisible({
        stream: {
          getVideoTracks: () => [{ readyState: "live", enabled: true, muted: false }],
        },
      }),
    ).toBe(true);
  });

  it("drops muted media and keeps a live audio track", () => {
    expect(
      remoteMediaLive({
        stream: {
          getTracks: () => [{ readyState: "live", enabled: true, muted: true }],
        },
      }),
    ).toBe(false);
    expect(
      remoteMediaLive({
        stream: {
          getTracks: () => [{ readyState: "live", enabled: true, muted: false }],
        },
      }),
    ).toBe(true);
  });
});

describe("fmtBytes", () => {
  it("formats seeding progress", () => {
    expect(fmtBytes(512)).toBe("512 B");
    expect(fmtBytes(1536)).toBe("1.5 KB");
  });
});

describe("audio devices", () => {
  it("keeps a saved device only while it is still listed", () => {
    const mics = [{ deviceId: "a", kind: "audioinput", label: "Headset" }];
    expect(resolveAudioDeviceId(mics, "a")).toBe("a");
    expect(resolveAudioDeviceId(mics, "gone")).toBe("");
    expect(resolveAudioDeviceId([], "a")).toBe("");
  });

  it("uses an exact constraint only when a device is chosen", () => {
    expect(audioInputConstraint("")).toEqual({
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    });
    expect(audioInputConstraint("mic-1")).toEqual({
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
      deviceId: { exact: "mic-1" },
    });
  });

  it("names unlabeled devices and remembers the choice", () => {
    expect(audioDeviceLabel({ deviceId: "x", kind: "audioinput", label: "  " }, 0, "input")).toBe(
      "Microphone 1",
    );
    expect(audioDeviceLabel({ deviceId: "y", kind: "audiooutput", label: "Fones" }, 0, "output")).toBe(
      "Fones",
    );
    expect(
      audioDevicesOfKind(
        [
          { deviceId: "in", kind: "audioinput", label: "Mic" },
          { deviceId: "", kind: "audioinput", label: "ghost" },
          { deviceId: "out", kind: "audiooutput", label: "Speakers" },
        ],
        "audioinput",
      ).map((d) => d.deviceId),
    ).toEqual(["in"]);
    const store: Record<string, string> = {};
    const storage = {
      getItem: (key: string) => store[key] ?? null,
      setItem: (key: string, value: string) => {
        store[key] = value;
      },
      removeItem: (key: string) => {
        delete store[key];
      },
    };
    writeAudioPref("input", "mic-1", storage);
    expect(readAudioPref("input", storage)).toBe("mic-1");
    writeAudioPref("input", "", storage);
    expect(readAudioPref("input", storage)).toBe("");
    expect(audioSinkSupported({ setSinkId: async () => undefined })).toBe(true);
    expect(audioSinkSupported({})).toBe(false);
  });
});

describe("call tiles", () => {
  it("toggles maximize like Discord", () => {
    expect(toggleCallFocus(null, "user:aa")).toBe("user:aa");
    expect(toggleCallFocus("user:aa", "user:aa")).toBe(null);
    expect(toggleCallFocus("user:aa", "screen:aa")).toBe("screen:aa");
  });

  it("drops focus when the tile leaves the stage", () => {
    expect(keepCallFocus("screen:aa", ["user:aa", "user:bb"])).toBe(null);
    expect(keepCallFocus("screen:aa", ["user:aa", "screen:aa"])).toBe("screen:aa");
  });

  it("auto-maximizes a live screen until the user pins the grid", () => {
    expect(pickCallFocus(null, false, ["user:aa", "screen:aa"], ["screen:aa"])).toBe("screen:aa");
    expect(pickCallFocus(null, true, ["user:aa", "screen:aa"], ["screen:aa"])).toBe(null);
    expect(pickCallFocus("user:aa", true, ["user:aa", "screen:aa"], ["screen:aa"])).toBe("user:aa");
    expect(pickCallFocus("user:aa", false, ["user:aa", "screen:aa"], ["screen:aa"])).toBe(
      "screen:aa",
    );
  });

  it("prefers a live screen, then a single live camera", () => {
    expect(
      autoCallFocusIds([
        { id: "user:aa", screen: false, live: true },
        { id: "screen:aa", screen: true, live: true },
      ]),
    ).toEqual(["screen:aa"]);
    expect(
      autoCallFocusIds([
        { id: "user:aa", screen: false, live: false },
        { id: "user:bb", screen: false, live: true },
      ]),
    ).toEqual(["user:bb"]);
    expect(
      autoCallFocusIds([
        { id: "user:aa", screen: false, live: true },
        { id: "user:bb", screen: false, live: true },
      ]),
    ).toEqual([]);
  });

  it("grows the grid with people and screen cards together", () => {
    expect(callTileGridCols(2)).toBe(2);
    expect(callTileGridCols(5)).toBe(3);
  });

  it("detects WebKitGTK without treating Chromium as WebKit", () => {
    expect(
      isWebKitEngine(
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
      ),
    ).toBe(true);
    expect(
      isWebKitEngine(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
      ),
    ).toBe(false);
  });

  it("drops ICE URLs that WebKitGTK rejects", () => {
    expect(webkitSafeIceUrl("stun:stun.l.google.com:19302")).toBe(true);
    expect(webkitSafeIceUrl("turn:openrelay.metered.ca:80")).toBe(true);
    expect(webkitSafeIceUrl("turns:openrelay.metered.ca:443?transport=tcp")).toBe(false);
    expect(webkitSafeIceUrl("turn:openrelay.metered.ca:80?transport=tcp")).toBe(false);
    const servers = rtcIceServersFor(
      [
        { urls: ["stun:stun.l.google.com:19302", "stun:stun1.l.google.com:19302"] },
        {
          urls: [
            "turn:openrelay.metered.ca:80",
            "turns:openrelay.metered.ca:443?transport=tcp",
          ],
          username: "a",
          credential: "b",
        },
      ],
      true,
    );
    expect(servers[0].urls).toEqual(["stun:stun.l.google.com:19302", "stun:stun1.l.google.com:19302"]);
    expect(servers[1].urls).toBe("turn:openrelay.metered.ca:80");
  });

  it("falls back RTC configs until one constructor works", () => {
    const tries: RTCConfiguration[] = [];
    const Ctor = class {
      constructor(config?: RTCConfiguration) {
        tries.push(config ?? {});
        if (tries.length < 2) throw new Error("Invalid ICE server URL: turns:");
      }
    } as unknown as new (config?: RTCConfiguration) => RTCPeerConnection;
    const made = createRtcPeerConnection(
      Ctor,
      rtcPeerConfigs(
        [{ urls: "stun:stun.l.google.com:19302" }, { urls: "turns:bad" }],
        true,
      ),
    );
    expect(made.ok).toBe(true);
    expect(tries.length).toBeGreaterThan(1);
    const missing = createRtcPeerConnection(undefined, [{}]);
    expect(missing).toEqual({ ok: false, error: "RTCPeerConnection missing from webview" });
  });
});

describe("presence", () => {
  it("turns idle online into away, but keeps busy", () => {
    expect(effectivePresence("online", true)).toBe("away");
    expect(effectivePresence("busy", true)).toBe("busy");
    expect(effectivePresence("away", false)).toBe("away");
  });

  it("groups members and labels presence", () => {
    const groups = groupByPresence([
      { status: "online" as const },
      { status: "offline" as const },
      { status: "busy" as const },
      { status: "away" as const },
    ]);
    expect(groups.online).toHaveLength(1);
    expect(groups.offline).toHaveLength(1);
    expect(presenceLabel("away")).toBe("Away");
    expect(presenceLabel("busy")).toBe("Busy");
    expect(presenceLabel("online", true)).toBe("In voice");
  });
});

describe("call negotiation", () => {
  it("makes exactly one peer the polite one", () => {
    expect(rtcPolite("aa", "bb")).toBe(false);
    expect(rtcPolite("bb", "aa")).toBe(true);
  });

  it("does not treat a muted mic as a broken send path", () => {
    expect(shouldHealSend(4000, 40, false)).toBe(false);
    expect(shouldHealSend(4000, 40, true)).toBe(true);
    expect(shouldHealSend(4000, 4000, true)).toBe(false);
  });

  it("refuses to send an answer labeled as an offer", () => {
    expect(canPublishLocalSdp("offer", "offer")).toBe(true);
    expect(canPublishLocalSdp("offer", "answer")).toBe(false);
    expect(canPublishLocalSdp("answer", "offer")).toBe(false);
  });

  it("resends the local answer when the same offer arrives again", () => {
    expect(shouldResendCallAnswer("v=0", "v=0", "answer")).toBe(true);
    expect(shouldResendCallAnswer("v=0", "v=1", "answer")).toBe(false);
    expect(shouldResendCallAnswer("v=0", "v=0", "offer")).toBe(false);
    expect(shouldResendCallAnswer(undefined, "v=0", "answer")).toBe(false);
  });

  it("rejects answers whose m-line order does not match the local offer", () => {
    const offer3 = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\nm=video 9 UDP/TLS/RTP/SAVPF 97\r\n";
    const answer3 = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\nm=video 9 UDP/TLS/RTP/SAVPF 97\r\n";
    const answer6 =
      answer3 +
      "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\nm=video 9 UDP/TLS/RTP/SAVPF 97\r\n";
    const swapped = "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n";
    expect(sdpMediaLineKinds(offer3)).toEqual(["audio", "video", "video"]);
    expect(answerMatchesLocalOffer(offer3, answer3)).toBe(true);
    expect(answerMatchesLocalOffer(offer3, answer6)).toBe(false);
    expect(answerMatchesLocalOffer(offer3, swapped)).toBe(false);
    expect(answerMatchesLocalOffer(undefined, answer3)).toBe(false);
  });

  it("labels 1:1 vs group mesh until a hub exists", () => {
    expect(callPathMode(0)).toBe("1:1");
    expect(callPathMode(1)).toBe("1:1");
    expect(callPathMode(2)).toBe("mesh");
    expect(callPathMode(2, true)).toBe("HUB:N");
    expect(callPathHint("mesh")).toContain("HUB:N");
    expect(callPathHint("HUB:N")).toContain("elected hub");
    expect(iceTraceText("disconnected")).toBe("ICE disconnected");
    expect(iceTraceLevel("failed")).toBe("err");
  });

  it("elects a hub only with 3+ people, avoiding the community owner when possible", () => {
    expect(electCallHub(["aa", "bb"])).toBe(null);
    expect(electCallHub(["cc", "aa", "bb"])).toBe("aa");
    expect(electCallHub(["cc", "aa", "bb"], "aa")).toBe("bb");
    expect(electCallHub(["cc", "aa", "bb"], "cc")).toBe("aa");
    expect(electCallHub(["ff", "ee", "dd"], "ff")).toBe("dd");
  });

  it("hides a call tile only when the viewer opted out", () => {
    expect(wantsCallMedia([], "user:aa")).toBe(true);
    expect(wantsCallMedia(["user:aa"], "user:aa")).toBe(false);
    expect(wantsCallMedia(["screen:aa"], "user:aa")).toBe(true);
  });

  it("flags a voice room when anyone is sharing a screen", () => {
    expect(voiceRoomSharing(["me", "aa"], { aa: { sharingScreen: true } }, "me", false)).toBe(true);
    expect(voiceRoomSharing(["me"], {}, "me", true)).toBe(true);
    expect(voiceRoomSharing(["me", "aa"], { aa: { sharingScreen: false } }, "me", false)).toBe(
      false,
    );
  });

  it("keeps myCall on the preferred room when seated in more than one", () => {
    const voice = { sala: ["me"], currall: ["me", "aa"] };
    expect(pickMyCall(voice, "me", "currall")).toBe("currall");
    expect(pickMyCall(voice, "me", "")).toBe("currall");
    expect(pickMyCall(voice, "zz")).toBe(null);
  });

  it("holds the hub briefly when the roster dips to two", () => {
    expect(holdCallHub("aa", "aa", 3, 0, 1000).hub).toBe("aa");
    const dipped = holdCallHub("aa", null, 2, 0, 1000, 2000);
    expect(dipped.hub).toBe("aa");
    expect(dipped.holdUntil).toBe(3000);
    expect(holdCallHub("aa", null, 2, 3000, 2500, 2000).hub).toBe("aa");
    expect(holdCallHub("aa", null, 2, 3000, 4000, 2000).hub).toBe(null);
    expect(holdCallHub("aa", null, 1, 0, 1000).hub).toBe(null);
    expect(holdCallHub(null, "aa", 2, 0, 1000).hub).toBe(null);
  });

  it("ignores a bye from the previous call session", () => {
    expect(ignoreStaleCallBye(100, 200)).toBe(true);
    expect(ignoreStaleCallBye(200, 200)).toBe(false);
    expect(ignoreStaleCallBye(300, 200)).toBe(false);
    expect(ignoreStaleCallBye(undefined, 200)).toBe(false);
    expect(ignoreStaleCallSess("old", "new")).toBe(true);
    expect(ignoreStaleCallSess("new", "new")).toBe(false);
    expect(ignoreStaleCallSess(undefined, "new")).toBe(false);
    expect(callWantedMLines(0)).toBe(3);
    expect(callWantedMLines(1)).toBe(6);
  });

  it("stars leaves through the hub and keeps the hub linked to everyone", () => {
    const roster = ["aa", "bb", "cc"];
    expect(callWantedPeers("aa", roster, "aa")).toEqual(["bb", "cc"]);
    expect(callWantedPeers("bb", roster, "aa")).toEqual(["aa"]);
    expect(callWantedPeers("aa", ["aa", "bb"], null)).toEqual(["bb"]);
    expect(callForwardPeers("aa", "bb", roster)).toEqual(["cc"]);
    expect(callForwardPeers("aa", "aa", roster)).toEqual(["bb", "cc"]);
  });

  it("maps extra transceivers to the forwarded peer, not the hub", () => {
    expect(callSlotPeer(1, "hub", ["cc"])).toBe("hub");
    expect(callSlotPeer(4, "hub", ["cc"])).toBe("cc");
    expect(callSlotScreen("video", 1)).toBe(false);
    expect(callSlotScreen("video", 2)).toBe(true);
    expect(callSlotScreen("video", 4)).toBe(false);
    expect(callSlotScreen("video", 5)).toBe(true);
    expect(callSlotScreen("audio", 2)).toBe(false);
    expect(callPrimaryCount(1, 6)).toBe(3);
    expect(callPrimaryCount(0, 6)).toBe(6);
    expect(callTrackGone({ readyState: "live", enabled: true })).toBe(false);
    expect(callTrackGone({ readyState: "live", enabled: false })).toBe(true);
    expect(callTrackGone({ readyState: "ended", enabled: true })).toBe(true);
  });

  it("keeps audio off the screen transceiver", () => {
    const pick = pickCallTransceivers([
      { mid: "0", kind: "audio" },
      { mid: "1", kind: "video" },
      { mid: "2", kind: "video" },
    ]);
    expect(pick.audio).toBe(0);
    expect(pick.cam).toBe(1);
    expect(pick.screen).toBe(2);
    const guessed = pickCallTransceivers([{ mid: "0" }, { mid: "1" }, { mid: "2" }]);
    expect(guessed.audio).toBe(0);
    expect(guessed.screen).not.toBe(guessed.audio);
    const reordered = pickCallTransceivers([{ mid: "2" }, { mid: "0" }, { mid: "1" }]);
    expect(reordered.audio).toBe(1);
    expect(reordered.cam).toBe(2);
    expect(reordered.screen).toBe(0);
  });

  it("does not treat the camera transceiver as a screen share", () => {
    const list = [
      { mid: "0", kind: "audio" },
      { mid: "1", kind: "video" },
      { mid: "2", kind: "video" },
    ];
    expect(callTrackIsScreen("audio", 0, list)).toBe(false);
    expect(callTrackIsScreen("video", 1, list)).toBe(false);
    expect(callTrackIsScreen("video", 2, list)).toBe(true);
    expect(callTrackIsScreen("video", 1, list, 2)).toBe(false);
    expect(callTrackIsScreen("video", 1, [{ mid: "1", kind: "video" }])).toBe(false);
  });

  it("puts H264 first so Linux can decode screen share", () => {
    const ranked = preferH264Codecs([
      { mimeType: "video/VP8" },
      { mimeType: "video/H264" },
      { mimeType: "video/rtx" },
    ]);
    expect(ranked[0].mimeType).toBe("video/H264");
    expect(ranked.map((c) => c.mimeType)).toContain("video/VP8");
  });

  it("recognizes compact and wrapped invites", () => {
    expect(looksLikeInvite("cc/" + "A".repeat(24))).toBe(true);
    expect(looksLikeInvite("chaincord:" + "B".repeat(24))).toBe(true);
    expect(looksLikeInvite("cc/abc")).toBe(false);
    expect(looksLikeInvite("oi tudo bem")).toBe(false);
    expect(looksLikeInvite("c".repeat(40))).toBe(true);
    const msg = inviteShareText("Equipe", "cc/" + "A".repeat(24));
    expect(looksLikeInvite(msg)).toBe(true);
    expect(extractInvite(msg).startsWith("cc/")).toBe(true);
  });

  it("builds a paste-ready invite message", () => {
    const text = inviteShareText("  Equipe  ", "cc/abc");
    expect(text).toContain("Equipe");
    expect(text).toContain("cc/abc");
    expect(text).toContain("Join with invite");
  });
});
