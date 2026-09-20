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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use crate::store::Store;
use tauri::{AppHandle, Emitter, Manager};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::http::header::{HeaderValue, SEC_WEBSOCKET_PROTOCOL};
use tokio_tungstenite::{accept_hdr_async, connect_async, tungstenite::Message, WebSocketStream};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiCommunity {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiLiveCall {
    pub community_id: String,
    pub community_name: String,
    pub owner_key: String,
    pub room: String,
    pub voice: HashMap<String, Vec<String>>,
    pub profiles: HashMap<String, PeerProfile>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    pub public_key: String,
    pub display_name: String,
    pub avatar: String,
    pub community_name: String,
    pub community_id: String,
    pub communities: Vec<UiCommunity>,
    pub invite: String,
    pub listen_url: String,
    pub listen_urls: Vec<String>,
    pub peers: Vec<String>,
    pub text_channels: Vec<String>,
    pub call_rooms: Vec<String>,
    pub voice: HashMap<String, Vec<String>>,
    pub profiles: HashMap<String, PeerProfile>,
    pub owner_key: String,
    pub archive_bytes: u64,
    pub archive_messages: u32,
    pub seed_sent: u64,
    pub seed_total: u64,
    pub seed_active: bool,
    pub seeding: Vec<String>,
    pub live_call: Option<UiLiveCall>,
    pub archive_status: String,
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
    #[serde(default)]
    pub sharing_screen: bool,
}

impl Default for PeerProfile {
    fn default() -> Self {
        Self {
            display_name: String::new(),
            avatar: String::new(),
            muted: false,
            deafened: false,
            status: offline_status(),
            sharing_screen: false,
        }
    }
}

fn offline_status() -> String {
    "offline".into()
}

const PRESENCE_TTL_MS: i64 = 90_000;
const VOICE_JOIN_GRACE_MS: i64 = 8_000;
const VOICE_LEAVE_HOLD_MS: i64 = 30 * 60 * 1000;

#[derive(Clone, Serialize, Deserialize)]
pub struct UiMessage {
    pub sender: String,
    pub text: String,
    pub ts: i64,
    pub channel: String,
    #[serde(rename = "self")]
    pub is_self: bool,
    #[serde(default, rename = "communityId")]
    pub community_id: String,
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
    viewed_id: Option<String>,
    invite_peers: Vec<String>,
    relays: Vec<String>,
    listen_url: String,
    listen_urls: Vec<String>,
    listen_port: u16,
    known_peer_urls: HashSet<String>,
    text_channels: Vec<String>,
    call_rooms: Vec<String>,
    voice: HashMap<String, HashSet<String>>,
    voice_left: HashMap<String, i64>,
    voice_joined: HashMap<String, i64>,
    profiles: HashMap<String, PeerProfile>,
    muted: bool,
    deafened: bool,
    sharing_screen: bool,
    status: String,
    last_seen: HashMap<String, i64>,
    archive_messages: u32,
    archive_bytes: u64,
    seeded: HashMap<String, u32>,
    seeding: HashSet<String>,
    archive_status: String,
    /// blob_id awaiting shard-ack during leave handoff.
    handoff_wait: Option<(String, String)>,
    /// peer pk → (blob_id, shard indices they advertised).
    shard_inventory: HashMap<String, (String, Vec<u8>)>,
}

pub struct AppState {
    inner: Mutex<Inner>,
    remotes: Mutex<HashMap<String, mpsc::UnboundedSender<String>>>,
    link_pks: Mutex<HashMap<String, String>>,
    store: Mutex<Option<Store>>,
    pending: AtomicU64,
    relay_gen: AtomicU64,
    link_gen: AtomicU64,
    seen: Mutex<(HashSet<u64>, VecDeque<u64>)>,
    outbox: Mutex<VecDeque<String>>,
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
                viewed_id: None,
                invite_peers: Vec::new(),
                relays: Vec::new(),
                listen_url: String::new(),
                listen_urls: Vec::new(),
                listen_port: 0,
                known_peer_urls: HashSet::new(),
                text_channels: Vec::new(),
                call_rooms: Vec::new(),
                voice: HashMap::new(),
                voice_left: HashMap::new(),
                voice_joined: HashMap::new(),
                profiles: HashMap::new(),
                muted: false,
                deafened: false,
                sharing_screen: false,
                status: "online".into(),
                last_seen: HashMap::new(),
                archive_messages: 0,
                archive_bytes: 0,
                seeded: HashMap::new(),
                seeding: HashSet::new(),
                archive_status: "live".into(),
                handoff_wait: None,
                shard_inventory: HashMap::new(),
            }),
            remotes: Mutex::new(HashMap::new()),
            link_pks: Mutex::new(HashMap::new()),
            store: Mutex::new(None),
            pending: AtomicU64::new(1),
            relay_gen: AtomicU64::new(0),
            link_gen: AtomicU64::new(0),
            seen: Mutex::new((HashSet::new(), VecDeque::new())),
            outbox: Mutex::new(VecDeque::new()),
            #[cfg(target_os = "linux")]
            rtc: std::sync::Arc::new(crate::rtc::RtcHub::new()),
        }
    }

    pub fn snapshot(&self) -> UiState {
        let inner = self.inner.lock().expect("state");
        let live_id = inner.community.as_ref().map(|c| c.id.clone()).unwrap_or_default();
        let viewed_id = viewed_community_id(&inner).unwrap_or_default();
        let overlay = if !viewed_id.is_empty() && viewed_id != live_id {
            let guard = self.store.lock().expect("store");
            guard.as_ref().and_then(|store| store.load_session(&viewed_id))
        } else {
            None
        };
        let live_call = snapshot_live_call(&inner);
        let live_peers: Vec<String> = if overlay.is_some() {
            Vec::new()
        } else {
            self.remotes
                .lock()
                .expect("remotes")
                .keys()
                .filter(|k| is_direct_remote_key(k))
                .cloned()
                .collect()
        };
        let communities = {
            let guard = self.store.lock().expect("store");
            guard
                .as_ref()
                .map(|store| {
                    let mut list = store.list_communities();
                    list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                    list.into_iter()
                        .map(|c| UiCommunity {
                            id: c.id,
                            name: c.name,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        if let Some(session) = overlay {
            let (archive_messages, archive_bytes) = {
                let guard = self.store.lock().expect("store");
                let msgs = guard
                    .as_ref()
                    .map(|store| store.load_messages(&session.community.id))
                    .unwrap_or_default();
                let bytes = msgs
                    .iter()
                    .map(|m| (m.text.len() + m.sender.len() + m.channel.len() + 24) as u64)
                    .sum();
                (msgs.len() as u32, bytes)
            };
            let invite = encode_invite(&Invite::new(
                session.community.clone(),
                session.invite_peers.clone(),
                session.relays.clone(),
            ));
            let mut profiles = session.profiles.clone();
            for profile in profiles.values_mut() {
                profile.status = "offline".into();
                profile.sharing_screen = false;
            }
            return UiState {
                public_key: inner.identity.public_hex(),
                display_name: inner.display_name.clone(),
                avatar: inner.avatar.clone(),
                community_name: session.community.genesis.name.clone(),
                community_id: session.community.id.clone(),
                communities,
                invite,
                listen_url: inner.listen_url.clone(),
                listen_urls: inner.listen_urls.clone(),
                peers: live_peers,
                text_channels: session.text_channels.clone(),
                call_rooms: session.call_rooms.clone(),
                voice: HashMap::new(),
                profiles,
                owner_key: session.community.genesis.owner.clone(),
                archive_bytes,
                archive_messages,
                seed_sent: 1,
                seed_total: 1,
                seed_active: false,
                seeding: Vec::new(),
                live_call,
                archive_status: "live".into(),
            };
        }
        let invite = inner.community.as_ref().map(|c| {
            encode_invite(&Invite::new(
                c.clone(),
                invite_peer_list(&inner),
                inner.relays.clone(),
            ))
        });
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
            community_id: live_id,
            communities,
            invite: invite.unwrap_or_default(),
            listen_url: inner.listen_url.clone(),
            listen_urls: inner.listen_urls.clone(),
            peers: live_peers,
            text_channels: inner.text_channels.clone(),
            call_rooms: inner.call_rooms.clone(),
            voice: snapshot_voice(&inner),
            profiles: snapshot_profiles(&inner),
            owner_key: inner
                .community
                .as_ref()
                .map(|c| c.genesis.owner.clone())
                .unwrap_or_default(),
            archive_bytes: inner.archive_bytes,
            archive_messages: inner.archive_messages,
            seed_sent: seed.0,
            seed_total: seed.1,
            seed_active: seeding_visible(&inner),
            seeding: inner.seeding.iter().cloned().collect(),
            live_call,
            archive_status: inner.archive_status.clone(),
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

pub(crate) fn community_relays(app: &AppHandle) -> Vec<String> {
    let state = state_of(app);
    let inner = state.inner.lock().expect("state");
    crate::relay::effective(&inner.relays)
}

fn viewed_community_id(inner: &Inner) -> Option<String> {
    inner
        .viewed_id
        .as_ref()
        .filter(|id| !id.is_empty())
        .cloned()
        .or_else(|| inner.community.as_ref().map(|c| c.id.clone()))
}

fn seated_in_call(inner: &Inner) -> bool {
    let me = inner.identity.public_hex();
    inner.voice.values().any(|people| people.contains(&me))
}

fn snapshot_voice(inner: &Inner) -> HashMap<String, Vec<String>> {
    let mut rooms: Vec<_> = inner.voice.iter().collect();
    rooms.sort_by(|a, b| a.0.cmp(b.0));
    rooms
        .into_iter()
        .map(|(room, people)| {
            let mut list: Vec<String> = people.iter().cloned().collect();
            list.sort();
            (room.clone(), list)
        })
        .collect()
}

fn snapshot_live_call(inner: &Inner) -> Option<UiLiveCall> {
    let community = inner.community.as_ref()?;
    let me = inner.identity.public_hex();
    let room = inner
        .voice
        .iter()
        .find(|(_, people)| people.contains(&me))
        .map(|(room, _)| room.clone())?;
    Some(UiLiveCall {
        community_id: community.id.clone(),
        community_name: community.genesis.name.clone(),
        owner_key: community.genesis.owner.clone(),
        room,
        voice: snapshot_voice(inner),
        profiles: snapshot_profiles(inner),
    })
}

fn frame_matches_community(frame: &serde_json::Value, community_id: &str) -> bool {
    match frame
        .get("communityId")
        .and_then(|v| v.as_str())
        .filter(|id| !id.is_empty())
    {
        Some(id) => id == community_id,
        None => true,
    }
}

fn stamp_community(state: &AppState, json: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(json) else {
        return json.to_string();
    };
    let existing = value
        .get("communityId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !existing.is_empty() {
        return json.to_string();
    }
    let id = state
        .inner
        .lock()
        .expect("state")
        .community
        .as_ref()
        .map(|c| c.id.clone());
    let Some(id) = id else {
        return json.to_string();
    };
    value["communityId"] = serde_json::Value::String(id);
    value.to_string()
}

pub(crate) fn public_key(app: &AppHandle) -> String {
    state_of(app).inner.lock().expect("state").identity.public_hex()
}

pub(crate) fn current_relay_gen(app: &AppHandle) -> u64 {
    state_of(app).relay_gen.load(Ordering::SeqCst)
}

pub(crate) fn bump_relay_gen(app: &AppHandle) -> u64 {
    state_of(app).relay_gen.fetch_add(1, Ordering::SeqCst) + 1
}

pub(crate) fn is_own_relay_url(app: &AppHandle, url: &str) -> bool {
    let state = state_of(app);
    let inner = state.inner.lock().expect("state");
    is_self_url(&inner, url)
}

fn uses_local_hub(inner: &Inner) -> bool {
    inner.relays.is_empty() || inner.relays.iter().all(|url| is_self_url(inner, url))
}

fn refresh_owner_hub(inner: &mut Inner) {
    let Some(community) = inner.community.as_ref() else {
        return;
    };
    if community.genesis.owner != inner.identity.public_hex() {
        return;
    }
    if !uses_local_hub(inner) {
        return;
    }
    inner.relays = invite_peer_list(inner);
}

pub(crate) fn relay_is_current(app: &AppHandle, gen: u64) -> bool {
    state_of(app).relay_gen.load(Ordering::SeqCst) == gen
}

pub(crate) fn register_relay(app: &AppHandle, slot: &str, tx: mpsc::UnboundedSender<String>) {
    state_of(app)
        .remotes
        .lock()
        .expect("remotes")
        .insert(format!("relay:{slot}"), tx);
    flush_outbox(&state_of(app));
}

pub(crate) fn unregister_relay(app: &AppHandle, gen: u64, slot: &str) {
    if !relay_is_current(app, gen) {
        return;
    }
    state_of(app)
        .remotes
        .lock()
        .expect("remotes")
        .remove(&format!("relay:{slot}"));
}

pub(crate) fn ingest_from_relay(app: &AppHandle, raw: &str) {
    let state = state_of(app);
    let tx = {
        let remotes = state.remotes.lock().expect("remotes");
        remotes.get("relay").cloned()
    };
    let tx = tx.unwrap_or_else(|| mpsc::unbounded_channel().0);
    let mut from = Some("relay".into());
    handle_remote(app, raw, &mut from, &tx, state.link_gen.load(Ordering::SeqCst));
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
        refresh_owner_hub(&mut inner);
    }
    emit_state(app);
    persist_session(app);
    let state = state_of(app);
    if let Some((hello, peers)) = hello_and_peers_json(&state, true) {
        fanout(&state, &hello, None);
        fanout(&state, &peers, None);
    }
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
            relays: inner.relays.clone(),
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
    let id = if !msg.community_id.is_empty() {
        Some(msg.community_id.clone())
    } else {
        state
            .inner
            .lock()
            .expect("state")
            .community
            .as_ref()
            .map(|c| c.id.clone())
    };
    let Some(id) = id else {
        return;
    };
    let guard = state.store.lock().expect("store");
    if let Some(store) = guard.as_ref() {
        let _ = store.append_message(&id, msg);
    }
}

fn emit_and_store_message(app: &AppHandle, mut msg: UiMessage) {
    if msg.community_id.is_empty() {
        msg.community_id = community_id(app).unwrap_or_default();
    }
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
    let state = state_of(app);
    let id = state
        .inner
        .lock()
        .expect("state")
        .community
        .as_ref()
        .map(|c| c.id.clone());
    let Some(id) = id else {
        let mut inner = state.inner.lock().expect("state");
        inner.archive_messages = 0;
        inner.archive_bytes = 0;
        inner.archive_status = "live".into();
        return;
    };
    let msgs = {
        let guard = state.store.lock().expect("store");
        guard
            .as_ref()
            .map(|store| store.load_messages(&id))
            .unwrap_or_default()
    };
    let bytes = msgs
        .iter()
        .map(|m| (m.text.len() + m.sender.len() + m.channel.len() + 24) as u64)
        .sum();
    let local_indices: HashSet<u8> = {
        let guard = state.store.lock().expect("store");
        let mut set = HashSet::new();
        if let Some(store) = guard.as_ref() {
            if let Some((blob, _, _)) = store.archive_meta(&id) {
                for (i, _) in store.load_shards(&id, &blob) {
                    set.insert(i);
                }
            }
        }
        set
    };
    let (distinct, holders) = {
        let inner = state.inner.lock().expect("state");
        let mut set = local_indices;
        for (_blob, indices) in inner.shard_inventory.values() {
            for i in indices {
                set.insert(*i);
            }
        }
        (set.len(), inner.shard_inventory.len())
    };
    let status = crate::erasure::swarm_archive_status(msgs.len(), distinct, holders);
    let status_s = match status {
        crate::erasure::ArchiveStatus::Live => "live",
        crate::erasure::ArchiveStatus::PendingK => "pendingK",
        crate::erasure::ArchiveStatus::Lost => "lost",
    };
    {
        let mut inner = state.inner.lock().expect("state");
        inner.archive_messages = msgs.len() as u32;
        inner.archive_bytes = bytes;
        inner.archive_status = status_s.into();
    }
    rebuild_erasure_archive(app);
}

fn try_restore_from_shards(app: &AppHandle, community_id: &str, live_key: &str) {
    let state = state_of(app);
    let meta = {
        let guard = state.store.lock().expect("store");
        guard
            .as_ref()
            .and_then(|s| s.archive_meta(community_id))
    };
    let Some((blob_id, _, _)) = meta else {
        return;
    };
    let held = {
        let guard = state.store.lock().expect("store");
        guard
            .as_ref()
            .map(|s| s.load_shards(community_id, &blob_id))
            .unwrap_or_default()
    };
    if held.len() < crate::erasure::DATA_SHARDS {
        return;
    }
    let mut pieces: Vec<Option<Vec<u8>>> = vec![None; crate::erasure::TOTAL_SHARDS];
    for (index, data) in held {
        if (index as usize) < pieces.len() {
            pieces[index as usize] = Some(data);
        }
    }
    let Ok(sealed) = crate::erasure::reconstruct(&mut pieces) else {
        return;
    };
    let Some(plain) = crate::crypto::open_blob(live_key, &sealed) else {
        return;
    };
    let Ok(msgs) = serde_json::from_slice::<Vec<UiMessage>>(&plain) else {
        return;
    };
    {
        let guard = state.store.lock().expect("store");
        if let Some(store) = guard.as_ref() {
            for mut msg in msgs {
                msg.community_id = community_id.to_string();
                let _ = store.append_message(community_id, &msg);
            }
        }
    }
}

fn rebuild_erasure_archive(app: &AppHandle) {
    let state = state_of(app);
    let (id, live_key, me, members) = {
        let inner = state.inner.lock().expect("state");
        let Some(c) = inner.community.as_ref() else {
            return;
        };
        let mut members: Vec<String> = inner.profiles.keys().cloned().collect();
        let me = inner.identity.public_hex();
        if !members.iter().any(|p| p == &me) {
            members.push(me.clone());
        }
        (c.id.clone(), c.live_key.clone(), me, members)
    };
    let msgs = {
        let guard = state.store.lock().expect("store");
        guard
            .as_ref()
            .map(|store| store.load_messages(&id))
            .unwrap_or_default()
    };
    if msgs.is_empty() {
        try_restore_from_shards(app, &id, &live_key);
        return;
    }
    let plain = match serde_json::to_vec(&msgs) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Ok(sealed) = crate::crypto::seal_blob(&live_key, &plain) else {
        return;
    };
    let Ok(plan) = crate::erasure::plan_archive(&sealed, &members) else {
        return;
    };
    let keep = crate::erasure::my_shard_indices(&me, &plan.holders);
    {
        let guard = state.store.lock().expect("store");
        if let Some(store) = guard.as_ref() {
            let _ = store.replace_archive(
                &id,
                &plan.blob_id,
                msgs.len() as u32,
                &plan.holders,
                &keep,
                &plan.shards,
            );
        }
    }
    advertise_shards(app);
}

fn send_to_pk(state: &AppState, pk: &str, json: &str) -> bool {
    let json = stamp_community(state, json);
    let urls: Vec<String> = {
        let links = state.link_pks.lock().expect("link_pks");
        links
            .iter()
            .filter(|(_, p)| p.as_str() == pk)
            .map(|(u, _)| u.clone())
            .collect()
    };
    if urls.is_empty() {
        return false;
    }
    let remotes = state.remotes.lock().expect("remotes");
    let mut ok = false;
    for url in urls {
        if let Some(tx) = remotes.get(&url) {
            if tx.send(json.clone()).is_ok() {
                ok = true;
            }
        }
    }
    ok
}

fn shard_push_frames(app: &AppHandle, to_pk: &str) -> Result<String, String> {
    let state = state_of(app);
    let (id, me) = {
        let inner = state.inner.lock().expect("state");
        let id = inner
            .community
            .as_ref()
            .map(|c| c.id.clone())
            .ok_or_else(|| "not in a community".to_string())?;
        (id, inner.identity.public_hex())
    };
    let (blob_id, _count, holders) = {
        let guard = state.store.lock().expect("store");
        guard
            .as_ref()
            .and_then(|s| s.archive_meta(&id))
            .ok_or_else(|| "no archive shards yet".to_string())?
    };
    let indices = crate::erasure::my_shard_indices(&me, &holders);
    if indices.is_empty() {
        return Ok(blob_id);
    }
    let shards = {
        let guard = state.store.lock().expect("store");
        guard
            .as_ref()
            .map(|s| s.load_shards(&id, &blob_id))
            .unwrap_or_default()
    };
    use base64::Engine;
    for (index, data) in shards {
        if !indices.contains(&(index as usize)) {
            continue;
        }
        let frame = serde_json::json!({
            "type": "shard-push",
            "blobId": blob_id,
            "index": index,
            "data": base64::engine::general_purpose::STANDARD.encode(&data),
            "from": me,
            "to": to_pk,
        })
        .to_string();
        if !send_to_pk(&state, to_pk, &frame) {
            fanout(&state, &frame, None);
        }
    }
    let wait_frame = serde_json::json!({
        "type": "shard-handoff",
        "blobId": blob_id,
        "from": me,
        "to": to_pk,
        "holders": holders,
    })
    .to_string();
    if !send_to_pk(&state, to_pk, &wait_frame) {
        fanout(&state, &wait_frame, None);
    }
    {
        let mut inner = state.inner.lock().expect("state");
        inner.handoff_wait = Some((blob_id.clone(), to_pk.to_string()));
    }
    Ok(blob_id)
}

fn wait_handoff_ack(app: &AppHandle, blob_id: &str, timeout_ms: u64) -> bool {
    let start = std::time::Instant::now();
    while (start.elapsed().as_millis() as u64) < timeout_ms {
        {
            let state = state_of(app);
            let inner = state.inner.lock().expect("state");
            if inner.handoff_wait.is_none() {
                return true;
            }
            if inner
                .handoff_wait
                .as_ref()
                .is_some_and(|(b, _)| b != blob_id)
            {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn online_member_pks(state: &AppState) -> Vec<String> {
    let me = state.inner.lock().expect("state").identity.public_hex();
    let mut out: HashSet<String> = HashSet::new();
    {
        let inner = state.inner.lock().expect("state");
        for (pk, p) in &inner.profiles {
            if pk != &me && p.status != "offline" {
                out.insert(pk.clone());
            }
        }
    }
    {
        let links = state.link_pks.lock().expect("link_pks");
        for pk in links.values() {
            if pk != &me {
                out.insert(pk.clone());
            }
        }
    }
    let mut list: Vec<String> = out.into_iter().collect();
    list.sort();
    list
}

fn local_held_indices(state: &AppState) -> (String, String, Vec<u8>) {
    let id = state
        .inner
        .lock()
        .expect("state")
        .community
        .as_ref()
        .map(|c| c.id.clone())
        .unwrap_or_default();
    if id.is_empty() {
        return (String::new(), String::new(), Vec::new());
    }
    let guard = state.store.lock().expect("store");
    let Some(store) = guard.as_ref() else {
        return (id, String::new(), Vec::new());
    };
    let Some((blob, _, _)) = store.archive_meta(&id) else {
        return (id, String::new(), Vec::new());
    };
    let mut indices: Vec<u8> = store
        .load_shards(&id, &blob)
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    indices.sort_unstable();
    indices.dedup();
    (id, blob, indices)
}

fn shard_have_json(state: &AppState) -> Option<String> {
    let (id, blob, indices) = local_held_indices(state);
    if id.is_empty() || blob.is_empty() || indices.is_empty() {
        return None;
    }
    let me = state.inner.lock().expect("state").identity.public_hex();
    Some(
        serde_json::json!({
            "type": "shard-have",
            "blobId": blob,
            "indices": indices,
            "publicKey": me,
            "ts": now_ms(),
        })
        .to_string(),
    )
}

fn advertise_shards(app: &AppHandle) {
    let state = state_of(app);
    if let Some(json) = shard_have_json(&state) {
        fanout(&state, &json, None);
    }
}

fn push_shard_indices(app: &AppHandle, to_pk: &str, blob_id: &str, indices: &[u8]) {
    if to_pk.is_empty() || indices.is_empty() {
        return;
    }
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
    let me = state.inner.lock().expect("state").identity.public_hex();
    let shards = {
        let guard = state.store.lock().expect("store");
        guard
            .as_ref()
            .map(|s| s.load_shards(&id, blob_id))
            .unwrap_or_default()
    };
    use base64::Engine;
    let want: HashSet<u8> = indices.iter().copied().collect();
    for (index, data) in shards {
        if !want.contains(&index) {
            continue;
        }
        let frame = serde_json::json!({
            "type": "shard-push",
            "blobId": blob_id,
            "index": index,
            "data": base64::engine::general_purpose::STANDARD.encode(&data),
            "from": me,
            "to": to_pk,
        })
        .to_string();
        if !send_to_pk(&state, to_pk, &frame) {
            fanout(&state, &frame, None);
        }
    }
}

fn request_shards(app: &AppHandle, to_pk: &str, blob_id: &str, indices: &[u8]) {
    if indices.is_empty() {
        return;
    }
    let state = state_of(app);
    let me = state.inner.lock().expect("state").identity.public_hex();
    let frame = serde_json::json!({
        "type": "shard-need",
        "blobId": blob_id,
        "indices": indices,
        "publicKey": me,
        "to": to_pk,
        "ts": now_ms(),
    })
    .to_string();
    if !send_to_pk(&state, to_pk, &frame) {
        fanout(&state, &frame, None);
    }
}

fn reconcile_shards(app: &AppHandle) {
    let state = state_of(app);
    let (blob, mine) = {
        let (_id, blob, indices) = local_held_indices(&state);
        (blob, indices)
    };
    if blob.is_empty() {
        return;
    }
    let inventory: Vec<(String, String, Vec<u8>)> = {
        let inner = state.inner.lock().expect("state");
        inner
            .shard_inventory
            .iter()
            .map(|(pk, (b, idx))| (pk.clone(), b.clone(), idx.clone()))
            .collect()
    };
    for (pk, their_blob, their_idx) in inventory {
        if their_blob != blob {
            continue;
        }
        let need = crate::erasure::indices_to_pull(&mine, &their_idx);
        if !need.is_empty() {
            request_shards(app, &pk, &blob, &need);
        }
    }
    repair_under_replicated(app);
}

fn repair_under_replicated(app: &AppHandle) {
    let state = state_of(app);
    let online = online_member_pks(&state);
    let (blob, mine) = {
        let (_id, blob, indices) = local_held_indices(&state);
        (blob, indices)
    };
    if blob.is_empty() {
        return;
    }
    let me = state.inner.lock().expect("state").identity.public_hex();
    let inv_map: HashMap<String, Vec<u8>> = {
        let inner = state.inner.lock().expect("state");
        inner
            .shard_inventory
            .iter()
            .filter(|(_, (b, _))| b == &blob)
            .map(|(pk, (_, idx))| (pk.clone(), idx.clone()))
            .collect()
    };
    let mut copies = vec![0usize; crate::erasure::TOTAL_SHARDS];
    for i in &mine {
        if (*i as usize) < copies.len() {
            copies[*i as usize] += 1;
        }
    }
    for idx in inv_map.values() {
        for i in idx {
            if (*i as usize) < copies.len() {
                copies[*i as usize] += 1;
            }
        }
    }
    let online_n = online.len() + 1; // include self
    let weak = crate::erasure::under_replicated(
        &copies,
        online_n,
        crate::erasure::MIN_ONLINE_REPLICAS,
    );
    for i in weak {
        let i_u8 = i as u8;
        if mine.contains(&i_u8) {
            if let Some(target) =
                crate::erasure::pick_repair_target(&me, &online, &inv_map, i)
            {
                push_shard_indices(app, &target, &blob, &[i_u8]);
            }
            continue;
        }
        // Ask anyone who advertised it.
        for (pk, idx) in &inv_map {
            if idx.contains(&i_u8) {
                request_shards(app, pk, &blob, &[i_u8]);
                break;
            }
        }
    }
}

fn forget_peer_shards(app: &AppHandle, pk: &str) {
    if pk.is_empty() {
        return;
    }
    {
        let state = state_of(app);
        let mut inner = state.inner.lock().expect("state");
        inner.shard_inventory.remove(pk);
    }
    repair_under_replicated(app);
    refresh_archive(app);
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
        Some(serde_json::json!({ "type": "history", "communityId": id, "messages": wire }).to_string()),
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
        if inner.seeded.get(pk).copied().unwrap_or(0) >= count {
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

fn clear_stored_community(app: &AppHandle, community_id: &str) {
    let state = state_of(app);
    let guard = state.store.lock().expect("store");
    if let Some(store) = guard.as_ref() {
        let _ = store.delete_session(community_id);
    }
}

fn apply_session(inner: &mut Inner, session: crate::store::Session) {
    inner.community = Some(session.community);
    inner.viewed_id = None;
    inner.invite_peers = session.invite_peers;
    inner.relays = session.relays;
    inner.known_peer_urls = session.known_peer_urls;
    inner.text_channels = session.text_channels;
    inner.call_rooms = session.call_rooms;
    inner.profiles = session.profiles;
    for profile in inner.profiles.values_mut() {
        profile.status = "offline".into();
    }
    inner.voice.clear();
    inner.voice_left.clear();
    inner.voice_joined.clear();
    inner.last_seen.clear();
    inner.seeded.clear();
    inner.seeding.clear();
    inner.archive_messages = 0;
    inner.archive_bytes = 0;
    inner.archive_status = "live".into();
    inner.handoff_wait = None;
    inner.shard_inventory.clear();
    if inner.text_channels.is_empty() {
        inner.text_channels.push("general".into());
    }
}

fn unload_community(inner: &mut Inner) {
    inner.community = None;
    inner.viewed_id = None;
    inner.invite_peers.clear();
    inner.relays.clear();
    inner.known_peer_urls.clear();
    reset_rooms(inner);
    inner.text_channels.clear();
    inner.profiles.clear();
    inner.last_seen.clear();
    inner.voice_left.clear();
    inner.voice_joined.clear();
    inner.seeded.clear();
    inner.seeding.clear();
    inner.archive_messages = 0;
    inner.archive_bytes = 0;
    inner.archive_status = "live".into();
    inner.handoff_wait = None;
    inner.shard_inventory.clear();
    inner.voice.clear();
}

fn activate_loaded(app: &AppHandle, session: crate::store::Session, dial_peers: bool) {
    let peers = session.invite_peers.clone();
    let known: Vec<String> = session.known_peer_urls.iter().cloned().collect();
    let state = state_of(app);
    {
        let mut inner = state.inner.lock().expect("state");
        apply_session(&mut inner, session);
    }
    bump_links(&state);
    persist_session(app);
    refresh_archive(app);
    emit_state(app);
    if dial_peers {
        let urls = unique_urls(peers.into_iter().chain(known));
        connect_inviter(app.clone(), urls);
    }
    crate::relay::spawn(app.clone());
    nudge_community_sync(app.clone());
}

pub fn get_history(app: &AppHandle) -> Vec<UiMessage> {
    let state = state_of(app);
    let id = {
        let inner = state.inner.lock().expect("state");
        viewed_community_id(&inner)
    };
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
    let session = store.load_active_session();
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
        {
            let mut inner = state.inner.lock().expect("state");
            apply_session(&mut inner, session);
        }
        refresh_archive(app);
        crate::relay::spawn(app.clone());
        let urls: Vec<String> = {
            let inner = state.inner.lock().expect("state");
            unique_urls(
                inner
                    .known_peer_urls
                    .iter()
                    .cloned()
                    .chain(inner.invite_peers.iter().cloned()),
            )
        };
        connect_inviter(app.clone(), urls);
        nudge_community_sync(app.clone());
    }
    spawn_presence_pulse(app.clone());
    spawn_peer_redial(app.clone());
}

fn spawn_presence_pulse(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(8));
        let mut n = 0u32;
        loop {
            tick.tick().await;
            n = n.wrapping_add(1);
            let state = state_of(&app);
            let (in_community, in_voice) = {
                let inner = state.inner.lock().expect("state");
                (inner.community.is_some(), seated_in_call(&inner))
            };
            if !in_community {
                continue;
            }
            // Re-broadcast seats so a lost join does not leave a peer "sharing" with no room.
            if in_voice {
                fanout(&state, &voice_json(&state), None);
            }
            let json = presence_json(&state);
            fanout(&state, &json, None);
            let (leaves, stale): (Vec<(String, String)>, Vec<String>) = {
                let mut inner = state.inner.lock().expect("state");
                let leaves = expire_stale_presence(&mut inner, now_ms());
                let me = inner.identity.public_hex();
                let offline: Vec<String> = inner
                    .profiles
                    .iter()
                    .filter(|(pk, p)| *pk.as_str() != me && p.status == "offline")
                    .map(|(pk, _)| pk.clone())
                    .collect();
                for pk in &offline {
                    inner.shard_inventory.remove(pk);
                }
                (leaves, offline)
            };
            fanout_voice_leaves(&state, &leaves);
            if !stale.is_empty() || !leaves.is_empty() {
                repair_under_replicated(&app);
            }
            if n % 3 == 0 {
                advertise_shards(&app);
                reconcile_shards(&app);
            }
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
    // Hyper-V default switch — not a path friends can use.
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

fn should_dial(url: &str) -> bool {
    let Some((host, _)) = parse_ws(url) else {
        return false;
    };
    if is_loopback_host(&host) {
        return true;
    }
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        return usable_v4(v4);
    }
    if let Ok(v6) = host.parse::<Ipv6Addr>() {
        return usable_v6(v6);
    }
    false
}

fn invite_peer_list(inner: &Inner) -> Vec<String> {
    // Loopback in an invite makes the other person dial themselves.
    // WAN path is BYO mesh VPN only — never put a public IP in the invite.
    // Order: home LAN → mesh VPN (Hamachi/Radmin/Tailscale).
    let own = unique_urls(
        inner
            .listen_urls
            .iter()
            .cloned()
            .chain(std::iter::once(inner.listen_url.clone())),
    )
    .into_iter()
    .filter(|u| {
        should_dial(u)
            && parse_ws(u).is_some_and(|(h, _)| {
                !is_loopback_host(&h) && (is_private_host(&h) || is_mesh_vpn_host(&h))
            })
    })
    .collect::<Vec<_>>();
    let mut peers = prefer_reachability_order(prefer_non_loopback(own));
    peers.truncate(3);
    peers
}

fn is_private_host(host: &str) -> bool {
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        return v4.is_private() || v4.is_link_local();
    }
    if let Ok(v6) = host.parse::<Ipv6Addr>() {
        let s = v6.segments();
        // Unique local fc00::/7
        return s[0] & 0xfe00 == 0xfc00;
    }
    false
}

/// Free mesh VPNs users often run beside Chaincord (BYO VPN).
fn is_mesh_vpn_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    // Hamachi 25.0.0.0/8
    if o[0] == 25 {
        return true;
    }
    // Radmin VPN 26.0.0.0/8
    if o[0] == 26 {
        return true;
    }
    // Tailscale / similar CGNAT 100.64.0.0/10
    is_cgnat_v4(v4)
}

fn is_mesh_vpn_host(host: &str) -> bool {
    host.parse::<Ipv4Addr>()
        .ok()
        .is_some_and(is_mesh_vpn_v4)
}

fn prefer_reachability_order(urls: Vec<String>) -> Vec<String> {
    let mut lan = Vec::new();
    let mut mesh = Vec::new();
    for url in urls {
        let Some((host, _)) = parse_ws(&url) else {
            continue;
        };
        if is_private_host(&host) {
            lan.push(url);
        } else if is_mesh_vpn_host(&host) {
            mesh.push(url);
        }
        // Public / unknown hosts are never dialed from invites.
    }
    lan.extend(mesh);
    lan
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

fn is_relay_key(key: &str) -> bool {
    key == "relay" || key.starts_with("relay:")
}

fn is_direct_remote_key(key: &str) -> bool {
    !is_relay_key(key) && !key.starts_with("pending:")
}

fn live_direct_peer(state: &AppState) -> bool {
    state
        .remotes
        .lock()
        .expect("remotes")
        .keys()
        .any(|k| is_direct_remote_key(k))
}

fn prefer_non_loopback(urls: Vec<String>) -> Vec<String> {
    let lan: Vec<String> = urls
        .iter()
        .filter(|url| parse_ws(url).is_some_and(|(h, _)| !is_loopback_host(&h)))
        .cloned()
        .collect();
    if lan.is_empty() {
        urls
    } else {
        lan
    }
}

fn inviter_dial_urls(inner: &Inner, urls: Vec<String>) -> Vec<String> {
    let usable: Vec<String> = unique_urls(urls)
        .into_iter()
        .filter(|url| {
            (url.starts_with("ws://") || url.starts_with("wss://"))
                && !is_self_url(inner, url)
                && should_dial(url)
        })
        .collect();
    prefer_non_loopback(usable)
}

fn bump_links(state: &AppState) {
    state.link_gen.fetch_add(1, Ordering::SeqCst);
    state.remotes.lock().expect("remotes").clear();
    state.link_pks.lock().expect("link_pks").clear();
    state.outbox.lock().expect("outbox").clear();
    // Drop gossip dedup so a rejoin can accept the same layout/presence catch-up
    // payloads that were seen in a previous session on this process.
    let mut guard = state.seen.lock().expect("seen");
    guard.0.clear();
    guard.1.clear();
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
    Err("no free port".into())
}

fn allow_windows_listen(port: u16) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let end = port.saturating_add(19);
        let _ = std::process::Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                "name=Chaincord",
                "dir=in",
                "action=allow",
                "protocol=TCP",
                &format!("localport={port}-{end}"),
                "profile=any",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = port;
    }
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
    let peers: Vec<String> = {
        let state = state_of(&app);
        let inner = state.inner.lock().expect("state");
        unique_urls(
            inner
                .known_peer_urls
                .iter()
                .cloned()
                .chain(inner.invite_peers.iter().cloned()),
        )
    };
    for url in peers {
        connect_peer(app.clone(), url);
    }

    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| e.to_string())?;
    allow_windows_listen(port);

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let mqtt = std::sync::Arc::new(AtomicBool::new(false));
            let flag = mqtt.clone();
            let Ok(ws) = accept_hdr_async(stream, move |req: &Request, mut response: Response| {
                let wants_mqtt = req
                    .headers()
                    .get(SEC_WEBSOCKET_PROTOCOL)
                    .and_then(|h| h.to_str().ok())
                    .map(|v| {
                        v.split(',')
                            .any(|p| p.trim().eq_ignore_ascii_case("mqtt"))
                    })
                    .unwrap_or(false);
                if wants_mqtt {
                    flag.store(true, Ordering::SeqCst);
                    response.headers_mut().insert(
                        SEC_WEBSOCKET_PROTOCOL,
                        HeaderValue::from_static("mqtt"),
                    );
                }
                Ok(response)
            })
            .await
            else {
                return;
            };
            if mqtt.load(Ordering::SeqCst) {
                crate::relay::serve_hub(app, ws).await;
                return;
            }
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
    let json = stamp_community(state, json);
    let skip_relays = except.is_some_and(is_relay_key);
    let remotes = state.remotes.lock().expect("remotes");
    for (url, tx) in remotes.iter() {
        if Some(url.as_str()) == except {
            continue;
        }
        if skip_relays && is_relay_key(url) {
            continue;
        }
        let _ = tx.send(json.clone());
    }
}

fn remember_outbox(state: &AppState, json: &str) {
    if json.is_empty() || json.len() > 48_000 {
        return;
    }
    let mut q = state.outbox.lock().expect("outbox");
    if q.iter().any(|m| m == json) {
        return;
    }
    q.push_back(json.to_string());
    while q.len() > 80 {
        q.pop_front();
    }
}

pub(crate) fn flush_outbox(state: &AppState) {
    let msgs: Vec<String> = state
        .outbox
        .lock()
        .expect("outbox")
        .iter()
        .cloned()
        .collect();
    if msgs.is_empty() {
        return;
    }
    let remotes = state.remotes.lock().expect("remotes");
    if remotes.is_empty() {
        return;
    }
    for json in msgs {
        let json = stamp_community(state, &json);
        for tx in remotes.values() {
            let _ = tx.send(json.clone());
        }
    }
}

fn nudge_outbox(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        for delay in [400u64, 1200, 3000, 8000] {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let state = state_of(&app);
            if state.inner.lock().expect("state").community.is_none() {
                return;
            }
            flush_outbox(&state);
        }
    });
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
        "sharingScreen": inner.sharing_screen,
        // Unique per handshake so reconnects are not dropped by already_seen.
        "ts": now_ms(),
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
        out.push(
            serde_json::json!({ "type": "presence-state", "ts": now_ms(), "people": people })
                .to_string(),
        );
        out.push(history_request_json(&state));
        let (_, _, hist) = history_payload(&state);
        if let Some(hist) = hist {
            if hist.len() < 40_000 {
                out.push(hist);
            }
        }
        if let Some(have) = shard_have_json(&state) {
            out.push(have);
        }
        return out;
    }
    out.push(presence_json(&state));
    out.push(presence_state_json(&state));
    out.push(history_request_json(&state));
    if let (_, _, Some(hist)) = history_payload(&state) {
        out.push(hist);
    }
    if let Some(have) = shard_have_json(&state) {
        out.push(have);
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
    urls.retain(|u| {
        !u.is_empty() && parse_ws(u).is_some_and(|(h, _)| !is_loopback_host(&h))
    });
    urls
}

fn layout_json(state: &AppState) -> String {
    let inner = state.inner.lock().expect("state");
    serde_json::json!({
        "type": "layout",
        "textChannels": inner.text_channels,
        "callRooms": inner.call_rooms,
        // Unique per advertise so retries are not dropped by already_seen.
        "ts": now_ms(),
    })
    .to_string()
}

fn history_request_json(state: &AppState) -> String {
    let inner = state.inner.lock().expect("state");
    serde_json::json!({
        "type": "history-request",
        "publicKey": inner.identity.public_hex(),
        "historyCount": inner.archive_messages,
        "ts": now_ms(),
    })
    .to_string()
}

fn nudge_community_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        for delay in [400u64, 1200, 3000] {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let state = state_of(&app);
            if state.inner.lock().expect("state").community.is_none() {
                return;
            }
            // Relay/direct links may come up after join/create; re-advertise roster
            // and rooms so late peers catch up without waiting for a new hello.
            fanout(&state, &layout_json(&state), None);
            if voice_busy(&state) {
                fanout(&state, &voice_json(&state), None);
            }
            fanout(&state, &presence_json(&state), None);
            fanout(&state, &presence_state_json(&state), None);
            fanout(&state, &history_request_json(&state), None);
            flush_outbox(&state);
        }
    });
}

fn nudge_layout_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        for delay in [400u64, 1200, 2500] {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let state = state_of(&app);
            if state.inner.lock().expect("state").community.is_none() {
                return;
            }
            fanout(&state, &layout_json(&state), None);
        }
    });
}

