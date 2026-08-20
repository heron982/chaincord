use crate::crypto::{
    create_community, decode_invite, encode_invite, now_ms, open_chat, seal_chat, ChatPlain,
    Identity, Invite, WireMessage,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener as StdTcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use crate::store::Store;
use tauri::{AppHandle, Emitter, Manager};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Message, WebSocketStream};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    pub public_key: String,
    pub display_name: String,
    pub avatar: String,
    pub community_name: String,
    pub community_id: String,
    pub invite: String,
    pub listen_url: String,
    pub peers: Vec<String>,
    pub text_channels: Vec<String>,
    pub call_rooms: Vec<String>,
    pub voice: HashMap<String, Vec<String>>,
    pub profiles: HashMap<String, PeerProfile>,
    pub archive_bytes: u64,
    pub archive_messages: u32,
    pub seed_sent: u64,
    pub seed_total: u64,
    pub seed_active: bool,
    pub seeding: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerProfile {
    pub display_name: String,
    pub avatar: String,
    pub muted: bool,
    pub deafened: bool,
    #[serde(default = "offline_status")]
    pub status: String,
}

impl Default for PeerProfile {
    fn default() -> Self {
        Self {
            display_name: String::new(),
            avatar: String::new(),
            muted: false,
            deafened: false,
            status: offline_status(),
        }
    }
}

fn offline_status() -> String {
    "offline".into()
}

const PRESENCE_TTL_MS: i64 = 90_000;

#[derive(Clone, Serialize, Deserialize)]
pub struct UiMessage {
    pub sender: String,
    pub text: String,
    pub ts: i64,
    pub channel: String,
    #[serde(rename = "self")]
    pub is_self: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredProfile {
    display_name: String,
    avatar: String,
    secret_key: String,
}

struct Inner {
    identity: Identity,
    display_name: String,
    avatar: String,
    community: Option<crate::crypto::Community>,
    invite_peers: Vec<String>,
    listen_url: String,
    listen_urls: Vec<String>,
    listen_port: u16,
    known_peer_urls: HashSet<String>,
    text_channels: Vec<String>,
    call_rooms: Vec<String>,
    voice: HashMap<String, HashSet<String>>,
    profiles: HashMap<String, PeerProfile>,
    muted: bool,
    deafened: bool,
    status: String,
    last_seen: HashMap<String, i64>,
    archive_messages: u32,
    archive_bytes: u64,
    seeded: HashMap<String, u32>,
    seeding: HashSet<String>,
}

pub struct AppState {
    inner: Mutex<Inner>,
    remotes: Mutex<HashMap<String, mpsc::UnboundedSender<String>>>,
    link_pks: Mutex<HashMap<String, String>>,
    store: Mutex<Option<Store>>,
    pending: AtomicU64,
    relay_gen: AtomicU64,
    seen: Mutex<(HashSet<u64>, VecDeque<u64>)>,
    #[cfg(target_os = "linux")]
    pub rtc: std::sync::Arc<crate::rtc::RtcHub>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                identity: Identity::generate(),
                display_name: String::new(),
                avatar: String::new(),
                community: None,
                invite_peers: Vec::new(),
                listen_url: String::new(),
                listen_urls: Vec::new(),
                listen_port: 0,
                known_peer_urls: HashSet::new(),
                text_channels: Vec::new(),
                call_rooms: Vec::new(),
                voice: HashMap::new(),
                profiles: HashMap::new(),
                muted: false,
                deafened: false,
                status: "online".into(),
                last_seen: HashMap::new(),
                archive_messages: 0,
                archive_bytes: 0,
                seeded: HashMap::new(),
                seeding: HashSet::new(),
            }),
            remotes: Mutex::new(HashMap::new()),
            link_pks: Mutex::new(HashMap::new()),
            store: Mutex::new(None),
            pending: AtomicU64::new(1),
            relay_gen: AtomicU64::new(0),
            seen: Mutex::new((HashSet::new(), VecDeque::new())),
            #[cfg(target_os = "linux")]
            rtc: std::sync::Arc::new(crate::rtc::RtcHub::new()),
        }
    }

    pub fn snapshot(&self) -> UiState {
        let inner = self.inner.lock().expect("state");
        let invite = inner.community.as_ref().map(|c| {
            encode_invite(&Invite {
                v: 1,
                community: c.clone(),
                peers: invite_peer_list(&inner),
            })
        });
        let live_peers: Vec<String> = self
            .remotes
            .lock()
            .expect("remotes")
            .keys()
            .filter(|k| !k.starts_with("pending:"))
            .cloned()
            .collect();
        let seed = seed_progress(&inner);
        UiState {
            public_key: inner.identity.public_hex(),
            display_name: inner.display_name.clone(),
            avatar: inner.avatar.clone(),
            community_name: inner
                .community
                .as_ref()
                .map(|c| c.genesis.name.clone())
                .unwrap_or_default(),
            community_id: inner
                .community
                .as_ref()
                .map(|c| c.id.clone())
                .unwrap_or_default(),
            invite: invite.unwrap_or_default(),
            listen_url: inner.listen_url.clone(),
            peers: live_peers,
            text_channels: inner.text_channels.clone(),
            call_rooms: inner.call_rooms.clone(),
            voice: inner
                .voice
                .iter()
                .map(|(room, people)| {
                    (room.clone(), people.iter().cloned().collect::<Vec<_>>())
                })
                .collect(),
            profiles: snapshot_profiles(&inner),
            archive_bytes: inner.archive_bytes,
            archive_messages: inner.archive_messages,
            seed_sent: seed.0,
            seed_total: seed.1,
            seed_active: seeding_visible(&inner),
            seeding: inner.seeding.iter().cloned().collect(),
        }
    }

    pub fn voice_flags(&self) -> (bool, bool) {
        let inner = self.inner.lock().expect("state");
        (inner.muted, inner.deafened)
    }

    pub fn hear_peer(&self, pk: &str) {
        let mut inner = self.inner.lock().expect("state");
        touch_seen(&mut inner, pk);
    }
}

fn state_of(app: &AppHandle) -> Arc<AppState> {
    app.state::<Arc<AppState>>().inner().clone()
}

pub(crate) fn emit_state(app: &AppHandle) {
    let _ = app.emit("ui-state", state_of(app).snapshot());
}

pub(crate) fn community_id(app: &AppHandle) -> Option<String> {
    state_of(app)
        .inner
        .lock()
        .expect("state")
        .community
        .as_ref()
        .map(|c| c.id.clone())
}

pub(crate) fn public_key(app: &AppHandle) -> String {
    state_of(app).inner.lock().expect("state").identity.public_hex()
}

pub(crate) fn bump_relay_gen(app: &AppHandle) -> u64 {
    state_of(app).relay_gen.fetch_add(1, Ordering::SeqCst) + 1
}

pub(crate) fn relay_is_current(app: &AppHandle, gen: u64) -> bool {
    state_of(app).relay_gen.load(Ordering::SeqCst) == gen
}

pub(crate) fn register_relay(app: &AppHandle, tx: mpsc::UnboundedSender<String>) {
    state_of(app)
        .remotes
        .lock()
        .expect("remotes")
        .insert("relay".into(), tx);
}

pub(crate) fn unregister_relay(app: &AppHandle) {
    state_of(app)
        .remotes
        .lock()
        .expect("remotes")
        .remove("relay");
}

pub(crate) fn ingest_from_relay(app: &AppHandle, raw: &str) {
    let state = state_of(app);
    let tx = {
        let remotes = state.remotes.lock().expect("remotes");
        remotes.get("relay").cloned()
    };
    let Some(tx) = tx else {
        return;
    };
    let mut from = Some("relay".into());
    handle_remote(app, raw, &mut from, &tx);
}

pub(crate) fn add_listen_url(app: &AppHandle, url: String) {
    if url.is_empty() || !should_dial(&url) {
        return;
    }
    {
        let state = state_of(app);
        let mut inner = state.inner.lock().expect("state");
        if inner.listen_urls.iter().any(|u| u == &url) {
            return;
        }
        inner.listen_urls.push(url);
    }
    emit_state(app);
}

fn already_seen(app: &AppHandle, raw: &str) -> bool {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    raw.hash(&mut hasher);
    let h = hasher.finish();
    let state = state_of(app);
    let mut guard = state.seen.lock().expect("seen");
    let (set, order) = &mut *guard;
    if !set.insert(h) {
        return true;
    }
    order.push_back(h);
    while order.len() > 2000 {
        if let Some(old) = order.pop_front() {
            set.remove(&old);
        }
    }
    false
}

fn emit_message(app: &AppHandle, msg: UiMessage) {
    let _ = app.emit("ui-message", msg);
}

