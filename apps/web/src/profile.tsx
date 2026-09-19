import { FormEvent, useRef, useState } from "react";
import { presenceLabel } from "./logic";

export async function resizeAvatar(file: File): Promise<string> {
  const url = URL.createObjectURL(file);
  try {
    const img = await new Promise<HTMLImageElement>((resolve, reject) => {
      const el = new Image();
      el.onload = () => resolve(el);
      el.onerror = () => reject(new Error("imagem"));
      el.src = url;
    });
    const canvas = document.createElement("canvas");
    canvas.width = 128;
    canvas.height = 128;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas");
    const side = Math.min(img.width, img.height);
    ctx.drawImage(
      img,
      (img.width - side) / 2,
      (img.height - side) / 2,
      side,
      side,
      0,
      0,
      128,
      128,
    );
    return canvas.toDataURL("image/jpeg", 0.85);
  } finally {
    URL.revokeObjectURL(url);
  }
}

export function letterAvatar(name: string): string {
  const canvas = document.createElement("canvas");
  canvas.width = 128;
  canvas.height = 128;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";
  ctx.fillStyle = "#5865f2";
  ctx.fillRect(0, 0, 128, 128);
  ctx.fillStyle = "#fff";
  ctx.font = "bold 64px Segoe UI, sans-serif";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText((name.trim().slice(0, 1) || "?").toUpperCase(), 64, 72);
  return canvas.toDataURL("image/png");
}

export function ProfileEditor({
  title,
  subtitle,
  initialName,
  initialAvatar,
  submitLabel,
  onCancel,
  onSave,
}: {
  title: string;
  subtitle: string;
  initialName: string;
  initialAvatar: string;
  submitLabel: string;
  onCancel?: () => void;
  onSave: (name: string, avatar: string) => Promise<void>;
}) {
  const [name, setName] = useState(initialName);
  const [avatar, setAvatar] = useState(initialAvatar);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const next = name.trim();
    if (next.length < 2 || next.length > 32) {
      setErr("Name must be 2–32 characters.");
      return;
    }
    setBusy(true);
    setErr(null);
    try {
      await onSave(next, avatar || letterAvatar(next));
    } catch (caught) {
      setErr(String(caught));
      setBusy(false);
    }
  };

  return (
    <form className="stack profile-form" onSubmit={(e) => void submit(e)}>
      <h2>{title}</h2>
      <p>{subtitle}</p>
      <button
        type="button"
        className="avatar-pick"
        onClick={() => fileRef.current?.click()}
        title="Choose photo"
      >
        {avatar ? <img src={avatar} alt="" /> : <span>{(name.trim().slice(0, 1) || "+").toUpperCase()}</span>}
      </button>
      <input
        ref={fileRef}
        type="file"
        accept="image/*"
        hidden
        onChange={(e) => {
          const file = e.target.files?.[0];
          if (!file) return;
          void resizeAvatar(file)
            .then((data) => {
              setAvatar(data);
              setErr(null);
            })
            .catch(() => setErr("Couldn't read that image."));
        }}
      />
      <input
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="Display name"
        maxLength={32}
        autoFocus
      />
      {err && <p className="sys error">{err}</p>}
      <div className="row">
        <button type="submit" disabled={busy || name.trim().length < 2}>
          {submitLabel}
        </button>
        {onCancel && (
          <button type="button" className="ghost" onClick={onCancel}>
            Cancel
          </button>
        )}
      </div>
    </form>
  );
}

