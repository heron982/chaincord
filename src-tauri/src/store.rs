use crate::crypto::Community;
use crate::peer::{PeerProfile, UiMessage};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
}

pub struct Session {
    pub community: Community,
    pub invite_peers: Vec<String>,
    pub known_peer_urls: HashSet<String>,
    pub text_channels: Vec<String>,
    pub call_rooms: Vec<String>,
    pub profiles: HashMap<String, PeerProfile>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS community (
                id TEXT PRIMARY KEY,
                genesis TEXT NOT NULL,
                signature TEXT NOT NULL,
                live_key TEXT NOT NULL,
                invite_peers TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE IF NOT EXISTS channels (
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                PRIMARY KEY (kind, name)
            );
            CREATE TABLE IF NOT EXISTS peers (
                url TEXT PRIMARY KEY
            );
            CREATE TABLE IF NOT EXISTS profiles (
                public_key TEXT PRIMARY KEY,
                display_name TEXT NOT NULL DEFAULT '',
                avatar TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                community_id TEXT NOT NULL,
                sender TEXT NOT NULL,
                text TEXT NOT NULL,
                ts INTEGER NOT NULL,
                channel TEXT NOT NULL,
                is_self INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS messages_community_ts
                ON messages (community_id, ts);
            ",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn save_profile(&self, secret_key: &str, display_name: &str, avatar: &str) -> Result<(), String> {
        let conn = self.conn.lock().expect("store");
        let mut stmt = conn
            .prepare("INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)")
            .map_err(|e| e.to_string())?;
        stmt.execute(params!["secret_key", secret_key])
            .map_err(|e| e.to_string())?;
        stmt.execute(params!["display_name", display_name])
            .map_err(|e| e.to_string())?;
        stmt.execute(params!["avatar", avatar])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load_profile(&self) -> Option<(String, String, String)> {
        let conn = self.conn.lock().expect("store");
        let secret: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'secret_key'",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()?;
        if secret.is_empty() {
            return None;
        }
        let display_name: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'display_name'",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or_default();
        let avatar: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'avatar'",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or_default();
        Some((secret, display_name, avatar))
    }

    pub fn save_session(&self, session: &Session) -> Result<(), String> {
        let conn = self.conn.lock().expect("store");
        conn.execute("DELETE FROM community", [])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM channels", [])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM peers", [])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM profiles", [])
            .map_err(|e| e.to_string())?;
        let genesis = serde_json::to_string(&session.community.genesis).map_err(|e| e.to_string())?;
        let invite_peers = serde_json::to_string(&session.invite_peers).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO community(id, genesis, signature, live_key, invite_peers) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session.community.id,
                genesis,
                session.community.signature,
                session.community.live_key,
                invite_peers
            ],
        )
        .map_err(|e| e.to_string())?;
        for name in &session.text_channels {
            conn.execute(
                "INSERT OR REPLACE INTO channels(kind, name) VALUES ('text', ?1)",
                params![name],
            )
            .map_err(|e| e.to_string())?;
        }
        for name in &session.call_rooms {
            conn.execute(
                "INSERT OR REPLACE INTO channels(kind, name) VALUES ('call', ?1)",
                params![name],
            )
            .map_err(|e| e.to_string())?;
        }
        for url in &session.known_peer_urls {
            conn.execute("INSERT OR REPLACE INTO peers(url) VALUES (?1)", params![url])
                .map_err(|e| e.to_string())?;
        }
        for (pk, profile) in &session.profiles {
            conn.execute(
                "INSERT OR REPLACE INTO profiles(public_key, display_name, avatar) VALUES (?1, ?2, ?3)",
                params![pk, profile.display_name, profile.avatar],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn load_session(&self) -> Option<Session> {
        let conn = self.conn.lock().expect("store");
        let row = conn
            .query_row(
                "SELECT id, genesis, signature, live_key, invite_peers FROM community LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .ok()
            .flatten()?;
        let (id, genesis_raw, signature, live_key, invite_raw) = row;
        let genesis = serde_json::from_str(&genesis_raw).ok()?;
        let invite_peers: Vec<String> = serde_json::from_str(&invite_raw).unwrap_or_default();
        let community = Community {
            id,
            genesis,
            signature,
            live_key,
        };
        let mut text_channels = Vec::new();
        let mut call_rooms = Vec::new();
        let mut stmt = conn
            .prepare("SELECT kind, name FROM channels")
            .ok()?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).ok()?;
        for row in rows.flatten() {
            if row.0 == "call" {
                call_rooms.push(row.1);
            } else {
                text_channels.push(row.1);
            }
        }
        if text_channels.is_empty() {
            text_channels.push("general".into());
        }
        let mut known_peer_urls = HashSet::new();
        let mut stmt = conn.prepare("SELECT url FROM peers").ok()?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).ok()?;
        for url in rows.flatten() {
            known_peer_urls.insert(url);
        }
        let mut profiles = HashMap::new();
        let mut stmt = conn
            .prepare("SELECT public_key, display_name, avatar FROM profiles")
            .ok()?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .ok()?;
        for (pk, display_name, avatar) in rows.flatten() {
            profiles.insert(
                pk,
                PeerProfile {
                    display_name,
                    avatar,
                    muted: false,
                    deafened: false,
                    status: "offline".into(),
                },
            );
        }
        Some(Session {
            community,
            invite_peers,
            known_peer_urls,
            text_channels,
            call_rooms,
            profiles,
        })
    }

    pub fn clear_session(&self) -> Result<(), String> {
        let conn = self.conn.lock().expect("store");
        conn.execute("DELETE FROM community", [])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM channels", [])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM peers", [])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM profiles", [])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM messages", [])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn append_message(&self, community_id: &str, msg: &UiMessage) -> Result<(), String> {
        let conn = self.conn.lock().expect("store");
        conn.execute(
            "INSERT INTO messages(community_id, sender, text, ts, channel, is_self) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                community_id,
                msg.sender,
                msg.text,
                msg.ts,
                msg.channel,
                if msg.is_self { 1 } else { 0 }
            ],
        )
        .map_err(|e| e.to_string())?;
        let extra: i64 = conn
            .query_row(
                "SELECT COUNT(*) - 5000 FROM messages WHERE community_id = ?1",
                params![community_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if extra > 0 {
            let _ = conn.execute(
                "DELETE FROM messages WHERE id IN (
                    SELECT id FROM messages WHERE community_id = ?1 ORDER BY ts ASC LIMIT ?2
                )",
                params![community_id, extra],
            );
        }
        Ok(())
    }