fn persist_profile(app: &AppHandle) {
    let state = state_of(app);
    let (secret, name, avatar) = {
        let inner = state.inner.lock().expect("state");
        (
            inner.identity.secret_hex(),
            inner.display_name.clone(),
            inner.avatar.clone(),
        )
    };
    let guard = state.store.lock().expect("store");
    if let Some(store) = guard.as_ref() {
        let _ = store.save_profile(&secret, &name, &avatar);
    }
}

fn persist_session(app: &AppHandle) {
    let state = state_of(app);
    let session = {
        let inner = state.inner.lock().expect("state");
        let Some(community) = inner.community.clone() else {
            return;
        };
        crate::store::Session {
            community,
            invite_peers: inner.invite_peers.clone(),
            known_peer_urls: inner.known_peer_urls.clone(),
            text_channels: inner.text_channels.clone(),
            call_rooms: inner.call_rooms.clone(),
            profiles: inner.profiles.clone(),
        }
    };
    let guard = state.store.lock().expect("store");
    if let Some(store) = guard.as_ref() {
        let _ = store.save_session(&session);
    }
}

fn persist_message(app: &AppHandle, msg: &UiMessage) {
    let state = state_of(app);
    let id = state
        .inner
        .lock()
        .expect("state")
        .community
        .as_ref()
        .map(|c| c.id.clone());
    let Some(id) = id else {
        return;
    };
    let guard = state.store.lock().expect("store");
    if let Some(store) = guard.as_ref() {
        let _ = store.append_message(&id, msg);
    }
}

fn emit_and_store_message(app: &AppHandle, msg: UiMessage) {
    persist_message(app, &msg);
    refresh_archive(app);
    {
        let state = state_of(app);
        let mut inner = state.inner.lock().expect("state");
        let count = inner.archive_messages;
        let me = inner.identity.public_hex();
        let keys: Vec<String> = inner
            .profiles
            .keys()
            .filter(|pk| **pk != me)
            .cloned()
            .collect();
        for pk in keys {
            if inner.seeded.get(&pk).copied().unwrap_or(0) + 1 >= count {
                inner.seeded.insert(pk, count);
            }
        }
    }
    emit_message(app, msg);
    emit_state(app);
}

fn refresh_archive(app: &AppHandle) {
    let msgs = get_history(app);
    let bytes = msgs
        .iter()
        .map(|m| (m.text.len() + m.sender.len() + m.channel.len() + 24) as u64)
        .sum();
    let state = state_of(app);
    let mut inner = state.inner.lock().expect("state");
    inner.archive_messages = msgs.len() as u32;
    inner.archive_bytes = bytes;
}

fn seed_progress(inner: &Inner) -> (u64, u64) {
    let me = inner.identity.public_hex();
    let mut involved: HashSet<&String> = inner.seeding.iter().collect();
    involved.extend(inner.seeded.keys().filter(|pk| **pk != me));
    let n = involved.len() as u64;
    let msgs = inner.archive_messages;
    let bytes = inner.archive_bytes;
    if n == 0 || msgs == 0 {
        return (1, 1);
    }
    let done = involved
        .iter()
        .filter(|pk| {
            !inner.seeding.contains(**pk) && inner.seeded.get(**pk).copied().unwrap_or(0) >= msgs
        })
        .count() as u64;
    if bytes == 0 {
        (done, n)
    } else {
        (done * bytes, n * bytes)
    }
}

fn history_payload(state: &AppState) -> (u32, u64, Option<String>) {
    let id = state
        .inner
        .lock()
        .expect("state")
        .community
        .as_ref()
        .map(|c| c.id.clone());
    let Some(id) = id else {
        return (0, 0, None);
    };
    let msgs = {
        let guard = state.store.lock().expect("store");
        guard
            .as_ref()
            .map(|store| store.load_messages(&id))
            .unwrap_or_default()
    };
    let count = msgs.len() as u32;
    let bytes = msgs
        .iter()
        .map(|m| (m.text.len() + m.sender.len() + m.channel.len() + 24) as u64)
        .sum();
    if msgs.is_empty() {
        return (0, bytes, None);
    }
    let wire: Vec<serde_json::Value> = msgs
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "sender": m.sender,
                "text": m.text,
                "ts": m.ts,
                "channel": m.channel,
            })
        })
        .collect();
    (
        count,
        bytes,
        Some(serde_json::json!({ "type": "history", "messages": wire }).to_string()),
    )
}

fn maybe_seed_peer(app: &AppHandle, pk: &str, their_count: u32) {
    if pk.is_empty() {
        return;
    }
    let state = state_of(app);
    let me = state.inner.lock().expect("state").identity.public_hex();
    if pk == me {
        return;
    }
    let (count, _, json) = history_payload(&state);
    {
        let mut inner = state.inner.lock().expect("state");
        if their_count >= count || json.is_none() {
            inner.seeded.insert(pk.to_string(), count);
            inner.seeding.remove(pk);
            drop(inner);
            emit_state(app);
            return;
        }
        inner.seeding.insert(pk.to_string());
    }
    emit_state(app);
    if let Some(hist) = json {
        fanout(&state, &hist, None);
    }
    {
        let mut inner = state.inner.lock().expect("state");
        inner.seeded.insert(pk.to_string(), count);
        inner.seeding.remove(pk);
    }
    emit_state(app);
}

fn clear_stored_session(app: &AppHandle) {
    let state = state_of(app);
    let guard = state.store.lock().expect("store");
    if let Some(store) = guard.as_ref() {
        let _ = store.clear_session();
    }
}

pub fn get_history(app: &AppHandle) -> Vec<UiMessage> {
    let state = state_of(app);
    let id = state
        .inner
        .lock()
        .expect("state")
        .community
        .as_ref()
        .map(|c| c.id.clone());
    let Some(id) = id else {
        return Vec::new();
    };
    let guard = state.store.lock().expect("store");
    guard
        .as_ref()
        .map(|store| store.load_messages(&id))
        .unwrap_or_default()
}

pub fn boot(app: &AppHandle) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let Ok(store) = Store::open(&dir.join("chaincord.sqlite")) else {
        return;
    };
    let sqlite_profile = store.load_profile();
    let session = store.load_session();
    *state_of(app).store.lock().expect("store") = Some(store);

    if let Some((secret, name, avatar)) = sqlite_profile {
        if let Ok(identity) = Identity::from_secret_hex(&secret) {
            let state = state_of(app);
            let mut inner = state.inner.lock().expect("state");
            inner.identity = identity;
            inner.display_name = name;
            inner.avatar = avatar;
        }
    } else {
        load_json_profile(app);
        persist_profile(app);
    }

    if let Some(session) = session {
        let state = state_of(app);
        let mut inner = state.inner.lock().expect("state");
        inner.community = Some(session.community);
        inner.invite_peers = session.invite_peers;
        inner.known_peer_urls = session.known_peer_urls;
        inner.text_channels = session.text_channels;
        inner.call_rooms = session.call_rooms;
        inner.profiles = session.profiles;
        for profile in inner.profiles.values_mut() {
            profile.status = "offline".into();
        }
        inner.last_seen.clear();
        drop(inner);
        refresh_archive(app);
        crate::relay::spawn(app.clone());
    }
    spawn_presence_pulse(app.clone());
}

fn spawn_presence_pulse(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(8));
        loop {
            tick.tick().await;
            let state = state_of(&app);
            let in_community = state
                .inner
                .lock()
                .expect("state")
                .community
                .is_some();
            if !in_community {
                continue;
            }
            let json = presence_json(&state);
            fanout(&state, &json, None);
            let leaves = {
                let mut inner = state.inner.lock().expect("state");
                expire_stale_presence(&mut inner, now_ms())
            };
            fanout_voice_leaves(&state, &leaves);
            emit_state(&app);
        }
    });
}

fn lan_ipv4s() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    if let Ok(ifaces) = local_ip_address::list_afinet_netifas() {
        for (_name, ip) in ifaces {
            if let IpAddr::V4(v4) = ip {
                if usable_v4(v4) && !out.contains(&v4) {
                    out.push(v4);
                }
            }
        }
    }
    out
}

fn lan_ipv6s() -> Vec<Ipv6Addr> {
    let mut out = Vec::new();
    if let Ok(ifaces) = local_ip_address::list_afinet_netifas() {
        for (_name, ip) in ifaces {
            if let IpAddr::V6(v6) = ip {
                if usable_v6(v6) && !out.contains(&v6) {
                    out.push(v6);
                }
            }
        }
    }
    out
}

fn usable_v4(v4: Ipv4Addr) -> bool {
    if v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() {
        return false;
    }
    let o = v4.octets();
    if o[0] == 26 {
        return false;
    }
    if o[0] == 192 && o[1] == 168 && o[2] == 137 {
        return false;
    }
    true
}

