use crate::peer::{send_signal, AppState};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264, MIME_TYPE_PCMU};
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::policy::bundle_policy::RTCBundlePolicy;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::signaling_state::RTCSignalingState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType,
};
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
use webrtc::rtp::codecs::h264::H264Packet;
use webrtc::rtp::packetizer::Depacketizer;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

const PCMU_HZ: u32 = 8000;
const FRAME_SAMPLES: usize = 160;
const VIDEO_MS: u64 = 100;
const TRACE: &str = "ui-call-trace";
const LINK: &str = "ui-call-link";
const VIDEO: &str = "ui-call-video";

#[derive(Default)]
pub struct RtcHub {
    inner: Mutex<Option<Session>>,
    trace_seq: AtomicU64,
}

struct Session {
    me: String,
    room: String,
    app: AppHandle,
    peers: HashMap<String, Peer>,
    tracks: Arc<StdMutex<Vec<Arc<TrackLocalStaticSample>>>>,
    cam_tracks: Arc<StdMutex<Vec<Arc<TrackLocalStaticSample>>>>,
    screen_tracks: Arc<StdMutex<Vec<Arc<TrackLocalStaticSample>>>>,
    cam_jpeg: Arc<StdMutex<Option<Vec<u8>>>>,
    screen_jpeg: Arc<StdMutex<Option<Vec<u8>>>>,
    play: Arc<StdMutex<VecDeque<i16>>>,
    running: Arc<AtomicBool>,
}

struct Peer {
    pc: Arc<RTCPeerConnection>,
    track: Arc<TrackLocalStaticSample>,
    cam: Arc<TrackLocalStaticSample>,
    screen: Arc<TrackLocalStaticSample>,
    making_offer: AtomicBool,
    pending_ice: StdMutex<Vec<RTCIceCandidateInit>>,
}

