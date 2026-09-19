use crate::peer::{send_signal, AppState};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264, MIME_TYPE_PCMU};
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::offer_answer_options::RTCOfferOptions;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::policy::bundle_policy::RTCBundlePolicy;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::signaling_state::RTCSignalingState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType,
};
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::{RTCPFeedback, RTCRtpTransceiverInit};
use webrtc::rtp::codecs::h264::H264Packet;
use webrtc::rtp::packetizer::Depacketizer;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

const PCMU_HZ: u32 = 8000;
const FRAME_SAMPLES: usize = 160;
const VIDEO_MS: u64 = 100;
const LINK: &str = "ui-call-link";
const VIDEO: &str = "ui-call-video";

#[derive(Default)]
pub struct RtcHub {
    inner: Mutex<Option<Session>>,
    boot: Mutex<HashMap<String, Arc<Mutex<()>>>>,
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
    screen_share: Arc<AtomicBool>,
    play: Arc<StdMutex<MixBuf>>,
    running: Arc<AtomicBool>,
    hub: Option<String>,
    others: Vec<String>,
    last_sig: HashMap<String, i64>,
    last_sess: HashMap<String, String>,
}

struct MixBuf {
    acc: [i32; FRAME_SAMPLES],
}

struct Peer {
    pc: Arc<RTCPeerConnection>,
    track: Arc<TrackLocalStaticSample>,
    cam: Arc<TrackLocalStaticSample>,
    screen: Arc<TrackLocalStaticSample>,
    extras: Vec<String>,
    fwd_audio: HashMap<String, Arc<TrackLocalStaticSample>>,
    fwd_cam: HashMap<String, Arc<TrackLocalStaticSample>>,
    fwd_screen: HashMap<String, Arc<TrackLocalStaticSample>>,
    making_offer: AtomicBool,
    ice_retry: AtomicU64,
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
    ts: Option<i64>,
    sess: Option<String>,
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

fn wanted_peers(me: &str, others: &[String], hub: Option<&str>) -> Vec<String> {
    if hub.is_none() || others.len() < 2 {
        return others.to_vec();
    }
    let hub = hub.unwrap();
    if me == hub {
        others.to_vec()
    } else if others.iter().any(|p| p == hub) {
        vec![hub.to_string()]
    } else {
        others.to_vec()
    }
}

fn forward_peers(hub: &str, remote: &str, me: &str, others: &[String]) -> Vec<String> {
    let mut all: Vec<String> = others.to_vec();
    all.push(me.to_string());
    all.retain(|p| !p.is_empty() && p != hub && p != remote);
    all.sort();
    all.dedup();
    all
}

fn slot_peer<'a>(index: usize, remote: &'a str, extras: &'a [String]) -> &'a str {
    if index < 3 {
        remote
    } else {
        extras
            .get((index - 3) / 3)
            .map(String::as_str)
            .unwrap_or(remote)
    }
}

fn mix_ulaw_frame(buf: &mut MixBuf, payload: &[u8]) {
    for (i, b) in payload.iter().take(FRAME_SAMPLES).enumerate() {
        buf.acc[i] = buf.acc[i].saturating_add(i32::from(ulaw_to_linear(*b)));
    }
}

fn take_mix_frame(buf: &mut MixBuf) -> [i16; FRAME_SAMPLES] {
    let mut pcm = [0i16; FRAME_SAMPLES];
    for i in 0..FRAME_SAMPLES {
        pcm[i] = buf.acc[i].clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        buf.acc[i] = 0;
    }
    pcm
}

fn now_ms() -> i64 {
    crate::crypto::now_ms()
}

fn trace(app: &AppHandle, hub: &RtcHub, event: &str, level: &str, peer: Option<&str>) {
    let _ = hub.trace_seq.fetch_add(1, Ordering::Relaxed);
    crate::log::append_call(now_ms(), "call", peer, event, level);
    let _ = app;
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

fn h264_feedback() -> Vec<RTCPFeedback> {
    vec![
        RTCPFeedback {
            typ: "goog-remb".into(),
            parameter: String::new(),
        },
        RTCPFeedback {
            typ: "ccm".into(),
            parameter: "fir".into(),
        },
        RTCPFeedback {
            typ: "nack".into(),
            parameter: String::new(),
        },
        RTCPFeedback {
            typ: "nack".into(),
            parameter: "pli".into(),
        },
    ]
}

fn h264_cap() -> RTCRtpCodecCapability {
    RTCRtpCodecCapability {
        mime_type: MIME_TYPE_H264.to_owned(),
        clock_rate: 90000,
        sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
            .into(),
        rtcp_feedback: h264_feedback(),
        ..Default::default()
    }
}

fn h264_codec_at(pt: u8, profile: &str, mode: u8) -> RTCRtpCodecParameters {
    RTCRtpCodecParameters {
        capability: RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90000,
            sdp_fmtp_line: format!(
                "level-asymmetry-allowed=1;packetization-mode={mode};profile-level-id={profile}"
            ),
            rtcp_feedback: h264_feedback(),
            ..Default::default()
        },
        payload_type: pt,
        ..Default::default()
    }
}

async fn new_pc() -> Result<RTCPeerConnection, String> {
    let mut media = MediaEngine::default();
    media
        .register_codec(pcmu_codec(), RTPCodecType::Audio)
        .map_err(err)?;
    media
        .register_codec(h264_codec_at(102, "42e01f", 1), RTPCodecType::Video)
        .map_err(err)?;
    media
        .register_codec(h264_codec_at(103, "42001f", 1), RTPCodecType::Video)
        .map_err(err)?;
    media
        .register_codec(h264_codec_at(104, "4d001f", 1), RTPCodecType::Video)
        .map_err(err)?;
    media
        .register_codec(h264_codec_at(105, "42e01f", 0), RTPCodecType::Video)
        .map_err(err)?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media).map_err(err)?;
    let mut settings = SettingEngine::default();
    settings.set_ice_timeouts(
        Some(Duration::from_secs(15)),
        Some(Duration::from_secs(60)),
        Some(Duration::from_secs(2)),
    );
    let api = APIBuilder::new()
        .with_setting_engine(settings)
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
    play: Arc<StdMutex<MixBuf>>,
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
                    pcm = take_mix_frame(&mut buf);
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
        h264_cap(),
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
    let need = w.checked_mul(h)?.checked_mul(3)? as usize;
    if rgb.len() < need {
        return None;
    }
    let mut img = image::RgbImage::from_raw(w, h, rgb[..need].to_vec())?;
    if w > 640 {
        let nw = 640u32 & !1;
        let nh = ((h as u64 * nw as u64) / w as u64) as u32 & !1;
        img = image::imageops::resize(
            &img,
            nw.max(2),
            nh.max(2),
            image::imageops::FilterType::Triangle,
        );
    }
    let (ow, oh) = img.dimensions();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 40)
        .encode(img.as_raw(), ow, oh, image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(buf)
}

