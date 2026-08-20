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
  type UiState,
} from "./backend";
import { CallNet, type CallLink, type CallTrace, type RemoteMedia } from "./call";
import {
  callPathHint,
  effectivePresence,
  formatCallClock,
  fmtBytes,
  gradeLink,
  groupByPresence,
  isChatLine,
  isWebKitEngine,
  keepCallFocus,
  normalizePresence,
  PRESENCE_GROUPS,
  presenceLabel,
  screenTileId,
  seedPercent,
  toggleCallFocus,
  trackLooksLive,
  userTileId,
  type Presence,
} from "./logic";
import { ProfileEditor, UserPanel } from "./profile";
import { playCallJoin, playCallLeave, unlockSounds } from "./sounds";

type Line =
  | { kind: "chat"; sender: string; text: string; ts: number; self: boolean; channel: string }
  | { kind: "info"; text: string }
  | { kind: "error"; text: string };

type AddMode = "create" | "join";
type Selection = { kind: "text"; name: string } | { kind: "call"; name: string };
type RoomKind = "text" | "call";

export function App() {
  const [state, setState] = useState<UiState | null>(null);
  const [lines, setLines] = useState<Line[]>([]);
  const [name, setName] = useState("");
  const [invite, setInvite] = useState("");
  const [draft, setDraft] = useState("");
  const [copied, setCopied] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const [addMode, setAddMode] = useState<AddMode>("create");
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
  const [micMuted, setMicMuted] = useState(false);
  const [deafened, setDeafened] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const camVideo = useRef<HTMLVideoElement>(null);
  const screenVideo = useRef<HTMLVideoElement>(null);
  const camStream = useRef<MediaStream | null>(null);
  const screenStream = useRef<MediaStream | null>(null);
  const callRef = useRef<CallNet | null>(null);
  const [remotes, setRemotes] = useState<RemoteMedia[]>([]);
  const [callLink, setCallLink] = useState<CallLink | null>(null);
  const [callLog, setCallLog] = useState<CallTrace[]>([]);
  const [callTip, setCallTip] = useState(false);
  const [focusedTile, setFocusedTile] = useState<string | null>(null);
  const [myPresence, setMyPresence] = useState<Exclude<Presence, "offline">>("online");
  const [idle, setIdle] = useState(false);
  const shownPresence = effectivePresence(myPresence, idle);

  useEffect(() => {
    return backendSubscribe({
      onState: (next) => {
        setState(next);
        if (!next.communityId) {
          setSelected({ kind: "text", name: "general" });
        }
      },
      onMessage: (msg) => {
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
        void callRef.current?.handle(frame);
      },
    });
  }, []);

  useEffect(() => {
    if (!state?.communityId) {
      setLines([]);
      return;
    }
    void backendHistory().then((msgs) => {
      setLines((prev) => {
        const fromDb = msgs.map((msg) => ({
          kind: "chat" as const,
          sender: msg.sender,
          text: msg.text,
          ts: msg.ts,
          self: msg.self,
          channel: msg.channel || "general",
        }));
        const extras = prev.filter((line) => {
          if (line.kind !== "chat") return true;
          return !fromDb.some(
            (msg) => msg.ts === line.ts && msg.sender === line.sender && msg.text === line.text,
          );
        });
        return [...fromDb, ...extras];
      });
    });
  }, [state?.communityId]);

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

  const stopMedia = () => {
    camStream.current?.getTracks().forEach((t) => t.stop());
    screenStream.current?.getTracks().forEach((t) => t.stop());
    camStream.current = null;
    screenStream.current = null;
    setCamOn(false);
    setScreenOn(false);
    setFocusedTile(null);
    setCallNotice(null);
  };

  const meName = state?.displayName?.trim() || "você";

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

  const short = (k?: string) => (k ? k.slice(0, 8) : "…");
  const inCommunity = Boolean(state?.communityId);
  const otherMembers = Object.keys(state?.profiles ?? {}).filter(
    (pk) => pk && pk !== state?.publicKey,
  ).length;
  const liveLinks = (state?.peers ?? []).filter((url) => url !== "relay").length;
  const otherPeers = Math.max(liveLinks, otherMembers);
  const peerCount = otherPeers + (inCommunity ? 1 : 0);
  const alone = inCommunity && otherPeers === 0;
  const textChannels = state?.textChannels?.length ? state.textChannels : inCommunity ? ["general"] : [];
  const callRooms = state?.callRooms ?? [];
  const voice = state?.voice ?? {};
  const myCall = Object.entries(voice).find(([, people]) =>
    people.includes(state?.publicKey ?? ""),
  )?.[0];
  const callRoster = (myCall ? (voice[myCall] ?? []) : []).slice().sort().join(",");
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

  const peopleInSelectedCall =
    selected.kind === "call" ? (voice[selected.name] ?? []) : myCall ? (voice[myCall] ?? []) : [];
  const screenBlocked = false;
  const profiles = state?.profiles ?? {};
  const faceOf = (pk: string) => {
    if (pk === state?.publicKey) {
      return {
        name: meName,
        avatar: state.avatar,
        muted: micMuted,
        deafened,
        status: shownPresence,
      };
    }
    const p = profiles[pk];
    return {
      name: p?.displayName?.trim() || short(pk),
      avatar: p?.avatar || "",
      muted: Boolean(p?.muted),
      deafened: Boolean(p?.deafened),
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
    if (!myCall || !state?.publicKey) {
      callRef.current?.stop();
      callRef.current = null;
      setRemotes([]);
      setCallLink(null);
      setCallLog([]);
      setCallTip(false);
      setFocusedTile(null);
      return;
    }
    const room = myCall;
    const me = state.publicKey;
    const net = new CallNet(
      me,
      room,
      (frame) => {
        void backendSendRtc(frame).catch((err) => {
          setCallNotice(`Não deu para enviar o sinal da call: ${String(err)}`);
        });
      },
      (media) => {
        setRemotes((prev) => {
          const rest = prev.filter((x) => !(x.peer === media.peer && x.screen === media.screen));
          const live = media.stream.getTracks().some((t) => t.readyState === "live" && t.enabled);
          if (media.screen && !live) return rest;
          return [...rest, media];
        });
      },
      (peer) => setRemotes((prev) => prev.filter((x) => x.peer !== peer)),
      setCallLink,
      (line) => setCallLog((prev) => [...prev.slice(-99), line]),
    );
    callRef.current = net;
    net.setCamera(camStream.current);
    net.setScreen(screenStream.current);
    net.sync((voice[room] ?? []).filter((pk) => pk !== me));
    return () => {
      net.stop();
      if (callRef.current === net) callRef.current = null;
    };
  }, [myCall, state?.publicKey]);

  useEffect(() => {
    if (!myCall || !state?.publicKey) return;
    callRef.current?.sync((voice[myCall] ?? []).filter((pk) => pk !== state.publicKey));
  }, [voice, myCall, state?.publicKey]);

  const copyInvite = async () => {
    if (!state?.invite) return;
    await navigator.clipboard.writeText(state.invite);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
    setMenu(null);
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
      <button type="button" onClick={() => void copyInvite()}>
        {copied ? "Convite copiado" : "Copiar convite"}
      </button>
      <button type="button" className="danger" onClick={openLeave}>
        Sair da comunidade
      </button>
    </div>
  );

  const onCreate = async (e: FormEvent) => {
    e.preventDefault();
    try {
      await backendCreate(name);
      setLines([]);
      setSelected({ kind: "text", name: "general" });
      setAddOpen(false);
      setName("");
      stopMedia();
    } catch (err) {
      setLines((prev) => [...prev, { kind: "error", text: String(err) }]);
    }
  };

  const onJoin = async (e: FormEvent) => {
    e.preventDefault();
    try {
      await backendJoin(invite);
      setLines([]);
      setSelected({ kind: "text", name: "general" });
      setAddOpen(false);
      setInvite("");
      stopMedia();
    } catch (err) {
      setLines((prev) => [...prev, { kind: "error", text: String(err) }]);
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
          const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
          stream.getAudioTracks().forEach((track) => {
            track.enabled = !micMuted;
          });
          attachMic(stream);
        } catch {
          setCallNotice("Sem microfone neste dispositivo. A sala abre; câmera ainda pode ligar.");
        }
      } else {
        attachMic(camStream.current);
      }
    } catch (err) {
      setCallNotice(String(err));
    }
  };

  const hangUp = async () => {
    stopMedia();
    try {
      await backendLeaveCall();
      setSelected({ kind: "text", name: "general" });
    } catch (err) {
      setLines((prev) => [...prev, { kind: "error", text: String(err) }]);
    }
  };

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
      setCallNotice("Não deu para ligar a câmera neste dispositivo.");
    }
  };

  const toggleScreen = async () => {
    if (screenBlocked) return;
    if (screenOn) {
      screenStream.current?.getTracks().forEach((t) => t.stop());
      screenStream.current = null;
      setScreenOn(false);
      callRef.current?.setScreen(null);
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
      });
      if (screenVideo.current) screenVideo.current.srcObject = stream;
      setScreenOn(true);
      callRef.current?.setScreen(stream);
      setCallNotice(null);
    } catch {
      setCallNotice("Não deu para compartilhar a tela neste dispositivo.");
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
          <p className="muted">Abrindo…</p>
        ) : (
          <div className="panel">
            <h2>Isso não é o app</h2>
            <p className="muted">
              Esta aba do browser só tem a interface. O núcleo (convite, chat, call) vive na janela
              nativa. Feche esta aba e use o Chaincord que o <code>tauri:dev</code> abre, ou o
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
            title="Criar seu perfil"
            subtitle="Escolha um nome e uma foto. É assim que você aparece nas comunidades."
            initialName=""
            initialAvatar=""
            submitLabel="Entrar"
            onSave={saveProfile}
          />
        </div>
      </div>
    );
  }

  return (
    <div className="shell">
      <aside className="rail" aria-label="Comunidades">
        {inCommunity && (
          <button
            className="guild active"
            title={state?.communityName}
            onContextMenu={(e) => {
              e.preventDefault();
              e.stopPropagation();
              setMenu({ kind: "ctx", x: e.clientX, y: e.clientY });
            }}
          >
            {state?.communityName?.slice(0, 1).toUpperCase() || "?"}
          </button>
        )}
        <button
          className="guild ghost"
          title="Criar ou entrar numa comunidade"
          onClick={() => {
            setAddMode("create");
            setAddOpen(true);
          }}
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
                  <span className="muted">E2EE · {peerCount} no histórico</span>
                </span>
                <span className="chevron">{menu?.kind === "head" ? "▴" : "▾"}</span>
              </button>
              {menu?.kind === "head" && guildMenu}
            </div>

            <div className="nav-block">
              <div className="nav-label-row">
                <span className="nav-label">Canais de texto</span>
                <button className="add-ch" title="Novo canal" onClick={() => setNewRoomOpen("text")}>
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
                <span className="nav-label">Salas de chamada</span>
                <button className="add-ch" title="Nova sala" onClick={() => setNewRoomOpen("call")}>
                  +
                </button>
              </div>
              {callRooms.length === 0 && (
                <p className="muted pad">Nenhuma sala. Crie uma para câmera e tela.</p>
              )}
              {callRooms.map((room) => {
                const here = voice[room] ?? [];
                const mine = myCall === room;
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
            <strong>Nenhuma comunidade</strong>
            <p className="muted">
              Use o <b>+</b> na barra de comunidades para criar uma ou entrar com um convite.
            </p>
          </div>
        )}
        </div>
        {myCall && (
          <VoiceDock
            room={myCall}
            community={state?.communityName ?? ""}
            link={callLink}
            open={callTip}
            onToggle={() => setCallTip((v) => !v)}
            onClose={() => setCallTip(false)}
            onHang={() => void hangUp()}
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
            onSettings={() => setSettingsOpen(true)}
          />
        )}
      </aside>

      <section className="main">
        {inCommunity ? (
          selected.kind === "call" ? (
            <>
              <header className="topbar">
                <div>
                  <h1>🔊 {selected.name}</h1>
                </div>
              </header>
              <div className="stage">
                {(() => {
                  const people = peopleInSelectedCall.length
                    ? peopleInSelectedCall
                    : [state.publicKey];
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
                      showVideo: mine
                        ? camOn
                        : Boolean(remote?.stream?.getVideoTracks().some((t) => t.readyState === "live")),
                      muted: face.muted,
                      deafened: face.deafened,
                      local: mine,
                      screen: false,
                      speakingStream: face.muted
                        ? null
                        : mine
                          ? camStream.current
                          : remote?.stream ?? null,
                    });
                    const screen = mine
                      ? screenOn
                        ? { stream: screenStream.current }
                        : null
                      : remotes.find((r) => r.peer === pk && r.screen);
                    const screenLive = Boolean(
                      screen?.stream?.getVideoTracks().some((t) => trackLooksLive(t)),
                    );
                    if (screen?.stream && (mine ? screenOn : screenLive)) {
                      tiles.push({
                        id: screenTileId(pk),
                        name: `${face.name} · tela`,
                        avatar: face.avatar,
                        stream: screen.stream,
                        showVideo: true,
                        muted: false,
                        deafened: false,
                        local: mine,
                        screen: true,
                        speakingStream: null,
                      });
                    }
                  }
                  const ids = tiles.map((tile) => tile.id);
                  const focus = keepCallFocus(focusedTile, ids);
                  const focused = tiles.find((tile) => tile.id === focus) ?? null;
                  const rest = focused ? tiles.filter((tile) => tile.id !== focus) : tiles;
                  const onToggle = (id: string) => setFocusedTile((cur) => toggleCallFocus(cur, id));
                  return (
                    <div className={focused ? "stage-body focused" : "stage-body"}>
                      {focused && (
                        <div className="tile-focus">
                          <CallCard
                            {...focused}
                            outputMuted={deafened}
                            expanded
                            onToggle={() => onToggle(focused.id)}
                          />
                        </div>
                      )}
                      {(!focused || rest.length > 0) && (
                        <div className={focused ? "tiles strip-bottom" : "tiles"}>
                          {rest.map((tile) => (
                            <CallCard
                              key={tile.id}
                              {...tile}
                              outputMuted={deafened}
                              onToggle={() => onToggle(tile.id)}
                            />
                          ))}
                        </div>
                      )}
                    </div>
                  );
                })()}
                {callNotice && <p className="sys error stage-note">{callNotice}</p>}
                <CallTracePanel lines={callLog} nameOf={(pk) => faceOf(pk).name} />
                {remotes
                  .filter((remote) => !remote.screen)
                  .map((remote) => (
                    <RemoteAudio
                      key={`audio:${remote.peer}`}
                      stream={remote.stream}
                      muted={deafened}
                    />
                  ))}
                <div className="call-bar">
                  <button type="button" className={camOn ? "on" : ""} onClick={() => void toggleCam()}>
                    {camOn ? "Desligar câmera" : "Câmera"}
                  </button>
                  <button
                    type="button"
                    className={screenOn ? "on" : ""}
                    disabled={screenBlocked}
                    title={
                      screenBlocked
                        ? "Grupo (3+) precisa de um desktop SFU na call"
                        : "Compartilhar tela"
                    }
                    onClick={() => void toggleScreen()}
                  >
                    Tela
                  </button>
                  <button type="button" className="hang" onClick={() => void hangUp()}>
                    Desligar
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
                    {state?.communityName} · E2EE · {peerCount} storage peers
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
                  placeholder={`Mensagem em #${selected.name}`}
                />
                <button type="submit">Enviar</button>
              </form>
            </>
          )
        ) : (
          <div className="welcome">
            <h1>Chaincord</h1>
            <p>Nenhuma comunidade selecionada.</p>
            <p className="muted">
              Clique no <b>+</b> na barra de comunidades para criar uma ou entrar com um convite.
            </p>
            <button type="button" onClick={() => setAddOpen(true)}>
              Criar ou entrar
            </button>
          </div>
        )}
      </section>

      {inCommunity && (
        <aside className="members-pane" aria-label="Membros">
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
                              ? `${presenceLabel(status)} · enviando histórico`
                              : presenceLabel(status)
                          }
                        />
                      </span>
                      <span>{mine ? meName : face.name}</span>
                    </div>
                  );
                })}
              </div>
            );
          })}
        </aside>
      )}

      {addOpen && (
        <div className="modal" role="dialog" aria-label="Adicionar comunidade">
          <div className="panel">
            <h2>Comunidade</h2>
            {inCommunity && (
              <p>Neste MVP só cabe uma comunidade por vez. Entrar ou criar substitui a atual.</p>
            )}
            <div className="tabs">
              <button
                type="button"
                className={addMode === "create" ? "tab active" : "tab"}
                onClick={() => setAddMode("create")}
              >
                Criar
              </button>
              <button
                type="button"
                className={addMode === "join" ? "tab active" : "tab"}
                onClick={() => setAddMode("join")}
              >
                Entrar
              </button>
            </div>
            {addMode === "create" ? (
              <form onSubmit={onCreate} className="stack">
                <input
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="Nome da comunidade"
                  autoFocus
                />
                <div className="row">
                  <button type="submit" disabled={!name.trim()}>
                    Criar
                  </button>
                  <button type="button" className="ghost" onClick={() => setAddOpen(false)}>
                    Cancelar
                  </button>
                </div>
              </form>
            ) : (
              <form onSubmit={onJoin} className="stack">
                <textarea
                  value={invite}
                  onChange={(e) => setInvite(e.target.value)}
                  placeholder="Colar convite"
                  autoFocus
                />
                <div className="row">
                  <button type="submit" disabled={!invite.trim()}>
                    Entrar
                  </button>
                  <button type="button" className="ghost" onClick={() => setAddOpen(false)}>
                    Cancelar
                  </button>
                </div>
              </form>
            )}
          </div>
        </div>
      )}

      {newRoomOpen && (
        <div className="modal" role="dialog">
          <div className="panel">
            <h2>{newRoomOpen === "call" ? "Nova sala de chamada" : "Novo canal de texto"}</h2>
            <form onSubmit={onAddRoom} className="stack">
              <input
                value={newRoomName}
                onChange={(e) => setNewRoomName(e.target.value)}
                placeholder={newRoomOpen === "call" ? "ex: geral" : "ex: development"}
                autoFocus
              />
              <div className="row">
                <button type="submit" disabled={!newRoomName.trim()}>
                  Criar
                </button>
                <button
                  type="button"
                  className="ghost"
                  onClick={() => {
                    setNewRoomOpen(null);
                    setNewRoomName("");
                  }}
                >
                  Cancelar
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {leaveOpen && (
        <div className="modal" role="dialog">
          <div className="panel">
            <h2>Sair de {state?.communityName}</h2>
            {alone ? (
              <>
                <p>
                  Você é o único membro. Não há para quem transferir pedaços do histórico. Sair
                  apaga a cópia neste dispositivo.
                </p>
                <div className="row">
                  <button type="button" onClick={() => void onLeave()}>
                    Sair
                  </button>
                  <button type="button" className="ghost" onClick={() => setLeaveOpen(false)}>
                    Cancelar
                  </button>
                </div>
              </>
            ) : (
              <>
                <p>
                  Há {otherPeers} outro(s) membro(s). No produto, a saída só completa depois de
                  passar a parcela de histórico (def. 20). O handoff ainda não está ligado neste
                  MVP.
                </p>
                <div className="row">
                  <button type="button" disabled>
                    Transferir e sair
                  </button>
                  <button type="button" className="secondary" onClick={() => void onLeave()}>
                    Sair mesmo assim
                  </button>
                  <button type="button" className="ghost" onClick={() => setLeaveOpen(false)}>
                    Cancelar
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      )}
      {menu?.kind === "ctx" && guildMenu}
      {settingsOpen && (
        <div className="modal" role="dialog" aria-label="Configurações do usuário">
          <div className="panel">
            <ProfileEditor
              title="Meu perfil"
              subtitle="Nome e foto que as outras pessoas veem."
              initialName={state.displayName}
              initialAvatar={state.avatar}
              submitLabel="Salvar"
              onCancel={() => setSettingsOpen(false)}
              onSave={saveProfile}
            />
          </div>
        </div>
      )}
    </div>
  );
}

function CallTracePanel({
  lines,
  nameOf,
}: {
  lines: CallTrace[];
  nameOf: (pk: string) => string;
}) {
  const scroller = useRef<HTMLOListElement>(null);
  useEffect(() => {
    scroller.current?.scrollTo(0, scroller.current.scrollHeight);
  }, [lines]);
  const mode = lines[lines.length - 1]?.mode ?? "1:1";
  return (
    <div className="call-trace">
      <header>
        <strong>{mode}</strong>
        <span>{callPathHint(mode)}</span>
      </header>
      <ol ref={scroller}>
        {lines.length === 0 && <li className="info">Aguardando enlaces…</li>}
        {lines.map((line) => (
          <li key={line.id} className={line.level}>
            <span className="t">{formatCallClock(line.t)}</span>
            <span className="m">{line.mode}</span>
            <span className="w">{line.peer ? nameOf(line.peer) : "sala"}</span>
            <span className="e">{line.event}</span>
          </li>
        ))}
      </ol>
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
}: {
  room: string;
  community: string;
  link: CallLink | null;
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  onHang: () => void;
}) {
  const grade = gradeLink(link);
  const title =
    grade === "wait"
      ? "Conectando…"
      : grade === "off"
        ? "Sem conexão"
        : grade === "bad"
          ? "Conexão instável"
          : "Voz conectada";
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
            <p>{ping ? `Latência ${ping}` : "Medindo latência…"}</p>
            <p>
              {link?.lossPct != null ? `Perda ${link.lossPct}%` : "Perda —"}
              {link?.jitterMs != null ? ` · jitter ${link.jitterMs} ms` : ""}
            </p>
            <p>
              {link && link.peers > 0
                ? `${link.live}/${link.peers} ligações ativas`
                : "Só você na sala"}
            </p>
            <p>{callPathHint(mode)}</p>
          </div>
        )}
      </div>
      <div className={`voice-dock-meta ${grade}`}>
        <strong>{title}</strong>
        <span>{sub}</span>
      </div>
      <button type="button" className="hang-mini" title="Desligar" onClick={onHang}>
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
        {fmtBytes(Math.min(sent, total))} de {fmtBytes(total)}
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
  speakingStream: MediaStream | null;
};

