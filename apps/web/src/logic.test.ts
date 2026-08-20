import { describe, expect, it } from "vitest";
import {
  callTileGridCols,
  effectivePresence,
  fmtBytes,
  gradeLink,
  groupByPresence,
  isChatLine,
  isWebKitEngine,
  keepCallFocus,
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
  callPathMode,
  callPathHint,
  iceTraceText,
  iceTraceLevel,
  pickCallTransceivers,
  toggleCallFocus,
  trackLooksLive,
  remoteScreenVisible,
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
    expect(trackLooksLive(null)).toBe(false);
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
});

describe("fmtBytes", () => {
  it("formats seeding progress", () => {
    expect(fmtBytes(512)).toBe("512 B");
    expect(fmtBytes(1536)).toBe("1.5 KB");
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
    expect(missing).toEqual({ ok: false, error: "RTCPeerConnection ausente no webview" });
  });
});

describe("presence", () => {
  it("turns idle online into away, but keeps busy", () => {
    expect(effectivePresence("online", true)).toBe("away");
    expect(effectivePresence("busy", true)).toBe("busy");
    expect(effectivePresence("away", false)).toBe("away");
  });

  it("groups members and labels in Portuguese", () => {
    const groups = groupByPresence([
      { status: "online" as const },
      { status: "offline" as const },
      { status: "busy" as const },
      { status: "away" as const },
    ]);
    expect(groups.online).toHaveLength(1);
    expect(groups.offline).toHaveLength(1);
    expect(presenceLabel("away")).toBe("Ausente");
    expect(presenceLabel("busy")).toBe("Ocupado");
    expect(presenceLabel("online", true)).toBe("Em voz");
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

  it("labels 1:1 vs group mesh until a hub exists", () => {
    expect(callPathMode(0)).toBe("1:1");
    expect(callPathMode(1)).toBe("1:1");
    expect(callPathMode(2)).toBe("mesh");
    expect(callPathMode(2, true)).toBe("HUB:N");
    expect(callPathHint("mesh")).toContain("HUB:N");
    expect(iceTraceText("disconnected")).toBe("ICE caiu");
    expect(iceTraceLevel("failed")).toBe("err");
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
});