fn grab_screen_gtk() -> Option<Vec<u8>> {
    use gdk::prelude::*;
    let screen = gdk::Screen::default()?;
    let root = screen.root_window()?;
    let w = root.width();
    let h = root.height();
    if w < 2 || h < 2 {
        return None;
    }
    let pix = root.pixbuf(0, 0, w, h)?;
    let pw = pix.width() as u32;
    let ph = pix.height() as u32;
    let stride = pix.rowstride() as usize;
    let ch = pix.n_channels() as usize;
    if ch < 3 {
        return None;
    }
    let data = pix.pixel_bytes()?;
    let bytes = data.as_ref();
    let mut rgb = Vec::with_capacity((pw * ph * 3) as usize);
    for y in 0..ph as usize {
        let row = bytes.get(y * stride..)?;
        for x in 0..pw as usize {
            let i = x * ch;
            rgb.extend_from_slice(row.get(i..i + 3)?);
        }
    }
    rgb_jpeg(pw, ph, &rgb)
}

fn grab_screen_on_main(app: &AppHandle) -> Option<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let _ = app.run_on_main_thread(move || {
        let _ = tx.send(grab_screen_gtk());
    });
    rx.recv_timeout(Duration::from_millis(500)).ok().flatten()
}

fn spawn_screen_cap(
    app: AppHandle,
    traces: Arc<RtcHub>,
    me: String,
    running: Arc<AtomicBool>,
    sharing: Arc<AtomicBool>,
    jpeg: Arc<StdMutex<Option<Vec<u8>>>>,
) {
    std::thread::spawn(move || {
        let mut announced = false;
        while running.load(Ordering::Relaxed) {
            if !sharing.load(Ordering::Relaxed) {
                announced = false;
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            let Some(frame) = grab_screen_on_main(&app) else {
                if !announced {
                    announced = true;
                    trace(
                        &app,
                        traces.as_ref(),
                        "captura de tela falhou",
                        "warn",
                        None,
                    );
                }
                std::thread::sleep(Duration::from_millis(400));
                continue;
            };
            if !announced {
                announced = true;
                trace(&app, traces.as_ref(), "tela nativa enviando", "ok", None);
            }
            if let Ok(mut g) = jpeg.lock() {
                *g = Some(frame.clone());
            }
            emit_video(&app, &me, true, &frame);
            std::thread::sleep(Duration::from_millis(VIDEO_MS));
        }
    });
}

fn jpeg_to_h264(encoder: &mut openh264::encoder::Encoder, jpeg: &[u8]) -> Option<Vec<u8>> {
    let (w, h, rgb) = even_rgb(jpeg)?;
    let src = openh264::formats::RgbSliceU8::new(&rgb, (w as usize, h as usize));
    let yuv = openh264::formats::YUVBuffer::from_rgb8_source(src);
    encoder.encode(&yuv).ok().map(|bs| bs.to_vec())
}

fn video_encoder(screen: bool) -> Result<openh264::encoder::Encoder, openh264::Error> {
    let cfg = openh264::encoder::EncoderConfig::new()
        .set_bitrate_bps(if screen { 1_800_000 } else { 800_000 })
        .max_frame_rate(10.0)
        .enable_skip_frame(false)
        .rate_control_mode(openh264::encoder::RateControlMode::Bitrate)
        .usage_type(if screen {
            openh264::encoder::UsageType::ScreenContentRealTime
        } else {
            openh264::encoder::UsageType::CameraVideoRealTime
        });
    openh264::encoder::Encoder::with_api_config(openh264::OpenH264API::from_source(), cfg)
}

fn spawn_video_send(
    app: AppHandle,
    traces: Arc<RtcHub>,
    label: &'static str,
    screen: bool,
    running: Arc<AtomicBool>,
    jpeg: Arc<StdMutex<Option<Vec<u8>>>>,
    tracks: Arc<StdMutex<Vec<Arc<TrackLocalStaticSample>>>>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);
    let enc_run = running.clone();
    std::thread::spawn(move || {
        let Ok(mut encoder) = video_encoder(screen) else {
            trace(&app, traces.as_ref(), &format!("{label} encoder falhou"), "err", None);
            return;
        };
        let mut ticks = 0u32;
        let mut announced = false;
        while enc_run.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(VIDEO_MS));
            ticks = ticks.saturating_add(1);
            let live = jpeg.lock().ok().and_then(|g| g.clone());
            let (frame, real) = match live {
                Some(frame) => (frame, true),
                None if ticks % 5 == 1 => {
                    let rgb = vec![0u8; 320 * 176 * 3];
                    let Some(keep) = rgb_jpeg(320, 176, &rgb) else {
                        continue;
                    };
                    (keep, false)
                }
                None => continue,
            };
            if !real || ticks % 12 == 0 {
                encoder.force_intra_frame();
            }
            let Some(h264) = jpeg_to_h264(&mut encoder, &frame) else {
                if let Ok(next) = video_encoder(screen) {
                    encoder = next;
                }
                continue;
            };
            if h264.is_empty() {
                continue;
            }
            if real && !announced {
                announced = true;
                trace(
                    &app,
                    traces.as_ref(),
                    &format!("{label} enviando"),
                    "ok",
                    None,
                );
            }
            let _ = tx.try_send(h264);
        }
    });
    tauri::async_runtime::spawn(async move {
        while let Some(h264) = rx.recv().await {
            let list = tracks.lock().map(|t| t.clone()).unwrap_or_default();
            if list.is_empty() {
                continue;
            }
            let sample = Sample {
                data: Bytes::from(h264),
                duration: Duration::from_millis(VIDEO_MS),
                ..Default::default()
            };
            for track in list {
                let _ = track.write_sample(&sample).await;
            }
        }
    });
}