fn usable_v6(v6: Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_multicast() || v6.is_unspecified() {
        return false;
    }
    let s = v6.segments();
    if s[0] & 0xfe00 == 0xfc00 {
        return false;
    }
    if s[0] & 0xffc0 == 0xfe80 {
        return false;
    }
    if s[0] == 0x2001 && s[1] == 0 {
        return false;
    }
    if s[0] == 0x2002 {
        return false;
    }
    true
}

fn advertise_urls(port: u16) -> Vec<String> {
    let mut urls = vec![format!("ws://127.0.0.1:{port}")];
    if let Ok(forced) = std::env::var("CHAINCORD_HOST") {
        if !forced.is_empty() {
            let url = if forced.starts_with("ws://") || forced.starts_with("wss://") {
                forced
            } else {
                format!("ws://{forced}:{port}")
            };
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
    }
    for ip in lan_ipv4s() {
        let url = format!("ws://{ip}:{port}");
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    for ip in lan_ipv6s() {
        let url = format!("ws://[{ip}]:{port}");
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    urls
}

fn parse_ws(url: &str) -> Option<(String, u16)> {
    let rest = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))?;
    let rest = rest.split('/').next().unwrap_or(rest);
    let (host, port) = rest.rsplit_once(':')?;
    let host = host.trim_matches(|c| c == '[' || c == ']').to_string();
    let port = port.parse().ok()?;
    Some((host, port))
}

fn is_loopback_host(host: &str) -> bool {
    host == "127.0.0.1" || host == "localhost" || host == "::1"
}

fn is_self_url(inner: &Inner, url: &str) -> bool {
    if url == inner.listen_url || inner.listen_urls.iter().any(|u| u == url) {
        return true;
    }
    let Some((host, port)) = parse_ws(url) else {
        return false;
    };
    if inner.listen_port == 0 || port != inner.listen_port {
        return false;
    }
    is_loopback_host(&host)
        || inner.listen_urls.iter().any(|u| {
            parse_ws(u)
                .map(|(h, _)| h == host)
                .unwrap_or(false)
        })
}

fn is_cgnat_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..128).contains(&o[1])
}

fn same_home_lan(a: Ipv4Addr, b: Ipv4Addr) -> bool {
    let ao = a.octets();
    let bo = b.octets();
    if ao[0] == 10 && bo[0] == 10 {
        return ao[1] == bo[1];
    }
    if ao[0] == 192 && ao[1] == 168 && bo[0] == 192 && bo[1] == 168 {
        return ao[2] == bo[2];
    }
    if ao[0] == 172 && (16..=31).contains(&ao[1]) && bo[0] == 172 && (16..=31).contains(&bo[1]) {
        return ao[1] == bo[1] && ao[2] == bo[2];
    }
    a == b
}

fn on_same_lan(url: &str) -> bool {
    should_dial(url)
}

fn should_dial(url: &str) -> bool {
    let Some((host, _)) = parse_ws(url) else {
        return false;
    };
    if is_loopback_host(&host) {
        return true;
    }
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        if !usable_v4(v4) {
            return false;
        }
        if v4.is_private() || is_cgnat_v4(v4) {
            return lan_ipv4s().iter().any(|mine| same_home_lan(*mine, v4));
        }
        return true;
    }
    if let Ok(v6) = host.parse::<Ipv6Addr>() {
        return usable_v6(v6);
    }
    false
}

fn invite_peer_list(inner: &Inner) -> Vec<String> {
    let mut peers = inner
        .listen_urls
        .iter()
        .filter(|u| should_dial(u))
        .cloned()
        .collect::<Vec<_>>();
    if peers.is_empty() && !inner.listen_url.is_empty() && should_dial(&inner.listen_url) {
        peers.push(inner.listen_url.clone());
    }
    for extra in inner.invite_peers.iter().chain(inner.known_peer_urls.iter()) {
        if extra.is_empty() || !should_dial(extra) || peers.iter().any(|p| p == extra) {
            continue;
        }
        peers.push(extra.clone());
    }
    peers
}

fn unique_urls(urls: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out = Vec::new();
    for url in urls {
        let url = url.trim().to_string();
        if url.is_empty() || out.iter().any(|u| u == &url) {
            continue;
        }
        out.push(url);
    }
    out
}

fn pick_port(start: u16) -> Result<u16, String> {
    let preferred = std::env::var("CHAINCORD_PEER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(start);
    for port in preferred..preferred.saturating_add(20) {
        if StdTcpListener::bind(("0.0.0.0", port)).is_ok() {
            return Ok(port);
        }
    }
    Err("nenhuma porta livre".into())
}

pub async fn run_listener(app: AppHandle) -> Result<(), String> {
    let port = pick_port(7340)?;
    let urls = advertise_urls(port);
    let listen_url = urls
        .iter()
        .find(|u| parse_ws(u).is_some_and(|(h, _)| !is_loopback_host(&h)))
        .cloned()
        .unwrap_or_else(|| urls[0].clone());
    {
        let state = state_of(&app);
        let mut inner = state.inner.lock().expect("state");
        inner.listen_port = port;
        inner.listen_urls = urls;
        inner.listen_url = listen_url;
    }
    emit_state(&app);
    crate::nat::spawn(app.clone(), port);
    let peers: Vec<String> = state_of(&app)
        .inner
        .lock()
        .expect("state")
        .known_peer_urls
        .iter()
        .cloned()
        .collect();
    for url in peers {
        connect_peer(app.clone(), url);
    }

    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| e.to_string())?;

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let Ok(ws) = accept_async(stream).await else {
                return;
            };
            let (tx, rx) = mpsc::unbounded_channel::<String>();
            let key = format!(
                "pending:{}",
                state_of(&app).pending.fetch_add(1, Ordering::Relaxed)
            );
            state_of(&app)
                .remotes
                .lock()
                .expect("remotes")
                .insert(key.clone(), tx.clone());
            serve_connected(app, ws, Some(key), tx, rx).await;
        });
    }
}

fn fanout(state: &AppState, json: &str, except: Option<&str>) {
    let remotes = state.remotes.lock().expect("remotes");
    for (url, tx) in remotes.iter() {
        if Some(url.as_str()) == except {
            continue;
        }
        let _ = tx.send(json.to_string());
    }
}

fn hello_and_peers_json(state: &AppState, compact: bool) -> Option<(String, String)> {
    let inner = state.inner.lock().expect("state");
    let community = inner.community.as_ref()?;
    let hello = serde_json::json!({
        "type": "hello",
        "publicKey": inner.identity.public_hex(),
        "communityId": community.id,
        "listenUrl": inner.listen_url,
        "listenUrls": inner.listen_urls,
        "displayName": inner.display_name,
        "avatar": if compact { "" } else { inner.avatar.as_str() },
        "historyCount": inner.archive_messages,
        "status": own_status(&inner),
        "muted": inner.muted,
        "deafened": inner.deafened,
    });
    drop(inner);
    let peers = serde_json::json!({ "type": "peers", "urls": gossip_urls(state) });
    Some((hello.to_string(), peers.to_string()))
}

pub(crate) fn handshake_messages(app: &AppHandle, compact: bool) -> Vec<String> {
    let state = state_of(app);
    let Some((hello, peers)) = hello_and_peers_json(&state, compact) else {
        return Vec::new();
    };
    let mut out = vec![hello, peers, layout_json(&state)];
    if voice_is_busy(&state.inner.lock().expect("state")) {
        out.push(voice_json(&state));
    }
    if compact {
        let inner = state.inner.lock().expect("state");
        out.push(presence_frame(&inner, true));
        let people = live_profiles(&inner, now_ms(), true);
        drop(inner);
        out.push(serde_json::json!({ "type": "presence-state", "people": people }).to_string());
        let (_, _, hist) = history_payload(&state);
        if let Some(hist) = hist {
            if hist.len() < 40_000 {
                out.push(hist);
            }
        }
        return out;
    }
    out.push(presence_json(&state));
    out.push(presence_state_json(&state));
    if let (_, _, Some(hist)) = history_payload(&state) {
        out.push(hist);
    }
    out
}

fn gossip_urls(state: &AppState) -> Vec<String> {
    let inner = state.inner.lock().expect("state");
    let mut urls: Vec<String> = inner.listen_urls.clone();
    if urls.is_empty() {
        urls.push(inner.listen_url.clone());
    }
    for url in &inner.known_peer_urls {
        if parse_ws(url).is_some_and(|(h, _)| is_loopback_host(&h)) {
            continue;
        }
        if !urls.iter().any(|u| u == url) {
            urls.push(url.clone());
        }
    }
    urls.retain(|u| !u.is_empty());
    urls
}

fn layout_json(state: &AppState) -> String {
    let inner = state.inner.lock().expect("state");
    serde_json::json!({
        "type": "layout",
        "textChannels": inner.text_channels,
        "callRooms": inner.call_rooms,
    })
    .to_string()
}