#[derive(Deserialize)]
pub struct RtcFrameIn {
    room: String,
    from: String,
    to: String,
    kind: String,
    sdp: Option<String>,
    candidate: Option<serde_json::Value>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CallTrace {
    id: u64,
    t: i64,
    mode: String,
    peer: Option<String>,
    event: String,
    level: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CallLink {
    phase: String,
    rtt_ms: Option<i64>,
    loss_pct: Option<f64>,
    jitter_ms: Option<i64>,
    peers: usize,
    live: usize,
    mode: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CallVideo {
    peer: String,
    screen: bool,
    jpeg: String,
}

impl RtcHub {
    pub fn new() -> Self {
        Self::default()
    }
}

fn state(app: &AppHandle) -> Arc<AppState> {
    app.state::<Arc<AppState>>().inner().clone()
}

fn polite(me: &str, peer: &str) -> bool {
    me > peer
}

fn now_ms() -> i64 {
    crate::crypto::now_ms()
}

fn trace(app: &AppHandle, hub: &RtcHub, event: &str, level: &str, peer: Option<&str>) {
    let id = hub.trace_seq.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = app.emit(
        TRACE,
        CallTrace {
            id,
            t: now_ms(),
            mode: "1:1".into(),
            peer: peer.map(str::to_string),
            event: event.into(),
            level: level.into(),
        },
    );
}

fn ice_servers() -> Vec<RTCIceServer> {
    vec![
        RTCIceServer {
            urls: vec![
                "stun:stun.l.google.com:19302".into(),
                "stun:stun1.l.google.com:19302".into(),
                "stun:stun.cloudflare.com:3478".into(),
            ],
            ..Default::default()
        },
        RTCIceServer {
            urls: vec![
                "turn:openrelay.metered.ca:80".into(),
                "turn:openrelay.metered.ca:443".into(),
            ],
            username: "openrelayproject".into(),
            credential: "openrelayproject".into(),
        },
    ]
}

fn pcmu_codec() -> RTCRtpCodecParameters {
    RTCRtpCodecParameters {
        capability: RTCRtpCodecCapability {
            mime_type: MIME_TYPE_PCMU.to_owned(),
            clock_rate: PCMU_HZ,
            channels: 1,
            ..Default::default()
        },
        payload_type: 0,
        ..Default::default()
    }
}

fn h264_codec() -> RTCRtpCodecParameters {
    RTCRtpCodecParameters {
        capability: RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90000,
            sdp_fmtp_line:
                "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".into(),
            ..Default::default()
        },
        payload_type: 125,
        ..Default::default()
    }
}

async fn new_pc() -> Result<RTCPeerConnection, String> {
    let mut media = MediaEngine::default();
    media
        .register_codec(pcmu_codec(), RTPCodecType::Audio)
        .map_err(err)?;
    media
        .register_codec(h264_codec(), RTPCodecType::Video)
        .map_err(err)?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media).map_err(err)?;
    let api = APIBuilder::new()
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .build();
    api.new_peer_connection(RTCConfiguration {
        ice_servers: ice_servers(),
        bundle_policy: RTCBundlePolicy::MaxBundle,
        ..Default::default()
    })
    .await
    .map_err(err)
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn linear_to_ulaw(sample: i16) -> u8 {
    const BIAS: i16 = 0x84;
    const CLIP: i16 = 32635;
    let mut pcm = sample;
    let sign = if pcm < 0 { 0x80u8 } else { 0 };
    if pcm < 0 {
        pcm = !pcm;
    }
    if pcm > CLIP {
        pcm = CLIP;
    }
    pcm = pcm.saturating_add(BIAS);
    let mut exp = 7u8;
    let mut mask = 0x4000i16;
    while exp > 0 {
        if pcm & mask != 0 {
            break;
        }
        exp -= 1;
        mask >>= 1;
    }
    let mantissa = ((pcm >> (exp + 3)) & 0x0F) as u8;
    !(sign | (exp << 4) | mantissa)
}

fn ulaw_to_linear(ulaw: u8) -> i16 {
    let ulaw = !ulaw;
    let exponent = (ulaw >> 4) & 0x07;
    let mantissa = ulaw & 0x0F;
    let mut sample = ((mantissa as i16) << 3) + 0x84;
    sample <<= exponent;
    sample -= 0x84;
    if ulaw & 0x80 != 0 {
        -sample
    } else {
        sample
    }
}

const PA_STREAM_PLAYBACK: i32 = 1;
const PA_STREAM_RECORD: i32 = 2;
const PA_SAMPLE_S16LE: i32 = 3;

#[repr(C)]
struct PaSampleSpec {
    format: i32,
    rate: u32,
    channels: u8,
}

enum PaSimple {}

extern "C" {
    fn pa_simple_new(
        server: *const i8,
        name: *const i8,
        dir: i32,
        dev: *const i8,
        stream_name: *const i8,
        ss: *const PaSampleSpec,
        map: *const u8,
        attr: *const u8,
        error: *mut i32,
    ) -> *mut PaSimple;
    fn pa_simple_read(s: *mut PaSimple, data: *mut u8, bytes: usize, error: *mut i32) -> i32;
    fn pa_simple_write(s: *mut PaSimple, data: *const u8, bytes: usize, error: *mut i32) -> i32;
    fn pa_simple_free(s: *mut PaSimple);
}

fn pulse_open(dir: i32, name: &str) -> Option<*mut PaSimple> {
    let spec = PaSampleSpec {
        format: PA_SAMPLE_S16LE,
        rate: PCMU_HZ,
        channels: 1,
    };
    let app = std::ffi::CString::new("chaincord").ok()?;
    let stream = std::ffi::CString::new(name).ok()?;
    let mut err = 0i32;
    let ptr = unsafe {
        pa_simple_new(
            std::ptr::null(),
            app.as_ptr(),
            dir,
            std::ptr::null(),
            stream.as_ptr(),
            &spec,
            std::ptr::null(),
            std::ptr::null(),
            &mut err,
        )
    };
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

fn spawn_pulse(
    app: AppHandle,
    running: Arc<AtomicBool>,
    tracks: Arc<StdMutex<Vec<Arc<TrackLocalStaticSample>>>>,
    play: Arc<StdMutex<VecDeque<i16>>>,
) -> bool {
    let rec_run = running.clone();
    let rec_app = app;
    let rec_tracks = tracks;
    std::thread::spawn(move || {
        let mut pcm = [0i16; FRAME_SAMPLES];
        while rec_run.load(Ordering::Relaxed) {
            let Some(rec) = pulse_open(PA_STREAM_RECORD, "mic") else {
                std::thread::sleep(Duration::from_millis(400));
                continue;
            };
            while rec_run.load(Ordering::Relaxed) {
                let mut err = 0i32;
                let rc = unsafe {
                    pa_simple_read(
                        rec,
                        pcm.as_mut_ptr() as *mut u8,
                        FRAME_SAMPLES * 2,
                        &mut err,
                    )
                };
                if rc < 0 {
                    break;
                }
                let muted = state(&rec_app).voice_flags().0;
                let frame: Vec<u8> = pcm
                    .iter()
                    .map(|s| if muted { 0xFF } else { linear_to_ulaw(*s) })
                    .collect();
                let list = rec_tracks.lock().map(|t| t.clone()).unwrap_or_default();
                if list.is_empty() {
                    continue;
                }
                tauri::async_runtime::spawn(async move {
                    let sample = Sample {
                        data: Bytes::from(frame),
                        duration: Duration::from_millis(20),
                        ..Default::default()
                    };
                    for track in list {
                        let _ = track.write_sample(&sample).await;
                    }
                });
            }
            unsafe { pa_simple_free(rec) };
            if rec_run.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    });

    if let Some(out) = pulse_open(PA_STREAM_PLAYBACK, "call") {
        let out_bits = out as usize;
        let play_run = running;
        std::thread::spawn(move || {
            let out = out_bits as *mut PaSimple;
            let mut pcm = [0i16; FRAME_SAMPLES];
            while play_run.load(Ordering::Relaxed) {
                {
                    let Ok(mut buf) = play.lock() else {
                        break;
                    };
                    for slot in pcm.iter_mut() {
                        *slot = buf.pop_front().unwrap_or(0);
                    }
                }
                let mut err = 0i32;
                let rc = unsafe {
                    pa_simple_write(
                        out,
                        pcm.as_ptr() as *const u8,
                        FRAME_SAMPLES * 2,
                        &mut err,
                    )
                };
                if rc < 0 {
                    break;
                }
            }
            unsafe { pa_simple_free(out) };
        });
    }
    true
}

fn video_sample_track(id: &str) -> Arc<TrackLocalStaticSample> {
    Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90000,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                .into(),
            ..Default::default()
        },
        id.into(),
        "chaincord".into(),
    ))
}

fn even_rgb(jpeg: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(jpeg).ok()?.to_rgb8();
    let (w, h) = img.dimensions();
    let w = w & !1;
    let h = h & !1;
    if w < 2 || h < 2 {
        return None;
    }
    let cropped = image::imageops::crop_imm(&img, 0, 0, w, h).to_image();
    Some((w, h, cropped.into_raw()))
}

fn rgb_jpeg(w: u32, h: u32, rgb: &[u8]) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 52)
        .encode(rgb, w, h, image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(buf)
}