fn video_is_screen(mid: Option<&str>, stream_id: &str, track_id: &str) -> bool {
    let blob = format!("{stream_id} {track_id}").to_lowercase();
    if blob.contains("screen") || blob.contains("display") {
        return true;
    }
    match mid.and_then(|m| m.parse::<usize>().ok()) {
        Some(n) => n % 3 == 2,
        None => false,
    }
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

fn collect_fwd(
    session: &Session,
    from: &str,
    kind: &str,
) -> Vec<Arc<TrackLocalStaticSample>> {
    if session.hub.as_deref() != Some(session.me.as_str()) {
        return Vec::new();
    }
    session
        .peers
        .iter()
        .filter(|(id, _)| id.as_str() != from)
        .filter_map(|(_, peer)| match kind {
            "audio" => peer.fwd_audio.get(from).cloned(),
            "screen" => peer.fwd_screen.get(from).cloned(),
            _ => peer.fwd_cam.get(from).cloned(),
        })
        .collect()
}

fn forward_video(hub: Arc<RtcHub>, from: String, screen: bool, data: Vec<u8>) {
    if data.is_empty() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let tracks = {
            let guard = hub.inner.lock().await;
            let Some(session) = guard.as_ref() else {
                return;
            };
            collect_fwd(session, &from, if screen { "screen" } else { "cam" })
        };
        let sample = Sample {
            data: Bytes::from(data),
            duration: Duration::from_millis(VIDEO_MS),
            ..Default::default()
        };
        for track in tracks {
            let _ = track.write_sample(&sample).await;
        }
    });
}

