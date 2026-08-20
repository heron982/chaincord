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

pub fn encode_invite(invite: &Invite) -> String {
    let json = serde_json::to_string(invite).expect("invite json");
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(json.as_bytes())
}

pub fn decode_invite(raw: &str) -> Result<Invite, String> {
    use base64::Engine;
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&compact)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&compact))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&compact))
        .map_err(|_| "convite invalido".to_string())?;
    let json = String::from_utf8(bytes).map_err(|_| "convite invalido".to_string())?;
    let invite: Invite =
        serde_json::from_str(&json).map_err(|_| "convite invalido".to_string())?;
    if invite.v != 1 || !verify_community(&invite.community) {
        return Err("convite invalido".into());
    }
    Ok(invite)
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
        let invite = Invite {
            v: 1,
            community: community.clone(),
            peers: vec!["ws://127.0.0.1:7340".into()],
        };
        let encoded = encode_invite(&invite);
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

        let url_safe = encoded.replace('+', "-").replace('/', "_");
        let decoded_url = decode_invite(&url_safe).expect("url-safe invite");
        assert_eq!(decoded_url.community.id, community.id);
    }

    #[test]
    fn decode_invite_rejects_tampered_community() {
        let owner = Identity::generate();
        let mut invite = Invite {
            v: 1,
            community: create_community(&owner, "sala"),
            peers: vec![],
        };
        invite.community.genesis.name = "outra".into();
        let encoded = encode_invite(&invite);
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