fn voice_json(state: &AppState) -> String {
    let inner = state.inner.lock().expect("state");
    let mut rooms = serde_json::Map::new();
    let mut keys: Vec<String> = inner.voice.keys().cloned().collect();
    keys.sort();
    for room in keys {
        let Some(people) = inner.voice.get(&room) else {
            continue;
        };
        let mut list: Vec<String> = people.iter().cloned().collect();
        list.sort();
        rooms.insert(room, serde_json::json!(list));
    }
    serde_json::json!({
        "type": "voice-state",
        "publicKey": inner.identity.public_hex(),
        "rooms": rooms,
    })
    .to_string()
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
        "sharingScreen": inner.sharing_screen,
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
        inner.sharing_screen = false;
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
        "sharingScreen": inner.sharing_screen,
        "status": own_status(inner),
        // Heartbeats are otherwise identical and already_seen would drop them,
        // so last_seen would expire and peers would look offline.
        "ts": now_ms(),
    })
    .to_string()
}

fn presence_state_json(state: &AppState) -> String {
    let inner = state.inner.lock().expect("state");
    serde_json::json!({
        "type": "presence-state",
        "ts": now_ms(),
        "people": live_profiles(&inner, now_ms(), false),
    })
    .to_string()
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
            sharing_screen: inner.sharing_screen,
            status: own_status(inner),
        },
    );
    for (pk, profile) in map.iter_mut() {
        if *pk != me {
            profile.status = live_status(inner, pk, now);
            if profile.sharing_screen && !peer_in_voice(inner, pk) {
                profile.sharing_screen = false;
            }
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
            sharing_screen: inner.sharing_screen,
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
            profile.sharing_screen = false;
        }
    }
    scrub_unseated_sharing(inner);
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
    if let Some(profile) = inner.profiles.get_mut(pk) {
        profile.sharing_screen = false;
    }
    normalize_voice(inner);
    rooms
}