fn jpeg_to_h264(encoder: &mut openh264::encoder::Encoder, jpeg: &[u8]) -> Option<Vec<u8>> {
    let (w, h, rgb) = even_rgb(jpeg)?;
    let src = openh264::formats::RgbSliceU8::new(&rgb, (w as usize, h as usize));
    let yuv = openh264::formats::YUVBuffer::from_rgb8_source(src);
    encoder.encode(&yuv).ok().map(|bs| bs.to_vec())
}

fn h264_to_jpeg(decoder: &mut openh264::decoder::Decoder, nals: &[u8]) -> Option<Vec<u8>> {
    use openh264::formats::YUVSource;
    let yuv = decoder.decode(nals).ok().flatten()?;
    let mut rgb = vec![0u8; yuv.estimate_rgb_u8_size()];
    yuv.write_rgb8(&mut rgb);
    let (w, h) = yuv.dimensions();
    rgb_jpeg(w as u32, h as u32, &rgb)
}

fn spawn_video_send(
    running: Arc<AtomicBool>,
    jpeg: Arc<StdMutex<Option<Vec<u8>>>>,
    tracks: Arc<StdMutex<Vec<Arc<TrackLocalStaticSample>>>>,
) {
    std::thread::spawn(move || {
        let Ok(mut encoder) = openh264::encoder::Encoder::new() else {
            return;
        };
        let mut ticks = 0u32;
        while running.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(VIDEO_MS));
            let Some(frame) = jpeg.lock().ok().and_then(|g| g.clone()) else {
                continue;
            };
            ticks += 1;
            if ticks % 24 == 1 {
                encoder.force_intra_frame();
            }
            let Some(h264) = jpeg_to_h264(&mut encoder, &frame) else {
                continue;
            };
            if h264.is_empty() {
                continue;
            }
            let list = tracks.lock().map(|t| t.clone()).unwrap_or_default();
            if list.is_empty() {
                continue;
            }
            tauri::async_runtime::spawn(async move {
                let sample = Sample {
                    data: Bytes::from(h264),
                    duration: Duration::from_millis(VIDEO_MS),
                    ..Default::default()
                };
                for track in list {
                    let _ = track.write_sample(&sample).await;
                }
            });
        }
    });
}