    pub fn load_messages(&self, community_id: &str) -> Vec<UiMessage> {
        let conn = self.conn.lock().expect("store");
        let Ok(mut stmt) = conn.prepare(
            "SELECT sender, text, ts, channel, is_self FROM messages
             WHERE community_id = ?1 ORDER BY ts ASC, id ASC LIMIT 5000",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map(params![community_id], |row| {
            Ok(UiMessage {
                sender: row.get(0)?,
                text: row.get(1)?,
                ts: row.get(2)?,
                channel: row.get(3)?,
                is_self: row.get::<_, i64>(4)? != 0,
            })
        });
        match rows {
            Ok(iter) => iter.flatten().collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{create_community, Identity};

    fn temp_store() -> (Store, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "chaincord-store-test-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path).expect("open store");
        (store, path)
    }

    #[test]
    fn profile_and_session_roundtrip() {
        let (store, path) = temp_store();
        store
            .save_profile("aa".repeat(32).as_str(), "Felipe", "data:image/png;base64,xx")
            .expect("save profile");
        let loaded = store.load_profile().expect("load profile");
        assert_eq!(loaded.1, "Felipe");
        assert_eq!(loaded.2, "data:image/png;base64,xx");

        let owner = Identity::generate();
        let community = create_community(&owner, "sala");
        let mut profiles = HashMap::new();
        profiles.insert(
            owner.public_hex(),
            PeerProfile {
                display_name: "Felipe".into(),
                avatar: String::new(),
                muted: false,
                deafened: false,
                status: "offline".into(),
            },
        );
        store
            .save_session(&Session {
                community: community.clone(),
                invite_peers: vec!["ws://127.0.0.1:7340".into()],
                known_peer_urls: HashSet::from(["ws://192.168.100.2:7340".into()]),
                text_channels: vec!["general".into(), "random".into()],
                call_rooms: vec!["lobby".into()],
                profiles,
            })
            .expect("save session");
        let session = store.load_session().expect("load session");
        assert_eq!(session.community.id, community.id);
        assert_eq!(session.invite_peers, vec!["ws://127.0.0.1:7340".to_string()]);
        assert!(session.known_peer_urls.contains("ws://192.168.100.2:7340"));
        assert!(session.text_channels.contains(&"random".to_string()));
        assert_eq!(session.call_rooms, vec!["lobby".to_string()]);
        assert_eq!(
            session.profiles.get(&owner.public_hex()).unwrap().display_name,
            "Felipe"
        );

        store
            .append_message(
                &community.id,
                &UiMessage {
                    sender: owner.public_hex(),
                    text: "oi".into(),
                    ts: 1,
                    channel: "general".into(),
                    is_self: true,
                },
            )
            .expect("append");
        let msgs = store.load_messages(&community.id);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "oi");
        assert!(msgs[0].is_self);

        store.clear_session().expect("clear");
        assert!(store.load_session().is_none());
        drop(store);
        let _ = std::fs::remove_file(path);
    }
}