fn peer_in_voice(inner: &Inner, pk: &str) -> bool {
    !pk.is_empty() && inner.voice.values().any(|people| people.contains(pk))
}

/// Screen-share flags only make sense while seated; clear ghosts after leave/desync.
fn scrub_unseated_sharing(inner: &mut Inner) {
    let me = inner.identity.public_hex();
    let seated: HashSet<String> = inner
        .voice
        .values()
        .flat_map(|people| people.iter().cloned())
        .collect();
    for (pk, profile) in inner.profiles.iter_mut() {
        if pk != &me && profile.sharing_screen && !seated.contains(pk) {
            profile.sharing_screen = false;
        }
    }
    if !seated.contains(&me) {
        inner.sharing_screen = false;
    }
}

fn mark_peer_offline(inner: &mut Inner, pk: &str) -> Vec<String> {
    inner.last_seen.remove(pk);
    if let Some(profile) = inner.profiles.get_mut(pk) {
        profile.status = "offline".into();
        profile.sharing_screen = false;
    }
    // Voice leave is a separate frame. MQTT wills / presence blips must not
    // yank someone out of the call or the hub flaps 3↔2.
    Vec::new()
}

#[allow(dead_code)]
fn drop_disconnected_peer(inner: &mut Inner, pk: &str) -> Vec<(String, String)> {
    if pk.is_empty() || pk == inner.identity.public_hex() {
        return Vec::new();
    }
    mark_peer_offline(inner, pk);
    drop_from_voice(inner, pk)
        .into_iter()
        .map(|room| (pk.to_string(), room))
        .collect()
}