fn voice_json(state: &AppState) -> String {
    let inner = state.inner.lock().expect("state");
    let rooms: HashMap<String, Vec<String>> = inner
        .voice
        .iter()
        .map(|(room, people)| (room.clone(), people.iter().cloned().collect()))
        .collect();
    serde_json::json!({ "type": "voice-state", "rooms": rooms }).to_string()
}

fn voice_is_busy(inner: &Inner) -> bool {
    inner.voice.values().any(|people| !people.is_empty())
}

fn voice_busy(state: &AppState) -> bool {
    voice_is_busy(&state.inner.lock().expect("state"))
}

fn seeding_visible(inner: &Inner) -> bool {
    !inner.seeding.is_empty()
}

pub(crate) fn goodbye_json(app: &AppHandle) -> String {
    let state = state_of(app);
    let inner = state.inner.lock().expect("state");
    serde_json::json!({
        "type": "presence",
        "publicKey": inner.identity.public_hex(),
        "displayName": inner.display_name,
        "status": "offline",
        "muted": inner.muted,
        "deafened": inner.deafened,
    })
    .to_string()
}

pub fn announce_gone(app: &AppHandle) {
    let state = state_of(app);
    let (pk, rooms) = {
        let mut inner = state.inner.lock().expect("state");
        if inner.community.is_none() {
            return;
        }
        inner.status = "offline".into();
        let pk = inner.identity.public_hex();
        let rooms = drop_from_voice(&mut inner, &pk);
        (pk, rooms)
    };
    fanout(&state, &goodbye_json(app), None);
    fanout_voice_leaves(
        &state,
        &rooms
            .into_iter()
            .map(|room| (pk.clone(), room))
            .collect::<Vec<_>>(),
    );
}

fn presence_json(state: &AppState) -> String {
    let inner = state.inner.lock().expect("state");
    presence_frame(&inner, false)
}

fn presence_frame(inner: &Inner, compact: bool) -> String {
    serde_json::json!({
        "type": "presence",
        "publicKey": inner.identity.public_hex(),
        "displayName": inner.display_name,
        "avatar": if compact { "" } else { inner.avatar.as_str() },
        "muted": inner.muted,
        "deafened": inner.deafened,
        "status": own_status(inner),
    })
    .to_string()
}

fn presence_state_json(state: &AppState) -> String {
    let inner = state.inner.lock().expect("state");
    serde_json::json!({ "type": "presence-state", "people": live_profiles(&inner, now_ms(), false) }).to_string()
}

fn snapshot_profiles(inner: &Inner) -> HashMap<String, PeerProfile> {
    let now = now_ms();
    let me = inner.identity.public_hex();
    let mut map = inner.profiles.clone();
    map.insert(
        me.clone(),
        PeerProfile {
            display_name: inner.display_name.clone(),
            avatar: inner.avatar.clone(),
            muted: inner.muted,
            deafened: inner.deafened,
            status: own_status(inner),
        },
    );
    for (pk, profile) in map.iter_mut() {
        if *pk != me {
            profile.status = live_status(inner, pk, now);
        }
    }
    map
}

fn own_status(inner: &Inner) -> String {
    normalize_status(&inner.status)
}

fn normalize_status(raw: &str) -> String {
    match raw {
        "away" | "ausente" | "idle" => "away".into(),
        "busy" | "ocupado" | "dnd" => "busy".into(),
        "offline" => "offline".into(),
        "online" | "" => "online".into(),
        _ => "online".into(),
    }
}

fn live_status(inner: &Inner, pk: &str, now: i64) -> String {
    if pk == inner.identity.public_hex() {
        return own_status(inner);
    }
    let last = inner.last_seen.get(pk).copied().unwrap_or(0);
    if last == 0 || now.saturating_sub(last) > PRESENCE_TTL_MS {
        return "offline".into();
    }
    let raw = inner
        .profiles
        .get(pk)
        .map(|p| p.status.as_str())
        .unwrap_or("online");
    if raw == "offline" || raw.is_empty() {
        "online".into()
    } else {
        normalize_status(raw)
    }
}

fn live_profiles(inner: &Inner, now: i64, strip_avatars: bool) -> HashMap<String, PeerProfile> {
    let me = inner.identity.public_hex();
    let mut people = HashMap::new();
    people.insert(
        me.clone(),
        PeerProfile {
            display_name: inner.display_name.clone(),
            avatar: if strip_avatars {
                String::new()
            } else {
                inner.avatar.clone()
            },
            muted: inner.muted,
            deafened: inner.deafened,
            status: own_status(inner),
        },
    );
    for (pk, profile) in &inner.profiles {
        if pk == &me {
            continue;
        }
        let status = live_status(inner, pk, now);
        if status == "offline" {
            continue;
        }
        let mut next = profile.clone();
        next.status = status;
        if strip_avatars {
            next.avatar.clear();
        }
        people.insert(pk.clone(), next);
    }
    people
}

fn expire_stale_presence(inner: &mut Inner, now: i64) -> Vec<(String, String)> {
    let me = inner.identity.public_hex();
    let stale: Vec<String> = inner
        .last_seen
        .iter()
        .filter(|(pk, ts)| **pk != me && now.saturating_sub(**ts) > PRESENCE_TTL_MS)
        .map(|(pk, _)| pk.clone())
        .collect();
    for pk in stale {
        inner.last_seen.remove(&pk);
        if let Some(profile) = inner.profiles.get_mut(&pk) {
            profile.status = "offline".into();
        }
    }
    Vec::new()
}

fn drop_from_voice(inner: &mut Inner, pk: &str) -> Vec<String> {
    if pk.is_empty() {
        return Vec::new();
    }
    let rooms: Vec<String> = inner
        .voice
        .iter()
        .filter(|(_, people)| people.contains(pk))
        .map(|(room, _)| room.clone())
        .collect();
    for people in inner.voice.values_mut() {
        people.remove(pk);
    }
    normalize_voice(inner);
    rooms
}

fn mark_peer_offline(inner: &mut Inner, pk: &str) -> Vec<String> {
    inner.last_seen.remove(pk);
    if let Some(profile) = inner.profiles.get_mut(pk) {
        profile.status = "offline".into();
    }
    drop_from_voice(inner, pk)
}

fn drop_disconnected_peer(inner: &mut Inner, pk: &str) -> Vec<(String, String)> {
    if pk.is_empty() || pk == inner.identity.public_hex() {
        return Vec::new();
    }
    mark_peer_offline(inner, pk)
        .into_iter()
        .map(|room| (pk.to_string(), room))
        .collect()
}

fn remember_link_pk(state: &AppState, url: Option<&str>, pk: &str) {
    let Some(url) = url else {
        return;
    };
    if url.is_empty() || url == "relay" || pk.is_empty() {
        return;
    }
    state
        .link_pks
        .lock()
        .expect("link_pks")
        .insert(url.to_string(), pk.to_string());
}

fn on_peer_socket_closed(app: &AppHandle, url: Option<String>) {
    let Some(url) = url else {
        return;
    };
    let state = state_of(app);
    state.remotes.lock().expect("remotes").remove(&url);
    if url == "relay" {
        emit_state(app);
        return;
    }
    let pk = {
        let mut map = state.link_pks.lock().expect("link_pks");
        let pk = map.remove(&url);
        if let Some(ref pk) = pk {
            map.retain(|_, v| v != pk);
        }
        pk
    };
    let Some(pk) = pk else {
        emit_state(app);
        return;
    };
    let leaves = {
        let mut inner = state.inner.lock().expect("state");
        drop_disconnected_peer(&mut inner, &pk)
    };
    let gone = serde_json::json!({
        "type": "presence",
        "publicKey": pk,
        "status": "offline",
    })
    .to_string();
    fanout(&state, &gone, None);
    fanout_voice_leaves(&state, &leaves);
    emit_state(app);
}

fn voice_leave_json(room: &str, pk: &str) -> String {
    serde_json::json!({
        "type": "voice",
        "room": room,
        "action": "leave",
        "publicKey": pk,
        "ts": now_ms(),
    })
    .to_string()
}

fn fanout_voice_leaves(state: &AppState, leaves: &[(String, String)]) {
    for (pk, room) in leaves {
        fanout(state, &voice_leave_json(room, pk), None);
    }
}

fn touch_seen(inner: &mut Inner, pk: &str) {
    if pk.is_empty() || pk == inner.identity.public_hex() {
        return;
    }
    inner.last_seen.insert(pk.to_string(), now_ms());
}