fn emit_video(app: &AppHandle, peer: &str, screen: bool, jpeg: &[u8]) {
    let _ = app.emit(
        VIDEO,
        CallVideo {
            peer: peer.into(),
            screen,
            jpeg: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, jpeg),
        },
    );
}

fn spawn_video_recv(
    app: AppHandle,
    traces: Arc<RtcHub>,
    peer: String,
    screen: bool,
) -> std::sync::mpsc::SyncSender<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(2);
    std::thread::spawn(move || {
        let Ok(mut decoder) = openh264::decoder::Decoder::new() else {
            return;
        };
        let mut seen = false;
        while let Ok(nals) = rx.recv() {
            let Some(jpeg) = h264_to_jpeg(&mut decoder, &nals) else {
                continue;
            };
            if !seen {
                seen = true;
                trace(
                    &app,
                    traces.as_ref(),
                    if screen {
                        "tela chegou"
                    } else {
                        "vídeo chegou"
                    },
                    "ok",
                    Some(&peer),
                );
            }
            emit_video(&app, &peer, screen, &jpeg);
        }
    });
    tx
}

async fn bind_pc(
    app: AppHandle,
    hub_traces: Arc<RtcHub>,
    me: String,
    room: String,
    peer: String,
    pc: Arc<RTCPeerConnection>,
    play: Arc<StdMutex<VecDeque<i16>>>,
    heard: Arc<AtomicBool>,
) {
    let app_ice = app.clone();
    let me_ice = me.clone();
    let room_ice = room.clone();
    let peer_ice = peer.clone();
    pc.on_ice_candidate(Box::new(move |cand| {
        let app = app_ice.clone();
        let me = me_ice.clone();
        let room = room_ice.clone();
        let peer = peer_ice.clone();
        Box::pin(async move {
            let candidate = match cand {
                Some(c) => match c.to_json() {
                    Ok(init) => serde_json::json!({
                        "candidate": init.candidate,
                        "sdpMid": init.sdp_mid,
                        "sdpMLineIndex": init.sdp_mline_index,
                        "usernameFragment": init.username_fragment,
                    }),
                    Err(_) => return,
                },
                None => serde_json::Value::Null,
            };
            let _ = send_signal(
                &app,
                serde_json::json!({
                    "type": "rtc",
                    "room": room,
                    "from": me,
                    "to": peer,
                    "kind": "ice",
                    "candidate": candidate,
                }),
            );
        })
    }));

    let app_pc = app.clone();
    let traces = hub_traces.clone();
    let peer_pc = peer.clone();
    pc.on_peer_connection_state_change(Box::new(move |st| {
        let app = app_pc.clone();
        let traces = traces.clone();
        let peer = peer_pc.clone();
        Box::pin(async move {
            let (event, level) = match st {
                RTCPeerConnectionState::Connecting => ("conectando", "info"),
                RTCPeerConnectionState::Connected => ("enlace ok", "ok"),
                RTCPeerConnectionState::Disconnected => ("enlace caiu", "warn"),
                RTCPeerConnectionState::Failed => ("enlace falhou", "err"),
                RTCPeerConnectionState::Closed => ("enlace fechou", "err"),
                _ => return,
            };
            trace(&app, traces.as_ref(), event, level, Some(&peer));
            emit_link(&app, traces.as_ref()).await;
        })
    }));

    let app_ice_st = app.clone();
    let traces_ice = hub_traces.clone();
    let peer_ice_st = peer.clone();
    pc.on_ice_connection_state_change(Box::new(move |st| {
        let app = app_ice_st.clone();
        let traces = traces_ice.clone();
        let peer = peer_ice_st.clone();
        Box::pin(async move {
            let (event, level) = match st {
                RTCIceConnectionState::Checking => ("ICE negociando", "info"),
                RTCIceConnectionState::Connected | RTCIceConnectionState::Completed => {
                    ("ICE ok", "ok")
                }
                RTCIceConnectionState::Disconnected => ("ICE caiu", "warn"),
                RTCIceConnectionState::Failed => ("ICE falhou", "err"),
                RTCIceConnectionState::Closed => ("ICE fechou", "err"),
                _ => return,
            };
            trace(&app, traces.as_ref(), event, level, Some(&peer));
        })
    }));

    let app_tr = app.clone();
    let traces_tr = hub_traces;
    let peer_tr = peer;
    let play_tr = play;
    let heard_tr = heard;
    let deafened_app = app.clone();
    pc.on_track(Box::new(move |track, _recv, xcvr| {
        let app = app_tr.clone();
        let traces = traces_tr.clone();
        let peer = peer_tr.clone();
        let play = play_tr.clone();
        let heard = heard_tr.clone();
        let deafened_app = deafened_app.clone();
        Box::pin(async move {
            if track.kind() != RTPCodecType::Audio {
                let mid = xcvr.mid();
                let screen = matches!(mid.as_deref(), Some("2"));
                let tx = spawn_video_recv(app, traces, peer, screen);
                let mut depacketizer = H264Packet::default();
                let mut acc = Vec::new();
                loop {
                    let Ok((pkt, _)) = track.read_rtp().await else {
                        break;
                    };
                    if pkt.payload.is_empty() {
                        continue;
                    }
                    if let Ok(nal) = depacketizer.depacketize(&pkt.payload) {
                        acc.extend_from_slice(&nal);
                    }
                    if pkt.header.marker && !acc.is_empty() {
                        let _ = tx.try_send(std::mem::take(&mut acc));
                    }
                    if acc.len() > 1_000_000 {
                        acc.clear();
                    }
                }
                return;
            }
            loop {
                let Ok((pkt, _)) = track.read_rtp().await else {
                    break;
                };
                if pkt.payload.is_empty() {
                    continue;
                }
                if !heard.swap(true, Ordering::Relaxed) {
                    trace(
                        &app,
                        traces.as_ref(),
                        "áudio chegou",
                        "ok",
                        Some(&peer),
                    );
                }
                if state(&deafened_app).voice_flags().1 {
                    continue;
                }
                if let Ok(mut buf) = play.lock() {
                    for b in pkt.payload.iter() {
                        buf.push_back(ulaw_to_linear(*b));
                    }
                    if buf.len() > PCMU_HZ as usize * 2 {
                        let extra = buf.len() - PCMU_HZ as usize;
                        buf.drain(..extra);
                    }
                }
            }
        })
    }));
}

