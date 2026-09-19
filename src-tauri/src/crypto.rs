use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct Identity {
    pub signing: SigningKey,
}

impl Identity {
    pub fn generate() -> Self {
        Self {
            signing: SigningKey::generate(&mut OsRng),
        }
    }

    pub fn from_secret_hex(secret: &str) -> Result<Self, String> {
        let bytes = hex::decode(secret).map_err(|e| e.to_string())?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "chave secreta invalida".to_string())?;
        Ok(Self {
            signing: SigningKey::from_bytes(&arr),
        })
    }

    pub fn public_hex(&self) -> String {
        hex::encode(self.signing.verifying_key().as_bytes())
    }

    pub fn secret_hex(&self) -> String {
        hex::encode(self.signing.to_bytes())
    }

    pub fn sign(&self, msg: &[u8]) -> String {
        hex::encode(self.signing.sign(msg).to_bytes())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Genesis {
    pub name: String,
    pub owner: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Community {
    pub id: String,
    pub genesis: Genesis,
    pub signature: String,
    #[serde(rename = "liveKey")]
    pub live_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invite {
    pub v: u8,
    pub community: Community,
    pub peers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relays: Vec<String>,
}

impl Invite {
    pub fn new(community: Community, peers: Vec<String>, relays: Vec<String>) -> Self {
        let relays: Vec<String> = relays.into_iter().filter(|s| !s.is_empty()).collect();
        Self {
            v: if relays.is_empty() { 2 } else { 3 },
            community,
            peers,
            relays,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatPlain {
    pub channel: String,
    pub text: String,
    pub ts: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireMessage {
    #[serde(rename = "type")]
    pub kind: String,
    pub sender: String,
    pub nonce: String,
    pub ciphertext: String,
    pub signature: String,
    #[serde(default, rename = "communityId", skip_serializing_if = "String::is_empty")]
    pub community_id: String,
}

pub fn canonical_genesis(genesis: &Genesis) -> String {
    format!(
        r#"{{"name":{},"owner":{},"createdAt":{}}}"#,
        serde_json::to_string(&genesis.name).unwrap(),
        serde_json::to_string(&genesis.owner).unwrap(),
        genesis.created_at
    )
}

pub fn create_community(owner: &Identity, name: &str) -> Community {
    let genesis = Genesis {
        name: {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                "comunidade".into()
            } else {
                trimmed.into()
            }
        },
        owner: owner.public_hex(),
        created_at: now_ms(),
    };
    let payload = canonical_genesis(&genesis);
    let bytes = payload.as_bytes();
    let signature = owner.sign(bytes);
    let id = hex::encode(Sha256::digest(bytes));
    let live_key = random_hex(32);
    Community {
        id,
        genesis,
        signature,
        live_key,
    }
}

pub fn verify_community(community: &Community) -> bool {
    if community.genesis.owner.len() != 64 {
        return false;
    }
    let payload = canonical_genesis(&community.genesis);
    let bytes = payload.as_bytes();
    let id = hex::encode(Sha256::digest(bytes));
    if id != community.id {
        return false;
    }
    verify_ed25519(&community.genesis.owner, bytes, &community.signature)
}

fn invite_prefix(raw: &str) -> &str {
    raw.strip_prefix("cc/")
        .or_else(|| raw.strip_prefix("CC/"))
        .or_else(|| raw.strip_prefix("chaincord:"))
        .unwrap_or(raw)
}

fn extract_invite_blob(raw: &str) -> &str {
    let mut i = 0;
    while i < raw.len() {
        let rest = &raw[i..];
        if rest.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("cc/")) {
            return rest;
        }
        if rest
            .get(..10)
            .is_some_and(|p| p.eq_ignore_ascii_case("chaincord:"))
        {
            return rest;
        }
        i += rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
    }
    raw
}

fn pack_url_list(out: &mut Vec<u8>, urls: &[String]) {
    let list: Vec<&str> = urls
        .iter()
        .map(|p| p.as_str())
        .filter(|p| !p.is_empty() && p.len() <= 255)
        .take(4)
        .collect();
    out.push(list.len() as u8);
    for url in list {
        let bytes = url.as_bytes();
        out.push(bytes.len() as u8);
        out.extend_from_slice(bytes);
    }
}

fn pack_invite(invite: &Invite) -> Result<Vec<u8>, String> {
    let name = invite.community.genesis.name.as_bytes();
    if name.is_empty() || name.len() > 64 {
        return Err("nome da comunidade invalido".into());
    }
    let owner = hex::decode(&invite.community.genesis.owner).map_err(|_| "convite invalido")?;
    let signature = hex::decode(&invite.community.signature).map_err(|_| "convite invalido")?;
    let live_key = hex::decode(&invite.community.live_key).map_err(|_| "convite invalido")?;
    if owner.len() != 32 || signature.len() != 64 || live_key.len() != 32 {
        return Err("convite invalido".into());
    }
    let version = if invite.relays.iter().any(|r| !r.is_empty()) {
        3
    } else {
        2
    };
    let mut out = Vec::with_capacity(1 + 1 + name.len() + 32 + 8 + 64 + 32 + 2 + 128);
    out.push(version);
    out.push(name.len() as u8);
    out.extend_from_slice(name);
    out.extend_from_slice(&owner);
    out.extend_from_slice(&invite.community.genesis.created_at.to_be_bytes());
    out.extend_from_slice(&signature);
    out.extend_from_slice(&live_key);
    pack_url_list(&mut out, &invite.peers);
    if version == 3 {
        pack_url_list(&mut out, &invite.relays);
    }
    Ok(out)
}

fn unpack_url_list(bytes: &[u8], i: &mut usize) -> Result<Vec<String>, String> {
    let take = |i: &mut usize, n: usize| -> Result<&[u8], String> {
        let end = i
            .checked_add(n)
            .ok_or_else(|| "convite invalido".to_string())?;
        if end > bytes.len() {
            return Err("convite invalido".into());
        }
        let slice = &bytes[*i..end];
        *i = end;
        Ok(slice)
    };
    let count = take(i, 1)?[0] as usize;
    if count > 8 {
        return Err("convite invalido".into());
    }
    let mut urls = Vec::with_capacity(count);
    for _ in 0..count {
        let len = take(i, 1)?[0] as usize;
        let url = String::from_utf8(take(i, len)?.to_vec()).map_err(|_| "convite invalido")?;
        if !url.is_empty() {
            urls.push(url);
        }
    }
    Ok(urls)
}

fn unpack_invite(bytes: &[u8]) -> Result<Invite, String> {
    let mut i = 0usize;
    let take = |i: &mut usize, n: usize| -> Result<&[u8], String> {
        let end = i.checked_add(n).ok_or_else(|| "convite invalido".to_string())?;
        if end > bytes.len() {
            return Err("convite invalido".into());
        }
        let slice = &bytes[*i..end];
        *i = end;
        Ok(slice)
    };
    let version = take(&mut i, 1)?[0];
    if version != 2 && version != 3 {
        return Err("convite invalido".into());
    }
    let name_len = take(&mut i, 1)?[0] as usize;
    if name_len == 0 || name_len > 64 {
        return Err("convite invalido".into());
    }
    let name = String::from_utf8(take(&mut i, name_len)?.to_vec()).map_err(|_| "convite invalido")?;
    let owner = hex::encode(take(&mut i, 32)?);
    let created_raw = take(&mut i, 8)?;
    let mut created_buf = [0u8; 8];
    created_buf.copy_from_slice(created_raw);
    let created_at = i64::from_be_bytes(created_buf);
    let signature = hex::encode(take(&mut i, 64)?);
    let live_key = hex::encode(take(&mut i, 32)?);
    let peers = unpack_url_list(bytes, &mut i)?;
    let relays = if version == 3 {
        unpack_url_list(bytes, &mut i)?
    } else {
        Vec::new()
    };
    if i != bytes.len() {
        return Err("convite invalido".into());
    }
    let genesis = Genesis {
        name,
        owner,
        created_at,
    };
    let id = hex::encode(Sha256::digest(canonical_genesis(&genesis).as_bytes()));
    let invite = Invite {
        v: version,
        community: Community {
            id,
            genesis,
            signature,
            live_key,
        },
        peers,
        relays,
    };
    if !verify_community(&invite.community) {
        return Err("convite invalido".into());
    }
    Ok(invite)
}

fn decode_invite_json(bytes: &[u8]) -> Result<Invite, String> {
    let json = String::from_utf8(bytes.to_vec()).map_err(|_| "convite invalido".to_string())?;
    let invite: Invite =
        serde_json::from_str(&json).map_err(|_| "convite invalido".to_string())?;
    if (invite.v != 1 && invite.v != 2 && invite.v != 3) || !verify_community(&invite.community) {
        return Err("convite invalido".into());
    }
    Ok(invite)
}

pub fn encode_invite(invite: &Invite) -> String {
    use base64::Engine;
    let packed = pack_invite(invite).expect("invite pack");
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(packed);
    format!("cc/{body}")
}

pub fn decode_invite(raw: &str) -> Result<Invite, String> {
    use base64::Engine;
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let blob = extract_invite_blob(&compact);
    let body = invite_prefix(blob);
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(body))
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(body))
        .map_err(|_| "convite invalido".to_string())?;
    if matches!(bytes.first(), Some(2) | Some(3)) {
        return unpack_invite(&bytes);
    }
    decode_invite_json(&bytes)
}

pub fn seal_chat(identity: &Identity, live_key: &str, plain: &ChatPlain) -> Result<WireMessage, String> {
    let nonce_bytes = random_bytes(24);
    let body = serde_json::to_vec(&ChatPlain {
        channel: plain.channel.clone(),
        text: plain.text.clone(),
        ts: plain.ts,
    })
    .map_err(|e| e.to_string())?;
    let key = hex_to_32(live_key)?;
    let cipher = XChaCha20Poly1305::new((&key).into());
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, body.as_ref())
        .map_err(|_| "falha ao cifrar".to_string())?;
    let mut to_sign = nonce_bytes.clone();
    to_sign.extend_from_slice(&ciphertext);
    Ok(WireMessage {
        kind: "chat".into(),
        sender: identity.public_hex(),
        nonce: hex::encode(&nonce_bytes),
        ciphertext: hex::encode(&ciphertext),
        signature: identity.sign(&to_sign),
        community_id: String::new(),
    })
}

pub fn open_chat(live_key: &str, msg: &WireMessage) -> Option<ChatPlain> {
    let nonce = hex::decode(&msg.nonce).ok()?;
    let ciphertext = hex::decode(&msg.ciphertext).ok()?;
    if nonce.len() != 24 {
        return None;
    }
    let mut to_sign = nonce.clone();
    to_sign.extend_from_slice(&ciphertext);
    if !verify_ed25519(&msg.sender, &to_sign, &msg.signature) {
        return None;
    }
    let key = hex_to_32(live_key).ok()?;
    let cipher = XChaCha20Poly1305::new((&key).into());
    let nonce_arr = XNonce::from_slice(&nonce);
    let body = cipher.decrypt(nonce_arr, ciphertext.as_ref()).ok()?;
    serde_json::from_slice(&body).ok()
}

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn verify_ed25519(public_hex: &str, message: &[u8], signature_hex: &str) -> bool {
    let pk = match hex::decode(public_hex) {
        Ok(b) if b.len() == 32 => b,
        _ => return false,
    };
    let sig_bytes = match hex::decode(signature_hex) {
        Ok(b) if b.len() == 64 => b,
        _ => return false,
    };
    let vk = match VerifyingKey::from_bytes(pk.as_slice().try_into().unwrap()) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let sig = match Signature::from_slice(&sig_bytes) {
        Ok(s) => s,
        Err(_) => return false,
    };
    vk.verify(message, &sig).is_ok()
}

fn hex_to_32(hex_str: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex_str).map_err(|_| "chave invalida".to_string())?;
    bytes
        .try_into()
        .map_err(|_| "chave invalida".to_string())
}

fn random_bytes(n: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    OsRng.fill_bytes(&mut buf);
    buf
}

fn random_hex(n: usize) -> String {
    hex::encode(random_bytes(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_genesis_keeps_name_owner_created_at_order() {
        let genesis = Genesis {
            name: "sala".into(),
            owner: "aa".into(),
            created_at: 42,
        };
        let canonical = canonical_genesis(&genesis);
        let name_at = canonical.find("\"name\"").unwrap();
        let owner_at = canonical.find("\"owner\"").unwrap();
        let created_at = canonical.find("\"createdAt\"").unwrap();
        assert!(name_at < owner_at);
        assert!(owner_at < created_at);
        assert_eq!(canonical, r#"{"name":"sala","owner":"aa","createdAt":42}"#);
    }

    #[test]
    fn invite_roundtrip_ignores_whitespace_and_url_safe_base64() {
        let owner = Identity::generate();
        let community = create_community(&owner, "  Chaincord  ");
        assert!(verify_community(&community));
        assert_eq!(community.genesis.name, "Chaincord");
        let invite = Invite::new(community.clone(), vec!["ws://127.0.0.1:7340".into()], vec![]);
        let encoded = encode_invite(&invite);
        assert!(encoded.starts_with("cc/"));
        assert!(
            encoded.len() < 360,
            "compact invite should stay short, got {}",
            encoded.len()
        );
        let wrapped = encoded
            .chars()
            .enumerate()
            .flat_map(|(i, c)| {
                if i > 0 && i % 16 == 0 {
                    vec!['\n', c]
                } else {
                    vec![c]
                }
            })
            .collect::<String>();
        let decoded = decode_invite(&wrapped).expect("whitespace invite");
        assert_eq!(decoded.community.id, community.id);
        assert_eq!(decoded.peers, invite.peers);
        assert!(decoded.relays.is_empty());
        assert_eq!(decoded.community.genesis.name, "Chaincord");

        let decoded_prefix = decode_invite(&encoded).expect("prefixed invite");
        assert_eq!(decoded_prefix.community.id, community.id);

        let wrapped_msg = format!(
            "Convite Chaincord — {}\nAbra o app → Entrar com convite e cole isto:\n{}",
            community.genesis.name, encoded
        );
        let from_msg = decode_invite(&wrapped_msg).expect("share message invite");
        assert_eq!(from_msg.community.id, community.id);
    }

    #[test]
    fn invite_carries_custom_relay() {
        use base64::Engine;
        let owner = Identity::generate();
        let community = create_community(&owner, "sala");
        let invite = Invite::new(
            community.clone(),
            vec!["ws://127.0.0.1:7340".into()],
            vec!["wss://broker.example/mqtt".into()],
        );
        let encoded = encode_invite(&invite);
        let body = encoded.strip_prefix("cc/").expect("prefix");
        let packed = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body)
            .expect("b64");
        assert_eq!(packed.first().copied(), Some(3));
        let decoded = decode_invite(&encoded).expect("v3 invite");
        assert_eq!(decoded.relays, vec!["wss://broker.example/mqtt".to_string()]);
        assert_eq!(decoded.peers, invite.peers);
        assert_eq!(decoded.community.id, community.id);
    }

    #[test]
    fn legacy_json_invite_still_decodes() {
        use base64::Engine;
        let owner = Identity::generate();
        let community = create_community(&owner, "legado");
        let invite = Invite {
            v: 1,
            community: community.clone(),
            peers: vec!["ws://192.168.0.2:7340".into()],
            relays: vec![],
        };
        let json = serde_json::to_string(&invite).unwrap();
        let legacy = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
        let decoded = decode_invite(&legacy).expect("legacy invite");
        assert_eq!(decoded.community.id, community.id);
        assert_eq!(decoded.peers, invite.peers);
    }

    #[test]
    fn decode_invite_rejects_tampered_community() {
        let owner = Identity::generate();
        let mut invite = Invite {
            v: 2,
            community: create_community(&owner, "sala"),
            peers: vec![],
            relays: vec![],
        };
        invite.community.genesis.name = "outra".into();
        // Re-pack would recompute id; tamper the packed bytes instead via encode then mutate name in struct
        // and force JSON path:
        use base64::Engine;
        let json = serde_json::to_string(&Invite {
            v: 1,
            community: invite.community.clone(),
            peers: vec![],
            relays: vec![],
        })
        .unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
        assert!(decode_invite(&encoded).is_err());
        assert!(decode_invite("nao-e-base64%%").is_err());
    }

    #[test]
    fn seal_and_open_chat_roundtrip() {
        let identity = Identity::generate();
        let live_key = hex::encode([7u8; 32]);
        let plain = ChatPlain {
            channel: "general".into(),
            text: "oi".into(),
            ts: 99,
        };
        let wire = seal_chat(&identity, &live_key, &plain).expect("seal");
        let opened = open_chat(&live_key, &wire).expect("open");
        assert_eq!(opened.text, "oi");
        assert_eq!(opened.channel, "general");
        assert_eq!(wire.sender, identity.public_hex());
        assert!(open_chat(&hex::encode([8u8; 32]), &wire).is_none());
    }

    #[test]
    fn identity_survives_secret_hex() {
        let original = Identity::generate();
        let restored = Identity::from_secret_hex(&original.secret_hex()).expect("secret");
        assert_eq!(original.public_hex(), restored.public_hex());
        assert_eq!(original.public_hex().len(), 64);
    }
}
