use crate::peer;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::AppHandle;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{HeaderValue, SEC_WEBSOCKET_PROTOCOL};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

struct Hub {
    next: AtomicU64,
    clients: Mutex<HashMap<u64, mpsc::UnboundedSender<String>>>,
}

fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| Hub {
        next: AtomicU64::new(1),
        clients: Mutex::new(HashMap::new()),
    })
}

fn hub_broadcast(except: u64, payload: &str) {
    let clients = hub().clients.lock().unwrap_or_else(|e| e.into_inner());
    for (id, tx) in clients.iter() {
        if *id != except {
            let _ = tx.send(payload.to_string());
        }
    }
}

pub fn spawn(app: AppHandle) {
    let Some(community_id) = peer::community_id(&app) else {
        return;
    };
    if relay_disabled() {
        return;
    }
    let gen = peer::bump_relay_gen(&app);
    let pk = peer::public_key(&app);
    let pk16 = pk[..16.min(pk.len())].to_string();
    for (i, url) in peer::community_relays(&app).into_iter().enumerate() {
        if peer::is_own_relay_url(&app, &url) {
            continue;
        }
        let app = app.clone();
        let community_id = community_id.clone();
        let client_id = format!("cc{pk16}{i}");
        let slot = i.to_string();
        tauri::async_runtime::spawn(async move {
            let mut backoff = 2u64;
            loop {
                if !peer::relay_is_current(&app, gen) {
                    return;
                }
                match connect_mqtt(&url, &client_id).await {
                    Ok(ws) => {
                        backoff = 2;
                        let _ = pump(&app, gen, &community_id, &slot, ws).await;
                    }
                    Err(_) => {}
                }
                if !peer::relay_is_current(&app, gen) {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(15);
            }
        });
    }
}

fn relay_disabled() -> bool {
    matches!(
        std::env::var("CHAINCORD_RELAY").ok().as_deref(),
        Some("off") | Some("0") | Some("false")
    )
}

pub fn effective(stored: &[String]) -> Vec<String> {
    stored
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn parse_one(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("informe um relé ws:// ou wss://".into());
    }
    if url.len() > 255 {
        return Err("relé muito longo".into());
    }
    if url.chars().any(char::is_whitespace) {
        return Err("relé invalido".into());
    }
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("wss://") || lower.starts_with("ws://")) {
        return Err("relé deve ser ws:// ou wss://".into());
    }
    let rest = if lower.starts_with("wss://") {
        &url[6..]
    } else {
        &url[5..]
    };
    if rest.is_empty() || rest.starts_with('/') {
        return Err("relé invalido".into());
    }
    Ok(url.to_string())
}

pub fn choose_for_create(custom: Option<&str>) -> Result<Vec<String>, String> {
    if let Some(raw) = custom.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(vec![parse_one(raw)?]);
    }
    match std::env::var("CHAINCORD_RELAY") {
        Ok(url) if relay_off(&url) => Ok(Vec::new()),
        Ok(url) if !url.trim().is_empty() => Ok(vec![parse_one(&url)?]),
        _ => Ok(Vec::new()),
    }
}

fn relay_off(value: &str) -> bool {
    matches!(value.trim(), "off" | "0" | "false")
}

fn topic(community_id: &str) -> String {
    format!("cc/v1/{community_id}")
}

fn mqtt_connack() -> Vec<u8> {
    vec![0x20, 0x02, 0x00, 0x00]
}

fn mqtt_pingresp() -> Vec<u8> {
    vec![0xD0, 0x00]
}

fn mqtt_suback(id: u16) -> Vec<u8> {
    let mut payload = id.to_be_bytes().to_vec();
    payload.push(1);
    packet(0x90, &payload)
}

fn mqtt_kind(packet: &[u8]) -> u8 {
    packet.first().copied().unwrap_or(0) & 0xF0
}

fn mqtt_fixed_header_len(packet: &[u8]) -> Option<usize> {
    if packet.is_empty() {
        return None;
    }
    let mut idx = 1usize;
    loop {
        if idx >= packet.len() {
            return None;
        }
        let byte = packet[idx];
        idx += 1;
        if byte & 0x80 == 0 {
            return Some(idx);
        }
        if idx > 5 {
            return None;
        }
    }
}