async fn emit_link(app: &AppHandle, hub: &RtcHub) {
    let guard = hub.inner.lock().await;
    let Some(session) = guard.as_ref() else {
        return;
    };
    let mut live = 0usize;
    let mut failed = 0usize;
    let mut connecting = 0usize;
    let n = session.peers.len();
    for peer in session.peers.values() {
        match peer.pc.connection_state() {
            RTCPeerConnectionState::Connected => live += 1,
            RTCPeerConnectionState::Failed => failed += 1,
            _ => connecting += 1,
        }
    }
    let phase = if n == 0 {
        "connected"
    } else if live == 0 && failed > 0 {
        "failed"
    } else if live == 0 {
        "connecting"
    } else if failed > 0 || connecting > 0 {
        "unstable"
    } else {
        "connected"
    };
    let _ = app.emit(
        LINK,
        CallLink {
            phase: phase.into(),
            rtt_ms: None,
            loss_pct: Some(0.0),
            jitter_ms: None,
            peers: n,
            live,
            mode: if n <= 1 { "1:1" } else { "mesh" }.into(),
        },
    );
}

async fn ensure_peer(hub: Arc<RtcHub>, peer: &str) -> Result<Arc<RTCPeerConnection>, String> {
    {
        let guard = hub.inner.lock().await;
        if let Some(session) = guard.as_ref() {
            if let Some(existing) = session.peers.get(peer) {
                return Ok(existing.pc.clone());
            }
        }
    }
    let (app, me, room, play, tracks, cam_tracks, screen_tracks) = {
        let guard = hub.inner.lock().await;
        let session = guard.as_ref().ok_or("call nativa parada")?;
        (
            session.app.clone(),
            session.me.clone(),
            session.room.clone(),
            session.play.clone(),
            session.tracks.clone(),
            session.cam_tracks.clone(),
            session.screen_tracks.clone(),
        )
    };
    let pc = Arc::new(new_pc().await?);
    let audio = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_PCMU.to_owned(),
            clock_rate: PCMU_HZ,
            channels: 1,
            ..Default::default()
        },
        "audio".into(),
        "chaincord".into(),
    ));
    let cam = video_sample_track("cam");
    let screen = video_sample_track("screen");
    bind_pc(
        app.clone(),
        hub.clone(),
        me,
        room,
        peer.to_string(),
        pc.clone(),
        play,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    if pc.get_transceivers().await.is_empty() {
        for (track, id) in [
            (
                Arc::clone(&audio) as Arc<dyn TrackLocal + Send + Sync>,
                "audio",
            ),
            (Arc::clone(&cam) as Arc<dyn TrackLocal + Send + Sync>, "cam"),
            (
                Arc::clone(&screen) as Arc<dyn TrackLocal + Send + Sync>,
                "screen",
            ),
        ] {
            let _ = id;
            pc.add_transceiver_from_track(
                track,
                Some(RTCRtpTransceiverInit {
                    direction: RTCRtpTransceiverDirection::Sendrecv,
                    send_encodings: vec![],
                }),
            )
            .await
            .map_err(err)?;
        }
    }

    {
        let mut guard = hub.inner.lock().await;
        let session = guard.as_mut().ok_or("call nativa parada")?;
        if let Some(existing) = session.peers.get(peer) {
            let _ = pc.close().await;
            return Ok(existing.pc.clone());
        }
        session.peers.insert(
            peer.to_string(),
            Peer {
                pc: pc.clone(),
                track: audio.clone(),
                cam: cam.clone(),
                screen: screen.clone(),
                making_offer: AtomicBool::new(false),
                pending_ice: StdMutex::new(Vec::new()),
            },
        );
        if let Ok(mut list) = tracks.lock() {
            list.push(audio);
        }
        if let Ok(mut list) = cam_tracks.lock() {
            list.push(cam);
        }
        if let Ok(mut list) = screen_tracks.lock() {
            list.push(screen);
        }
    }
    trace(&app, hub.as_ref(), "enlace aberto", "info", Some(peer));
    Ok(pc)
}

