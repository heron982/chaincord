let ctx: AudioContext | null = null;

function audio(): AudioContext | null {
  const Ctor = window.AudioContext || (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (!Ctor) return null;
  try {
    if (!ctx) ctx = new Ctor();
    if (ctx.state === "suspended") void ctx.resume();
    return ctx;
  } catch {
    return null;
  }
}

function tone(freq: number, start: number, dur: number, volume: number) {
  const ac = audio();
  if (!ac) return;
  const osc = ac.createOscillator();
  const gain = ac.createGain();
  osc.type = "sine";
  osc.frequency.value = freq;
  const t0 = ac.currentTime + start;
  gain.gain.setValueAtTime(0.0001, t0);
  gain.gain.exponentialRampToValueAtTime(volume, t0 + 0.018);
  gain.gain.exponentialRampToValueAtTime(0.0001, t0 + dur);
  osc.connect(gain);
  gain.connect(ac.destination);
  osc.start(t0);
  osc.stop(t0 + dur + 0.03);
}

export function unlockSounds() {
  audio();
}

export async function setSoundSink(deviceId: string) {
  const ac = audio();
  const ctxSink = ac as AudioContext & { setSinkId?: (id: string) => Promise<void> };
  if (ctxSink?.setSinkId) await ctxSink.setSinkId(deviceId).catch(() => undefined);
}

export function playCallJoin() {
  tone(587, 0, 0.11, 0.09);
  tone(880, 0.09, 0.16, 0.11);
}

export function playCallLeave() {
  tone(659, 0, 0.1, 0.09);
  tone(392, 0.09, 0.18, 0.1);
}

export async function playOutputTest(deviceId = "") {
  const ac = audio();
  if (!ac) return;
  const ctxSink = ac as AudioContext & { setSinkId?: (id: string) => Promise<void> };
  if (ctxSink.setSinkId) {
    await ctxSink.setSinkId(deviceId).catch(() => undefined);
    tone(880, 0, 0.28, 0.16);
    tone(1174, 0.2, 0.32, 0.14);
    return;
  }
  const dest = ac.createMediaStreamDestination();
  const osc = ac.createOscillator();
  const gain = ac.createGain();
  osc.type = "sine";
  osc.frequency.value = 880;
  const t0 = ac.currentTime;
  gain.gain.setValueAtTime(0.0001, t0);
  gain.gain.exponentialRampToValueAtTime(0.16, t0 + 0.02);
  gain.gain.exponentialRampToValueAtTime(0.0001, t0 + 0.42);
  osc.connect(gain);
  gain.connect(dest);
  const el = document.createElement("audio");
  el.autoplay = true;
  el.srcObject = dest.stream;
  const sink = el as HTMLAudioElement & { setSinkId?: (id: string) => Promise<void> };
  if (sink.setSinkId) await sink.setSinkId(deviceId).catch(() => undefined);
  await el.play().catch(() => undefined);
  osc.start(t0);
  osc.stop(t0 + 0.48);
  window.setTimeout(() => {
    el.srcObject = null;
  }, 700);
}