fn apply_presence(inner: &mut Inner, frame: &serde_json::Value, from_snapshot: bool) -> Vec<String> {
    let pk = json_str(frame.get("publicKey"));
    if pk.is_empty() || pk == inner.identity.public_hex() {
        return Vec::new();
    }
    let status_raw = frame.get("status").and_then(|v| v.as_str());
    {
        let entry = inner.profiles.entry(pk.clone()).or_default();
        let name = json_str(frame.get("displayName"));
        if !name.is_empty() {
            entry.display_name = name;
        }
        let avatar = json_str(frame.get("avatar"));
        if !avatar.is_empty() {
            entry.avatar = avatar;
        }
        if let Some(muted) = frame.get("muted").and_then(|v| v.as_bool()) {
            entry.muted = muted;
        }
        if let Some(deafened) = frame.get("deafened").and_then(|v| v.as_bool()) {
            entry.deafened = deafened;
        }
        if status_raw != Some("offline") && (!from_snapshot || status_raw.is_some()) {
            entry.status = normalize_status(status_raw.unwrap_or("online"));
        }
    }
    if status_raw == Some("offline") {
        return mark_peer_offline(inner, &pk);
    }
    if !from_snapshot || status_raw.is_some() {
        inner.last_seen.insert(pk, now_ms());
    }
    Vec::new()
}

fn json_str(value: Option<&serde_json::Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    value.to_string().trim_matches('"').to_string()
}

fn slug_name(name: &str) -> String {
    let trimmed = name.trim();
    let lowered = trimmed.to_lowercase().replace(' ', "-");
    let cleaned: String = lowered
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if cleaned.is_empty() {
        "sala".into()
    } else {
        cleaned
    }
}

fn ensure_general(channels: &mut Vec<String>) {
    if !channels.iter().any(|c| c == "general") {
        channels.insert(0, "general".into());
    }
}

fn merge_unique(dst: &mut Vec<String>, incoming: &[String]) {
    for name in incoming {
        let slug = slug_name(name);
        if !dst.iter().any(|x| x == &slug) {
            dst.push(slug);
        }
    }
}

fn normalize_voice(inner: &mut Inner) {
    let mut next: HashMap<String, HashSet<String>> = HashMap::new();
    for (room, people) in inner.voice.drain() {
        let key = slug_name(&room);
        if key.is_empty() {
            continue;
        }
        next.entry(key).or_default().extend(people);
    }
    inner.voice = next;
}

fn merge_voice_rooms(inner: &mut Inner, rooms: &serde_json::Map<String, serde_json::Value>) {
    let now = now_ms();
    for (room, people) in rooms {
        let incoming: HashSet<String> = people
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .filter(|pk| live_status(inner, pk, now) != "offline")
                    .collect()
            })
            .unwrap_or_default();
        if incoming.is_empty() {
            continue;
        }
        inner
            .voice
            .entry(slug_name(room))
            .or_default()
            .extend(incoming);
    }
    normalize_voice(inner);
}

fn apply_voice_action(inner: &mut Inner, room: &str, action: &str, pk: &str) {
    let room = slug_name(room);
    if room.is_empty() || pk.is_empty() {
        return;
    }
    if action == "join" {
        for people in inner.voice.values_mut() {
            people.remove(pk);
        }
        inner.voice.entry(room).or_default().insert(pk.to_string());
    } else if action == "leave" {
        if pk == inner.identity.public_hex() {
            return;
        }
        if let Some(people) = inner.voice.get_mut(&room) {
            people.remove(pk);
        }
    }
    normalize_voice(inner);
}

fn reset_rooms(inner: &mut Inner) {
    inner.text_channels = vec!["general".into()];
    inner.call_rooms.clear();
    inner.voice.clear();
}

pub fn connect_peer(app: AppHandle, url: String) {
    if url.is_empty() || !(url.starts_with("ws://") || url.starts_with("wss://")) {
        return;
    }
    if !should_dial(&url) {
        return;
    }
    let state = state_of(&app);
    {
        let mut inner = state.inner.lock().expect("state");
        if inner.community.is_none() || is_self_url(&inner, &url) {
            return;
        }
        inner.known_peer_urls.insert(url.clone());
    }
    if state.remotes.lock().expect("remotes").contains_key(&url) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        dial_and_serve(app, url).await;
    });
}

fn connect_inviter(app: AppHandle, urls: Vec<String>) {
    tauri::async_runtime::spawn(async move {
        let state = state_of(&app);
        let urls: Vec<String> = unique_urls(urls)
            .into_iter()
            .filter(|url| {
                (url.starts_with("ws://") || url.starts_with("wss://"))
                    && !is_self_url(&state.inner.lock().expect("state"), url)
                    && on_same_lan(url)
            })
            .collect();
        for url in urls {
            let already_live = state
                .remotes
                .lock()
                .expect("remotes")
                .keys()
                .any(|k| !k.starts_with("pending:"));
            if already_live {
                return;
            }
            match dial_ws(&url).await {
                Ok(ws) => {
                    {
                        let mut inner = state.inner.lock().expect("state");
                        inner.known_peer_urls.insert(url.clone());
                    }
                    let (tx, rx) = mpsc::unbounded_channel::<String>();
                    state
                        .remotes
                        .lock()
                        .expect("remotes")
                        .insert(url.clone(), tx.clone());
                    emit_state(&app);
                    serve_connected(app, ws, Some(url), tx, rx).await;
                    return;
                }
                Err(_) => {}
            }
        }
    });
}

async fn dial_ws(
    url: &str,
) -> Result<
    WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    match tokio::time::timeout(Duration::from_secs(2), connect_async(url)).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err("tempo esgotado".into()),
    }
}

async fn dial_and_serve(app: AppHandle, url: String) {
    match dial_ws(&url).await {
        Ok(ws) => {
            let (tx, rx) = mpsc::unbounded_channel::<String>();
            state_of(&app)
                .remotes
                .lock()
                .expect("remotes")
                .insert(url.clone(), tx.clone());
            emit_state(&app);
            serve_connected(app, ws, Some(url), tx, rx).await;
        }
        Err(_) => {}
    }
}

async fn serve_connected<S>(
    app: AppHandle,
    ws: WebSocketStream<S>,
    mut known_url: Option<String>,
    tx: mpsc::UnboundedSender<String>,
    mut rx: mpsc::UnboundedReceiver<String>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut stream) = ws.split();
    for msg in handshake_messages(&app, false) {
        let _ = tx.send(msg);
    }

    loop {
        tokio::select! {
            outgoing = rx.recv() => {
                let Some(text) = outgoing else { break; };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break; };
                let Message::Text(text) = msg else { continue; };
                handle_remote(&app, text.as_str(), &mut known_url, &tx);
            }
        }
    }

    on_peer_socket_closed(&app, known_url);
}