async fn flush_ice(peer: &Peer) {
    let Some(remote) = peer.pc.remote_description().await else {
        return;
    };
    let _ = remote;
    let queued = {
        let Ok(mut q) = peer.pending_ice.lock() else {
            return;
        };
        std::mem::take(&mut *q)
    };
    for cand in queued {
        let _ = peer.pc.add_ice_candidate(cand).await;
    }
}

async fn send_local(
    app: &AppHandle,
    me: &str,
    room: &str,
    peer: &str,
    kind: &str,
    pc: &RTCPeerConnection,
) {
    tokio::time::sleep(Duration::from_millis(400)).await;
    let Some(desc) = pc.local_description().await else {
        return;
    };
    let want = if kind == "offer" {
        desc.sdp_type == webrtc::peer_connection::sdp::sdp_type::RTCSdpType::Offer
    } else {
        desc.sdp_type == webrtc::peer_connection::sdp::sdp_type::RTCSdpType::Answer
    };
    if !want {
        return;
    }
    let _ = send_signal(
        app,
        serde_json::json!({
            "type": "rtc",
            "room": room,
            "from": me,
            "to": peer,
            "kind": kind,
            "sdp": desc.sdp,
        }),
    );
}

async fn offer_now(hub: Arc<RtcHub>, peer: &str) -> Result<(), String> {
    let (app, me, room, pc) = {
        let guard = hub.inner.lock().await;
        let session = guard.as_ref().ok_or("call nativa parada")?;
        if polite(&session.me, peer) {
            return Ok(());
        }
        let p = session.peers.get(peer).ok_or("peer ausente")?;
        if p.pc.signaling_state() != RTCSignalingState::Stable {
            return Ok(());
        }
        p.making_offer.store(true, Ordering::Relaxed);
        (
            session.app.clone(),
            session.me.clone(),
            session.room.clone(),
            p.pc.clone(),
        )
    };
    let result = async {
        let offer = pc.create_offer(None).await.map_err(err)?;
        if pc.signaling_state() != RTCSignalingState::Stable {
            return Ok(());
        }
        pc.set_local_description(offer).await.map_err(err)?;
        send_local(&app, &me, &room, peer, "offer", pc.as_ref()).await;
        trace(&app, hub.as_ref(), "oferta enviada", "info", Some(peer));
        Ok(())
    }
    .await;
    if let Some(session) = hub.inner.lock().await.as_ref() {
        if let Some(p) = session.peers.get(peer) {
            p.making_offer.store(false, Ordering::Relaxed);
        }
    }
    result
}