function RemoteAudio({ stream, muted }: { stream: MediaStream; muted: boolean }) {
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
  return <audio ref={ref} className="remote-audio-sink" autoPlay playsInline />;
}

function CallCard({
  name,
  avatar,
  stream,
  showVideo,
  muted,
  deafened,
  local: _local = false,
  outputMuted: _outputMuted = false,
  speakingStream = null,
  screen = false,
  expanded = false,
  onToggle,
}: {
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
  expanded?: boolean;
  onToggle?: () => void;
}) {
  const ref = useRef<HTMLVideoElement>(null);
  const speaking = useSpeaking(muted ? null : speakingStream);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.srcObject = showVideo && stream ? stream : null;
    if (showVideo && stream) void el.play().catch(() => undefined);
  }, [stream, showVideo]);
  const cls = [
    "tile-card",
    speaking ? "speaking" : "",
    screen ? "screen" : "",
    expanded ? "expanded" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div
      role="button"
      tabIndex={0}
      className={cls}
      onClick={onToggle}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onToggle?.();
        }
      }}
      title={expanded ? "Voltar à grade" : "Ampliar"}
    >
      {showVideo && stream ? (
        <video ref={ref} autoPlay playsInline muted />
      ) : (
        <span className="tile-face">
          {avatar ? <img src={avatar} alt="" /> : name.slice(0, 1).toUpperCase()}
        </span>
      )}
      <span className="tile-tag">
        <span>{name}</span>
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