fn handle_remote(
    app: &AppHandle,
    raw: &str,
    from_url: &mut Option<String>,
    tx: &mpsc::UnboundedSender<String>,
) {
    if already_seen(app, raw) {
        return;
    }
    let state = state_of(app);
    let Ok(frame) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    let kind = frame.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "hello" => {
            let community_id = frame
                .get("communityId")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let listen = frame
                .get("listenUrl")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            {
                let inner = state.inner.lock().expect("state");
                if inner.community.as_ref().map(|c| c.id.as_str()) != Some(community_id.as_str()) {
                    return;
                }
            }
            {
                let mut inner = state.inner.lock().expect("state");
                if !listen.is_empty() {
                    inner.known_peer_urls.insert(listen.clone());
                }
                if let Some(urls) = frame.get("listenUrls").and_then(|v| v.as_array()) {
                    for url in urls.iter().filter_map(|v| v.as_str()) {
                        if parse_ws(url).is_some_and(|(h, _)| is_loopback_host(&h)) {
                            continue;
                        }
                        inner.known_peer_urls.insert(url.to_string());
                    }
                }
                apply_presence(&mut inner, &frame, false);
            }
            let hello_pk = json_str(frame.get("publicKey"));
            remember_link_pk(&state, from_url.as_deref(), &hello_pk);
            if !listen.is_empty() {
                remember_link_pk(&state, Some(listen.as_str()), &hello_pk);
            }
            let their_history = frame
                .get("historyCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            if from_url.as_deref() != Some("relay") && !listen.is_empty() {
                let mut remotes = state.remotes.lock().expect("remotes");
                if let Some(old) = from_url.as_ref() {
                    if old != &listen {
                        if let Some(sender) = remotes.remove(old) {
                            remotes.insert(listen.clone(), sender);
                        } else {
                            remotes.insert(listen.clone(), tx.clone());
                        }
                    }
                } else {
                    remotes.insert(listen.clone(), tx.clone());
                }
                *from_url = Some(listen);
            }
            emit_state(app);
            persist_session(app);
            fanout(&state, &layout_json(&state), None);
            if voice_busy(&state) {
                fanout(&state, &voice_json(&state), None);
            }
            fanout(&state, &presence_json(&state), None);
            fanout(&state, &presence_state_json(&state), None);
            maybe_seed_peer(app, &hello_pk, their_history);
        }
        "peers" => {
            if from_url.as_deref() == Some("relay") {
                return;
            }
            let urls = frame
                .get("urls")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for url in urls {
                let Some(u) = url.as_str() else { continue };
                if parse_ws(u).is_some_and(|(h, _)| is_loopback_host(&h)) {
                    continue;
                }
                connect_peer(app.clone(), u.to_string());
            }
        }
        "chat" => {
            let Ok(wire) = serde_json::from_value::<WireMessage>(frame) else {
                return;
            };
            let (live_key, my_pk) = {
                let inner = state.inner.lock().expect("state");
                let Some(c) = inner.community.as_ref() else {
                    return;
                };
                if wire.sender == inner.identity.public_hex() {
                    return;
                }
                (c.live_key.clone(), inner.identity.public_hex())
            };
            let Some(plain) = open_chat(&live_key, &wire) else {
                return;
            };
            emit_and_store_message(
                app,
                UiMessage {
                    sender: wire.sender.clone(),
                    text: plain.text,
                    ts: plain.ts,
                    channel: plain.channel,
                    is_self: wire.sender == my_pk,
                },
            );
            {
                let mut inner = state.inner.lock().expect("state");
                touch_seen(&mut inner, &wire.sender);
            }
            fanout(&state, raw, from_url.as_deref());
        }
        "layout" => {
            let texts: Vec<String> = frame
                .get("textChannels")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let calls: Vec<String> = frame
                .get("callRooms")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            {
                let mut inner = state.inner.lock().expect("state");
                merge_unique(&mut inner.text_channels, &texts);
                ensure_general(&mut inner.text_channels);
                merge_unique(&mut inner.call_rooms, &calls);
            }
            emit_state(app);
            persist_session(app);
            fanout(&state, raw, from_url.as_deref());
        }
        "voice-state" => {
            if let Some(rooms) = frame.get("rooms").and_then(|v| v.as_object()) {
                let mut inner = state.inner.lock().expect("state");
                merge_voice_rooms(&mut inner, rooms);
            }
            emit_state(app);
            fanout(&state, raw, from_url.as_deref());
        }
        "voice" => {
            let room = frame
                .get("room")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let action = frame.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let pk = frame
                .get("publicKey")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            {
                let mut inner = state.inner.lock().expect("state");
                apply_voice_action(&mut inner, room, action, pk);
                if action == "join" {
                    touch_seen(&mut inner, pk);
                }
            }
            emit_state(app);
            fanout(&state, raw, from_url.as_deref());
            if action == "join" {
                fanout(&state, &voice_json(&state), None);
            }
        }
        "presence" => {
            let leaves = {
                let mut inner = state.inner.lock().expect("state");
                apply_presence(&mut inner, &frame, false)
                    .into_iter()
                    .map(|room| (json_str(frame.get("publicKey")), room))
                    .collect::<Vec<_>>()
            };
            emit_state(app);
            persist_session(app);
            fanout(&state, raw, from_url.as_deref());
            fanout_voice_leaves(&state, &leaves);
        }
        "presence-state" => {
            if let Some(people) = frame.get("people").and_then(|v| v.as_object()) {
                let mut inner = state.inner.lock().expect("state");
                for (pk, value) in people {
                    let mut payload = serde_json::json!({
                        "publicKey": pk,
                        "displayName": json_str(value.get("displayName")),
                        "avatar": json_str(value.get("avatar")),
                        "muted": value.get("muted"),
                        "deafened": value.get("deafened"),
                    });
                    if let Some(status) = value
                        .get("status")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        payload["status"] = serde_json::Value::String(status.to_string());
                    }
                    apply_presence(&mut inner, &payload, true);
                }
            }
            emit_state(app);
            persist_session(app);
        }
        "rtc" => {
            let _ = app.emit("ui-rtc", frame);
            fanout(&state, raw, from_url.as_deref());
        }
        "history" => {
            let my_pk = state.inner.lock().expect("state").identity.public_hex();
            let existing = get_history(app);
            let Some(arr) = frame.get("messages").and_then(|v| v.as_array()) else {
                return;
            };
            for item in arr {
                let sender = item
                    .get("sender")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let text = item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let ts = item.get("ts").and_then(|v| v.as_i64()).unwrap_or(0);
                let channel = item
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("general")
                    .to_string();
                if sender.is_empty() || text.is_empty() {
                    continue;
                }
                if existing
                    .iter()
                    .any(|m| m.sender == sender && m.ts == ts && m.text == text)
                {
                    continue;
                }
                emit_and_store_message(
                    app,
                    UiMessage {
                        is_self: sender == my_pk,
                        sender,
                        text,
                        ts,
                        channel,
                    },
                );
            }
            refresh_archive(app);
            emit_state(app);
        }
        _ => {}
    }
}

pub fn send_signal(app: &AppHandle, frame: serde_json::Value) -> Result<(), String> {
    if frame.get("type").and_then(|v| v.as_str()) != Some("rtc") {
        return Err("sinal invalido".into());
    }
    let state = state_of(app);
    let json = frame.to_string();
    fanout(&state, &json, None);
    Ok(())
}

pub fn create_local(app: &AppHandle, name: &str) {
    let state = state_of(app);
    clear_stored_session(app);
    {
        let mut inner = state.inner.lock().expect("state");
        inner.community = Some(create_community(&inner.identity, name));
        inner.invite_peers = inner.listen_urls.clone();
        inner.known_peer_urls.clear();
        inner.profiles.clear();
        inner.last_seen.clear();
        inner.seeded.clear();
        inner.seeding.clear();
        inner.archive_messages = 0;
        inner.archive_bytes = 0;
        reset_rooms(&mut inner);
    }
    state.remotes.lock().expect("remotes").clear();
    persist_session(app);
    emit_state(app);
    crate::relay::spawn(app.clone());
}

pub fn join_community(app: &AppHandle, raw_invite: &str) -> Result<(), String> {
    let invite = decode_invite(raw_invite)?;
    let peers = invite.peers.clone();
    let state = state_of(app);
    clear_stored_session(app);
    {
        let mut inner = state.inner.lock().expect("state");
        inner.community = Some(invite.community);
        inner.invite_peers = peers.clone();
        inner.known_peer_urls.clear();
        inner.profiles.clear();
        inner.last_seen.clear();
        inner.seeded.clear();
        inner.seeding.clear();
        inner.archive_messages = 0;
        inner.archive_bytes = 0;
        reset_rooms(&mut inner);
    }
    state.remotes.lock().expect("remotes").clear();
    persist_session(app);
    emit_state(app);
    connect_inviter(app.clone(), peers);
    crate::relay::spawn(app.clone());
    Ok(())
}

pub fn send_chat(app: &AppHandle, text: &str, channel: &str) -> Result<(), String> {
    let state = state_of(app);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let channel = slug_name(channel);
    let (wire, sender, ts, channel) = {
        let inner = state.inner.lock().expect("state");
        let Some(community) = inner.community.as_ref() else {
            return Err("Entre ou crie uma comunidade primeiro.".into());
        };
        let channel = if inner.text_channels.iter().any(|c| c == &channel) {
            channel
        } else {
            "general".into()
        };
        let plain = ChatPlain {
            channel: channel.clone(),
            text: trimmed.into(),
            ts: now_ms(),
        };
        let wire = seal_chat(&inner.identity, &community.live_key, &plain)?;
        (wire, inner.identity.public_hex(), plain.ts, channel)
    };
    let json = serde_json::to_string(&wire).map_err(|e| e.to_string())?;
    fanout(&state, &json, None);
    emit_and_store_message(
        app,
        UiMessage {
            sender,
            text: trimmed.into(),
            ts,
            channel,
            is_self: true,
        },
    );
    Ok(())
}

pub fn add_room(app: &AppHandle, kind: &str, name: &str) -> Result<(), String> {
    let slug = slug_name(name);
    let state = state_of(app);
    {
        let mut inner = state.inner.lock().expect("state");
        if inner.community.is_none() {
            return Err("Entre ou crie uma comunidade primeiro.".into());
        }
        if kind == "call" {
            if !inner.call_rooms.iter().any(|c| c == &slug) {
                inner.call_rooms.push(slug);
            }
        } else {
            if slug != "general" && !inner.text_channels.iter().any(|c| c == &slug) {
                inner.text_channels.push(slug);
            }
            ensure_general(&mut inner.text_channels);
        }
    }
    let json = layout_json(&state);
    fanout(&state, &json, None);
    persist_session(app);
    emit_state(app);
    Ok(())
}