export function UserPanel({
  name,
  avatar,
  presence,
  inVoice,
  micMuted,
  deafened,
  onMic,
  onDeafen,
  onPresence,
  onSettings,
}: {
  name: string;
  avatar: string;
  presence: "online" | "away" | "busy";
  inVoice: boolean;
  micMuted: boolean;
  deafened: boolean;
  onMic: () => void;
  onDeafen: () => void;
  onPresence: (status: "online" | "away" | "busy") => void;
  onSettings: (tab?: "profile" | "audio") => void;
}) {
  const [open, setOpen] = useState(false);
  const label = inVoice && presence !== "busy" ? "In voice" : presenceLabel(presence);
  return (
    <div className="user-panel">
      <button type="button" className="user-chip" onClick={() => onSettings()} title="My profile">
        <span className="user-avatar">
          {avatar ? <img src={avatar} alt="" /> : <span>{name.slice(0, 1).toUpperCase()}</span>}
          <i className={`presence-dot ${presence}`} />
        </span>
        <span className="user-meta">
          <strong>{name}</strong>
          <em>{label}</em>
        </span>
      </button>
      <div className="user-status-wrap">
        <button
          type="button"
          className="ghost user-status-open"
          title="Change status"
          onClick={(e) => {
            e.stopPropagation();
            setOpen((v) => !v);
          }}
        >
          ▾
        </button>
        {open && (
          <div className="status-menu" onClick={(e) => e.stopPropagation()}>
            {(
              [
                ["online", "Online"],
                ["away", "Away"],
                ["busy", "Busy"],
              ] as const
            ).map(([id, text]) => (
              <button
                type="button"
                key={id}
                className={presence === id ? "active" : ""}
                onClick={() => {
                  onPresence(id);
                  setOpen(false);
                }}
              >
                <i className={`presence-dot ${id}`} />
                {text}
              </button>
            ))}
          </div>
        )}
      </div>
      <div className="user-actions">
        <button
          type="button"
          className={micMuted ? "off" : ""}
          title={micMuted ? "Unmute mic · right-click for devices" : "Mute mic · right-click for devices"}
          onClick={onMic}
          onContextMenu={(e) => {
            e.preventDefault();
            onSettings("audio");
          }}
        >
          <IconMic muted={micMuted} />
        </button>
        <button
          type="button"
          className={deafened ? "off" : ""}
          title={deafened ? "Undeafen · right-click for devices" : "Deafen · right-click for devices"}
          onClick={onDeafen}
          onContextMenu={(e) => {
            e.preventDefault();
            onSettings("audio");
          }}
        >
          <IconHead muted={deafened} />
        </button>
        <button type="button" title="User settings" onClick={() => onSettings()}>
          <IconGear />
        </button>
      </div>
    </div>
  );
}

function IconMic({ muted }: { muted: boolean }) {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden>
      <path
        fill="currentColor"
        d="M12 14a3 3 0 0 0 3-3V6a3 3 0 1 0-6 0v5a3 3 0 0 0 3 3Zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.92V20H9v2h6v-2h-2v-2.08A7 7 0 0 0 19 11h-2Z"
      />
      {muted && <path stroke="currentColor" strokeWidth="2" d="M4 4l16 16" />}
    </svg>
  );
}

function IconHead({ muted }: { muted: boolean }) {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden>
      <path
        fill="currentColor"
        d="M12 3a9 9 0 0 0-9 9v7a2 2 0 0 0 2 2h2v-8H5a7 7 0 0 1 14 0h-2v8h2a2 2 0 0 0 2-2v-7a9 9 0 0 0-9-9Z"
      />
      {muted && <path stroke="currentColor" strokeWidth="2" d="M4 4l16 16" />}
    </svg>
  );
}

function IconGear() {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden>
      <path
        fill="currentColor"
        d="M19.14 12.94c.04-.31.06-.63.06-.94s-.02-.63-.06-.94l2.03-1.58a.5.5 0 0 0 .12-.64l-1.92-3.32a.5.5 0 0 0-.6-.22l-2.39.96a7.2 7.2 0 0 0-1.63-.94l-.36-2.54a.5.5 0 0 0-.5-.42h-3.84a.5.5 0 0 0-.5.42l-.36 2.54c-.59.24-1.13.55-1.63.94l-2.39-.96a.5.5 0 0 0-.6.22L2.7 8.84a.5.5 0 0 0 .12.64l2.03 1.58c-.04.31-.06.63-.06.94s.02.63.06.94L2.82 14.52a.5.5 0 0 0-.12.64l1.92 3.32c.13.23.4.32.64.22l2.39-.96c.5.39 1.04.7 1.63.94l.36 2.54c.05.24.26.42.5.42h3.84c.24 0 .45-.18.5-.42l.36-2.54c.59-.24 1.13-.55 1.63-.94l2.39.96c.24.1.51 0 .64-.22l1.92-3.32a.5.5 0 0 0-.12-.64l-2.03-1.58ZM12 15.5A3.5 3.5 0 1 1 12 8.5a3.5 3.5 0 0 1 0 7Z"
      />
    </svg>
  );
}