async fn drop_peer(hub: &RtcHub, peer: &str, silent: bool) {
    let (app, me, room, pc, track, cam, screen) = {
        let mut guard = hub.inner.lock().await;
        let Some(session) = guard.as_mut() else {
            return;
        };
        let Some(p) = session.peers.remove(peer) else {
            return;
        };
        if let Ok(mut list) = session.tracks.lock() {
            list.retain(|t| !Arc::ptr_eq(t, &p.track));
        }
        if let Ok(mut list) = session.cam_tracks.lock() {
            list.retain(|t| !Arc::ptr_eq(t, &p.cam));
        }
        if let Ok(mut list) = session.screen_tracks.lock() {
            list.retain(|t| !Arc::ptr_eq(t, &p.screen));
        }
        (
            session.app.clone(),
            session.me.clone(),
            session.room.clone(),
            p.pc,
            p.track,
            p.cam,
            p.screen,
        )
    };
    let _ = (track, cam, screen);
    if !silent {
        let _ = send_signal(
            &app,
            serde_json::json!({
                "type": "rtc",
                "room": room,
                "from": me,
                "to": peer,
                "kind": "bye",
            }),
        );
    }
    let _ = pc.close().await;
    trace(&app, hub, "enlace fechado", "warn", Some(peer));
}

pub async fn start(app: &AppHandle, room: String, me: String) -> Result<(), String> {
    let hub = state(app).rtc.clone();
    {
        let mut guard = hub.inner.lock().await;
        if let Some(old) = guard.take() {
            old.running.store(false, Ordering::Relaxed);
            for p in old.peers.values() {
                let _ = p.pc.close().await;
            }
        }
        let play = Arc::new(StdMutex::new(VecDeque::new()));
        let tracks = Arc::new(StdMutex::new(Vec::new()));
        let cam_tracks = Arc::new(StdMutex::new(Vec::new()));
        let screen_tracks = Arc::new(StdMutex::new(Vec::new()));
        let cam_jpeg = Arc::new(StdMutex::new(None));
        let screen_jpeg = Arc::new(StdMutex::new(None));
        let running = Arc::new(AtomicBool::new(true));
        if !spawn_pulse(
            app.clone(),
            running.clone(),
            tracks.clone(),
            play.clone(),
        ) {
            trace(app, &hub, "mic nativo indisponível", "warn", None);
        }
        spawn_video_send(running.clone(), cam_jpeg.clone(), cam_tracks.clone());
        spawn_video_send(running.clone(), screen_jpeg.clone(), screen_tracks.clone());
        let hub_link = hub.clone();
        let app_link = app.clone();
        let run_link = running.clone();
        tauri::async_runtime::spawn(async move {
            while run_link.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_secs(1)).await;
                emit_link(&app_link, hub_link.as_ref()).await;
            }
        });
        *guard = Some(Session {
            me,
            room,
            app: app.clone(),
            peers: HashMap::new(),
            tracks,
            cam_tracks,
            screen_tracks,
            cam_jpeg,
            screen_jpeg,
            play,
            running,
        });
    }
    trace(app, &hub, "motor nativo (WebKit sem WebRTC)", "info", None);
    Ok(())
}

pub async fn stop(app: &AppHandle) -> Result<(), String> {
    let hub = state(app).rtc.clone();
    let old = hub.inner.lock().await.take();
    if let Some(old) = old {
        old.running.store(false, Ordering::Relaxed);
        for (peer, p) in old.peers {
            let _ = send_signal(
                &old.app,
                serde_json::json!({
                    "type": "rtc",
                    "room": old.room,
                    "from": old.me,
                    "to": peer,
                    "kind": "bye",
                }),
            );
            let _ = p.pc.close().await;
        }
    }
    Ok(())
}

