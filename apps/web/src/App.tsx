import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import {
  backendAddRoom,
  backendChat,
  backendCreate,
  backendJoin,
  backendJoinCall,
  backendLeave,
  backendLeaveCall,
  backendHistory,
  backendPresence,
  backendSaveProfile,
  backendSendRtc,
  backendSubscribe,
  backendSwitchCommunity,
  rtcPeerConnectionMissing,
  type RtcFrame,
  type UiState,
} from "./backend";
import { CallNet, lastNativeFrame, type CallLink, type RemoteMedia } from "./call";
import {
  callPathHint,
  electCallHub,
  pickMyCall,
  effectivePresence,
  fmtBytes,
  gradeLink,
  groupByPresence,
  isChatLine,
  isWebKitEngine,
  inviteShareText,
  extractInvite,
  looksLikeInvite,
  pickCallFocus,
  autoCallFocusIds,
  toggleCallFocus,
  callTileGridCols,
  normalizePresence,
  PRESENCE_GROUPS,
  presenceLabel,
  remoteScreenPresent,
  remoteScreenVisible,
  remoteVideoVisible,
  screenTileId,
  seedPercent,
  userTileId,
  wantsCallMedia,
  voiceRoomSharing,
  audioInputConstraint,
  readAudioPref,
  writeAudioPref,
  type Presence,
} from "./logic";
import { AudioSettings } from "./audio";
import { ProfileEditor, UserPanel } from "./profile";
import { playCallJoin, playCallLeave, setSoundSink, unlockSounds } from "./sounds";

type Line =
  | { kind: "chat"; sender: string; text: string; ts: number; self: boolean; channel: string }
  | { kind: "info"; text: string }
  | { kind: "error"; text: string };

type AddMode = "create" | "join";
type Selection = { kind: "text"; name: string } | { kind: "call"; name: string };
type RoomKind = "text" | "call";

function fullscreenRoot(): Element | null {
  const doc = document as Document & { webkitFullscreenElement?: Element | null };
  return document.fullscreenElement ?? doc.webkitFullscreenElement ?? null;
}