fn remember_link_pk(state: &AppState, url: Option<&str>, pk: &str) {
    let Some(url) = url else {
        return;
    };
    if url.is_empty() || is_relay_key(url) || pk.is_empty() {
        return;
    }
    state
        .link_pks
        .lock()
        .expect("link_pks")
        .insert(url.to_string(), pk.to_string());
}

fn on_peer_socket_closed(app: &AppHandle, url: Option<String>, gen: u64) {
    let state = state_of(app);
    if state.link_gen.load(Ordering::SeqCst) != gen {
        return;
    }
    let Some(url) = url else {
        return;
    };
    state.remotes.lock().expect("remotes").remove(&url);
    if is_relay_key(&url) {
        emit_state(app);
        return;
    }
    state.link_pks.lock().expect("link_pks").remove(&url);
    emit_state(app);
    // Direct sockets die behind NAT all the time. Presence lives on MQTT
    // heartbeats / TTL — do not broadcast "offline" or the other person
    // vanishes while they are still sending chat.
    connect_peer(app.clone(), url);
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

fn ignore_relay_offline(inner: &Inner, pk: &str, from_relay: bool, status: Option<&str>) -> bool {
    if !from_relay || status != Some("offline") || pk.is_empty() {
        return false;
    }
    let Some(last) = inner.last_seen.get(pk) else {
        return false;
    };
    now_ms().saturating_sub(*last) < 45_000
}

fn apply_presence(inner: &mut Inner, frame: &serde_json::Value, from_snapshot: bool) -> Vec<String> {
    let pk = json_str(frame.get("publicKey"));
    if pk.is_empty() || pk == inner.identity.public_hex() {
        return Vec::new();
    }
    let status_raw = frame.get("status").and_then(|v| v.as_str());
    let seated = peer_in_voice(inner, &pk);
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
        if let Some(sharing) = frame.get("sharingScreen").and_then(|v| v.as_bool()) {
            // Ignore "sharing" from someone who is not in any voice room on our view.
            entry.sharing_screen = sharing && seated;
        } else if !seated {
            entry.sharing_screen = false;
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
        "room".into()
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

fn voice_left_held(inner: &Inner, pk: &str, now: i64) -> bool {
    inner
        .voice_left
        .get(pk)
        .is_some_and(|ts| now.saturating_sub(*ts) < VOICE_LEAVE_HOLD_MS)
}

fn recently_voice_joined(inner: &Inner, pk: &str, now: i64) -> bool {
    inner
        .voice_joined
        .get(pk)
        .is_some_and(|ts| now.saturating_sub(*ts) < VOICE_JOIN_GRACE_MS)
}

fn mark_voice_left(inner: &mut Inner, pk: &str) {
    if pk.is_empty() || pk == inner.identity.public_hex() {
        return;
    }
    inner.voice_left.insert(pk.to_string(), now_ms());
    inner.voice_joined.remove(pk);
    drop_from_voice(inner, pk);
}

fn note_voice_join(inner: &mut Inner, pk: &str) {
    inner.voice_left.remove(pk);
    inner.voice_joined.insert(pk.to_string(), now_ms());
}

fn merge_voice_rooms(
    inner: &mut Inner,
    rooms: &serde_json::Map<String, serde_json::Value>,
    from_pk: &str,
) {
    let now = now_ms();
    let me = inner.identity.public_hex();
    let mut listed: Vec<(String, HashSet<String>)> = Vec::new();
    for (room, people) in rooms {
        let room = slug_name(room);
        if room.is_empty() {
            continue;
        }
        let incoming: Vec<String> = people
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .filter(|pk| !pk.is_empty() && pk != &me)
                    .collect()
            })
            .unwrap_or_default();
        if !incoming.is_empty() {
            listed.push((room.clone(), incoming.iter().cloned().collect()));
        }
        for pk in incoming {
            // Sender asserting their own seat always wins over a stale leave-hold.
            if !from_pk.is_empty() && pk == from_pk {
                for seated in inner.voice.values_mut() {
                    seated.remove(&pk);
                }
                inner.voice.entry(room.clone()).or_default().insert(pk.clone());
                note_voice_join(inner, &pk);
                continue;
            }
            if voice_left_held(inner, &pk, now) {
                continue;
            }
            let already = inner.voice.iter().any(|(_, seated)| seated.contains(&pk));
            if already {
                continue;
            }
            inner.voice.entry(room.clone()).or_default().insert(pk.clone());
            note_voice_join(inner, &pk);
        }
    }
    if !from_pk.is_empty() && from_pk != me {
        let drop: Vec<String> = listed
            .iter()
            .filter(|(_, set)| set.contains(from_pk))
            .flat_map(|(room, set)| {
                inner
                    .voice
                    .get(room)
                    .into_iter()
                    .flatten()
                    .filter(|pk| {
                        pk.as_str() != me
                            && pk.as_str() != from_pk
                            && !set.contains(*pk)
                            && !recently_voice_joined(inner, pk, now)
                    })
                    .cloned()
            })
            .collect();
        for pk in drop {
            mark_voice_left(inner, &pk);
        }
    }
    normalize_voice(inner);
    scrub_unseated_sharing(inner);
}

fn apply_voice_action(inner: &mut Inner, room: &str, action: &str, pk: &str) {
    let room = slug_name(room);
    if room.is_empty() || pk.is_empty() {
        return;
    }
    // Only join_call / leave_call mutate our own seat.
    if pk == inner.identity.public_hex() {
        return;
    }
    if action == "join" {
        note_voice_join(inner, pk);
        for people in inner.voice.values_mut() {
            people.remove(pk);
        }
        inner.voice.entry(room).or_default().insert(pk.to_string());
    } else if action == "leave" {
        mark_voice_left(inner, pk);
    }
    normalize_voice(inner);
}

fn reset_rooms(inner: &mut Inner) {
    inner.text_channels = vec!["general".into()];
    inner.call_rooms.clear();
    inner.voice.clear();
    inner.voice_left.clear();
    inner.voice_joined.clear();
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
        for attempt in 0..12u32 {
            if live_direct_peer(&state) {
                return;
            }
            let filtered = {
                let inner = state.inner.lock().expect("state");
                if inner.community.is_none() {
                    return;
                }
                inviter_dial_urls(&inner, urls.clone())
            };
            for url in filtered {
                if live_direct_peer(&state) {
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
            tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt + 1))).await;
        }
    });
}