pub async fn sync(app: &AppHandle, peers: Vec<String>) -> Result<(), String> {
    let hub = state(app).rtc.clone();
    let want: Vec<String> = {
        let guard = hub.inner.lock().await;
        let session = guard.as_ref().ok_or("call nativa parada")?;
        peers
            .into_iter()
            .filter(|p| !p.is_empty() && p != &session.me)
            .collect()
    };
    let existing: Vec<String> = {
        let guard = hub.inner.lock().await;
        guard
            .as_ref()
            .map(|s| s.peers.keys().cloned().collect())
            .unwrap_or_default()
    };
    for id in existing.iter().filter(|id| !want.contains(id)) {
        drop_peer(&hub, id, false).await;
    }
    for id in &want {
        let created = {
            let guard = hub.inner.lock().await;
            !guard
                .as_ref()
                .map(|s| s.peers.contains_key(id))
                .unwrap_or(true)
        };
        ensure_peer(hub.clone(), id).await?;
        if created {
            offer_now(hub.clone(), id).await?;
        }
    }
    emit_link(app, &hub).await;
    Ok(())
}

pub async fn handle(app: &AppHandle, frame: RtcFrameIn) -> Result<(), String> {
    let hub = state(app).rtc.clone();
    let (me, room) = {
        let guard = hub.inner.lock().await;
        let session = guard.as_ref().ok_or("call nativa parada")?;
        if frame.room != session.room || frame.from == session.me || frame.to != session.me {
            return Ok(());
        }
        (session.me.clone(), session.room.clone())
    };
    if frame.kind == "bye" {
        drop_peer(&hub, &frame.from, true).await;
        return Ok(());
    }
    let pc = ensure_peer(hub.clone(), &frame.from).await?;
    if frame.kind == "offer" {
        if let Some(sdp) = frame.sdp {
            let collision = {
                let guard = hub.inner.lock().await;
                let session = guard.as_ref().ok_or("call nativa parada")?;
                let p = session.peers.get(&frame.from).ok_or("peer ausente")?;
                p.making_offer.load(Ordering::Relaxed)
                    || p.pc.signaling_state() != RTCSignalingState::Stable
            };
            if collision && !polite(&me, &frame.from) {
                return Ok(());
            }
            let desc = RTCSessionDescription::offer(sdp).map_err(err)?;
            pc.set_remote_description(desc).await.map_err(err)?;
            if let Some(session) = hub.inner.lock().await.as_ref() {
                if let Some(p) = session.peers.get(&frame.from) {
                    flush_ice(p).await;
                }
            }
            trace(app, &hub, "oferta recebida", "info", Some(&frame.from));
            let answer = pc.create_answer(None).await.map_err(err)?;
            pc.set_local_description(answer).await.map_err(err)?;
            send_local(app, &me, &room, &frame.from, "answer", pc.as_ref()).await;
            trace(app, &hub, "resposta enviada", "info", Some(&frame.from));
        }
    } else if frame.kind == "answer" {
        if let Some(sdp) = frame.sdp {
            if pc.signaling_state() == RTCSignalingState::HaveLocalOffer {
                let desc = RTCSessionDescription::answer(sdp).map_err(err)?;
                pc.set_remote_description(desc).await.map_err(err)?;
                if let Some(session) = hub.inner.lock().await.as_ref() {
                    if let Some(p) = session.peers.get(&frame.from) {
                        flush_ice(p).await;
                    }
                }
                trace(app, &hub, "resposta recebida", "ok", Some(&frame.from));
            }
        }
    } else if frame.kind == "ice" {
        let init = match frame.candidate {
            None | Some(serde_json::Value::Null) => RTCIceCandidateInit::default(),
            Some(value) => serde_json::from_value(value).unwrap_or_default(),
        };
        if pc.remote_description().await.is_none() {
            if let Some(session) = hub.inner.lock().await.as_ref() {
                if let Some(p) = session.peers.get(&frame.from) {
                    if let Ok(mut q) = p.pending_ice.lock() {
                        q.push(init);
                    }
                }
            }
            return Ok(());
        }
        let _ = pc.add_ice_candidate(init).await;
    }
    Ok(())
}

pub async fn push_frame(app: &AppHandle, screen: bool, jpeg: String) -> Result<(), String> {
    let hub = state(app).rtc.clone();
    let bytes = if jpeg.is_empty() {
        None
    } else {
        Some(
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, jpeg.as_bytes())
                .map_err(err)?,
        )
    };
    let guard = hub.inner.lock().await;
    let Some(session) = guard.as_ref() else {
        return Ok(());
    };
    let slot = if screen {
        &session.screen_jpeg
    } else {
        &session.cam_jpeg
    };
    if let Ok(mut g) = slot.lock() {
        *g = bytes;
    }
    Ok(())
}