function prefStore() {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function requestStageFullscreen(el: HTMLElement | null) {
  if (!el || fullscreenRoot()) return;
  try {
    const webkit = el as HTMLElement & { webkitRequestFullscreen?: () => Promise<void> | void };
    const req = el.requestFullscreen?.bind(el) ?? webkit.webkitRequestFullscreen?.bind(el);
    void Promise.resolve(req?.()).catch(() => undefined);
  } catch {
    /* WebView can reject without a user gesture */
  }
}

function exitStageFullscreen() {
  if (!fullscreenRoot()) return;
  const doc = document as Document & { webkitExitFullscreen?: () => Promise<void> | void };
  const exit = document.exitFullscreen?.bind(document) ?? doc.webkitExitFullscreen?.bind(document);
  void Promise.resolve(exit?.()).catch(() => undefined);
}

export function App() {
  const [state, setState] = useState<UiState | null>(null);
  const [lines, setLines] = useState<Line[]>([]);
  const [name, setName] = useState("");
  const [invite, setInvite] = useState("");
  const [draft, setDraft] = useState("");
  const [copied, setCopied] = useState<"code" | "message" | null>(null);
  const [addOpen, setAddOpen] = useState(false);
  const [addMode, setAddMode] = useState<AddMode>("create");
  const [addError, setAddError] = useState<string | null>(null);
  const [addBusy, setAddBusy] = useState(false);
  const [shareOpen, setShareOpen] = useState(false);
  const [autoCopyInvite, setAutoCopyInvite] = useState(false);
  const [leaveOpen, setLeaveOpen] = useState(false);
  const [selected, setSelected] = useState<Selection>({ kind: "text", name: "general" });
  const [newRoomOpen, setNewRoomOpen] = useState<RoomKind | null>(null);
  const [newRoomName, setNewRoomName] = useState("");
  const [camOn, setCamOn] = useState(false);
  const [screenOn, setScreenOn] = useState(false);
  const [menu, setMenu] = useState<null | { kind: "head" } | { kind: "ctx"; x: number; y: number }>(
    null,
  );
  const [callNotice, setCallNotice] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<"profile" | "audio">("profile");
  const [audioInputId, setAudioInputId] = useState(() => readAudioPref("input", prefStore()));
  const [audioOutputId, setAudioOutputId] = useState(() => readAudioPref("output", prefStore()));
  const [micEpoch, setMicEpoch] = useState(0);
  const [micMuted, setMicMuted] = useState(false);
  const [deafened, setDeafened] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const camVideo = useRef<HTMLVideoElement>(null);
  const screenVideo = useRef<HTMLVideoElement>(null);
  const camStream = useRef<MediaStream | null>(null);
  const screenStream = useRef<MediaStream | null>(null);
  const callRef = useRef<CallNet | null>(null);
  const callRoomRef = useRef<string | null>(null);
  const stickyCallRef = useRef<string | null>(null);
  const rtcBuf = useRef<RtcFrame[]>([]);
  const [remotes, setRemotes] = useState<RemoteMedia[]>([]);
  const [callLink, setCallLink] = useState<CallLink | null>(null);
  const [callTip, setCallTip] = useState(false);
  const [focusedTile, setFocusedTile] = useState<string | null>(null);
  const [focusPinned, setFocusPinned] = useState(false);
  const [theaterOpen, setTheaterOpen] = useState(false);
  const [hiddenMedia, setHiddenMedia] = useState<string[]>([]);
  const stageRef = useRef<HTMLDivElement>(null);
  const shownFocusRef = useRef<string | null>(null);
  const theaterRef = useRef(false);
  const [joiningRoom, setJoiningRoom] = useState<string | null>(null);
  const [myPresence, setMyPresence] = useState<Exclude<Presence, "offline">>("online");
  const [idle, setIdle] = useState(false);
  const shownPresence = effectivePresence(myPresence, idle);

  const communityRef = useRef("");
  const viewedRef = useRef("");

  useEffect(() => {
    return backendSubscribe({
      onState: (next) => {
        communityRef.current = next.communityId;
        setState(next);
        if (!next.communityId) {
          setSelected({ kind: "text", name: "general" });
        }
      },
      onMessage: (msg) => {
        if (msg.communityId && communityRef.current && msg.communityId !== communityRef.current) {
          return;
        }
        setLines((prev) => [
          ...prev,
          {
            kind: "chat",
            sender: msg.sender,
            text: msg.text,
            ts: msg.ts,
            self: msg.self,
            channel: msg.channel || "general",
          },
        ]);
      },
      onInfo: () => {},
      onError: () => {},
      onRtc: (frame) => {
        const net = callRef.current;
        if (net) {
          void net.handle(frame);
          return;
        }
        rtcBuf.current.push(frame);
        if (rtcBuf.current.length > 80) rtcBuf.current = rtcBuf.current.slice(-80);
      },
    });
  }, []);

  useEffect(() => {
    if (!state?.communityId) {
      setLines([]);
      setSelected({ kind: "text", name: "general" });
      viewedRef.current = "";
      return;
    }
    const id = state.communityId;
    if (viewedRef.current !== id) {
      viewedRef.current = id;
      setSelected({ kind: "text", name: "general" });
    }
    void backendHistory().then((msgs) => {
      if (communityRef.current !== id) return;
      const fromDb = msgs.map((msg) => ({
        kind: "chat" as const,
        sender: msg.sender,
        text: msg.text,
        ts: msg.ts,
        self: msg.self,
        channel: msg.channel || "general",
      }));
      setLines(fromDb);
    });
  }, [state?.communityId, state?.archiveMessages]);

  useEffect(() => {
    scroller.current?.scrollTo({ top: scroller.current.scrollHeight });
  }, [lines, selected]);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [menu]);

  useEffect(() => {
    if (myPresence !== "online") {
      setIdle(false);
      return;
    }
    const arm = () => window.setTimeout(() => setIdle(true), 5 * 60 * 1000);
    let timer = arm();
    const bump = () => {
      setIdle(false);
      window.clearTimeout(timer);
      timer = arm();
    };
    window.addEventListener("pointerdown", bump);
    window.addEventListener("keydown", bump);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("pointerdown", bump);
      window.removeEventListener("keydown", bump);
    };
  }, [myPresence]);

  useEffect(() => {
    if (!state?.communityId) return;
    void backendPresence(micMuted, deafened, shownPresence);
  }, [state?.communityId, micMuted, deafened, shownPresence]);

  useEffect(() => {
    void setSoundSink(audioOutputId);
  }, [audioOutputId]);

  const stopMedia = () => {
    camStream.current?.getTracks().forEach((t) => t.stop());
    screenStream.current?.getTracks().forEach((t) => t.stop());
    camStream.current = null;
    screenStream.current = null;
    setCamOn(false);
    setScreenOn(false);
    setFocusedTile(null);
    setFocusPinned(false);
    setTheaterOpen(false);
    exitStageFullscreen();
    setCallNotice(null);
  };

  const meName = state?.displayName?.trim() || "you";

  const saveProfile = async (displayName: string, avatar: string) => {
    await backendSaveProfile(displayName, avatar);
    setSettingsOpen(false);
  };

  const toggleMic = () => {
    const next = !micMuted;
    setMicMuted(next);
    camStream.current?.getAudioTracks().forEach((track) => {
      track.enabled = !next;
    });
    void backendPresence(next, deafened, shownPresence);
  };

  const toggleDeafen = () => {
    const next = !deafened;
    setDeafened(next);
    const mute = next || micMuted;
    if (next) {
      setMicMuted(true);
      camStream.current?.getAudioTracks().forEach((track) => {
        track.enabled = false;
      });
    }
    void backendPresence(mute, next, shownPresence);
  };

  const getMicStream = async (deviceId: string) => {
    try {
      return await navigator.mediaDevices.getUserMedia({
        audio: audioInputConstraint(deviceId),
      });
    } catch (err) {
      if (!deviceId) throw err;
      setAudioInputId("");
      writeAudioPref("input", "", prefStore());
      return await navigator.mediaDevices.getUserMedia({
        audio: audioInputConstraint(""),
      });
    }
  };

  const applyMicDevice = async (deviceId: string) => {
    setAudioInputId(deviceId);
    writeAudioPref("input", deviceId, prefStore());
    if (!myCall && !camStream.current) return;
    const incoming = await getMicStream(deviceId);
    const track = incoming.getAudioTracks()[0];
    if (!track) throw new Error("no audio");
    const mixed = camStream.current ?? new MediaStream();
    mixed.getAudioTracks().forEach((old) => {
      old.stop();
      mixed.removeTrack(old);
    });
    track.enabled = !micMuted;
    mixed.addTrack(track);
    camStream.current = mixed;
    callRef.current?.setCamera(mixed);
    setMicEpoch((n) => n + 1);
  };

  const applySpeaker = (deviceId: string) => {
    setAudioOutputId(deviceId);
    writeAudioPref("output", deviceId, prefStore());
    void setSoundSink(deviceId);
  };

  const openSettings = (tab: "profile" | "audio" = "profile") => {
    setSettingsTab(tab);
    setSettingsOpen(true);
  };

  const short = (k?: string) => (k ? k.slice(0, 8) : "…");
  const inCommunity = Boolean(state?.communityId);
  const otherMembers = Object.keys(state?.profiles ?? {}).filter(
    (pk) => pk && pk !== state?.publicKey,
  ).length;
  const liveLinks = (state?.peers ?? []).filter(
    (url) => url && url !== "relay" && !url.startsWith("relay:") && !url.startsWith("pending:"),
  ).length;
  const otherPeers = Math.max(liveLinks, otherMembers);
  const peerCount = otherPeers + (inCommunity ? 1 : 0);
  const alone = inCommunity && otherPeers === 0;
  const textChannels = state?.textChannels?.length ? state.textChannels : inCommunity ? ["general"] : [];
  const callRooms = [...new Set(state?.callRooms ?? [])];
  const voice = state?.liveCall?.communityId === state?.communityId
    ? (state?.liveCall?.voice ?? state?.voice ?? {})
    : (state?.voice ?? {});
  const myCallRaw = state?.liveCall?.room ?? pickMyCall(
    voice,
    state?.publicKey ?? "",
    selected.kind === "call" ? selected.name : "",
  );
  if (myCallRaw) stickyCallRef.current = myCallRaw;
  const myCall = myCallRaw;
  const callVoice = state?.liveCall?.voice ?? voice;
  const callOwner = state?.liveCall?.ownerKey ?? state?.ownerKey ?? "";
  const callCommunityName = state?.liveCall?.communityName ?? state?.communityName ?? "";
  const callRoster = (myCall ? (callVoice[myCall] ?? []) : []).slice().sort().join(",");
  const callRosterRef = useRef<string | null>(null);

  useEffect(() => {
    const prev = callRosterRef.current;
    if (!myCall) {
      if (prev) playCallLeave();
      callRosterRef.current = null;
      return;
    }
    callRosterRef.current = callRoster;
    if (prev === null) {
      playCallJoin();
      return;
    }
    if (prev === callRoster) return;
    const before = new Set(prev.split(",").filter(Boolean));
    const after = new Set(callRoster.split(",").filter(Boolean));
    let joined = false;
    let left = false;
    for (const pk of after) if (!before.has(pk)) joined = true;
    for (const pk of before) if (!after.has(pk)) left = true;
    if (joined) playCallJoin();
    if (left) playCallLeave();
  }, [myCall, callRoster]);

  const members = useMemo(() => {
    if (!inCommunity || !state) return [];
    const seen = new Set<string>();
    const list: string[] = [];
    const add = (pk: string) => {
      if (!pk || seen.has(pk)) return;
      seen.add(pk);
      list.push(pk);
    };
    add(state.publicKey);
    for (const pk of Object.keys(state.profiles ?? {})) add(pk);
    for (const people of Object.values(state.voice ?? {})) {
      for (const pk of people) add(pk);
    }
    return list;
  }, [inCommunity, state]);

  const channelLines = lines.filter(
    (line) => isChatLine(line.kind) && selected.kind === "text" && line.channel === selected.name,
  );

  const peopleInSelectedCall = [
    ...new Set(
      selected.kind === "call" ? (voice[selected.name] ?? []) : myCall ? (voice[myCall] ?? []) : [],
    ),
  ];
  const screenBlocked = false;
  const profiles = state?.profiles ?? {};
  const seatedInVoice = (pk: string) =>
    Object.values(voice).some((people) => people.includes(pk));
  const faceOf = (pk: string) => {
    if (pk === state?.publicKey) {
      return {
        name: meName,
        avatar: state.avatar,
        muted: micMuted,
        deafened,
        sharingScreen: screenOn,
        status: shownPresence,
      };
    }
    const p = profiles[pk];
    return {
      name: p?.displayName?.trim() || short(pk),
      avatar: p?.avatar || "",
      muted: Boolean(p?.muted),
      deafened: Boolean(p?.deafened),
      sharingScreen: Boolean(p?.sharingScreen) && seatedInVoice(pk),
      status: normalizePresence(p?.status),
    };
  };
  const groupedMembers = groupByPresence(
    members.map((pk) => ({
      pk,
      status: pk === state?.publicKey ? shownPresence : normalizePresence(profiles[pk]?.status),
    })),
  );

  useEffect(() => {
    return () => {
      callRef.current?.stop(true);
      callRef.current = null;
      callRoomRef.current = null;
    };
  }, []);

  useEffect(() => {
    const me = state?.publicKey;
    if (!me) return;

    if (!myCall) {
      const leaveTimer = window.setTimeout(() => {
        if (callRef.current) {
          callRef.current.stop(true);
          callRef.current = null;
          callRoomRef.current = null;
          stickyCallRef.current = null;
          rtcBuf.current = [];
          setRemotes([]);
          setCallLink(null);
          setCallTip(false);
          setFocusedTile(null);
          setFocusPinned(false);
          setTheaterOpen(false);
          exitStageFullscreen();
          setHiddenMedia([]);
        }
      }, 4000);
      return () => window.clearTimeout(leaveTimer);
    }

    const room = myCall;
    if (
      callRef.current &&
      callRoomRef.current &&
      callRoomRef.current.toLowerCase() === room.toLowerCase()
    ) {
      return;
    }

    if (callRef.current) {
      callRef.current.stop(true);
      callRef.current = null;
    }

    const net = new CallNet(
      me,
      room,
      (frame) => {
        void backendSendRtc(frame).catch((err) => {
          setCallNotice(`Couldn't send call signal: ${String(err)}`);
        });
      },
      (media) => {
        setRemotes((prev) => {
          const rest = prev.filter((x) => !(x.peer === media.peer && x.screen === media.screen));
          if (media.screen && !remoteScreenPresent(media)) return rest;
          return [...rest, media];
        });
      },
      (peer) => setRemotes((prev) => prev.filter((x) => x.peer !== peer)),
      setCallLink,
      (line) => {
        void line;
      },
    );
    callRef.current = net;
    callRoomRef.current = room;
    net.setCamera(camStream.current);
    net.setScreen(screenStream.current);
    const others = (callVoice[room] ?? []).filter((pk) => pk !== me);
    net.sync(others, electCallHub([me, ...others], callOwner));
    const pending = rtcBuf.current;
    rtcBuf.current = [];
    for (const frame of pending) void net.handle(frame);
  }, [myCall, state?.publicKey]);

  useEffect(() => {
    if (!myCall || !state?.publicKey) return;
    const others = (callVoice[myCall] ?? []).filter((pk) => pk !== state.publicKey);
    callRef.current?.sync(others, electCallHub([state.publicKey, ...others], callOwner));
  }, [callVoice, myCall, state?.publicKey, callOwner]);

  const copyInvite = async (kind: "code" | "message" = "message") => {
    if (!state?.invite) return;
    const text =
      kind === "message"
        ? inviteShareText(state.communityName, state.invite)
        : state.invite;
    await navigator.clipboard.writeText(text);
    setCopied(kind);
    window.setTimeout(() => setCopied(null), 1800);
    setMenu(null);
  };

  useEffect(() => {
    if (!autoCopyInvite || !shareOpen || !state?.invite) return;
    setAutoCopyInvite(false);
    void copyInvite("message");
  }, [autoCopyInvite, shareOpen, state?.invite, state?.communityName]);

  const pasteInvite = async () => {
    try {
      const text = (await navigator.clipboard.readText()).trim();
      if (text) setInvite(text);
    } catch {
      setAddError("Couldn't read the clipboard. Paste with Ctrl+V.");
    }
  };

  const openAdd = (mode: AddMode) => {
    setAddMode(mode);
    setAddError(null);
    setAddBusy(false);
    setName("");
    setInvite("");
    setAddOpen(true);
    if (mode === "join") {
      void navigator.clipboard
        .readText()
        .then((text) => {
          if (looksLikeInvite(text)) setInvite(text.trim());
        })
        .catch(() => undefined);
    }
  };

  const openLeave = () => {
    setMenu(null);
    setLeaveOpen(true);
  };

  const guildMenu = (
    <div
      className={menu?.kind === "ctx" ? "guild-menu floating" : "guild-menu"}
      style={menu?.kind === "ctx" ? { top: menu.y, left: menu.x } : undefined}
      onClick={(e) => e.stopPropagation()}
    >
      <button type="button" onClick={() => void copyInvite("message")}>
        {copied ? "Invite copied" : "Copy invite"}
      </button>
      <button
        type="button"
        onClick={() => {
          setMenu(null);
          setShareOpen(true);
        }}
      >
        Show invite
      </button>
      <button type="button" className="danger" onClick={openLeave}>
        Leave community
      </button>
    </div>
  );

  const onCreate = async (e: FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed || addBusy) return;
    setAddBusy(true);
    setAddError(null);
    try {
      await backendCreate(trimmed);
      setLines([]);
      setSelected({ kind: "text", name: "general" });
      setAddOpen(false);
      setName("");
      setAutoCopyInvite(true);
      setShareOpen(true);
      stopMedia();
    } catch (err) {
      setAddError(String(err));
    } finally {
      setAddBusy(false);
    }
  };

  const onJoin = async (e: FormEvent) => {
    e.preventDefault();
    const code = extractInvite(invite);
    if (!code || addBusy) return;
    setAddBusy(true);
    setAddError(null);
    try {
      await backendJoin(code);
      setLines([]);
      setSelected({ kind: "text", name: "general" });
      setAddOpen(false);
      setInvite("");
      stopMedia();
    } catch (err) {
      setAddError(String(err).replace(/^Error:\s*/i, "") || "Invalid invite.");
    } finally {
      setAddBusy(false);
    }
  };

  const onLeave = async () => {
    try {
      stopMedia();
      await backendLeave();
      setLines([]);
      setLeaveOpen(false);
      setDraft("");
      setSelected({ kind: "text", name: "general" });
    } catch (err) {
      setLines((prev) => [...prev, { kind: "error", text: String(err) }]);
    }
  };

  const onChat = async (e: FormEvent) => {
    e.preventDefault();
    if (selected.kind !== "text") return;
    try {
      await backendChat(draft, selected.name);
      setDraft("");
    } catch (err) {
      setLines((prev) => [...prev, { kind: "error", text: String(err) }]);
    }
  };

  const onAddRoom = async (e: FormEvent) => {
    e.preventDefault();
    if (!newRoomOpen) return;
    try {
      await backendAddRoom(newRoomOpen, newRoomName);
      const slug = newRoomName.trim().toLowerCase().replace(/\s+/g, "-");
      setSelected({ kind: newRoomOpen, name: slug });
      setNewRoomName("");
      setNewRoomOpen(null);
    } catch (err) {
      setLines((prev) => [...prev, { kind: "error", text: String(err) }]);
    }
  };

  const enterCall = async (room: string) => {
    unlockSounds();
    setSelected({ kind: "call", name: room });
    setJoiningRoom(room);
    setCallNotice(null);
    try {
      await backendJoinCall(room);
      void backendPresence(micMuted, deafened, shownPresence);
      const attachMic = (stream: MediaStream) => {
        camStream.current = stream;
        const tryAttach = (n = 0) => {
          if (callRef.current) {
            callRef.current.setCamera(stream);
            return;
          }
          if (n < 30) window.setTimeout(() => tryAttach(n + 1), 50);
        };
        tryAttach();
      };
      if (!camStream.current) {
        try {
          const stream = await getMicStream(audioInputId);
          stream.getAudioTracks().forEach((track) => {
            track.enabled = !micMuted;
          });
          attachMic(stream);
          setMicEpoch((n) => n + 1);
        } catch {
          setCallNotice("No microphone on this device. The room still opens; you can turn the camera on.");
        }
      } else {
        attachMic(camStream.current);
      }
    } catch (err) {
      setCallNotice(String(err));
      setSelected({ kind: "text", name: "general" });
    } finally {
      setJoiningRoom(null);
    }
  };

  const hangUp = async () => {
    stopMedia();
    callRef.current?.stop(false);
    callRef.current = null;
    callRoomRef.current = null;
    rtcBuf.current = [];
    setJoiningRoom(null);
    setSelected({ kind: "text", name: "general" });
    setCamOn(false);
    setScreenOn(false);
    setRemotes([]);
    setCallLink(null);
    setFocusedTile(null);
    setFocusPinned(false);
    setTheaterOpen(false);
    exitStageFullscreen();
    setHiddenMedia([]);
    try {
      await backendLeaveCall();
    } catch (err) {
      setCallNotice(String(err));
      window.setTimeout(() => setCallNotice(null), 4000);
    }
  };

  useEffect(() => {
    if (selected.kind !== "call") return;
    if (joiningRoom === selected.name) return;
    if (myCall === selected.name) return;
    if (myCall) return;
    setSelected({ kind: "text", name: "general" });
    stopMedia();
    setCamOn(false);
    setScreenOn(false);
    setRemotes([]);
    setCallLink(null);
    setFocusedTile(null);
    setFocusPinned(false);
    setTheaterOpen(false);
    exitStageFullscreen();
    setHiddenMedia([]);
  }, [myCall, selected, joiningRoom]);

  theaterRef.current = theaterOpen;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (fullscreenRoot()) {
        exitStageFullscreen();
        setTheaterOpen(false);
        return;
      }
      if (theaterRef.current) {
        setTheaterOpen(false);
        return;
      }
      if (!shownFocusRef.current) return;
      setFocusedTile(null);
      setFocusPinned(true);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    const onFs = () => {
      if (!fullscreenRoot()) setTheaterOpen(false);
    };
    document.addEventListener("fullscreenchange", onFs);
    document.addEventListener("webkitfullscreenchange", onFs);
    return () => {
      document.removeEventListener("fullscreenchange", onFs);
      document.removeEventListener("webkitfullscreenchange", onFs);
    };
  }, []);

  useEffect(() => {
    if (!theaterOpen || shownFocusRef.current) return;
    setTheaterOpen(false);
    exitStageFullscreen();
  }, [theaterOpen, remotes, camOn, screenOn, focusedTile, focusPinned]);

  const toggleCam = async () => {
    if (camOn) {
      camStream.current?.getVideoTracks().forEach((track) => {
        track.stop();
        camStream.current?.removeTrack(track);
      });
      setCamOn(false);
      if (camVideo.current) camVideo.current.srcObject = null;
      callRef.current?.setCamera(camStream.current);
      return;
    }
    try {
      const video = await navigator.mediaDevices.getUserMedia({ video: true });
      const track = video.getVideoTracks()[0];
      if (!track) throw new Error("no video");
      const mixed = camStream.current ?? new MediaStream();
      mixed.getVideoTracks().forEach((old) => {
        old.stop();
        mixed.removeTrack(old);
      });
      mixed.addTrack(track);
      camStream.current = mixed;
      if (camVideo.current) camVideo.current.srcObject = mixed;
      setCamOn(true);
      callRef.current?.setCamera(mixed);
      setCallNotice(null);
    } catch {
      setCallNotice("Couldn't turn the camera on on this device.");
    }
  };

  const toggleScreen = async () => {
    if (screenBlocked) return;
    if (screenOn) {
      screenStream.current?.getTracks().forEach((t) => t.stop());
      screenStream.current = null;
      setScreenOn(false);
      callRef.current?.setScreen(null);
      void backendPresence(micMuted, deafened, shownPresence, false);
      if (rtcPeerConnectionMissing()) {
        await callRef.current?.setScreenNative(false).catch(() => undefined);
      }
      return;
    }
    if (rtcPeerConnectionMissing()) {
      try {
        await callRef.current?.setScreenNative(true);
        setScreenOn(true);
        void backendPresence(micMuted, deafened, shownPresence, true);
        setCallNotice(null);
      } catch {
        setCallNotice("Couldn't share your screen on this device.");
      }
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getDisplayMedia({
        video: {
          frameRate: { ideal: 15, max: 24 },
          width: { max: 1920 },
          height: { max: 1080 },
        },
        audio: false,
      });
      const track = stream.getVideoTracks()[0];
      if (track) track.contentHint = "detail";
      screenStream.current = stream;
      stream.getVideoTracks()[0]?.addEventListener("ended", () => {
        screenStream.current = null;
        setScreenOn(false);
        callRef.current?.setScreen(null);
        void backendPresence(micMuted, deafened, shownPresence, false);
      });
      if (screenVideo.current) screenVideo.current.srcObject = stream;
      setScreenOn(true);
      void backendPresence(micMuted, deafened, shownPresence, true);
      callRef.current?.setScreen(stream);
      setCallNotice(null);
    } catch {
      setCallNotice("Couldn't share your screen on this device.");
    }
  };

  useEffect(() => {
    if (camVideo.current && camStream.current) camVideo.current.srcObject = camStream.current;
  }, [camOn, selected]);
  useEffect(() => {
    if (screenVideo.current && screenStream.current) screenVideo.current.srcObject = screenStream.current;
  }, [screenOn, selected]);

  if (!state) {
    const inApp = Boolean(
      (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__,
    );
    return (
      <div className="setup-screen">
        {inApp ? (
          <p className="muted">Opening…</p>
        ) : (
          <div className="panel">
            <h2>This isn't the app</h2>
            <p className="muted">
              This browser tab is only the UI. The core (invites, chat, calls) runs in the native
              window. Close this tab and use the Chaincord window from <code>tauri:dev</code> or the
              <code>.exe</code>.
            </p>
          </div>
        )}
      </div>
    );
  }

  if (!state.displayName) {
    return (
      <div className="setup-screen">
        <div className="panel">
          <ProfileEditor
            title="Create your profile"
            subtitle="Pick a name and photo. This is how you show up in communities."
            initialName=""
            initialAvatar=""
            submitLabel="Continue"
            onSave={saveProfile}
          />
        </div>
      </div>
    );
  }

  return (
    <div className="shell">
      {callNotice && <div className="app-toast">{callNotice}</div>}
      <aside className="rail" aria-label="Communities">
        {(state?.communities ?? []).map((guild) => {
          const active = guild.id === state?.communityId;
          return (
            <button
              key={guild.id}
              className={active ? "guild active" : "guild"}
              title={guild.name}
              onClick={() => {
                if (active) return;
                void backendSwitchCommunity(guild.id).catch((err) => {
                  setCallNotice(String(err));
                  window.setTimeout(() => setCallNotice(null), 4000);
                });
              }}
              onContextMenu={(e) => {
                if (!active) return;
                e.preventDefault();
                e.stopPropagation();
                setMenu({ kind: "ctx", x: e.clientX, y: e.clientY });
              }}
            >
              {guild.name.slice(0, 1).toUpperCase() || "?"}
            </button>
          );
        })}
        <button
          className="guild ghost"
          title="Create or join a community"
          onClick={() => openAdd("create")}
        >
          +
        </button>
      </aside>

      <aside className="sidebar">
        <div className="sidebar-main">
        {inCommunity ? (
          <>
            <div className="side-head">
              <button
                type="button"
                className="guild-name"
                onClick={(e) => {
                  e.stopPropagation();
                  setMenu((cur) => (cur?.kind === "head" ? null : { kind: "head" }));
                }}
              >
                <span>
                  <strong>{state?.communityName}</strong>
                  <span className="muted">
                    {alone ? "E2EE · just you here" : `E2EE · ${peerCount} in history`}
                  </span>
                </span>
                <span className="chevron">{menu?.kind === "head" ? "▴" : "▾"}</span>
              </button>
              {menu?.kind === "head" && guildMenu}
            </div>

            <div className="nav-block">
              <div className="nav-label-row">
                <span className="nav-label">Text channels</span>
                <button className="add-ch" title="New channel" onClick={() => setNewRoomOpen("text")}>
                  +
                </button>
              </div>
              {textChannels.map((ch) => (
                <button
                  key={ch}
                  className={selected.kind === "text" && selected.name === ch ? "nav-item active" : "nav-item"}
                  onClick={() => setSelected({ kind: "text", name: ch })}
                >
                  # {ch}
                </button>
              ))}
            </div>

            <div className="nav-block grow">
              <div className="nav-label-row">
                <span className="nav-label">Voice rooms</span>
                <button className="add-ch" title="New room" onClick={() => setNewRoomOpen("call")}>
                  +
                </button>
              </div>
              {callRooms.length === 0 && (
                <p className="muted pad">No rooms yet. Create one for camera and screen share.</p>
              )}
              {callRooms.map((room) => {
                const here = [...new Set(voice[room] ?? [])];
                const mine = myCall === room;
                const sharing = voiceRoomSharing(here, profiles, state?.publicKey ?? "", screenOn);
                return (
                  <div className="voice-group" key={room}>
                    <button
                      className={
                        selected.kind === "call" && selected.name === room
                          ? "nav-item voice active"
                          : mine
                            ? "nav-item voice joined"
                            : "nav-item voice"
                      }
                      onClick={() => void enterCall(room)}
                    >
                      <span className="voice-ico">🔊</span>
                      <span>{room}</span>
                      {sharing && (
                        <span className="voice-live" title="Someone is sharing their screen">
                          <ScreenIcon />
                        </span>
                      )}
                    </button>
                    {here.length > 0 && (
                      <div className="voice-people">
                        {here.map((pk) => {
                          const face = faceOf(pk);
                          return (
                            <div
                              className={pk === state?.publicKey ? "voice-user self" : "voice-user"}
                              key={pk}
                            >
                              <span className="voice-ava">
                                {face.avatar ? (
                                  <img src={face.avatar} alt="" />
                                ) : (
                                  face.name.slice(0, 1).toUpperCase()
                                )}
                              </span>
                              <span className="voice-nick">{face.name}</span>
                              <span className="voice-flags">
                                {face.sharingScreen && (
                                  <span title="Sharing screen">
                                    <ScreenIcon />
                                  </span>
                                )}
                                {face.muted && <MuteIcon />}
                                {face.deafened && <DeafIcon />}
                              </span>
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </>
        ) : (
          <div className="empty-side">
            <strong>No communities</strong>
            <p className="muted">Create a room or join with a short invite.</p>
            <div className="row">
              <button type="button" onClick={() => openAdd("create")}>
                Create
              </button>
              <button type="button" className="secondary" onClick={() => openAdd("join")}>
                Join
              </button>
            </div>
          </div>
        )}
        </div>
        {myCall && (
          <VoiceDock
            room={myCall}
            community={callCommunityName}
            link={callLink}
            open={callTip}
            onToggle={() => setCallTip((v) => !v)}
            onClose={() => setCallTip(false)}
            onHang={() => void hangUp()}
            onOpen={() => {
              const id = state?.liveCall?.communityId;
              if (id && id !== state?.communityId) {
                void backendSwitchCommunity(id);
              }
              setSelected({ kind: "call", name: myCall });
            }}
          />
        )}
        {inCommunity && state?.seedActive && (
          <SeedCard
            messages={state.archiveMessages ?? 0}
            sent={state.seedSent ?? 0}
            total={state.seedTotal ?? 1}
          />
        )}
        {state?.displayName && (
          <UserPanel
            name={state.displayName}
            avatar={state.avatar}
            presence={shownPresence}
            inVoice={Boolean(myCall)}
            micMuted={micMuted}
            deafened={deafened}
            onMic={toggleMic}
            onDeafen={toggleDeafen}
            onPresence={setMyPresence}
            onSettings={openSettings}
          />
        )}
      </aside>

      <section className="main">
        {inCommunity ? (
          selected.kind === "call" && (myCall === selected.name || joiningRoom === selected.name) ? (
            <>
              <header className="topbar">
                <div>
                  <h1>🔊 {selected.name}</h1>
                </div>
              </header>
              <div className="stage" ref={stageRef}>
                {(() => {
                  const people = peopleInSelectedCall.length
                    ? peopleInSelectedCall
                    : myCall === selected.name || joiningRoom === selected.name
                      ? [state.publicKey]
                      : [];
                  if (!people.length) {
                    return (
                      <div className="stage-empty">
                        <p className="muted">No one in the room right now.</p>
                      </div>
                    );
                  }
                  const hubPk = electCallHub(people, callOwner);
                  const tiles: CallTile[] = [];
                  for (const pk of people) {
                    const mine = pk === state.publicKey;
                    const face = faceOf(pk);
                    const remote = remotes.find((r) => r.peer === pk && !r.screen);
                    const camStreamFor = mine
                      ? camOn
                        ? camStream.current
                        : null
                      : remote?.stream ?? null;
                    tiles.push({
                      id: userTileId(pk),
                      name: face.name,
                      avatar: face.avatar,
                      stream: camStreamFor,
                      showVideo: mine ? camOn : remoteVideoVisible(remote),
                      muted: face.muted,
                      deafened: face.deafened,
                      local: mine,
                      screen: false,
                      hub: Boolean(hubPk && pk === hubPk),
                      sharing: mine ? screenOn : face.sharingScreen,
                      speakingStream: face.muted
                        ? null
                        : mine
                          ? camStream.current
                          : remote?.stream ?? null,
                    });
                    const screen = mine
                      ? screenOn
                        ? { stream: screenStream.current ?? new MediaStream(), frames: true }
                        : null
                      : remotes.find((r) => r.peer === pk && r.screen);
                    const screenLive = mine
                      ? screenOn
                      : Boolean(screen && remoteScreenVisible(screen));
                    // Only mount a screen tile when video is actually flowing — the
                    // always-on screen transceiver otherwise looks like a ghost "live".
                    if (screenLive) {
                      tiles.push({
                        id: screenTileId(pk),
                        name: `${face.name} · screen`,
                        avatar: face.avatar,
                        stream: screen?.stream ?? new MediaStream(),
                        showVideo: true,
                        muted: false,
                        deafened: false,
                        local: mine,
                        screen: true,
                        hub: false,
                        sharing: true,
                        speakingStream: null,
                      });
                    }
                  }
                  const ids = tiles.map((tile) => tile.id);
                  const autoIds = autoCallFocusIds(
                    tiles.map((tile) => ({
                      id: tile.id,
                      screen: tile.screen,
                      live: tile.showVideo && wantsCallMedia(hiddenMedia, tile.id),
                    })),
                  );
                  const focus = pickCallFocus(focusedTile, focusPinned, ids, autoIds);
                  shownFocusRef.current = focus;
                  const focused = tiles.find((tile) => tile.id === focus) ?? null;
                  const rest = focused ? tiles.filter((tile) => tile.id !== focus) : tiles;
                  const onToggle = (id: string) => {
                    const next = toggleCallFocus(focus, id);
                    setFocusPinned(true);
                    setFocusedTile(next);
                    if (!next) {
                      setTheaterOpen(false);
                      exitStageFullscreen();
                    }
                  };
                  const onTheater = (id: string) => {
                    if (theaterOpen && focus === id) {
                      setTheaterOpen(false);
                      exitStageFullscreen();
                      return;
                    }
                    setFocusPinned(true);
                    setFocusedTile(id);
                    setTheaterOpen(true);
                    requestStageFullscreen(stageRef.current);
                  };
                  const onWatch = (id: string) => {
                    setHiddenMedia((prev) => {
                      const hiding = !prev.includes(id);
                      if (hiding && focus === id) {
                        setFocusPinned(true);
                        setFocusedTile(null);
                        setTheaterOpen(false);
                        exitStageFullscreen();
                      }
                      return hiding ? [...prev, id] : prev.filter((item) => item !== id);
                    });
                  };
                  const cardProps = (tile: CallTile) => ({
                    ...tile,
                    outputMuted: deafened,
                    watching: wantsCallMedia(hiddenMedia, tile.id),
                    onWatch: () => onWatch(tile.id),
                    onToggle: () => onToggle(tile.id),
                    onTheater: () => onTheater(tile.id),
                  });
                  return (
                    <div
                      className={[
                        "stage-body",
                        focused ? "focused" : "",
                        focused && theaterOpen ? "theater" : "",
                      ]
                        .filter(Boolean)
                        .join(" ")}
                    >
                      {focused && (
                        <div className="tile-focus">
                          <CallCard
                            key={focused.id}
                            {...cardProps(focused)}
                            expanded
                            theater={theaterOpen}
                          />
                        </div>
                      )}
                      {(!focused || rest.length > 0) && (
                        <div
                          className={focused ? "tiles strip-bottom" : "tiles"}
                          style={
                            focused
                              ? undefined
                              : {
                                  gridTemplateColumns: `repeat(${callTileGridCols(rest.length)}, minmax(0, 1fr))`,
                                }
                          }
                        >
                          {rest.map((tile) => (
                            <CallCard key={tile.id} {...cardProps(tile)} />
                          ))}
                        </div>
                      )}
                    </div>
                  );
                })()}
                {callNotice && <p className="sys error stage-note">{callNotice}</p>}
                <div className="call-bar">
                  <button type="button" className={camOn ? "on" : ""} onClick={() => void toggleCam()}>
                    {camOn ? "Turn off camera" : "Camera"}
                  </button>
                  <button
                    type="button"
                    className={screenOn ? "on" : ""}
                    disabled={screenBlocked}
                    title={
                      screenBlocked
                        ? "Group (3+) needs a desktop hub in the call"
                        : "Share screen"
                    }
                    onClick={() => void toggleScreen()}
                  >
                    Screen
                  </button>
                  <button
                    type="button"
                    className="hang"
                    onClick={() => void hangUp()}
                    disabled={myCall !== selected.name}
                  >
                    Disconnect
                  </button>
                </div>
              </div>
            </>
          ) : (
            <>
              <header className="topbar">
                <div>
                  <h1># {selected.name}</h1>
                  <p className="muted">
                    {state?.communityName} · E2EE · {peerCount} peers
                  </p>
                </div>
              </header>

              <div className="messages" ref={scroller}>
                {channelLines.map((line, i) =>
                  isChatLine(line.kind) ? (
                    <article className="msg" key={i}>
                      <div className={line.self ? "who self" : "who"}>
                        {line.self ? meName : faceOf(line.sender).name}
                        <time>{new Date(line.ts).toLocaleTimeString()}</time>
                      </div>
                      <p>{line.text}</p>
                    </article>
                  ) : (
                    <p className={line.kind === "error" ? "sys error" : "sys"} key={i}>
                      {line.text}
                    </p>
                  ),
                )}
              </div>

              <form className="composer" onSubmit={onChat}>
                <input
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  placeholder={`Message in #${selected.name}`}
                />
                <button type="submit">Send</button>
              </form>
            </>
          )
        ) : (
          <div className="welcome">
            <h1>Chaincord</h1>
            <p>Peer-to-peer chat and calls, no central server.</p>
            <p className="muted">Create a community or paste an invite to join.</p>
            <div className="welcome-actions">
              <button type="button" onClick={() => openAdd("create")}>
                Create community
              </button>
              <button type="button" className="secondary" onClick={() => openAdd("join")}>
                Join with invite
              </button>
            </div>
          </div>
        )}
      </section>

      {inCommunity && (
        <aside className="members-pane" aria-label="Members">
          {alone && (
            <p className="alone-hint">
              <strong>Just you for now.</strong> Copy the invite from the community menu and send it.
              Both people need the app open. Over the internet, your PC must accept incoming
              connections (same network, VPS, or open port).
            </p>
          )}
          {PRESENCE_GROUPS.map((group) => {
            const people = groupedMembers[group.id];
            if (!people.length) return null;
            return (
              <div className="member-group" key={group.id}>
                <div className="nav-label">
                  {group.label} — {people.length}
                </div>
                {people.map((row) => {
                  const pk = row.pk;
                  const mine = pk === state?.publicKey;
                  const face = faceOf(pk);
                  const pulsing = mine
                    ? Boolean(state?.seedActive)
                    : Boolean(state?.seeding?.includes(pk));
                  const status = row.status;
                  return (
                    <div className={`member ${status}`} key={pk}>
                      <span className="member-ava">
                        {face.avatar ? (
                          <img src={face.avatar} alt="" />
                        ) : (
                          face.name.slice(0, 1).toUpperCase()
                        )}
                        <span
                          className={`presence-dot ${status}${pulsing ? " seeding" : ""}`}
                          title={
                            pulsing
                              ? `${presenceLabel(status)} · syncing history`
                              : presenceLabel(status)
                          }
                        />
                      </span>
                      <span>{mine ? meName : face.name}</span>
                      {face.sharingScreen && seatedInVoice(pk) && (
                        <span className="member-live" title="Sharing screen">
                          <ScreenIcon />
                        </span>
                      )}
                    </div>
                  );
                })}
              </div>
            );
          })}
        </aside>
      )}

      {addOpen && (
        <div
          className="modal"
          role="dialog"
          aria-label={addMode === "create" ? "Create community" : "Join with invite"}
          onClick={() => !addBusy && setAddOpen(false)}
        >
          <div className="panel" onClick={(e) => e.stopPropagation()}>
            <h2>{addMode === "create" ? "Create community" : "Join with invite"}</h2>
            {inCommunity && (
              <p className="field-hint">
                You stay in <b>{state?.communityName}</b>. Creating or joining adds another
                community to the sidebar — it does not remove this one.
              </p>
            )}
            <div className="tabs">
              <button
                type="button"
                className={addMode === "create" ? "tab active" : "tab"}
                onClick={() => {
                  setAddMode("create");
                  setAddError(null);
                }}
              >
                Create
              </button>
              <button
                type="button"
                className={addMode === "join" ? "tab active" : "tab"}
                onClick={() => {
                  setAddMode("join");
                  setAddError(null);
                }}
              >
                Join
              </button>
            </div>
            {addMode === "create" ? (
              <form onSubmit={onCreate} className="stack">
                <label className="field">
                  <span>Name</span>
                  <input
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="e.g. team, friends"
                    maxLength={64}
                    autoFocus
                    disabled={addBusy}
                  />
                </label>
                <p className="field-hint">
                  After that you copy an invite. New members don't configure a server — the
                  creator's app is the relay when their machine accepts connections.
                </p>
                {addError && <p className="form-error">{addError}</p>}
                <div className="row">
                  <button type="submit" disabled={!name.trim() || addBusy}>
                    {addBusy ? "Creating…" : "Create"}
                  </button>
                  <button
                    type="button"
                    className="ghost"
                    disabled={addBusy}
                    onClick={() => setAddOpen(false)}
                  >
                    Cancel
                  </button>
                </div>
              </form>
            ) : (
              <form onSubmit={onJoin} className="stack">
                <label className="field">
                  <span>Invite</span>
                  <textarea
                    value={invite}
                    onChange={(e) => setInvite(e.target.value)}
                    placeholder="Paste the cc/… code or the full message"
                    rows={4}
                    autoFocus
                    disabled={addBusy}
                    spellCheck={false}
                  />
                </label>
                <p className="field-hint">
                  You can paste the full WhatsApp/Discord message. The app finds the code.
                </p>
                {addError && <p className="form-error">{addError}</p>}
                <div className="row">
                  <button type="submit" disabled={!invite.trim() || addBusy}>
                    {addBusy ? "Joining…" : "Join"}
                  </button>
                  <button
                    type="button"
                    className="ghost"
                    disabled={addBusy}
                    onClick={() => void pasteInvite()}
                  >
                    Paste
                  </button>
                  <button
                    type="button"
                    className="ghost"
                    disabled={addBusy}
                    onClick={() => setAddOpen(false)}
                  >
                    Cancel
                  </button>
                </div>
              </form>
            )}
          </div>
        </div>
      )}

      {shareOpen && state?.invite && (
        <div
          className="modal"
          role="dialog"
          aria-label="Share invite"
          onClick={() => setShareOpen(false)}
        >
          <div className="panel invite-share" onClick={(e) => e.stopPropagation()}>
            <h2>Invite to {state.communityName}</h2>
            <p>
              Send this message. Recipients open Chaincord, go to{" "}
              <b>Join with invite</b>, and paste.
            </p>
            <pre className="invite-preview">{inviteShareText(state.communityName, state.invite)}</pre>
            <code className="invite-code" title="Click to select">
              {state.invite}
            </code>
            <p className="field-hint">
              Keep the app open. Same network is enough. From another network, your PC must accept
              incoming connections.
            </p>
            <div className="row">
              <button type="button" onClick={() => void copyInvite("message")}>
                {copied === "message" ? "Message copied" : "Copy message"}
              </button>
              <button
                type="button"
                className="ghost"
                onClick={() => void copyInvite("code")}
              >
                {copied === "code" ? "Code copied" : "Code only"}
              </button>
              <button type="button" className="ghost" onClick={() => setShareOpen(false)}>
                Done
              </button>
            </div>
          </div>
        </div>
      )}

      {newRoomOpen && (
        <div className="modal" role="dialog">
          <div className="panel">
            <h2>{newRoomOpen === "call" ? "New voice room" : "New text channel"}</h2>
            <form onSubmit={onAddRoom} className="stack">
              <input
                value={newRoomName}
                onChange={(e) => setNewRoomName(e.target.value)}
                placeholder={newRoomOpen === "call" ? "e.g. general" : "e.g. development"}
                autoFocus
              />
              <div className="row">
                <button type="submit" disabled={!newRoomName.trim()}>
                  Create
                </button>
                <button
                  type="button"
                  className="ghost"
                  onClick={() => {
                    setNewRoomOpen(null);
                    setNewRoomName("");
                  }}
                >
                  Cancel
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {leaveOpen && (
        <div className="modal" role="dialog">
          <div className="panel">
            <h2>Leave {state?.communityName}</h2>
            {alone ? (
              <>
                <p>
                  You're the only member online in this community. Leaving removes only it from this
                  device; other communities in the sidebar stay.
                </p>
                <div className="row">
                  <button type="button" onClick={() => void onLeave()}>
                    Leave
                  </button>
                  <button type="button" className="ghost" onClick={() => setLeaveOpen(false)}>
                    Cancel
                  </button>
                </div>
              </>
            ) : (
              <>
                <p>
                  {otherPeers} other member(s) are here. In the full product, leaving finishes after
                  handing off your history slice (default 20). Handoff is not wired in this MVP yet.
                </p>
                <div className="row">
                  <button type="button" disabled>
                    Transfer and leave
                  </button>
                  <button type="button" className="secondary" onClick={() => void onLeave()}>
                    Leave anyway
                  </button>
                  <button type="button" className="ghost" onClick={() => setLeaveOpen(false)}>
                    Cancel
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      )}
      {menu?.kind === "ctx" && guildMenu}
      {settingsOpen && (
        <div
          className="modal"
          role="dialog"
          aria-label="User settings"
          onClick={() => setSettingsOpen(false)}
        >
          <div className="panel settings-panel" onClick={(e) => e.stopPropagation()}>
            <div className="tabs">
              <button
                type="button"
                className={settingsTab === "profile" ? "tab active" : "tab"}
                onClick={() => setSettingsTab("profile")}
              >
                Profile
              </button>
              <button
                type="button"
                className={settingsTab === "audio" ? "tab active" : "tab"}
                onClick={() => setSettingsTab("audio")}
              >
                Audio
              </button>
            </div>
            {settingsTab === "profile" ? (
              <ProfileEditor
                title="My profile"
                subtitle="Name and photo others see."
                initialName={state.displayName}
                initialAvatar={state.avatar}
                submitLabel="Save"
                onCancel={() => setSettingsOpen(false)}
                onSave={saveProfile}
              />
            ) : (
              <AudioSettings
                key={micEpoch}
                inputId={audioInputId}
                outputId={audioOutputId}
                liveInput={myCall ? camStream.current : null}
                onInput={(id) => void applyMicDevice(id)}
                onOutput={applySpeaker}
                onClose={() => setSettingsOpen(false)}
              />
            )}
          </div>
        </div>
      )}
      {remotes
        .filter((remote) => !remote.screen && remote.stream.getAudioTracks().length > 0)
        .map((remote) => (
          <RemoteAudio
            key={`audio:${remote.peer}`}
            stream={remote.stream}
            muted={deafened}
            sinkId={audioOutputId}
          />
        ))}
    </div>
  );
}

function VoiceDock({
  room,
  community,
  link,
  open,
  onToggle,
  onClose,
  onHang,
  onOpen,
}: {
  room: string;
  community: string;
  link: CallLink | null;
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  onHang: () => void;
  onOpen?: () => void;
}) {
  const grade = gradeLink(link);
  const title =
    grade === "wait"
      ? "Connecting…"
      : grade === "off"
        ? "No connection"
        : grade === "bad"
          ? "Unstable connection"
          : "Voice connected";
  const mode = link?.mode ?? "1:1";
  const ping = link?.rttMs != null ? `${link.rttMs} ms` : null;
  const where = community ? `${room} / ${community}` : room;
  const sub = ping ? `${mode} · ${where} · ${ping}` : `${mode} · ${where}`;
  useEffect(() => {
    if (!open) return;
    const close = () => onClose();
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [open, onClose]);
  return (
    <div className="voice-dock">
      <div className="voice-signal-wrap">
        <button
          type="button"
          className={`voice-signal ${grade}`}
          title={ping ? `${title} · ${ping}` : title}
          onClick={(e) => {
            e.stopPropagation();
            onToggle();
          }}
        >
          <SignalGlyph grade={grade} />
        </button>
        {open && (
          <div className="voice-tip" onClick={(e) => e.stopPropagation()}>
            <strong>{title}</strong>
            <p>{ping ? `Latency ${ping}` : "Measuring latency…"}</p>
            <p>
              {link?.lossPct != null ? `Loss ${link.lossPct}%` : "Loss —"}
              {link?.jitterMs != null ? ` · jitter ${link.jitterMs} ms` : ""}
            </p>
            <p>
              {link && link.peers > 0
                ? `${link.live}/${link.peers} active links`
                : "Only you in the room"}
            </p>
            <p>{callPathHint(mode)}</p>
          </div>
        )}
      </div>
      <div
        className={`voice-dock-meta ${grade}`}
        role={onOpen ? "button" : undefined}
        onClick={onOpen}
        style={onOpen ? { cursor: "pointer" } : undefined}
      >
        <strong>{title}</strong>
        <span>{sub}</span>
      </div>
      <button type="button" className="hang-mini" title="Disconnect" onClick={onHang}>
        <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden>
          <path
            fill="currentColor"
            d="M6.6 10.8c1.4 2.8 3.8 5.1 6.6 6.6l2.2-2.2c.3-.3.7-.4 1.1-.2 1.2.4 2.5.6 3.8.6.6 0 1 .4 1 1V20c0 .6-.4 1-1 1C10.6 21 3 13.4 3 4c0-.6.4-1 1-1h3.5c.6 0 1 .4 1 1 0 1.3.2 2.6.6 3.8.1.4 0 .8-.3 1.1l-2.2 2.2Z"
            transform="rotate(135 12 12)"
          />
        </svg>
      </button>
    </div>
  );
}

function SignalGlyph({ grade }: { grade: SignalGrade }) {
  const bars = grade === "off" ? 0 : grade === "bad" ? 1 : grade === "ok" ? 2 : 3;
  const dim = "rgba(255,255,255,.22)";
  const warn = grade === "ok" || grade === "bad" || grade === "off";
  return (
    <span className="signal-glyph">
      <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden>
        <circle cx="12" cy="18.4" r="1.55" fill={bars >= 1 ? "currentColor" : dim} />
        <path
          d="M8.15 14.55c2.05-1.85 5.65-1.85 7.7 0"
          fill="none"
          stroke={bars >= 2 ? "currentColor" : dim}
          strokeWidth="2.1"
          strokeLinecap="round"
        />
        <path
          d="M5.6 11.35c3.5-3.2 9.3-3.2 12.8 0"
          fill="none"
          stroke={bars >= 3 ? "currentColor" : dim}
          strokeWidth="2.1"
          strokeLinecap="round"
        />
        <path
          d="M3.2 8.2c4.8-4.35 12.8-4.35 17.6 0"
          fill="none"
          stroke={bars >= 3 ? "currentColor" : dim}
          strokeWidth="2.1"
          strokeLinecap="round"
        />
      </svg>
      {warn && (
        <span className="signal-warn" aria-hidden>
          !
        </span>
      )}
    </span>
  );
}

function SeedCard({
  messages,
  sent,
  total,
}: {
  messages: number;
  sent: number;
  total: number;
}) {
  const pct = seedPercent(sent, total);
  return (
    <div className="seed-card">
      <div className="seed-card-head">
        <span>Seeding</span>
        <span>{pct}%</span>
      </div>
      <div className="seed-bar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
        <i style={{ width: `${pct}%` }} />
      </div>
      <span className="muted">
        {fmtBytes(Math.min(sent, total))} of {fmtBytes(total)}
        {messages > 0 ? ` · ${messages} msg` : ""}
      </span>
    </div>
  );
}

type CallTile = {
  id: string;
  name: string;
  avatar: string;
  stream: MediaStream | null;
  showVideo: boolean;
  muted: boolean;
  deafened: boolean;
  local: boolean;
  screen: boolean;
  hub: boolean;
  sharing?: boolean;
  speakingStream: MediaStream | null;
};

function RemoteAudio({
  stream,
  muted,
  sinkId = "",
}: {
  stream: MediaStream;
  muted: boolean;
  sinkId?: string;
}) {
  const ref = useRef<HTMLAudioElement>(null);
  const audioId = stream.getAudioTracks().map((track) => track.id).join(",");
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.srcObject = stream;
    el.muted = muted;
    const kick = () => {
      el.muted = muted;
      void el.play().catch(() => undefined);
    };
    kick();
    el.addEventListener("canplay", kick);
    window.addEventListener("click", kick);
    return () => {
      el.removeEventListener("canplay", kick);
      window.removeEventListener("click", kick);
      el.srcObject = null;
    };
  }, [stream, muted, audioId]);
  useEffect(() => {
    const el = ref.current as (HTMLAudioElement & { setSinkId?: (id: string) => Promise<void> }) | null;
    if (!el?.setSinkId) return;
    void el.setSinkId(sinkId).catch(() => undefined);
  }, [sinkId, audioId]);
  return <audio ref={ref} className="remote-audio-sink" autoPlay playsInline />;
}

function FrameImg({ tileId }: { tileId: string }) {
  const ref = useRef<HTMLImageElement>(null);
  const [has, setHas] = useState(() => Boolean(lastNativeFrame(tileId)));
  useEffect(() => {
    const img = ref.current;
    const existing = lastNativeFrame(tileId);
    if (img && existing) {
      img.src = existing;
      setHas(true);
    }
    const on = (e: Event) => {
      const detail = (e as CustomEvent<{ id: string; url: string }>).detail;
      if (!detail || detail.id !== tileId || !ref.current) return;
      if (detail.url) {
        ref.current.src = detail.url;
        setHas(true);
      } else {
        setHas(false);
      }
    };
    window.addEventListener("chaincord-frame", on);
    return () => window.removeEventListener("chaincord-frame", on);
  }, [tileId]);
  return <img ref={ref} className={has ? "tile-media" : "tile-media wait"} alt="" />;
}

function CallCard({
  id,
  name,
  avatar,
  stream,
  showVideo,
  muted,
  deafened,
  local = false,
  outputMuted: _outputMuted = false,
  speakingStream = null,
  screen = false,
  hub = false,
  sharing = false,
  expanded = false,
  theater = false,
  watching = true,
  onWatch,
  onToggle,
  onTheater,
}: {
  id: string;
  name: string;
  avatar: string;
  stream: MediaStream | null;
  showVideo: boolean;
  muted: boolean;
  deafened: boolean;
  local?: boolean;
  outputMuted?: boolean;
  speakingStream?: MediaStream | null;
  screen?: boolean;
  hub?: boolean;
  sharing?: boolean;
  expanded?: boolean;
  theater?: boolean;
  watching?: boolean;
  onWatch?: () => void;
  onToggle?: () => void;
  onTheater?: () => void;
}) {
  const ref = useRef<HTMLVideoElement>(null);
  const speaking = useSpeaking(muted ? null : speakingStream);
  const live = showVideo && watching;
  const paint = live && (isWebKitEngine() || Boolean(stream?.getVideoTracks().length === 0));
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (paint) {
      el.srcObject = null;
      return;
    }
    el.srcObject = live && stream ? stream : null;
    if (live && stream) void el.play().catch(() => undefined);
  }, [stream, live, paint]);
  const canWatch = showVideo || screen;
  const pausedMsg = !watching
    ? local
      ? "Preview hidden"
      : screen
        ? "Not watching screen"
        : "Not watching camera"
    : screen && !showVideo
      ? "Waiting for screen"
      : "";
  const cls = [
    "tile-card",
    speaking ? "speaking" : "",
    screen ? "screen" : "",
    expanded ? "expanded" : "",
    theater ? "theater" : "",
    !watching && canWatch ? "paused" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div
      role="button"
      tabIndex={0}
      className={cls}
      onClick={() => {
        if (!watching && canWatch) onWatch?.();
        else if (theater) onTheater?.();
        else onToggle?.();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          if (!watching && canWatch) onWatch?.();
          else if (theater) onTheater?.();
          else onToggle?.();
        }
      }}
      title={theater ? "Exit fullscreen" : expanded ? "Minimize" : "Expand"}
    >
      {live && paint ? (
        <FrameImg tileId={id} />
      ) : live && stream ? (
        <video ref={ref} autoPlay playsInline muted />
      ) : (
        <div className="tile-paused">
          <span className="tile-face">
            {avatar ? <img src={avatar} alt="" /> : name.slice(0, 1).toUpperCase()}
          </span>
          {pausedMsg ? <span className="tile-paused-msg">{pausedMsg}</span> : null}
        </div>
      )}
      <div className="tile-actions">
        {canWatch && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onWatch?.();
            }}
          >
            {watching ? (local ? "Hide" : "Stop watching") : local ? "Show" : "Watch"}
          </button>
        )}
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onToggle?.();
          }}
        >
          {expanded ? "Minimize" : "Expand"}
        </button>
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onTheater?.();
          }}
        >
          {theater ? "Exit fullscreen" : "Fullscreen"}
        </button>
      </div>
      <span className="tile-tag">
        <span>{name}</span>
        {hub && <HubIcon />}
        {(screen || sharing) && <ScreenIcon />}
        {muted && <MuteIcon />}
        {deafened && <DeafIcon />}
      </span>
    </div>
  );
}

function useSpeaking(stream: MediaStream | null) {
  const [speaking, setSpeaking] = useState(false);
  useEffect(() => {
    if (!stream?.getAudioTracks().length || isWebKitEngine()) {
      setSpeaking(false);
      return;
    }
    let ctx: AudioContext | null = null;
    let src: MediaStreamAudioSourceNode | null = null;
    let raf = 0;
    try {
      ctx = new AudioContext();
      src = ctx.createMediaStreamSource(stream);
      const analyser = ctx.createAnalyser();
      analyser.fftSize = 512;
      src.connect(analyser);
      const data = new Uint8Array(analyser.frequencyBinCount);
      const loop = () => {
        analyser.getByteFrequencyData(data);
        const avg = data.reduce((sum, n) => sum + n, 0) / data.length;
        setSpeaking(avg > 18);
        raf = requestAnimationFrame(loop);
      };
      raf = requestAnimationFrame(loop);
    } catch {
      setSpeaking(false);
      return;
    }
    return () => {
      cancelAnimationFrame(raf);
      src?.disconnect();
      void ctx?.close();
    };
  }, [stream]);
  return speaking;
}

function ScreenIcon() {
  return (
    <svg className="screen-live" viewBox="0 0 24 24" width="14" height="14" aria-hidden>
      <path
        fill="currentColor"
        d="M3 4h18a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1h-7v2h3v2H8v-2h3v-2H3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1Zm1 2v10h16V6H4Z"
      />
    </svg>
  );
}

function HubIcon() {
  return (
    <svg
      className="hub"
      viewBox="0 0 24 24"
      width="14"
      height="14"
      aria-hidden
      title="this person is forwarding the call"
    >
      <path
        fill="currentColor"
        d="M12 2a3 3 0 0 1 1 5.83V11h4a3 3 0 1 1 0 2h-4v3.17A3 3 0 1 1 11 16.17V13H7a3 3 0 1 1 0-2h4V7.83A3 3 0 0 1 12 2Z"
      />
    </svg>
  );
}

function MuteIcon() {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden>
      <path
        fill="currentColor"
        d="M12 14a3 3 0 0 0 3-3V6a3 3 0 1 0-6 0v5a3 3 0 0 0 3 3Zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.92V20H9v2h6v-2h-2v-2.08A7 7 0 0 0 19 11h-2Z"
      />
      <path stroke="currentColor" strokeWidth="2" d="M4 4l16 16" />
    </svg>
  );
}

function DeafIcon() {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden>
      <path
        fill="currentColor"
        d="M12 3a9 9 0 0 0-9 9v7a2 2 0 0 0 2 2h2v-8H5a7 7 0 0 1 14 0h-2v8h2a2 2 0 0 0 2-2v-7a9 9 0 0 0-9-9Z"
      />
      <path stroke="currentColor" strokeWidth="2" d="M4 4l16 16" />
    </svg>
  );
}