fn spawn_peer_redial(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(8));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let state = state_of(&app);
            if live_direct_peer(&state) {
                continue;
            }
            let urls = {
                let inner = state.inner.lock().expect("state");
                if inner.community.is_none() {
                    continue;
                }
                unique_urls(
                    inner
                        .known_peer_urls
                        .iter()
                        .cloned()
                        .chain(inner.invite_peers.iter().cloned()),
                )
            };
            for url in urls {
                connect_peer(app.clone(), url);
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
    match tokio::time::timeout(Duration::from_secs(8), connect_async(url)).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err("timed out".into()),
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
    let gen = state_of(&app).link_gen.load(Ordering::SeqCst);
    let (mut sink, mut stream) = ws.split();
    for msg in handshake_messages(&app, false) {
        let _ = tx.send(stamp_community(&state_of(&app), &msg));
    }
    flush_outbox(&state_of(&app));

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
                handle_remote(&app, text.as_str(), &mut known_url, &tx, gen);
            }
        }
    }

    on_peer_socket_closed(&app, known_url, gen);
}

fn handle_remote(
    app: &AppHandle,
    raw: &str,
    from_url: &mut Option<String>,
    tx: &mpsc::UnboundedSender<String>,
    gen: u64,
) {
    if state_of(app).link_gen.load(Ordering::SeqCst) != gen {
        return;
    }
    let Ok(frame) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    let kind = frame.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let duplicate = already_seen(app, raw);
    // Identical RTC retries must still reach the UI: the first copy often
    // arrives before CallNet exists. Do not fanout duplicates or MQTT loops.
    if duplicate && kind != "rtc" {
        return;
    }
    let state = state_of(app);
    let live_id = state
        .inner
        .lock()
        .expect("state")
        .community
        .as_ref()
        .map(|c| c.id.clone());
    let Some(live_id) = live_id else {
        return;
    };
    if !frame_matches_community(&frame, &live_id) {
        return;
    }
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
            let extra_urls: Vec<String> = {
                let inner = state.inner.lock().expect("state");
                unique_urls(
                    std::iter::once(listen.clone()).chain(
                        frame
                            .get("listenUrls")
                            .and_then(|v| v.as_array())
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v.as_str().map(str::to_string)),
                    ),
                )
                .into_iter()
                .filter(|url| should_dial(url) && !is_self_url(&inner, url))
                .collect()
            };
            if !is_relay_key(from_url.as_deref().unwrap_or("")) && !listen.is_empty() {
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
            advertise_shards(app);
            reconcile_shards(app);
            let our_count = state
                .inner
                .lock()
                .expect("state")
                .archive_messages;
            if their_history > our_count {
                let _ = tx.send(history_request_json(&state));
            }
            for url in extra_urls {
                connect_peer(app.clone(), url);
            }
        }
        "history-request" => {
            let pk = json_str(frame.get("publicKey"));
            let their_count = frame
                .get("historyCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            maybe_seed_peer(app, &pk, their_count);
        }
        "peers" => {
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
                    community_id: live_id.clone(),
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
                merge_voice_rooms(&mut inner, rooms, &json_str(frame.get("publicKey")));
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
            if action == "join" || action == "leave" {
                fanout(&state, &voice_json(&state), None);
            }
        }
        "presence" => {
            let from_relay = is_relay_key(from_url.as_deref().unwrap_or(""));
            let pk = json_str(frame.get("publicKey"));
            let status = frame.get("status").and_then(|v| v.as_str());
            {
                let inner = state.inner.lock().expect("state");
                if ignore_relay_offline(&inner, &pk, from_relay, status) {
                    return;
                }
            }
            let leaves = {
                let mut inner = state.inner.lock().expect("state");
                apply_presence(&mut inner, &frame, false)
                    .into_iter()
                    .map(|room| (json_str(frame.get("publicKey")), room))
                    .collect::<Vec<_>>()
            };
            if status == Some("offline") && !pk.is_empty() {
                forget_peer_shards(app, &pk);
            }
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
                        "sharingScreen": value.get("sharingScreen"),
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
            fanout(&state, raw, from_url.as_deref());
        }
        "rtc" => {
            let _ = app.emit("ui-rtc", frame);
            if !duplicate {
                fanout(&state, raw, from_url.as_deref());
            }
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
                        community_id: live_id.clone(),
                    },
                );
            }
            refresh_archive(app);
            emit_state(app);
        }
        "shard-push" => {
            let me = state.inner.lock().expect("state").identity.public_hex();
            let to = json_str(frame.get("to"));
            if !to.is_empty() && to != me {
                fanout(&state, raw, from_url.as_deref());
                return;
            }
            let blob_id = json_str(frame.get("blobId"));
            let from = json_str(frame.get("from"));
            let index = frame.get("index").and_then(|v| v.as_u64()).unwrap_or(99) as u8;
            let data_b64 = json_str(frame.get("data"));
            use base64::Engine;
            let Ok(data) = base64::engine::general_purpose::STANDARD.decode(data_b64.as_bytes())
            else {
                return;
            };
            if blob_id.is_empty() || index as usize >= crate::erasure::TOTAL_SHARDS {
                return;
            }
            {
                let guard = state.store.lock().expect("store");
                if let Some(store) = guard.as_ref() {
                    let _ = store.put_shard(&live_id, &blob_id, index, &data);
                }
            }
            {
                let mut inner = state.inner.lock().expect("state");
                if let Some(entry) = inner.shard_inventory.get_mut(&from) {
                    if entry.0 == blob_id && !entry.1.contains(&index) {
                        entry.1.push(index);
                        entry.1.sort_unstable();
                    }
                } else if !from.is_empty() {
                    inner
                        .shard_inventory
                        .insert(from.clone(), (blob_id.clone(), vec![index]));
                }
            }
            let ack = serde_json::json!({
                "type": "shard-ack",
                "blobId": blob_id,
                "index": index,
                "from": me,
                "to": from,
            })
            .to_string();
            if !from.is_empty() {
                let _ = send_to_pk(&state, &from, &ack);
            } else {
                fanout(&state, &ack, None);
            }
            refresh_archive(app);
            emit_state(app);
            advertise_shards(app);
        }
        "shard-handoff" => {
            let me = state.inner.lock().expect("state").identity.public_hex();
            let to = json_str(frame.get("to"));
            if !to.is_empty() && to != me {
                fanout(&state, raw, from_url.as_deref());
                return;
            }
            let blob_id = json_str(frame.get("blobId"));
            let from = json_str(frame.get("from"));
            let holders: Vec<String> = frame
                .get("holders")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            if !blob_id.is_empty() && !holders.is_empty() {
                let guard = state.store.lock().expect("store");
                if let Some(store) = guard.as_ref() {
                    let _ = store.set_archive_holders(&live_id, &blob_id, &holders);
                }
            }
            // Ack the whole handoff so leaver can proceed.
            let ack = serde_json::json!({
                "type": "shard-ack",
                "blobId": blob_id,
                "index": 255,
                "from": me,
                "to": from,
                "handoff": true,
            })
            .to_string();
            if !from.is_empty() {
                let _ = send_to_pk(&state, &from, &ack);
            } else {
                fanout(&state, &ack, None);
            }
        }
        "shard-ack" => {
            let me = state.inner.lock().expect("state").identity.public_hex();
            let to = json_str(frame.get("to"));
            if !to.is_empty() && to != me {
                fanout(&state, raw, from_url.as_deref());
                return;
            }
            let blob_id = json_str(frame.get("blobId"));
            let from = json_str(frame.get("from"));
            let mut inner = state.inner.lock().expect("state");
            if let Some((wait_blob, wait_pk)) = inner.handoff_wait.clone() {
                if wait_blob == blob_id && (from.is_empty() || from == wait_pk) {
                    inner.handoff_wait = None;
                }
            }
        }
        "shard-have" => {
            let pk = json_str(frame.get("publicKey"));
            let blob_id = json_str(frame.get("blobId"));
            let me = state.inner.lock().expect("state").identity.public_hex();
            if pk.is_empty() || pk == me || blob_id.is_empty() {
                return;
            }
            let indices: Vec<u8> = frame
                .get("indices")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            {
                let mut inner = state.inner.lock().expect("state");
                inner
                    .shard_inventory
                    .insert(pk.clone(), (blob_id.clone(), indices.clone()));
            }
            remember_link_pk(&state, from_url.as_deref(), &pk);
            let (_id, my_blob, mine) = local_held_indices(&state);
            if my_blob == blob_id || my_blob.is_empty() {
                let need = crate::erasure::indices_to_pull(&mine, &indices);
                if !need.is_empty() {
                    request_shards(app, &pk, &blob_id, &need);
                }
            }
            advertise_shards(app);
            repair_under_replicated(app);
            refresh_archive(app);
            emit_state(app);
            fanout(&state, raw, from_url.as_deref());
        }
        "shard-need" => {
            let me = state.inner.lock().expect("state").identity.public_hex();
            let to = json_str(frame.get("to"));
            let from = json_str(frame.get("publicKey"));
            if !to.is_empty() && to != me {
                fanout(&state, raw, from_url.as_deref());
                return;
            }
            let blob_id = json_str(frame.get("blobId"));
            let indices: Vec<u8> = frame
                .get("indices")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            if from.is_empty() || blob_id.is_empty() || indices.is_empty() {
                return;
            }
            push_shard_indices(app, &from, &blob_id, &indices);
        }
        _ => {}
    }
}

