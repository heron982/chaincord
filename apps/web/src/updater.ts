import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type UpdateStatus =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up-to-date" }
  | { kind: "available"; version: string; notes: string }
  | { kind: "downloading"; version: string; progress: number }
  | { kind: "installing"; version: string }
  | { kind: "error"; message: string };

let pending: Update | null = null;

export async function checkForAppUpdate(): Promise<UpdateStatus> {
  try {
    const update = await check();
    pending = update;
    if (!update) return { kind: "up-to-date" };
    return {
      kind: "available",
      version: update.version,
      notes: update.body ?? "",
    };
  } catch (err) {
    pending = null;
    return {
      kind: "error",
      message: String(err).replace(/^Error:\s*/i, "") || "Update check failed",
    };
  }
}

export async function installAppUpdate(
  onProgress?: (pct: number) => void,
): Promise<UpdateStatus> {
  const update = pending;
  if (!update) {
    return { kind: "error", message: "No update ready. Check again first." };
  }
  try {
    let downloaded = 0;
    let total = 0;
    await update.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? 0;
        onProgress?.(0);
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        if (total > 0) onProgress?.(Math.min(99, Math.round((downloaded / total) * 100)));
      } else if (event.event === "Finished") {
        onProgress?.(100);
      }
    });
    pending = null;
    await relaunch();
    return { kind: "installing", version: update.version };
  } catch (err) {
    return {
      kind: "error",
      message: String(err).replace(/^Error:\s*/i, "") || "Update install failed",
    };
  }
}

export function isTauriRuntime(): boolean {
  return Boolean(
    (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__,
  );
}
