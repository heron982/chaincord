import { useEffect, useState } from "react";
import {
  audioDeviceLabel,
  audioDevicesOfKind,
  audioInputConstraint,
  audioSinkSupported,
  resolveAudioDeviceId,
  type AudioDeviceHint,
} from "./logic";
import { playOutputTest, unlockSounds } from "./sounds";

export function AudioSettings({
  inputId,
  outputId,
  liveInput,
  onInput,
  onOutput,
  onClose,
}: {
  inputId: string;
  outputId: string;
  liveInput: MediaStream | null;
  onInput: (id: string) => void | Promise<void>;
  onOutput: (id: string) => void;
  onClose: () => void;
}) {
  const [inputs, setInputs] = useState<AudioDeviceHint[]>([]);
  const [outputs, setOutputs] = useState<AudioDeviceHint[]>([]);
  const [preview, setPreview] = useState<MediaStream | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const sinkOk = audioSinkSupported();
  const meterStream = liveInput ?? preview;
  const listedInput = resolveAudioDeviceId(inputs, inputId);
  const listedOutput = resolveAudioDeviceId(outputs, outputId);

  useEffect(() => {
    let dead = false;
    const load = async () => {
      if (!navigator.mediaDevices?.enumerateDevices) {
        setErr("This device can't list audio hardware.");
        return;
      }
      try {
        const list = await navigator.mediaDevices.enumerateDevices();
        if (dead) return;
        setInputs(audioDevicesOfKind(list, "audioinput"));
        setOutputs(audioDevicesOfKind(list, "audiooutput"));
      } catch {
        if (!dead) setErr("Couldn't list audio devices.");
      }
    };
    void load();
    navigator.mediaDevices?.addEventListener("devicechange", load);
    return () => {
      dead = true;
      navigator.mediaDevices?.removeEventListener("devicechange", load);
    };
  }, [inputId, liveInput]);

  useEffect(() => {
    if (liveInput) {
      setPreview(null);
      return;
    }
    if (!navigator.mediaDevices?.getUserMedia) return;
    let dead = false;
    let stream: MediaStream | null = null;
    void (async () => {
      try {
        stream = await navigator.mediaDevices.getUserMedia({
          audio: audioInputConstraint(inputId),
        });
        if (dead) {
          stream.getTracks().forEach((track) => track.stop());
          return;
        }
        setPreview(stream);
        setErr(null);
        const list = await navigator.mediaDevices.enumerateDevices();
        if (dead) return;
        setInputs(audioDevicesOfKind(list, "audioinput"));
        setOutputs(audioDevicesOfKind(list, "audiooutput"));
      } catch {
        if (!dead) setErr("Allow the microphone to see devices and test audio.");
      }
    })();
    return () => {
      dead = true;
      stream?.getTracks().forEach((track) => track.stop());
    };
  }, [inputId, liveInput]);

  const changeInput = (id: string) => {
    setErr(null);
    void Promise.resolve(onInput(id)).catch(() => {
      setErr("Couldn't use this microphone.");
    });
  };

  return (
    <div className="stack audio-settings">
      <h2>Audio</h2>
      <p>If call mic or sound is on the wrong device, pick it here.</p>
      <label className="field">
        <span>Microphone</span>
        <select
          value={listedInput}
          onChange={(e) => changeInput(e.target.value)}
        >
          <option value="">System default</option>
          {inputs.map((device, i) => (
            <option key={device.deviceId} value={device.deviceId}>
              {audioDeviceLabel(device, i, "input")}
            </option>
          ))}
        </select>
      </label>
      <div className="field">
        <span>Input level</span>
        <MicMeter stream={meterStream} />
        <p className="field-hint">
          Say something. If the bar doesn't move, this isn't the right mic — or Windows is
          using the communications device.
        </p>
      </div>
      <label className="field">
        <span>Output</span>
        <select
          value={listedOutput}
          disabled={!sinkOk && outputs.length === 0}
          onChange={(e) => onOutput(e.target.value)}
        >
          <option value="">System default</option>
          {outputs.map((device, i) => (
            <option key={device.deviceId} value={device.deviceId}>
              {audioDeviceLabel(device, i, "output")}
            </option>
          ))}
        </select>
      </label>
      {!sinkOk && (
        <p className="field-hint">
          This device uses the system default output. Headphones may not show up in the list.
        </p>
      )}
      {err && <p className="sys error">{err}</p>}
      <div className="row">
        <button
          type="button"
          className="secondary"
          disabled={testing}
          onClick={() => {
            unlockSounds();
            setTesting(true);
            void playOutputTest(outputId).finally(() => setTesting(false));
          }}
        >
          Test sound
        </button>
        <button type="button" className="ghost" onClick={onClose}>
          Close
        </button>
      </div>
    </div>
  );
}

function MicMeter({ stream }: { stream: MediaStream | null }) {
  const [level, setLevel] = useState(0);
  const trackId = stream?.getAudioTracks().map((track) => track.id).join(",") ?? "";
  useEffect(() => {
    if (!stream?.getAudioTracks().length) {
      setLevel(0);
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
        setLevel(Math.min(100, Math.round((avg / 48) * 100)));
        raf = requestAnimationFrame(loop);
      };
      raf = requestAnimationFrame(loop);
    } catch {
      setLevel(0);
      return;
    }
    return () => {
      cancelAnimationFrame(raf);
      src?.disconnect();
      void ctx?.close();
    };
  }, [stream, trackId]);
  return (
    <div className="mic-meter" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={level}>
      <i style={{ width: `${level}%` }} />
    </div>
  );
}