pub fn send_signal(app: &AppHandle, frame: serde_json::Value) -> Result<(), String> {
    if frame.get("type").and_then(|v| v.as_str()) != Some("rtc") {
        return Err("invalid signal".into());
    }
    let state = state_of(app);
    let json = frame.to_string();
    let _ = already_seen(app, &json);
    fanout(&state, &json, None);
    Ok(())
}

pub fn create_local(app: &AppHandle, name: &str, relay: Option<&str>) -> Result<(), String> {
    let custom = crate::relay::choose_for_create(relay)?;
    let _ = clear_own_voice(app);
    // Keep other communities: only park the current one on disk.
    persist_session(app);
    let state = state_of(app);
    state.relay_gen.fetch_add(1, Ordering::SeqCst);
    {
        let mut inner = state.inner.lock().expect("state");
        inner.viewed_id = None;
        inner.community = Some(create_community(&inner.identity, name));
        inner.invite_peers = inner.listen_urls.clone();
        inner.relays = if custom.is_empty() {
            invite_peer_list(&inner)
        } else {
            custom
        };
        inner.known_peer_urls.clear();
        inner.profiles.clear();
        inner.last_seen.clear();
        inner.seeded.clear();
        inner.seeding.clear();
        inner.archive_messages = 0;
        inner.archive_bytes = 0;
        inner.voice.clear();
        reset_rooms(&mut inner);
    }
    bump_links(&state);
    persist_session(app);
    refresh_archive(app);
    emit_state(app);
    crate::relay::spawn(app.clone());
    nudge_community_sync(app.clone());
    Ok(())
}

pub fn join_community(app: &AppHandle, raw_invite: &str) -> Result<(), String> {
    let invite = decode_invite(raw_invite)?;
    let community_id = invite.community.id.clone();
    let peers = invite.peers.clone();
    let relays = invite.relays.clone();
    let state = state_of(app);

    // Already a member: just switch (and refresh invite peers).
    let existing = {
        let guard = state.store.lock().expect("store");
        guard
            .as_ref()
            .and_then(|store| store.load_session(&community_id))
    };
    if let Some(mut session) = existing {
        for url in &peers {
            if !url.is_empty() {
                session.known_peer_urls.insert(url.clone());
            }
        }
        if !peers.is_empty() {
            session.invite_peers = peers.clone();
        }
        if !relays.is_empty() {
            session.relays = relays;
        }
        let _ = clear_own_voice(app);
        persist_session(app);
        state.relay_gen.fetch_add(1, Ordering::SeqCst);
        activate_loaded(app, session, true);
        return Ok(());
    }

    let _ = clear_own_voice(app);
    persist_session(app);
    state.relay_gen.fetch_add(1, Ordering::SeqCst);
    {
        let mut inner = state.inner.lock().expect("state");
        inner.viewed_id = None;
        inner.community = Some(invite.community);
        inner.invite_peers = peers.clone();
        inner.relays = relays;
        inner.known_peer_urls.clear();
        for url in &peers {
            if should_dial(url) && !is_self_url(&inner, url) {
                inner.known_peer_urls.insert(url.clone());
            }
        }
        inner.profiles.clear();
        inner.last_seen.clear();
        inner.seeded.clear();
        inner.seeding.clear();
        inner.archive_messages = 0;
        inner.archive_bytes = 0;
        inner.voice.clear();
        reset_rooms(&mut inner);
    }
    bump_links(&state);
    persist_session(app);
    refresh_archive(app);
    emit_state(app);
    connect_inviter(app.clone(), peers);
    crate::relay::spawn(app.clone());
    nudge_community_sync(app.clone());
    Ok(())
}

pub fn switch_community(app: &AppHandle, community_id: &str) -> Result<(), String> {
    let state = state_of(app);
    let (live_id, in_call) = {
        let inner = state.inner.lock().expect("state");
        (
            inner.community.as_ref().map(|c| c.id.clone()),
            seated_in_call(&inner),
        )
    };
    if live_id.as_deref() == Some(community_id) {
        {
            let mut inner = state.inner.lock().expect("state");
            inner.viewed_id = None;
        }
        emit_state(app);
        return Ok(());
    }
    let session = {
        let guard = state.store.lock().expect("store");
        guard
            .as_ref()
            .and_then(|store| store.load_session(community_id))
            .ok_or_else(|| "community not found".to_string())?
    };
    if in_call {
        persist_session(app);
        {
            let mut inner = state.inner.lock().expect("state");
            inner.viewed_id = Some(session.community.id);
        }
        emit_state(app);
        return Ok(());
    }
    let _ = clear_own_voice(app);
    persist_session(app);
    state.relay_gen.fetch_add(1, Ordering::SeqCst);
    activate_loaded(app, session, true);
    Ok(())
}