fn mqtt_subscribe_id(packet: &[u8]) -> Option<u16> {
    if mqtt_kind(packet) != 0x80 {
        return None;
    }
    let idx = mqtt_fixed_header_len(packet)?;
    if idx + 2 > packet.len() {
        return None;
    }
    Some(u16::from_be_bytes([packet[idx], packet[idx + 1]]))
}

pub async fn serve_hub<S>(app: AppHandle, mut ws: WebSocketStream<S>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some(community_id) = peer::community_id(&app) else {
        let _ = ws.close(None).await;
        return;
    };
    let topic = topic(&community_id);
    let id = hub().next.fetch_add(1, Ordering::SeqCst);
    let slot = format!("hub{id}");
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    {
        let mut clients = hub().clients.lock().unwrap_or_else(|e| e.into_inner());
        clients.insert(id, tx.clone());
    }
    let gen = peer::current_relay_gen(&app);
    peer::register_relay(&app, &slot, tx);

    let mut buf = Vec::new();
    let mut pkt_id: u16 = 1;

    loop {
        if peer::community_id(&app).as_deref() != Some(community_id.as_str()) {
            break;
        }
        tokio::select! {
            outgoing = rx.recv() => {
                let Some(text) = outgoing else { break; };
                let payload = compact_wire(&text);
                if payload.len() > 48_000 {
                    continue;
                }
                pkt_id = next_mqtt_id(pkt_id);
                if ws
                    .send(Message::Binary(
                        mqtt_publish(&topic, payload.as_bytes(), pkt_id).into(),
                    ))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            incoming = ws.next() => {
                let Some(Ok(msg)) = incoming else { break; };
                let Ok(bytes) = ws_bytes(msg) else { continue; };
                buf.extend_from_slice(&bytes);
                let mut stop = false;
                while let Some(packet) = take_packet(&mut buf) {
                    match mqtt_kind(&packet) {
                        0x10 => {
                            if ws.send(Message::Binary(mqtt_connack().into())).await.is_err() {
                                stop = true;
                                break;
                            }
                        }
                        0x80 => {
                            if let Some(sid) = mqtt_subscribe_id(&packet) {
                                if ws.send(Message::Binary(mqtt_suback(sid).into())).await.is_err()
                                {
                                    stop = true;
                                    break;
                                }
                            }
                        }
                        0x30 => {
                            if let Some(pid) = mqtt_publish_id(&packet) {
                                let _ = ws.send(Message::Binary(mqtt_puback(pid).into())).await;
                            }
                            if let Some(payload) = mqtt_publish_payload(&packet) {
                                if let Ok(text) = String::from_utf8(payload) {
                                    hub_broadcast(id, &text);
                                    peer::ingest_from_relay(&app, &text);
                                }
                            }
                        }
                        0xC0 => {
                            let _ = ws.send(Message::Binary(mqtt_pingresp().into())).await;
                        }
                        0xE0 => {
                            stop = true;
                            break;
                        }
                        _ => {}
                    }
                }
                if stop {
                    break;
                }
            }
        }
    }

    hub()
        .clients
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    peer::unregister_relay(&app, gen, &slot);
    let _ = ws.close(None).await;
}

async fn connect_mqtt(
    url: &str,
    client_id: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    String,
> {
    let mut req = url
        .into_client_request()
        .map_err(|e| e.to_string())?;
    req.headers_mut().insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static("mqtt"),
    );
    let (mut ws, _) = tokio::time::timeout(Duration::from_secs(8), tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| "tempo esgotado".to_string())?
        .map_err(|e| e.to_string())?;
    ws.send(Message::Binary(mqtt_connect(client_id).into()))
        .await
        .map_err(|e| e.to_string())?;
    let connack = tokio::time::timeout(Duration::from_secs(8), ws.next())
        .await
        .map_err(|_| "sem CONNACK".to_string())?
        .ok_or_else(|| "relé fechou".to_string())?
        .map_err(|e| e.to_string())?;
    let data = ws_bytes(connack)?;
    if data.first() != Some(&0x20) || data.get(3).copied().unwrap_or(1) != 0 {
        return Err("CONNACK recusado".into());
    }
    Ok(ws)
}

async fn pump(
    app: &AppHandle,
    gen: u64,
    community_id: &str,
    slot: &str,
    mut ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<(), String> {
    let topic = topic(community_id);
    ws.send(Message::Binary(mqtt_subscribe(&topic, 1).into()))
        .await
        .map_err(|e| e.to_string())?;
    let mut buf = wait_suback(&mut ws).await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    peer::register_relay(app, slot, tx);
    let acks = drain_mqtt_publishes(app, &mut buf);
    for id in acks {
        let _ = ws.send(Message::Binary(mqtt_puback(id).into())).await;
    }
    let mut pkt_id: u16 = 1;
    for msg in peer::handshake_messages(app, true) {
        let payload = compact_wire(&msg);
        if payload.len() > 48_000 {
            continue;
        }
        pkt_id = next_mqtt_id(pkt_id);
        let _ = ws
            .send(Message::Binary(
                mqtt_publish(&topic, payload.as_bytes(), pkt_id).into(),
            ))
            .await;
    }
    peer::emit_state(app);

    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut replay = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(2),
        Duration::from_secs(10),
    );
    replay.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        if !peer::relay_is_current(app, gen) {
            peer::unregister_relay(app, gen, slot);
            let _ = ws.close(None).await;
            return Ok(());
        }
        tokio::select! {
            _ = ping.tick() => {
                if ws.send(Message::Binary(vec![0xC0, 0x00].into())).await.is_err() {
                    break;
                }
            }
            _ = replay.tick() => {
                for msg in peer::handshake_messages(app, true) {
                    let payload = compact_wire(&msg);
                    if payload.len() > 48_000 {
                        continue;
                    }
                    pkt_id = next_mqtt_id(pkt_id);
                    if ws
                        .send(Message::Binary(
                            mqtt_publish(&topic, payload.as_bytes(), pkt_id).into(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            outgoing = rx.recv() => {
                let Some(text) = outgoing else { break; };
                let payload = compact_wire(&text);
                if payload.len() > 48_000 {
                    continue;
                }
                pkt_id = next_mqtt_id(pkt_id);
                if ws
                    .send(Message::Binary(
                        mqtt_publish(&topic, payload.as_bytes(), pkt_id).into(),
                    ))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            incoming = ws.next() => {
                let Some(Ok(msg)) = incoming else { break; };
                let Ok(bytes) = ws_bytes(msg) else { continue; };
                buf.extend_from_slice(&bytes);
                while let Some(packet) = take_packet(&mut buf) {
                    if let Some(id) = mqtt_publish_id(&packet) {
                        let _ = ws.send(Message::Binary(mqtt_puback(id).into())).await;
                    }
                    if let Some(payload) = mqtt_publish_payload(&packet) {
                        if let Ok(text) = String::from_utf8(payload) {
                            peer::ingest_from_relay(app, &text);
                        }
                    }
                }
            }
        }
    }
    peer::unregister_relay(app, gen, slot);
    Err("relé desconectou".into())
}

type MqttWs = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

async fn wait_suback(ws: &mut MqttWs) -> Result<Vec<u8>, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut buf = Vec::new();
    loop {
        if mqtt_buf_has_type(&buf, 0x90) {
            return Ok(buf);
        }
        let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remain.is_zero() {
            return Ok(buf);
        }
        match tokio::time::timeout(remain, ws.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => buf.extend_from_slice(&b),
            Ok(Some(Ok(Message::Ping(p)))) => {
                let _ = ws.send(Message::Pong(p)).await;
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                return Err("relé fechou".into());
            }
            Ok(Some(Err(e))) => return Err(e.to_string()),
            Ok(Some(_)) => {}
            Err(_) => return Ok(buf),
        }
    }
}

fn mqtt_buf_has_type(buf: &[u8], header: u8) -> bool {
    let mut tmp = buf.to_vec();
    while let Some(pkt) = take_packet(&mut tmp) {
        if pkt.first().copied().unwrap_or(0) & 0xF0 == header {
            return true;
        }
    }
    false
}

fn drain_mqtt_publishes(app: &AppHandle, buf: &mut Vec<u8>) -> Vec<u16> {
    let mut acks = Vec::new();
    while let Some(packet) = take_packet(buf) {
        if let Some(id) = mqtt_publish_id(&packet) {
            acks.push(id);
        }
        if let Some(payload) = mqtt_publish_payload(&packet) {
            if let Ok(text) = String::from_utf8(payload) {
                peer::ingest_from_relay(app, &text);
            }
        }
    }
    acks
}

fn compact_wire(json: &str) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(json) else {
        return json.to_string();
    };
    if v.get("avatar").is_some() {
        v["avatar"] = serde_json::Value::String(String::new());
    }
    if let Some(people) = v.get_mut("people").and_then(|p| p.as_object_mut()) {
        for person in people.values_mut() {
            if let Some(obj) = person.as_object_mut() {
                obj.insert("avatar".into(), serde_json::Value::String(String::new()));
            }
        }
    }
    v.to_string()
}

fn ws_bytes(msg: Message) -> Result<Vec<u8>, String> {
    match msg {
        Message::Binary(b) => Ok(b.to_vec()),
        Message::Text(t) => Ok(t.as_bytes().to_vec()),
        _ => Err("frame ignorado".into()),
    }
}

fn mqtt_str(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut v = Vec::with_capacity(2 + b.len());
    v.extend_from_slice(&(b.len() as u16).to_be_bytes());
    v.extend_from_slice(b);
    v
}

fn remaining_len(mut n: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if n == 0 {
            break;
        }
    }
    out
}

fn packet(header: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![header];
    out.extend(remaining_len(payload.len()));
    out.extend_from_slice(payload);
    out
}

fn mqtt_connect(client_id: &str) -> Vec<u8> {
    let mut payload = mqtt_str("MQTT");
    payload.push(4);
    payload.push(0x02);
    payload.extend_from_slice(&60u16.to_be_bytes());
    payload.extend(mqtt_str(client_id));
    packet(0x10, &payload)
}

fn mqtt_subscribe(topic: &str, packet_id: u16) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&packet_id.to_be_bytes());
    payload.extend(mqtt_str(topic));
    payload.push(1);
    packet(0x82, &payload)
}

fn mqtt_publish(topic: &str, body: &[u8], id: u16) -> Vec<u8> {
    let mut payload = mqtt_str(topic);
    payload.extend_from_slice(&id.to_be_bytes());
    payload.extend_from_slice(body);
    packet(0x32, &payload)
}

fn mqtt_puback(id: u16) -> Vec<u8> {
    packet(0x40, &id.to_be_bytes())
}

fn next_mqtt_id(id: u16) -> u16 {
    if id == u16::MAX {
        1
    } else {
        id + 1
    }
}

fn take_packet(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    if buf.len() < 2 {
        return None;
    }
    let mut idx = 1usize;
    let mut mul = 1usize;
    let mut len = 0usize;
    loop {
        if idx >= buf.len() {
            return None;
        }
        let byte = buf[idx];
        len += (byte & 0x7f) as usize * mul;
        idx += 1;
        if byte & 0x80 == 0 {
            break;
        }
        mul *= 128;
        if idx > 5 {
            buf.clear();
            return None;
        }
    }
    let total = idx + len;
    if buf.len() < total {
        return None;
    }
    Some(buf.drain(..total).collect())
}

fn mqtt_publish_payload(packet: &[u8]) -> Option<Vec<u8>> {
    if packet.is_empty() || packet[0] & 0xF0 != 0x30 {
        return None;
    }
    let qos = (packet[0] & 0x06) >> 1;
    let mut idx = 1usize;
    let mut mul = 1usize;
    loop {
        if idx >= packet.len() {
            return None;
        }
        let byte = packet[idx];
        idx += 1;
        if byte & 0x80 == 0 {
            break;
        }
        mul *= 128;
        if mul > 128 * 128 * 128 {
            return None;
        }
    }
    if idx + 2 > packet.len() {
        return None;
    }
    let topic_len = u16::from_be_bytes([packet[idx], packet[idx + 1]]) as usize;
    idx += 2;
    idx += topic_len;
    if qos > 0 {
        idx += 2;
    }
    if idx > packet.len() {
        return None;
    }
    Some(packet[idx..].to_vec())
}

fn mqtt_publish_id(packet: &[u8]) -> Option<u16> {
    if packet.is_empty() || packet[0] & 0xF0 != 0x30 {
        return None;
    }
    let qos = (packet[0] & 0x06) >> 1;
    if qos == 0 {
        return None;
    }
    let mut idx = 1usize;
    let mut mul = 1usize;
    loop {
        if idx >= packet.len() {
            return None;
        }
        let byte = packet[idx];
        idx += 1;
        if byte & 0x80 == 0 {
            break;
        }
        mul *= 128;
        if mul > 128 * 128 * 128 {
            return None;
        }
    }
    if idx + 2 > packet.len() {
        return None;
    }
    let topic_len = u16::from_be_bytes([packet[idx], packet[idx + 1]]) as usize;
    idx += 2 + topic_len;
    if idx + 2 > packet.len() {
        return None;
    }
    Some(u16::from_be_bytes([packet[idx], packet[idx + 1]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_wire_strips_avatars() {
        let raw = serde_json::json!({
            "type": "presence-state",
            "avatar": "data:image/png;base64,AAAA",
            "people": {
                "aa": { "displayName": "Ana", "avatar": "huge" }
            }
        })
        .to_string();
        let compact = compact_wire(&raw);
        let v: serde_json::Value = serde_json::from_str(&compact).unwrap();
        assert_eq!(v["avatar"], "");
        assert_eq!(v["people"]["aa"]["avatar"], "");
        assert_eq!(v["people"]["aa"]["displayName"], "Ana");
    }

    #[test]
    fn mqtt_publish_roundtrip_qos1() {
        let body = br#"{"type":"hello"}"#;
        let packet = mqtt_publish("cc/v1/abc", body, 7);
        assert_eq!(packet[0] & 0xF0, 0x30);
        assert_eq!((packet[0] & 0x06) >> 1, 1);
        assert_eq!(mqtt_publish_id(&packet), Some(7));
        assert_eq!(mqtt_publish_payload(&packet).unwrap(), body);
        assert_eq!(mqtt_puback(7)[0], 0x40);
        assert_eq!(next_mqtt_id(u16::MAX), 1);
        assert_eq!(next_mqtt_id(7), 8);
        let sub = mqtt_subscribe("cc/v1/abc", 9);
        assert_eq!(mqtt_subscribe_id(&sub), Some(9));
        assert_eq!(mqtt_connack(), vec![0x20, 0x02, 0x00, 0x00]);
    }

    #[test]
    fn take_packet_waits_for_complete_frame() {
        let packet = mqtt_publish("cc/v1/abc", b"ping", 1);
        let mut buf = packet[..4].to_vec();
        assert!(take_packet(&mut buf).is_none());
        buf.extend_from_slice(&packet[4..]);
        let taken = take_packet(&mut buf).expect("full packet");
        assert_eq!(taken, packet);
        assert!(buf.is_empty());
    }

    #[test]
    fn compact_wire_leaves_invalid_json_alone() {
        assert_eq!(compact_wire("not-json"), "not-json");
    }

    #[test]
    fn mqtt_connect_is_clean_session_without_will() {
        let packet = mqtt_connect("ccabc");
        assert_eq!(packet[0], 0x10);
        let mut idx = 1usize;
        loop {
            let byte = packet[idx];
            idx += 1;
            if byte & 0x80 == 0 {
                break;
            }
        }
        let proto_len = u16::from_be_bytes([packet[idx], packet[idx + 1]]) as usize;
        idx += 2 + proto_len;
        idx += 1;
        let flags = packet[idx];
        assert_eq!(flags, 0x02);
        idx += 1 + 2;
        let id_len = u16::from_be_bytes([packet[idx], packet[idx + 1]]) as usize;
        idx += 2 + id_len;
        assert_eq!(idx, packet.len());
    }

    #[test]
    fn parse_one_accepts_websocket_urls() {
        assert_eq!(
            parse_one("  wss://broker.example/mqtt ").unwrap(),
            "wss://broker.example/mqtt"
        );
        assert!(parse_one("http://broker.example/mqtt").is_err());
        assert!(parse_one("wss://").is_err());
        assert!(parse_one("").is_err());
    }

    #[test]
    fn empty_stored_relays_mean_no_broker() {
        assert!(effective(&[]).is_empty());
        assert_eq!(
            effective(&["wss://mine.example/mqtt".into()]),
            vec!["wss://mine.example/mqtt".to_string()]
        );
    }

    #[test]
    fn choose_for_create_keeps_custom_relay() {
        assert_eq!(
            choose_for_create(Some("wss://mine.example/mqtt")).unwrap(),
            vec!["wss://mine.example/mqtt".to_string()]
        );
        assert!(choose_for_create(Some("ftp://x")).is_err());
    }
}