pub fn join_call(app: &AppHandle, room: &str) -> Result<(), String> {
    let room = slug_name(room);
    let state = state_of(app);
    let pk = {
        let mut inner = state.inner.lock().expect("state");
        if inner.community.is_none() {
            return Err("Entre ou crie uma comunidade primeiro.".into());
        }
        if !inner.call_rooms.iter().any(|c| c == &room) {
            inner.call_rooms.push(room.clone());
        }
        let pk = inner.identity.public_hex();
        for people in inner.voice.values_mut() {
            people.remove(&pk);
        }
        inner.voice.entry(room.clone()).or_default().insert(pk.clone());
        normalize_voice(&mut inner);
        pk
    };
    let json = serde_json::json!({
        "type": "voice",
        "room": room,
        "action": "join",
        "publicKey": pk,
        "ts": now_ms(),
    })
    .to_string();
    fanout(&state, &json, None);
    fanout(&state, &voice_json(&state), None);
    fanout(&state, &presence_json(&state), None);
    fanout(&state, &presence_state_json(&state), None);
    persist_session(app);
    emit_state(app);
    let app2 = app.clone();
    let room2 = room.clone();
    let pk2 = pk.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let state = state_of(&app2);
        let retry = serde_json::json!({
            "type": "voice",
            "room": room2,
            "action": "join",
            "publicKey": pk2,
            "ts": now_ms(),
        })
        .to_string();
        fanout(&state, &retry, None);
        if voice_busy(&state) {
            fanout(&state, &voice_json(&state), None);
        }
    });
    Ok(())
}

pub fn publish_presence(
    app: &AppHandle,
    muted: bool,
    deafened: bool,
    status: Option<&str>,
) -> Result<(), String> {
    let state = state_of(app);
    {
        let mut inner = state.inner.lock().expect("state");
        inner.muted = muted;
        inner.deafened = deafened;
        if let Some(status) = status {
            inner.status = normalize_status(status);
        }
    }
    let json = presence_json(&state);
    fanout(&state, &json, None);
    emit_state(app);
    Ok(())
}

pub fn leave_call(app: &AppHandle) -> Result<(), String> {
    let state = state_of(app);
    let (pk, rooms) = {
        let mut inner = state.inner.lock().expect("state");
        let pk = inner.identity.public_hex();
        let rooms: Vec<String> = inner
            .voice
            .iter()
            .filter(|(_, people)| people.contains(&pk))
            .map(|(room, _)| room.clone())
            .collect();
        for people in inner.voice.values_mut() {
            people.remove(&pk);
        }
        (pk, rooms)
    };
    for room in rooms {
        let json = serde_json::json!({
            "type": "voice",
            "room": room,
            "action": "leave",
            "publicKey": pk,
        })
        .to_string();
        fanout(&state, &json, None);
    }
    emit_state(app);
    Ok(())
}

pub fn leave_community(app: &AppHandle) {
    let state = state_of(app);
    let leaves = {
        let mut inner = state.inner.lock().expect("state");
        inner.status = "offline".into();
        let pk = inner.identity.public_hex();
        drop_from_voice(&mut inner, &pk)
            .into_iter()
            .map(|room| (pk.clone(), room))
            .collect::<Vec<_>>()
    };
    fanout(&state, &presence_json(&state), None);
    fanout_voice_leaves(&state, &leaves);
    state.relay_gen.fetch_add(1, Ordering::SeqCst);
    {
        let mut inner = state.inner.lock().expect("state");
        inner.community = None;
        inner.invite_peers.clear();
        inner.known_peer_urls.clear();
        reset_rooms(&mut inner);
        inner.text_channels.clear();
        inner.profiles.clear();
        inner.last_seen.clear();
        inner.status = "online".into();
        inner.seeded.clear();
        inner.seeding.clear();
        inner.archive_messages = 0;
        inner.archive_bytes = 0;
    }
    clear_stored_session(app);
    state.remotes.lock().expect("remotes").clear();
    emit_state(app);
}

fn profile_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("profile.json"))
}