pub fn send_chat(app: &AppHandle, text: &str, channel: &str) -> Result<(), String> {
    let state = state_of(app);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let channel = slug_name(channel);
    let viewed = {
        let inner = state.inner.lock().expect("state");
        viewed_community_id(&inner)
    };
    let live = community_id(app);
    if viewed.as_ref() != live.as_ref() {
        let Some(viewed_id) = viewed else {
            return Err("Join or create a community first.".into());
        };
        let session = {
            let guard = state.store.lock().expect("store");
            guard
                .as_ref()
                .and_then(|store| store.load_session(&viewed_id))
                .ok_or_else(|| "community not found".to_string())?
        };
        let channel = if session.text_channels.iter().any(|c| c == &channel) {
            channel
        } else {
            "general".into()
        };
        let inner = state.inner.lock().expect("state");
        let plain = ChatPlain {
            channel: channel.clone(),
            text: trimmed.into(),
            ts: now_ms(),
        };
        let mut wire = seal_chat(&inner.identity, &session.community.live_key, &plain)?;
        wire.community_id = session.community.id.clone();
        let sender = inner.identity.public_hex();
        drop(inner);
        let json = serde_json::to_string(&wire).map_err(|e| e.to_string())?;
        remember_outbox(&state, &json);
        fanout(&state, &json, None);
        emit_and_store_message(
            app,
            UiMessage {
                sender,
                text: trimmed.into(),
                ts: plain.ts,
                channel,
                is_self: true,
                community_id: session.community.id,
            },
        );
        nudge_outbox(app.clone());
        return Ok(());
    }
    let (wire, sender, ts, channel) = {
        let inner = state.inner.lock().expect("state");
        let Some(community) = inner.community.as_ref() else {
            return Err("Join or create a community first.".into());
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
        let mut wire = seal_chat(&inner.identity, &community.live_key, &plain)?;
        wire.community_id = community.id.clone();
        (wire, inner.identity.public_hex(), plain.ts, channel)
    };
    let json = serde_json::to_string(&wire).map_err(|e| e.to_string())?;
    remember_outbox(&state, &json);
    fanout(&state, &json, None);
    emit_and_store_message(
        app,
        UiMessage {
            sender,
            text: trimmed.into(),
            ts,
            channel,
            is_self: true,
            community_id: live.unwrap_or_default(),
        },
    );
    nudge_outbox(app.clone());
    Ok(())
}

pub fn add_room(app: &AppHandle, kind: &str, name: &str) -> Result<(), String> {
    let slug = slug_name(name);
    let state = state_of(app);
    let (live, viewed) = {
        let inner = state.inner.lock().expect("state");
        (
            inner.community.as_ref().map(|c| c.id.clone()),
            viewed_community_id(&inner),
        )
    };
    if viewed.as_ref() != live.as_ref() {
        let Some(viewed_id) = viewed else {
            return Err("Join or create a community first.".into());
        };
        let mut session = {
            let guard = state.store.lock().expect("store");
            guard
                .as_ref()
                .and_then(|store| store.load_session(&viewed_id))
                .ok_or_else(|| "community not found".to_string())?
        };
        if kind == "call" {
            if !session.call_rooms.iter().any(|c| c == &slug) {
                session.call_rooms.push(slug);
            }
        } else if slug != "general" && !session.text_channels.iter().any(|c| c == &slug) {
            session.text_channels.push(slug);
        }
        {
            let guard = state.store.lock().expect("store");
            if let Some(store) = guard.as_ref() {
                store.write_session(&session)?;
            }
        }
        emit_state(app);
        return Ok(());
    }
    {
        let mut inner = state.inner.lock().expect("state");
        if inner.community.is_none() {
            return Err("Join or create a community first.".into());
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
    // Remotes (especially MQTT relay) may still be reconnecting; retry so peers
    // that missed the first fanout still learn the new room.
    nudge_layout_sync(app.clone());
    Ok(())
}

pub fn join_call(app: &AppHandle, room: &str) -> Result<(), String> {
    let room = slug_name(room);
    let state = state_of(app);
    let viewed = {
        let inner = state.inner.lock().expect("state");
        viewed_community_id(&inner)
    };
    let live = community_id(app);
    if viewed.as_ref() != live.as_ref() {
        let Some(viewed_id) = viewed else {
            return Err("Join or create a community first.".into());
        };
        let _ = clear_own_voice(app);
        persist_session(app);
        let session = {
            let guard = state.store.lock().expect("store");
            guard
                .as_ref()
                .and_then(|store| store.load_session(&viewed_id))
                .ok_or_else(|| "community not found".to_string())?
        };
        state.relay_gen.fetch_add(1, Ordering::SeqCst);
        activate_loaded(app, session, true);
    }
    let (pk, room_added) = {
        let mut inner = state.inner.lock().expect("state");
        if inner.community.is_none() {
            return Err("Join or create a community first.".into());
        }
        let room_added = !inner.call_rooms.iter().any(|c| c == &room);
        if room_added {
            inner.call_rooms.push(room.clone());
        }
        let pk = inner.identity.public_hex();
        for people in inner.voice.values_mut() {
            people.remove(&pk);
        }
        inner.voice.entry(room.clone()).or_default().insert(pk.clone());
        normalize_voice(&mut inner);
        (pk, room_added)
    };
    if room_added {
        fanout(&state, &layout_json(&state), None);
        nudge_layout_sync(app.clone());
    }
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
        let still_here = {
            let inner = state.inner.lock().expect("state");
            inner
                .voice
                .get(&room2)
                .is_some_and(|people| people.contains(&pk2))
        };
        if !still_here {
            return;
        }
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
    sharing_screen: Option<bool>,
) -> Result<(), String> {
    let state = state_of(app);
    {
        let mut inner = state.inner.lock().expect("state");
        inner.muted = muted;
        inner.deafened = deafened;
        if let Some(status) = status {
            inner.status = normalize_status(status);
        }
        if let Some(sharing) = sharing_screen {
            inner.sharing_screen = sharing;
        }
    }
    let json = presence_json(&state);
    fanout(&state, &json, None);
    emit_state(app);
    Ok(())
}

fn clear_own_voice(app: &AppHandle) -> Result<(), String> {
    let state = state_of(app);
    let (pk, rooms) = {
        let mut inner = state.inner.lock().expect("state");
        let pk = inner.identity.public_hex();
        inner.sharing_screen = false;
        let rooms: Vec<String> = inner
            .voice
            .iter()
            .filter(|(_, people)| people.contains(&pk))
            .map(|(room, _)| room.clone())
            .collect();
        for people in inner.voice.values_mut() {
            people.remove(&pk);
        }
        normalize_voice(&mut inner);
        (pk, rooms)
    };
    for room in &rooms {
        fanout(&state, &voice_leave_json(room, &pk), None);
    }
    fanout(&state, &voice_json(&state), None);
    fanout(&state, &presence_json(&state), None);
    emit_state(app);
    if rooms.is_empty() {
        return Ok(());
    }
    let app2 = app.clone();
    let rooms2 = rooms.clone();
    let pk2 = pk.clone();
    tauri::async_runtime::spawn(async move {
        for delay in [400u64, 1200, 2500] {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let state = state_of(&app2);
            let still_gone = {
                let inner = state.inner.lock().expect("state");
                !inner.voice.values().any(|people| people.contains(&pk2))
            };
            if !still_gone {
                return;
            }
            for room in &rooms2 {
                fanout(&state, &voice_leave_json(room, &pk2), None);
            }
            fanout(&state, &voice_json(&state), None);
        }
    });
    Ok(())
}

pub fn leave_call(app: &AppHandle) -> Result<(), String> {
    clear_own_voice(app)?;
    let state = state_of(app);
    let viewed = {
        let inner = state.inner.lock().expect("state");
        viewed_community_id(&inner)
    };
    let live = community_id(app);
    if viewed.as_ref() != live.as_ref() {
        if let Some(viewed_id) = viewed {
            persist_session(app);
            let session = {
                let guard = state.store.lock().expect("store");
                guard.as_ref().and_then(|store| store.load_session(&viewed_id))
            };
            if let Some(session) = session {
                state.relay_gen.fetch_add(1, Ordering::SeqCst);
                activate_loaded(app, session, true);
            }
        }
    }
    Ok(())
}

pub fn leave_community(app: &AppHandle, force: bool) -> Result<(), String> {
    let state = state_of(app);
    let (live_id, viewed_id, in_call, me, owner) = {
        let inner = state.inner.lock().expect("state");
        (
            inner.community.as_ref().map(|c| c.id.clone()),
            viewed_community_id(&inner),
            seated_in_call(&inner),
            inner.identity.public_hex(),
            inner
                .community
                .as_ref()
                .map(|c| c.genesis.owner.clone())
                .unwrap_or_default(),
        )
    };
    let Some(leaving_id) = viewed_id else {
        return Ok(());
    };
    if in_call && live_id.as_deref() != Some(leaving_id.as_str()) {
        clear_stored_community(app, &leaving_id);
        {
            let mut inner = state.inner.lock().expect("state");
            inner.viewed_id = None;
        }
        emit_state(app);
        return Ok(());
    }

    let online = online_member_pks(&state);
    if !force && !online.is_empty() {
        let need = {
            let guard = state.store.lock().expect("store");
            guard
                .as_ref()
                .and_then(|s| s.archive_meta(&leaving_id))
                .map(|(_, _, holders)| crate::erasure::handoff_indices(&me, &holders, &online))
                .unwrap_or_default()
        };
        if !need.is_empty() {
            let Some(target) = crate::erasure::pick_handoff_target(&me, &owner, &online) else {
                return Err(
                    "No online peer to take your history shards. Leave anyway to risk the archive."
                        .into(),
                );
            };
            let blob_id = shard_push_frames(app, &target)?;
            if !wait_handoff_ack(app, &blob_id, 4_000) {
                // Target may still have the bytes; continue leave but warn via status.
                let mut inner = state.inner.lock().expect("state");
                inner.handoff_wait = None;
            }
        }
    }

    let leaves = {
        let mut inner = state.inner.lock().expect("state");
        inner.status = "offline".into();
        inner.sharing_screen = false;
        let pk = inner.identity.public_hex();
        drop_from_voice(&mut inner, &pk)
            .into_iter()
            .map(|room| (pk.clone(), room))
            .collect::<Vec<_>>()
    };
    fanout(&state, &presence_json(&state), None);
    fanout_voice_leaves(&state, &leaves);
    state.relay_gen.fetch_add(1, Ordering::SeqCst);
    clear_stored_community(app, &leaving_id);
    let next = {
        let guard = state.store.lock().expect("store");
        guard.as_ref().and_then(|store| {
            let mut list = store.list_communities();
            list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            list.into_iter()
                .next()
                .and_then(|c| store.load_session(&c.id))
        })
    };
    if let Some(session) = next {
        activate_loaded(app, session, true);
        let mut inner = state.inner.lock().expect("state");
        inner.status = "online".into();
        return Ok(());
    }
    {
        let mut inner = state.inner.lock().expect("state");
        unload_community(&mut inner);
        inner.status = "online".into();
    }
    {
        let guard = state.store.lock().expect("store");
        if let Some(store) = guard.as_ref() {
            let _ = store.clear_active();
        }
    }
    bump_links(&state);
    emit_state(app);
    Ok(())
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
        return Err("Name must be 2–32 characters.".into());
    }
    if avatar.len() > 400_000 {
        return Err("Image is too large. Choose another.".into());
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
            viewed_id: None,
            invite_peers: Vec::new(),
            relays: Vec::new(),
            listen_url: String::new(),
            listen_urls: Vec::new(),
            listen_port: 0,
            known_peer_urls: HashSet::new(),
            text_channels: vec!["general".into()],
            call_rooms: Vec::new(),
            voice: HashMap::new(),
            voice_left: HashMap::new(),
            voice_joined: HashMap::new(),
            profiles: HashMap::new(),
            muted: false,
            deafened: false,
            sharing_screen: false,
            status: "online".into(),
            last_seen: HashMap::new(),
            archive_messages: 0,
            archive_bytes: 0,
            seeded: HashMap::new(),
            seeding: HashSet::new(),
            archive_status: "live".into(),
            handoff_wait: None,
            shard_inventory: HashMap::new(),
        }
    }

    #[test]
    fn slug_normalizes_room_names() {
        assert_eq!(slug_name("Lobby"), "lobby");
        assert_eq!(slug_name("  Sala Principal  "), "sala-principal");
        assert_eq!(slug_name("***"), "room");
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
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap(), "");
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
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap(), "");
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
    fn voice_leave_is_not_undone_by_stale_snapshot() {
        let mut inner = sample_inner();
        inner
            .voice
            .insert("lobby".into(), HashSet::from(["alice".into(), "bob".into()]));
        apply_voice_action(&mut inner, "lobby", "leave", "alice");
        assert!(!inner.voice.get("lobby").unwrap().contains("alice"));
        let frame = serde_json::json!({ "rooms": { "lobby": ["alice", "bob"] } });
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap(), "bob");
        assert!(!inner.voice.get("lobby").unwrap().contains("alice"));
        assert!(inner.voice.get("lobby").unwrap().contains("bob"));
        apply_voice_action(&mut inner, "lobby", "join", "alice");
        assert!(inner.voice.get("lobby").unwrap().contains("alice"));
    }

    #[test]
    fn voice_snapshot_from_peer_in_room_drops_leavers() {
        let mut inner = sample_inner();
        inner.voice.insert(
            "lobby".into(),
            HashSet::from(["alice".into(), "bob".into(), "carol".into()]),
        );
        let frame = serde_json::json!({ "rooms": { "lobby": ["bob", "carol"] } });
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap(), "bob");
        let people = inner.voice.get("lobby").unwrap();
        assert!(!people.contains("alice"));
        assert!(people.contains("bob"));
        assert!(people.contains("carol"));
    }

    #[test]
    fn voice_snapshot_keeps_recent_joiner_missing_from_peer() {
        let mut inner = sample_inner();
        apply_voice_action(&mut inner, "lobby", "join", "alice");
        apply_voice_action(&mut inner, "lobby", "join", "bob");
        let frame = serde_json::json!({ "rooms": { "lobby": ["bob"] } });
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap(), "bob");
        assert!(inner.voice.get("lobby").unwrap().contains("alice"));
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
    fn presence_carries_screen_share_and_keeps_it_unless_set() {
        let mut inner = sample_inner();
        inner
            .voice
            .insert("lobby".into(), HashSet::from(["aa".into()]));
        apply_presence(
            &mut inner,
            &serde_json::json!({
                "publicKey": "aa",
                "displayName": "Ana",
                "sharingScreen": true,
            }),
            false,
        );
        assert!(inner.profiles.get("aa").unwrap().sharing_screen);
        apply_presence(
            &mut inner,
            &serde_json::json!({
                "publicKey": "aa",
                "displayName": "Ana",
            }),
            false,
        );
        assert!(inner.profiles.get("aa").unwrap().sharing_screen);
        apply_presence(
            &mut inner,
            &serde_json::json!({
                "publicKey": "aa",
                "sharingScreen": false,
            }),
            false,
        );
        assert!(!inner.profiles.get("aa").unwrap().sharing_screen);
    }

    #[test]
    fn sharing_screen_ignored_when_peer_not_in_voice() {
        let mut inner = sample_inner();
        apply_presence(
            &mut inner,
            &serde_json::json!({
                "publicKey": "aa",
                "displayName": "Ana",
                "sharingScreen": true,
            }),
            false,
        );
        assert!(!inner.profiles.get("aa").unwrap().sharing_screen);
    }

    #[test]
    fn leaving_voice_clears_sharing_screen() {
        let mut inner = sample_inner();
        inner
            .voice
            .insert("lobby".into(), HashSet::from(["aa".into()]));
        inner.profiles.insert(
            "aa".into(),
            PeerProfile {
                display_name: "Ana".into(),
                sharing_screen: true,
                ..PeerProfile::default()
            },
        );
        drop_from_voice(&mut inner, "aa");
        assert!(!inner.profiles.get("aa").unwrap().sharing_screen);
        assert!(!peer_in_voice(&inner, "aa"));
    }

    #[test]
    fn voice_state_from_peer_overrides_leave_hold() {
        let mut inner = sample_inner();
        mark_voice_left(&mut inner, "bob");
        assert!(voice_left_held(&inner, "bob", now_ms()));
        let frame = serde_json::json!({ "rooms": { "lobby": ["bob"] } });
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap(), "bob");
        assert!(inner.voice.get("lobby").unwrap().contains("bob"));
        assert!(!voice_left_held(&inner, "bob", now_ms()));
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
    fn voice_state_does_not_duplicate_across_rooms() {
        let mut inner = sample_inner();
        inner.last_seen.insert("alice".into(), now_ms());
        inner
            .voice
            .insert("currall".into(), HashSet::from(["alice".into()]));
        let frame = serde_json::json!({ "rooms": { "sala": ["alice"] } });
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap(), "");
        assert!(inner.voice.get("currall").unwrap().contains("alice"));
        assert!(!inner
            .voice
            .get("sala")
            .is_some_and(|people| people.contains("alice")));
    }

    #[test]
    fn voice_state_seats_offline_looking_peer_once() {
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
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap(), "");
        assert!(inner.voice.get("lobby").unwrap().contains("aa"));
        assert!(inner.voice.get("lobby").unwrap().contains("bb"));
    }

    #[test]
    fn remote_voice_cannot_put_self_back_in_call() {
        let mut inner = sample_inner();
        let me = inner.identity.public_hex();
        inner
            .voice
            .insert("lobby".into(), HashSet::from(["bb".into()]));
        let frame = serde_json::json!({ "rooms": { "lobby": [me.clone(), "bb"] } });
        merge_voice_rooms(&mut inner, frame["rooms"].as_object().unwrap(), "");
        assert!(!inner.voice.get("lobby").unwrap().contains(&me));
        apply_voice_action(&mut inner, "lobby", "join", &me);
        assert!(!inner.voice.get("lobby").unwrap().contains(&me));
        apply_voice_action(&mut inner, "lobby", "leave", "bb");
        assert!(!inner.voice.get("lobby").unwrap().contains("bb"));
    }

    #[test]
    fn explicit_offline_presence_keeps_call_seat() {
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
        assert!(rooms.is_empty());
        assert!(inner.voice.get("lobby").unwrap().contains("aa"));
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
    fn hyperv_switch_is_not_usable_but_mesh_vpns_are() {
        assert!(!usable_v4(Ipv4Addr::new(192, 168, 137, 10)));
        assert!(usable_v4(Ipv4Addr::new(192, 168, 100, 10)));
        assert!(usable_v4(Ipv4Addr::new(25, 12, 34, 56))); // Hamachi
        assert!(usable_v4(Ipv4Addr::new(26, 1, 2, 3))); // Radmin
        assert!(usable_v4(Ipv4Addr::new(100, 64, 1, 2))); // Tailscale
        assert!(!usable_v6("2001:0:53aa:64c:0:5efe:c0a8:6401".parse().unwrap()));
        assert!(!usable_v6("2002:c0a8:1::1".parse().unwrap()));
    }

    #[test]
    fn should_not_dial_junk_adapters() {
        assert!(!should_dial("ws://192.168.137.1:7340"));
        assert!(should_dial("ws://26.12.34.56:7340"));
        assert!(should_dial("ws://25.1.2.3:7340"));
        assert!(should_dial("ws://127.0.0.1:7340"));
        assert!(should_dial("ws://192.168.1.9:7340"));
        assert!(should_dial("ws://203.0.113.10:7340"));
    }

    #[test]
    fn invite_omits_loopback_so_friends_do_not_dial_themselves() {
        let mut inner = sample_inner();
        inner.listen_port = 7340;
        inner.listen_url = "ws://127.0.0.1:7340".into();
        inner.listen_urls = vec![
            "ws://127.0.0.1:7340".into(),
            "ws://192.168.100.2:7340".into(),
        ];
        assert_eq!(
            invite_peer_list(&inner),
            vec!["ws://192.168.100.2:7340".to_string()]
        );
    }

    #[test]
    fn invite_omits_public_ip() {
        let mut inner = sample_inner();
        inner.listen_port = 7340;
        inner.listen_url = "ws://192.168.100.2:7340".into();
        inner.listen_urls = vec![
            "ws://203.0.113.10:7340".into(),
            "ws://192.168.100.2:7340".into(),
        ];
        assert_eq!(
            invite_peer_list(&inner),
            vec!["ws://192.168.100.2:7340".to_string()]
        );
    }

    #[test]
    fn invite_lists_mesh_vpn_and_omits_public_ip() {
        let mut inner = sample_inner();
        inner.listen_port = 7340;
        inner.listen_url = "ws://26.10.0.2:7340".into();
        inner.listen_urls = vec![
            "ws://203.0.113.10:7340".into(),
            "ws://26.10.0.2:7340".into(),
            "ws://25.1.2.3:7340".into(),
            "ws://192.168.1.9:7340".into(),
        ];
        assert_eq!(
            invite_peer_list(&inner),
            vec![
                "ws://192.168.1.9:7340".to_string(),
                "ws://26.10.0.2:7340".to_string(),
                "ws://25.1.2.3:7340".to_string(),
            ]
        );
    }

    #[test]
    fn parse_ws_handles_ipv6() {
        assert_eq!(
            parse_ws("ws://[2001:db8::1]:7340"),
            Some(("2001:db8::1".into(), 7340))
        );
    }

    #[test]
    fn prefer_lan_urls_over_loopback_invites() {
        let urls = prefer_non_loopback(vec![
            "ws://127.0.0.1:7340".into(),
            "ws://192.168.100.2:7340".into(),
            "ws://localhost:7340".into(),
        ]);
        assert_eq!(urls, vec!["ws://192.168.100.2:7340".to_string()]);
    }

    #[test]
    fn loopback_invite_kept_when_it_is_the_only_option() {
        let urls = prefer_non_loopback(vec!["ws://127.0.0.1:7340".into()]);
        assert_eq!(urls, vec!["ws://127.0.0.1:7340".to_string()]);
    }

    #[test]
    fn relay_is_not_a_direct_peer() {
        assert!(!is_direct_remote_key("relay"));
        assert!(!is_direct_remote_key("relay:0"));
        assert!(!is_direct_remote_key("pending:3"));
        assert!(is_direct_remote_key("ws://192.168.100.2:7340"));
    }

    #[test]
    fn mqtt_will_is_ignored_while_peer_was_just_seen() {
        let mut inner = sample_inner();
        inner.last_seen.insert("aa".into(), now_ms());
        assert!(ignore_relay_offline(&inner, "aa", true, Some("offline")));
        assert!(!ignore_relay_offline(&inner, "aa", false, Some("offline")));
        assert!(!ignore_relay_offline(&inner, "aa", true, Some("online")));
        inner.last_seen.insert("aa".into(), 1);
        assert!(!ignore_relay_offline(&inner, "aa", true, Some("offline")));
    }

    #[test]
    fn history_request_frame_asks_for_seed() {
        let inner = sample_inner();
        let frame = serde_json::json!({
            "type": "history-request",
            "publicKey": inner.identity.public_hex(),
            "historyCount": inner.archive_messages,
        });
        assert_eq!(frame["type"], "history-request");
        assert_eq!(frame["historyCount"], 0);
    }

    #[test]
    fn presence_heartbeats_are_not_byte_identical() {
        let inner = sample_inner();
        let a = presence_frame(&inner, false);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = presence_frame(&inner, false);
        assert_ne!(a, b, "presence frames need a changing ts so already_seen keeps heartbeats");
        let va: serde_json::Value = serde_json::from_str(&a).unwrap();
        let vb: serde_json::Value = serde_json::from_str(&b).unwrap();
        assert!(va.get("ts").and_then(|v| v.as_i64()).unwrap() > 0);
        assert_ne!(va.get("ts"), vb.get("ts"));
    }

    #[test]
    fn layout_advertise_includes_fresh_ts() {
        let state = AppState::new();
        {
            let mut inner = state.inner.lock().expect("state");
            *inner = sample_inner();
            inner.call_rooms.push("sala-1".into());
        }
        let a = layout_json(&state);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = layout_json(&state);
        assert_ne!(a, b);
        let va: serde_json::Value = serde_json::from_str(&a).unwrap();
        assert_eq!(va["type"], "layout");
        assert!(va["callRooms"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x.as_str() == Some("sala-1")));
        assert!(va.get("ts").and_then(|v| v.as_i64()).unwrap() > 0);
    }

    #[test]
    fn stamps_and_matches_community_id() {
        let state = AppState::new();
        let id = {
            let mut inner = state.inner.lock().expect("state");
            *inner = sample_inner();
            let community = create_community(&inner.identity, "alpha");
            let id = community.id.clone();
            inner.community = Some(community);
            id
        };
        let raw = layout_json(&state);
        let stamped = stamp_community(&state, &raw);
        let value: serde_json::Value = serde_json::from_str(&stamped).unwrap();
        assert_eq!(
            value.get("communityId").and_then(|v| v.as_str()),
            Some(id.as_str())
        );
        assert!(frame_matches_community(&value, &id));
        assert!(!frame_matches_community(&value, "other"));
        let legacy = serde_json::json!({ "type": "layout" });
        assert!(frame_matches_community(&legacy, &id));
    }

    #[test]
    fn viewed_id_keeps_the_live_community_while_in_a_call() {
        let mut inner = sample_inner();
        let community = create_community(&inner.identity, "live");
        let live_id = community.id.clone();
        inner.community = Some(community);
        inner
            .voice
            .insert("lobby".into(), HashSet::from([inner.identity.public_hex()]));
        inner.viewed_id = Some("viewed".into());
        assert!(seated_in_call(&inner));
        assert_eq!(viewed_community_id(&inner).as_deref(), Some("viewed"));
        assert_eq!(
            inner.community.as_ref().map(|c| c.id.as_str()),
            Some(live_id.as_str())
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