async fn forward_audio(hub: Arc<RtcHub>, from: String, payload: Vec<u8>) {
    if payload.is_empty() {
        return;
    }
    let tracks = {
        let guard = hub.inner.lock().await;
        let Some(session) = guard.as_ref() else {
            return;
        };
        collect_fwd(session, &from, "audio")
    };
    let sample = Sample {
        data: Bytes::from(payload),
        duration: Duration::from_millis(20),
        ..Default::default()
    };
    for track in tracks {
        let _ = track.write_sample(&sample).await;
    }
}

fn spawn_video_recv(
    app: AppHandle,
    traces: Arc<RtcHub>,
    peer: String,
    screen: bool,
) -> std::sync::mpsc::SyncSender<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
    std::thread::spawn(move || {
        let Ok(mut decoder) = openh264::decoder::Decoder::new() else {
            trace(&app, traces.as_ref(), "decoder H264 falhou", "err", Some(&peer));
            return;
        };
        let mut seen = false;
        let mut decode_err = 0u32;
        while let Ok(nals) = rx.recv() {
            match decoder.decode(&nals) {
                Ok(Some(yuv)) => {
                    use openh264::formats::YUVSource;
                    let mut rgb = vec![0u8; yuv.estimate_rgb_u8_size()];
                    yuv.write_rgb8(&mut rgb);
                    let (w, h) = yuv.dimensions();
                    let Some(jpeg) = rgb_jpeg(w as u32, h as u32, &rgb) else {
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
                Ok(None) => {}
                Err(e) => {
                    decode_err = decode_err.saturating_add(1);
                    if decode_err <= 3 {
                        trace(
                            &app,
                            traces.as_ref(),
                            &format!("H264: {e}"),
                            "warn",
                            Some(&peer),
                        );
                    }
                }
            }
        }
        if seen {
            emit_video(&app, &peer, screen, &[]);
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
    play: Arc<StdMutex<MixBuf>>,
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
    let pc_watch = pc.clone();
    pc.on_peer_connection_state_change(Box::new(move |st| {
        let app = app_pc.clone();
        let traces = traces.clone();
        let peer = peer_pc.clone();
        let pc_watch = pc_watch.clone();
        Box::pin(async move {
            if matches!(
                st,
                RTCPeerConnectionState::Closed | RTCPeerConnectionState::Failed
            ) {
                let live = {
                    let guard = traces.inner.lock().await;
                    guard
                        .as_ref()
                        .and_then(|s| s.peers.get(&peer))
                        .is_some_and(|p| Arc::ptr_eq(&p.pc, &pc_watch))
                };
                if !live {
                    return;
                }
            }
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
                    state(&app).hear_peer(&peer);
                    ("ICE ok", "ok")
                }
                RTCIceConnectionState::Disconnected => ("ICE caiu", "warn"),
                RTCIceConnectionState::Failed => ("ICE falhou", "err"),
                RTCIceConnectionState::Closed => ("ICE fechou", "err"),
                _ => return,
            };
            trace(&app, traces.as_ref(), event, level, Some(&peer));
            if matches!(
                st,
                RTCIceConnectionState::Disconnected | RTCIceConnectionState::Failed
            ) {
                let hub = traces.clone();
                tauri::async_runtime::spawn(async move {
                    relight_ice(hub, peer).await;
                });
            }
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
                let stream_id = track.stream_id();
                let track_id = track.id();
                let idx = mid.as_deref().and_then(|m| m.parse::<usize>().ok()).unwrap_or(0);
                let extras = {
                    let guard = traces.inner.lock().await;
                    guard
                        .as_ref()
                        .and_then(|s| s.peers.get(&peer))
                        .map(|p| p.extras.clone())
                        .unwrap_or_default()
                };
                let from_peer = slot_peer(idx, &peer, &extras).to_string();
                let screen = video_is_screen(mid.as_deref(), &stream_id, &track_id);
                let tx = spawn_video_recv(app.clone(), traces.clone(), from_peer.clone(), screen);
                let mut depacketizer = H264Packet::default();
                let mut acc = Vec::new();
                let mut last_ts: Option<u32> = None;
                let mut saw_rtp = false;
                let mut consecutive_err = 0u32;
                loop {
                    match track.read_rtp().await {
                        Ok((pkt, _)) => {
                            consecutive_err = 0;
                            if pkt.payload.is_empty() {
                                continue;
                            }
                            if !saw_rtp {
                                saw_rtp = true;
                                state(&app).hear_peer(&peer);
                                let codec = track.codec().capability.mime_type;
                                let label = if codec.is_empty() {
                                    "H264"
                                } else {
                                    &codec
                                };
                                let h264 = label.to_ascii_lowercase().contains("h264");
                                trace(
                                    &app,
                                    traces.as_ref(),
                                    &format!("vídeo RTP {label} mid={}", mid.as_deref().unwrap_or("?")),
                                    if h264 { "info" } else { "err" },
                                    Some(&peer),
                                );
                            }
                            if last_ts.is_some_and(|ts| ts != pkt.header.timestamp) && !acc.is_empty()
                            {
                                let frame = std::mem::take(&mut acc);
                                forward_video(traces.clone(), from_peer.clone(), screen, frame.clone());
                                let _ = tx.try_send(frame);
                                depacketizer = H264Packet::default();
                            }
                            last_ts = Some(pkt.header.timestamp);
                            if let Ok(nal) = depacketizer.depacketize(&pkt.payload) {
                                if !nal.is_empty() {
                                    acc.extend_from_slice(&nal);
                                }
                            }
                            if pkt.header.marker && !acc.is_empty() {
                                let frame = std::mem::take(&mut acc);
                                forward_video(traces.clone(), from_peer.clone(), screen, frame.clone());
                                let _ = tx.try_send(frame);
                            }
                            if acc.len() > 1_000_000 {
                                acc.clear();
                            }
                        }
                        Err(e) => {
                            consecutive_err = consecutive_err.saturating_add(1);
                            if consecutive_err == 1 {
                                trace(
                                    &app,
                                    traces.as_ref(),
                                    &format!("vídeo RTP: {e}"),
                                    "warn",
                                    Some(&peer),
                                );
                            }
                            let why = e.to_string().to_lowercase();
                            if consecutive_err > 30
                                || why.contains("closed")
                                || why.contains("eof")
                            {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(30)).await;
                        }
                    }
                }
                return;
            }
            let mut ticks = 0u32;
            loop {
                let Ok((pkt, _)) = track.read_rtp().await else {
                    break;
                };
                if pkt.payload.is_empty() {
                    continue;
                }
                ticks = ticks.saturating_add(1);
                if ticks == 1 || ticks % 50 == 0 {
                    state(&app).hear_peer(&peer);
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
                    mix_ulaw_frame(&mut buf, &pkt.payload);
                }
                let samples = pkt.payload.to_vec();
                let from_pk = peer.clone();
                let traces_fwd = traces.clone();
                tauri::async_runtime::spawn(async move {
                    forward_audio(traces_fwd, from_pk, samples).await;
                });
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
    let mut heard = Vec::new();
    for (id, peer) in &session.peers {
        match peer.pc.connection_state() {
            RTCPeerConnectionState::Connected => {
                live += 1;
                heard.push(id.clone());
            }
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
            mode: if session.hub.is_some() && session.others.len() >= 2 {
                "HUB:N".into()
            } else if n <= 1 {
                "1:1".into()
            } else {
                "mesh".into()
            },
        },
    );
    drop(guard);
    for id in heard {
        state(app).hear_peer(&id);
    }
}

async fn peer_gate(hub: &RtcHub, peer: &str) -> Arc<Mutex<()>> {
    let mut boot = hub.boot.lock().await;
    boot.entry(peer.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

async fn ensure_peer(hub: Arc<RtcHub>, peer: &str) -> Result<(Arc<RTCPeerConnection>, bool), String> {
    let gate = peer_gate(&hub, peer).await;
    let _busy = gate.lock().await;
    {
        let guard = hub.inner.lock().await;
        if let Some(session) = guard.as_ref() {
            if let Some(existing) = session.peers.get(peer) {
                return Ok((existing.pc.clone(), false));
            }
        }
    }
    let (app, me, room, play, tracks, cam_tracks, screen_tracks, extras, as_hub) = {
        let guard = hub.inner.lock().await;
        let session = guard.as_ref().ok_or("call nativa parada")?;
        let extras = session
            .hub
            .as_ref()
            .filter(|_| session.others.len() >= 2)
            .map(|h| forward_peers(h, peer, &session.me, &session.others))
            .unwrap_or_default();
        let as_hub = session.hub.as_deref() == Some(session.me.as_str());
        (
            session.app.clone(),
            session.me.clone(),
            session.room.clone(),
            session.play.clone(),
            session.tracks.clone(),
            session.cam_tracks.clone(),
            session.screen_tracks.clone(),
            extras,
            as_hub,
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
    for track in [
        Arc::clone(&audio) as Arc<dyn TrackLocal + Send + Sync>,
        Arc::clone(&cam) as Arc<dyn TrackLocal + Send + Sync>,
        Arc::clone(&screen) as Arc<dyn TrackLocal + Send + Sync>,
    ] {
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

    let mut fwd_audio = HashMap::new();
    let mut fwd_cam = HashMap::new();
    let mut fwd_screen = HashMap::new();
    let direction = if as_hub {
        RTCRtpTransceiverDirection::Sendonly
    } else {
        RTCRtpTransceiverDirection::Recvonly
    };
    for from in &extras {
        let audio_t = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_PCMU.to_owned(),
                clock_rate: PCMU_HZ,
                channels: 1,
                ..Default::default()
            },
            format!("fwd-a-{from}"),
            "chaincord".into(),
        ));
        let cam_t = video_sample_track(&format!("fwd-c-{from}"));
        let screen_t = video_sample_track(&format!("fwd-s-{from}"));
        for track in [
            Arc::clone(&audio_t) as Arc<dyn TrackLocal + Send + Sync>,
            Arc::clone(&cam_t) as Arc<dyn TrackLocal + Send + Sync>,
            Arc::clone(&screen_t) as Arc<dyn TrackLocal + Send + Sync>,
        ] {
            pc.add_transceiver_from_track(
                track,
                Some(RTCRtpTransceiverInit {
                    direction,
                    send_encodings: vec![],
                }),
            )
            .await
            .map_err(err)?;
        }
        if as_hub {
            fwd_audio.insert(from.clone(), audio_t);
            fwd_cam.insert(from.clone(), cam_t);
            fwd_screen.insert(from.clone(), screen_t);
        }
    }

    {
        let mut guard = hub.inner.lock().await;
        let Some(session) = guard.as_mut() else {
            drop(guard);
            let _ = pc.close().await;
            return Err("call nativa parada".into());
        };
        if let Some(existing) = session.peers.get(peer) {
            let existing_pc = existing.pc.clone();
            drop(guard);
            let _ = pc.close().await;
            return Ok((existing_pc, false));
        }
        session.peers.insert(
            peer.to_string(),
            Peer {
                pc: pc.clone(),
                track: audio.clone(),
                cam: cam.clone(),
                screen: screen.clone(),
                extras: extras.clone(),
                fwd_audio,
                fwd_cam,
                fwd_screen,
                making_offer: AtomicBool::new(false),
                ice_retry: AtomicU64::new(0),
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
    trace(&app, hub.as_ref(), "enlace aberto", "info", Some(peer));
    Ok((pc, true))
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
    if kind == "offer" && pc.signaling_state() != RTCSignalingState::HaveLocalOffer {
        return;
    }
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
    offer_now_opts(hub, peer, false).await
}

async fn relight_ice(hub: Arc<RtcHub>, peer: String) {
    tokio::time::sleep(Duration::from_millis(1600)).await;
    let app = {
        let guard = hub.inner.lock().await;
        let Some(session) = guard.as_ref() else {
            return;
        };
        if polite(&session.me, &peer) {
            return;
        }
        let Some(p) = session.peers.get(&peer) else {
            return;
        };
        let ice = p.pc.ice_connection_state();
        if !matches!(
            ice,
            RTCIceConnectionState::Disconnected | RTCIceConnectionState::Failed
        ) {
            return;
        }
        let now = now_ms() as u64;
        let prev = p.ice_retry.load(Ordering::Relaxed);
        if now.saturating_sub(prev) < 8000 {
            return;
        }
        p.ice_retry.store(now, Ordering::Relaxed);
        session.app.clone()
    };
    trace(&app, hub.as_ref(), "religando ICE", "warn", Some(&peer));
    let _ = offer_now_opts(hub, &peer, true).await;
}

async fn offer_now_opts(hub: Arc<RtcHub>, peer: &str, ice_restart: bool) -> Result<(), String> {
    let gate = peer_gate(&hub, peer).await;
    let _busy = gate.lock().await;
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
        if ice_restart {
            pc.restart_ice().await.map_err(err)?;
        }
        let opts = ice_restart.then_some(RTCOfferOptions {
            ice_restart: true,
            ..Default::default()
        });
        let offer = pc.create_offer(opts).await.map_err(err)?;
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
        let play = Arc::new(StdMutex::new(MixBuf {
            acc: [0; FRAME_SAMPLES],
        }));
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
        spawn_video_send(
            app.clone(),
            hub.clone(),
            "câmera",
            false,
            running.clone(),
            cam_jpeg.clone(),
            cam_tracks.clone(),
        );
        spawn_video_send(
            app.clone(),
            hub.clone(),
            "tela",
            true,
            running.clone(),
            screen_jpeg.clone(),
            screen_tracks.clone(),
        );
        let screen_share = Arc::new(AtomicBool::new(false));
        spawn_screen_cap(
            app.clone(),
            hub.clone(),
            me.clone(),
            running.clone(),
            screen_share.clone(),
            screen_jpeg.clone(),
        );
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
            screen_share,
            play,
            running,
            hub: None,
            others: Vec::new(),
            last_sig: HashMap::new(),
            last_sess: HashMap::new(),
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

pub async fn sync(app: &AppHandle, peers: Vec<String>, hub_pk: Option<String>) -> Result<(), String> {
    let hub = state(app).rtc.clone();
    let (me, others, next_hub) = {
        let mut guard = hub.inner.lock().await;
        let session = guard.as_mut().ok_or("call nativa parada")?;
        let others: Vec<String> = peers
            .into_iter()
            .filter(|p| !p.is_empty() && p != &session.me)
            .collect();
        let next_hub = if others.len() >= 2 { hub_pk } else { None };
        session.others = others.clone();
        session.hub = next_hub.clone();
        (session.me.clone(), others, next_hub)
    };
    let want = wanted_peers(&me, &others, next_hub.as_deref());
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
        let extras_now = next_hub
            .as_ref()
            .filter(|_| others.len() >= 2)
            .map(|h| forward_peers(h, id, &me, &others))
            .unwrap_or_default();
        let stale = {
            let guard = hub.inner.lock().await;
            guard
                .as_ref()
                .and_then(|s| s.peers.get(id))
                .is_some_and(|p| p.extras != extras_now)
        };
        if stale {
            drop_peer(&hub, id, false).await;
        }
        let created = ensure_peer(hub.clone(), id).await?.1;
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
        if session.hub.as_deref().is_some_and(|h| {
            session.others.len() >= 2 && session.me != *h && frame.from != *h
        }) {
            return Ok(());
        }
        (session.me.clone(), session.room.clone())
    };
    if frame.kind == "bye" {
        let ts = frame.ts.unwrap_or(0);
        let stale = {
            let guard = hub.inner.lock().await;
            guard.as_ref().is_some_and(|session| {
                let last = session.last_sig.get(&frame.from).copied().unwrap_or(0);
                let live_sess = session.last_sess.get(&frame.from);
                let stale_sess = match (&frame.sess, live_sess) {
                    (Some(bye), Some(live)) => bye != live,
                    _ => false,
                };
                stale_sess || (ts > 0 && last > 0 && ts < last)
            })
        };
        if stale {
            trace(app, &hub, "bye atrasado", "info", Some(&frame.from));
            return Ok(());
        }
        drop_peer(&hub, &frame.from, true).await;
        return Ok(());
    }
    if let Some(sess) = &frame.sess {
        if let Some(session) = hub.inner.lock().await.as_mut() {
            session.last_sess.insert(frame.from.clone(), sess.clone());
        }
    }
    if let Some(ts) = frame.ts {
        if let Some(session) = hub.inner.lock().await.as_mut() {
            let last = session.last_sig.entry(frame.from.clone()).or_insert(0);
            if ts >= *last {
                *last = ts;
            }
        }
    }
    let pc = ensure_peer(hub.clone(), &frame.from).await?.0;
    if frame.kind == "ice" {
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
        return Ok(());
    }
    let gate = peer_gate(&hub, &frame.from).await;
    let _busy = gate.lock().await;
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
            if pc.signaling_state() != RTCSignalingState::Stable {
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
            let video_ok = pc
                .local_description()
                .await
                .map(|d| d.sdp.to_ascii_uppercase().contains("H264"))
                .unwrap_or(false);
            trace(
                app,
                &hub,
                if video_ok {
                    "resposta enviada"
                } else {
                    "resposta sem H264"
                },
                if video_ok { "info" } else { "err" },
                Some(&frame.from),
            );
        }
    } else if frame.kind == "answer" {
        if let Some(sdp) = frame.sdp {
            if pc.signaling_state() != RTCSignalingState::HaveLocalOffer {
                return Ok(());
            }
            let sdp_up = sdp.to_ascii_uppercase();
            let has_h264 = sdp_up.contains("H264");
            let has_vp8 = sdp_up.contains("VP8");
            let desc = RTCSessionDescription::answer(sdp).map_err(err)?;
            match pc.set_remote_description(desc).await {
                Ok(()) => {}
                Err(_) if pc.signaling_state() == RTCSignalingState::Stable => return Ok(()),
                Err(e) => return Err(err(e)),
            }
            if let Some(session) = hub.inner.lock().await.as_ref() {
                if let Some(p) = session.peers.get(&frame.from) {
                    flush_ice(p).await;
                }
            }
            trace(
                app,
                &hub,
                &format!("resposta recebida h264={has_h264} vp8={has_vp8}"),
                if has_h264 { "ok" } else { "err" },
                Some(&frame.from),
            );
        }
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

pub async fn share_screen(app: &AppHandle, on: bool) -> Result<(), String> {
    let hub = state(app).rtc.clone();
    if on {
        let app2 = {
            let guard = hub.inner.lock().await;
            let session = guard.as_ref().ok_or("call nativa parada")?;
            session.app.clone()
        };
        if grab_screen_on_main(&app2).is_none() {
            return Err("captura de tela indisponível neste compositor".into());
        }
        let guard = hub.inner.lock().await;
        let session = guard.as_ref().ok_or("call nativa parada")?;
        session.screen_share.store(true, Ordering::Relaxed);
        trace(app, &hub, "tela nativa on", "info", None);
    } else {
        let guard = hub.inner.lock().await;
        let session = guard.as_ref().ok_or("call nativa parada")?;
        session.screen_share.store(false, Ordering::Relaxed);
        if let Ok(mut g) = session.screen_jpeg.lock() {
            *g = None;
        }
        let _ = session.app.emit(
            VIDEO,
            CallVideo {
                peer: session.me.clone(),
                screen: true,
                jpeg: String::new(),
            },
        );
        trace(app, &hub, "tela nativa off", "info", None);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_mid_is_not_screen() {
        assert!(!video_is_screen(Some("1"), "chaincord", "cam"));
        assert!(!video_is_screen(None, "chaincord", "cam"));
        assert!(!video_is_screen(Some("0"), "chaincord", "cam"));
    }

    #[test]
    fn second_video_mid_or_name_is_screen() {
        assert!(video_is_screen(Some("2"), "chaincord", "screen"));
        assert!(video_is_screen(Some("5"), "chaincord", "v1"));
        assert!(!video_is_screen(Some("4"), "chaincord", "v0"));
        assert!(video_is_screen(Some("1"), "screen-share", "track"));
        assert!(video_is_screen(None, "chaincord", "display"));
        assert!(!video_is_screen(Some("1"), "chaincord", "v0"));
    }

    #[test]
    fn mix_adds_two_talkers_instead_of_concatenating() {
        let mut buf = MixBuf {
            acc: [0; FRAME_SAMPLES],
        };
        mix_ulaw_frame(&mut buf, &[linear_to_ulaw(1000); FRAME_SAMPLES]);
        mix_ulaw_frame(&mut buf, &[linear_to_ulaw(1000); FRAME_SAMPLES]);
        let frame = take_mix_frame(&mut buf);
        assert!(frame[0] > 1000);
        assert_eq!(buf.acc[0], 0);
    }

    #[test]
    fn star_leaves_only_dial_the_hub() {
        let others = vec!["bb".into(), "cc".into()];
        assert_eq!(wanted_peers("aa", &others, Some("aa")), others);
        assert_eq!(wanted_peers("bb", &others, Some("aa")), vec!["aa".to_string()]);
        assert_eq!(forward_peers("aa", "bb", "aa", &others), vec!["cc".to_string()]);
        assert_eq!(slot_peer(4, "aa", &["cc".into()]), "cc");
    }
}