fn load_json_profile(app: &AppHandle) {
    let Ok(path) = profile_path(app) else {
        return;
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(stored) = serde_json::from_str::<StoredProfile>(&raw) else {
        return;
    };
    let Ok(identity) = Identity::from_secret_hex(&stored.secret_key) else {
        return;
    };
    let state = state_of(app);
    let mut inner = state.inner.lock().expect("state");
    inner.identity = identity;
    inner.display_name = stored.display_name;
    inner.avatar = stored.avatar;
}

pub fn save_profile(app: &AppHandle, display_name: &str, avatar: &str) -> Result<(), String> {
    let name = display_name.trim();
    if !(2..=32).contains(&name.chars().count()) {
        return Err("O nome precisa ter entre 2 e 32 caracteres.".into());
    }
    if avatar.len() > 400_000 {
        return Err("Foto grande demais. Escolha outra imagem.".into());
    }
    let path = profile_path(app)?;
    let secret = {
        let state = state_of(app);
        let mut inner = state.inner.lock().expect("state");
        inner.display_name = name.to_string();
        inner.avatar = avatar.to_string();
        inner.identity.secret_hex()
    };
    let stored = StoredProfile {
        display_name: name.to_string(),
        avatar: avatar.to_string(),
        secret_key: secret,
    };
    let json = serde_json::to_string_pretty(&stored).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    persist_profile(app);
    persist_session(app);
    let state = state_of(app);
    fanout(&state, &presence_json(&state), None);
    emit_state(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_inner() -> Inner {
        Inner {
            identity: Identity::generate(),
            display_name: "eu".into(),
            avatar: String::new(),
            community: None,
            invite_peers: Vec::new(),
            listen_url: String::new(),
            listen_urls: Vec::new(),
            listen_port: 0,
            known_peer_urls: HashSet::new(),
            text_channels: vec!["general".into()],
            call_rooms: Vec::new(),
            voice: HashMap::new(),
            profiles: HashMap::new(),
            muted: false,
            deafened: false,
            status: "online".into(),
            last_seen: HashMap::new(),
            archive_messages: 0,
            archive_bytes: 0,
            seeded: HashMap::new(),
            seeding: HashSet::new(),
        }
    }

    #[test]
    fn slug_normalizes_room_names() {
        assert_eq!(slug_name("Lobby"), "lobby");
        assert_eq!(slug_name("  Sala Principal  "), "sala-principal");
        assert_eq!(slug_name("***"), "sala");
    }

    #[test]
    fn ensure_general_is_first_and_unique() {
        let mut channels = vec!["random".into()];
        ensure_general(&mut channels);
        assert_eq!(channels[0], "general");
        ensure_general(&mut channels);
        assert_eq!(channels.iter().filter(|c| *c == "general").count(), 1);
    }

    #[test]
    fn merge_unique_slugs_and_dedupes() {
        let mut rooms = vec!["lobby".into()];
        merge_unique(&mut rooms, &["Lobby".into(), "hangout".into()]);
        assert_eq!(rooms, vec!["lobby".to_string(), "hangout".to_string()]);
    }

    #[test]
    fn normalize_voice_merges_lobby_casings() {
        let mut inner = sample_inner();
        inner.voice.insert("Lobby".into(), HashSet::from(["alice".into()]));
        inner.voice.insert("lobby".into(), HashSet::from(["bob".into()]));
        normalize_voice(&mut inner);
        assert_eq!(inner.voice.len(), 1);
        let people = inner.voice.get("lobby").expect("lobby");
        assert!(people.contains("alice"));
        assert!(people.contains("bob"));
    }

    #[test]
    fn empty_voice_state_does_not_wipe_roster() {
        let mut inner = sample_inner();
        inner
            .voice
            .insert("lobby".into(), HashSet::from(["alice".into()]));
        let frame = serde_json::json!({ "rooms": { "lobby": [] } });
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap());
        assert!(inner.voice.get("lobby").unwrap().contains("alice"));
    }

    #[test]
    fn voice_state_extends_existing_room() {
        let mut inner = sample_inner();
        inner
            .voice
            .insert("lobby".into(), HashSet::from(["alice".into()]));
        inner.last_seen.insert("bob".into(), now_ms());
        let frame = serde_json::json!({ "rooms": { "Lobby": ["bob"] } });
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap());
        let people = inner.voice.get("lobby").unwrap();
        assert!(people.contains("alice"));
        assert!(people.contains("bob"));
    }

    #[test]
    fn empty_voice_rooms_are_not_busy() {
        let mut inner = sample_inner();
        inner.voice.insert("lobby".into(), HashSet::new());
        assert!(!voice_is_busy(&inner));
        inner.voice.get_mut("lobby").unwrap().insert("alice".into());
        assert!(voice_is_busy(&inner));
    }

    #[test]
    fn voice_join_slugs_room_and_moves_peer() {
        let mut inner = sample_inner();
        inner
            .voice
            .insert("lobby".into(), HashSet::from(["alice".into()]));
        apply_voice_action(&mut inner, "Lobby", "join", "alice");
        apply_voice_action(&mut inner, "Hangout", "join", "alice");
        assert!(!inner.voice.get("lobby").unwrap().contains("alice"));
        assert!(inner.voice.get("hangout").unwrap().contains("alice"));
    }

    #[test]
    fn json_str_reads_nested_presence_names() {
        let frame = serde_json::json!({
            "publicKey": "aa",
            "displayName": "Ana"
        });
        assert_eq!(json_str(frame.get("displayName")), "Ana");
        let nested = serde_json::json!({ "displayName": "Ana" });
        let mut inner = sample_inner();
        apply_presence(
            &mut inner,
            &serde_json::json!({
                "publicKey": "aa",
                "displayName": json_str(nested.get("displayName")),
            }),
            false,
        );
        assert_eq!(inner.profiles.get("aa").unwrap().display_name, "Ana");
    }

    #[test]
    fn stale_members_are_offline_until_heartbeat() {
        let mut inner = sample_inner();
        inner.profiles.insert("aa".into(), PeerProfile {
            display_name: "Ana".into(),
            ..PeerProfile::default()
        });
        let now = 1_000_000;
        assert_eq!(live_status(&inner, "aa", now), "offline");
        inner.last_seen.insert("aa".into(), now);
        inner.profiles.get_mut("aa").unwrap().status = "away".into();
        assert_eq!(live_status(&inner, "aa", now), "away");
        expire_stale_presence(&mut inner, now + PRESENCE_TTL_MS + 1);
        assert_eq!(live_status(&inner, "aa", now + PRESENCE_TTL_MS + 1), "offline");
    }

    #[test]
    fn socket_drop_removes_peer_from_call() {
        let mut inner = sample_inner();
        inner.profiles.insert(
            "aa".into(),
            PeerProfile {
                display_name: "Ana".into(),
                status: "online".into(),
                ..PeerProfile::default()
            },
        );
        inner
            .voice
            .insert("lobby".into(), HashSet::from(["aa".into(), "bb".into()]));
        inner.last_seen.insert("aa".into(), now_ms());
        let leaves = drop_disconnected_peer(&mut inner, "aa");
        assert_eq!(leaves, vec![("aa".to_string(), "lobby".to_string())]);
        assert!(!inner.voice.get("lobby").unwrap().contains("aa"));
        assert!(inner.voice.get("lobby").unwrap().contains("bb"));
        assert_eq!(live_status(&inner, "aa", now_ms()), "offline");
    }

    #[test]
    fn stale_presence_does_not_kick_call() {
        let mut inner = sample_inner();
        inner.profiles.insert(
            "aa".into(),
            PeerProfile {
                display_name: "Ana".into(),
                ..PeerProfile::default()
            },
        );
        inner
            .voice
            .insert("lobby".into(), HashSet::from(["aa".into(), "bb".into()]));
        inner.last_seen.insert("aa".into(), 10);
        let leaves = expire_stale_presence(&mut inner, 10 + PRESENCE_TTL_MS + 1);
        assert!(inner.voice.get("lobby").unwrap().contains("aa"));
        assert!(inner.voice.get("lobby").unwrap().contains("bb"));
        assert!(leaves.is_empty());
        assert_eq!(live_status(&inner, "aa", 10 + PRESENCE_TTL_MS + 1), "offline");
    }

    #[test]
    fn voice_state_does_not_readd_offline_peer() {
        let mut inner = sample_inner();
        inner.profiles.insert(
            "aa".into(),
            PeerProfile {
                display_name: "Ana".into(),
                status: "offline".into(),
                ..PeerProfile::default()
            },
        );
        inner
            .voice
            .insert("lobby".into(), HashSet::from(["bb".into()]));
        let frame = serde_json::json!({ "rooms": { "lobby": ["aa", "bb"] } });
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap());
        assert!(!inner.voice.get("lobby").unwrap().contains("aa"));
        assert!(inner.voice.get("lobby").unwrap().contains("bb"));
    }

    #[test]
    fn explicit_offline_presence_drops_from_call() {
        let mut inner = sample_inner();
        inner
            .voice
            .insert("lobby".into(), HashSet::from(["aa".into()]));
        let rooms = apply_presence(
            &mut inner,
            &serde_json::json!({
                "publicKey": "aa",
                "displayName": "Ana",
                "status": "offline",
            }),
            false,
        );
        assert_eq!(rooms, vec!["lobby".to_string()]);
        assert!(inner
            .voice
            .get("lobby")
            .map(|people| !people.contains("aa"))
            .unwrap_or(true));
    }

    #[test]
    fn presence_snapshot_does_not_revive_ghosts() {
        let mut inner = sample_inner();
        apply_presence(
            &mut inner,
            &serde_json::json!({
                "publicKey": "ghost",
                "displayName": "Velho",
            }),
            true,
        );
        assert_eq!(live_status(&inner, "ghost", now_ms()), "offline");
        apply_presence(
            &mut inner,
            &serde_json::json!({
                "publicKey": "live",
                "displayName": "Ana",
                "status": "busy",
            }),
            true,
        );
        assert_eq!(live_status(&inner, "live", now_ms()), "busy");
        let people = live_profiles(&inner, now_ms(), false);
        assert!(!people.contains_key("ghost"));
        assert_eq!(people.get("live").unwrap().status, "busy");
    }

    #[test]
    fn explicit_offline_presence_drops_last_seen() {
        let mut inner = sample_inner();
        apply_presence(
            &mut inner,
            &serde_json::json!({
                "publicKey": "aa",
                "displayName": "Ana",
                "status": "online",
            }),
            false,
        );
        apply_presence(
            &mut inner,
            &serde_json::json!({
                "publicKey": "aa",
                "status": "offline",
            }),
            false,
        );
        assert_eq!(live_status(&inner, "aa", now_ms()), "offline");
    }

    #[test]
    fn seed_card_hidden_when_idle_even_with_offline_members() {
        let mut inner = sample_inner();
        inner.archive_messages = 10;
        inner.archive_bytes = 1_000;
        inner.profiles.insert("aaa".into(), PeerProfile::default());
        inner.profiles.insert("bbb".into(), PeerProfile::default());
        inner.seeded.insert("aaa".into(), 10);
        assert!(!seeding_visible(&inner));
        let (sent, total) = seed_progress(&inner);
        assert_eq!((sent, total), (1_000, 1_000));
    }

    #[test]
    fn seed_progress_ignores_offline_profiles() {
        let mut inner = sample_inner();
        inner.archive_messages = 8;
        inner.archive_bytes = 400;
        for i in 0..5 {
            inner
                .profiles
                .insert(format!("p{i}"), PeerProfile::default());
        }
        inner.seeding.insert("p0".into());
        inner.seeded.insert("p1".into(), 8);
        assert!(seeding_visible(&inner));
        let (sent, total) = seed_progress(&inner);
        assert_eq!((sent, total), (400, 800));
    }

    #[test]
    fn seed_progress_idle_without_jobs_is_full_not_half() {
        let mut inner = sample_inner();
        inner.archive_messages = 4;
        inner.archive_bytes = 200;
        inner.profiles.insert("offline".into(), PeerProfile::default());
        inner.profiles.insert("ghost".into(), PeerProfile::default());
        assert!(!seeding_visible(&inner));
        assert_eq!(seed_progress(&inner), (1, 1));
    }

    #[test]
    fn hamachi_and_hyperv_are_not_usable() {
        assert!(!usable_v4(Ipv4Addr::new(26, 1, 2, 3)));
        assert!(!usable_v4(Ipv4Addr::new(192, 168, 137, 10)));
        assert!(usable_v4(Ipv4Addr::new(192, 168, 100, 10)));
        assert!(!usable_v6("2001:0:53aa:64c:0:5efe:c0a8:6401".parse().unwrap()));
        assert!(!usable_v6("2002:c0a8:1::1".parse().unwrap()));
    }

    #[test]
    fn should_not_dial_junk_adapters() {
        assert!(!should_dial("ws://26.12.34.56:7340"));
        assert!(!should_dial("ws://192.168.137.1:7340"));
        assert!(should_dial("ws://127.0.0.1:7340"));
    }

    #[test]
    fn parse_ws_handles_ipv6() {
        assert_eq!(
            parse_ws("ws://[2001:db8::1]:7340"),
            Some(("2001:db8::1".into(), 7340))
        );
    }

    #[test]
    fn same_home_lan_requires_matching_subnet() {
        assert!(same_home_lan(
            Ipv4Addr::new(192, 168, 100, 2),
            Ipv4Addr::new(192, 168, 100, 9)
        ));
        assert!(!same_home_lan(
            Ipv4Addr::new(192, 168, 100, 2),
            Ipv4Addr::new(192, 168, 1, 9)
        ));
    }
}
