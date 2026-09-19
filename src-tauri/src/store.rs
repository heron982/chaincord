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
    pub relays: Vec<String>,
    pub known_peer_urls: HashSet<String>,
    pub text_channels: Vec<String>,
    pub call_rooms: Vec<String>,
    pub profiles: HashMap<String, PeerProfile>,
}

#[derive(Clone, Debug)]
pub struct CommunityInfo {
    pub id: String,
    pub name: String,
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
                invite_peers TEXT NOT NULL DEFAULT '[]',
                relays TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE IF NOT EXISTS channels (
                community_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                PRIMARY KEY (community_id, kind, name)
            );
            CREATE TABLE IF NOT EXISTS peers (
                community_id TEXT NOT NULL,
                url TEXT NOT NULL,
                PRIMARY KEY (community_id, url)
            );
            CREATE TABLE IF NOT EXISTS profiles (
                community_id TEXT NOT NULL,
                public_key TEXT NOT NULL,
                display_name TEXT NOT NULL DEFAULT '',
                avatar TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (community_id, public_key)
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
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate_v2()?;
        store.migrate_v3()?;
        Ok(store)
    }

    fn schema_version(&self) -> i64 {
        let conn = self.conn.lock().expect("store");
        conn.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| {
                let raw: String = row.get(0)?;
                Ok(raw.parse::<i64>().unwrap_or(1))
            },
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or(1)
    }

    fn set_schema_version(&self, version: i64) -> Result<(), String> {
        let conn = self.conn.lock().expect("store");
        conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
            params![version.to_string()],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Old DBs scoped channels/peers/profiles globally. Move them under the
    /// single community row (if any) and recreate tables with community_id.
    fn migrate_v2(&self) -> Result<(), String> {
        if self.schema_version() >= 2 {
            return Ok(());
        }
        let needs_migrate = {
            let conn = self.conn.lock().expect("store");
            let cols: Vec<String> = conn
                .prepare("PRAGMA table_info(channels)")
                .map_err(|e| e.to_string())?
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|e| e.to_string())?
                .filter_map(|c| c.ok())
                .collect();
            !cols.is_empty() && !cols.iter().any(|c| c == "community_id")
        };
        if needs_migrate {
            let session = self.load_session_legacy();
            {
                let conn = self.conn.lock().expect("store");
                conn.execute_batch(
                    "
                    DROP TABLE IF EXISTS channels;
                    DROP TABLE IF EXISTS peers;
                    DROP TABLE IF EXISTS profiles;
                    CREATE TABLE channels (
                        community_id TEXT NOT NULL,
                        kind TEXT NOT NULL,
                        name TEXT NOT NULL,
                        PRIMARY KEY (community_id, kind, name)
                    );
                    CREATE TABLE peers (
                        community_id TEXT NOT NULL,
                        url TEXT NOT NULL,
                        PRIMARY KEY (community_id, url)
                    );
                    CREATE TABLE profiles (
                        community_id TEXT NOT NULL,
                        public_key TEXT NOT NULL,
                        display_name TEXT NOT NULL DEFAULT '',
                        avatar TEXT NOT NULL DEFAULT '',
                        PRIMARY KEY (community_id, public_key)
                    );
                    ",
                )
                .map_err(|e| e.to_string())?;
            }
            if let Some(session) = session {
                let id = session.community.id.clone();
                self.save_session(&session)?;
                self.set_active(&id)?;
            }
        }
        self.set_schema_version(2)?;
        Ok(())
    }

    fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
        let sql = format!("PRAGMA table_info({table})");
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let cols = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .filter_map(|c| c.ok())
            .collect();
        Ok(cols)
    }

    fn migrate_v3(&self) -> Result<(), String> {
        let needs_column = {
            let conn = self.conn.lock().expect("store");
            let cols = Self::table_columns(&conn, "community")?;
            !cols.is_empty() && !cols.iter().any(|c| c == "relays")
        };
        if needs_column {
            let conn = self.conn.lock().expect("store");
            conn.execute(
                "ALTER TABLE community ADD COLUMN relays TEXT NOT NULL DEFAULT '[]'",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        if self.schema_version() < 3 {
            self.set_schema_version(3)?;
        }
        Ok(())
    }

    fn load_session_legacy(&self) -> Option<Session> {
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
            id: id.clone(),
            genesis,
            signature,
            live_key,
        };
        let mut text_channels = Vec::new();
        let mut call_rooms = Vec::new();
        if let Ok(mut stmt) = conn.prepare("SELECT kind, name FROM channels") {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                for row in rows.flatten() {
                    if row.0 == "call" {
                        call_rooms.push(row.1);
                    } else {
                        text_channels.push(row.1);
                    }
                }
            }
        }
        if text_channels.is_empty() {
            text_channels.push("general".into());
        }
        let mut known_peer_urls = HashSet::new();
        if let Ok(mut stmt) = conn.prepare("SELECT url FROM peers") {
            if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
                for url in rows.flatten() {
                    known_peer_urls.insert(url);
                }
            }
        }
        let mut profiles = HashMap::new();
        if let Ok(mut stmt) =
            conn.prepare("SELECT public_key, display_name, avatar FROM profiles")
        {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            }) {
                for (pk, display_name, avatar) in rows.flatten() {
                    profiles.insert(
                        pk,
                        PeerProfile {
                            display_name,
                            avatar,
                            muted: false,
                            deafened: false,
                            sharing_screen: false,
                            status: "offline".into(),
                        },
                    );
                }
            }
        }
        Some(Session {
            community,
            invite_peers,
            relays: Vec::new(),
            known_peer_urls,
            text_channels,
            call_rooms,
            profiles,
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

    pub fn set_active(&self, community_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().expect("store");
        conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('active_community', ?1)",
            params![community_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn clear_active(&self) -> Result<(), String> {
        let conn = self.conn.lock().expect("store");
        conn.execute("DELETE FROM meta WHERE key = 'active_community'", [])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn active_id(&self) -> Option<String> {
        let conn = self.conn.lock().expect("store");
        conn.query_row(
            "SELECT value FROM meta WHERE key = 'active_community'",
            [],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
        .filter(|id: &String| !id.is_empty())
    }

    pub fn list_communities(&self) -> Vec<CommunityInfo> {
        let conn = self.conn.lock().expect("store");
        let Ok(mut stmt) = conn.prepare("SELECT id, genesis FROM community ORDER BY id") else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (id, genesis_raw) in rows.flatten() {
            let name = serde_json::from_str::<serde_json::Value>(&genesis_raw)
                .ok()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "community".into());
            out.push(CommunityInfo { id, name });
        }
        out
    }

    pub fn save_session(&self, session: &Session) -> Result<(), String> {
        self.write_session(session)?;
        self.set_active(&session.community.id)
    }

    pub fn write_session(&self, session: &Session) -> Result<(), String> {
        let id = session.community.id.clone();
        let conn = self.conn.lock().expect("store");
        let genesis = serde_json::to_string(&session.community.genesis).map_err(|e| e.to_string())?;
        let invite_peers = serde_json::to_string(&session.invite_peers).map_err(|e| e.to_string())?;
        let relays = serde_json::to_string(&session.relays).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO community(id, genesis, signature, live_key, invite_peers, relays)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                genesis,
                session.community.signature,
                session.community.live_key,
                invite_peers,
                relays
            ],
        )
        .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM channels WHERE community_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM peers WHERE community_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM profiles WHERE community_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        for name in &session.text_channels {
            conn.execute(
                "INSERT OR REPLACE INTO channels(community_id, kind, name) VALUES (?1, 'text', ?2)",
                params![id, name],
            )
            .map_err(|e| e.to_string())?;
        }
        for name in &session.call_rooms {
            conn.execute(
                "INSERT OR REPLACE INTO channels(community_id, kind, name) VALUES (?1, 'call', ?2)",
                params![id, name],
            )
            .map_err(|e| e.to_string())?;
        }
        for url in &session.known_peer_urls {
            conn.execute(
                "INSERT OR REPLACE INTO peers(community_id, url) VALUES (?1, ?2)",
                params![id, url],
            )
            .map_err(|e| e.to_string())?;
        }
        for (pk, profile) in &session.profiles {
            conn.execute(
                "INSERT OR REPLACE INTO profiles(community_id, public_key, display_name, avatar)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, pk, profile.display_name, profile.avatar],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn load_session(&self, community_id: &str) -> Option<Session> {
        let conn = self.conn.lock().expect("store");
        let row = conn
            .query_row(
                "SELECT id, genesis, signature, live_key, invite_peers, relays FROM community WHERE id = ?1",
                params![community_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5).unwrap_or_else(|_| "[]".into()),
                    ))
                },
            )
            .optional()
            .ok()
            .flatten()?;
        let (id, genesis_raw, signature, live_key, invite_raw, relays_raw) = row;
        let genesis = serde_json::from_str(&genesis_raw).ok()?;
        let invite_peers: Vec<String> = serde_json::from_str(&invite_raw).unwrap_or_default();
        let relays: Vec<String> = serde_json::from_str(&relays_raw).unwrap_or_default();
        let community = Community {
            id: id.clone(),
            genesis,
            signature,
            live_key,
        };
        let mut text_channels = Vec::new();
        let mut call_rooms = Vec::new();
        let mut stmt = conn
            .prepare("SELECT kind, name FROM channels WHERE community_id = ?1")
            .ok()?;
        let rows = stmt
            .query_map(params![id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .ok()?;
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
        let mut stmt = conn
            .prepare("SELECT url FROM peers WHERE community_id = ?1")
            .ok()?;
        let rows = stmt.query_map(params![id], |row| row.get::<_, String>(0)).ok()?;
        for url in rows.flatten() {
            known_peer_urls.insert(url);
        }
        let mut profiles = HashMap::new();
        let mut stmt = conn
            .prepare(
                "SELECT public_key, display_name, avatar FROM profiles WHERE community_id = ?1",
            )
            .ok()?;
        let rows = stmt
            .query_map(params![id], |row| {
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
                    sharing_screen: false,
                    status: "offline".into(),
                },
            );
        }
        Some(Session {
            community,
            invite_peers,
            relays,
            known_peer_urls,
            text_channels,
            call_rooms,
            profiles,
        })
    }

    pub fn load_active_session(&self) -> Option<Session> {
        let active = self.active_id().or_else(|| {
            self.list_communities()
                .into_iter()
                .next()
                .map(|c| c.id)
        })?;
        self.load_session(&active)
    }

    pub fn delete_session(&self, community_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().expect("store");
        conn.execute("DELETE FROM community WHERE id = ?1", params![community_id])
            .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM channels WHERE community_id = ?1",
            params![community_id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM peers WHERE community_id = ?1",
            params![community_id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM profiles WHERE community_id = ?1",
            params![community_id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM messages WHERE community_id = ?1",
            params![community_id],
        )
        .map_err(|e| e.to_string())?;
        let active: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'active_community'",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        if active.as_deref() == Some(community_id) {
            conn.execute("DELETE FROM meta WHERE key = 'active_community'", [])
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn clear_session(&self) -> Result<(), String> {
        let ids: Vec<String> = {
            let conn = self.conn.lock().expect("store");
            let mut stmt = conn
                .prepare("SELECT id FROM community")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.flatten().collect()
        };
        for id in ids {
            self.delete_session(&id)?;
        }
        self.clear_active()?;
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
                community_id: community_id.to_string(),
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
    fn profile_and_multi_community_roundtrip() {
        let (store, path) = temp_store();
        store
            .save_profile("aa".repeat(32).as_str(), "Felipe", "data:image/png;base64,xx")
            .expect("save profile");
        let loaded = store.load_profile().expect("load profile");
        assert_eq!(loaded.1, "Felipe");

        let owner = Identity::generate();
        let a = create_community(&owner, "alpha");
        let b = create_community(&owner, "beta");
        store
            .save_session(&Session {
                community: a.clone(),
                invite_peers: vec!["ws://127.0.0.1:7340".into()],
                relays: vec!["wss://mine.example/mqtt".into()],
                known_peer_urls: HashSet::from(["ws://192.168.100.2:7340".into()]),
                text_channels: vec!["general".into(), "random".into()],
                call_rooms: vec!["lobby".into()],
                profiles: HashMap::new(),
            })
            .expect("save a");
        store
            .save_session(&Session {
                community: b.clone(),
                invite_peers: vec![],
                relays: vec![],
                known_peer_urls: HashSet::new(),
                text_channels: vec!["general".into()],
                call_rooms: vec![],
                profiles: HashMap::new(),
            })
            .expect("save b");

        let listed = store.list_communities();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|c| c.name == "alpha"));
        assert!(listed.iter().any(|c| c.name == "beta"));
        assert_eq!(store.active_id().as_deref(), Some(b.id.as_str()));

        let session_a = store.load_session(&a.id).expect("load a");
        assert!(session_a.text_channels.contains(&"random".to_string()));
        assert_eq!(session_a.call_rooms, vec!["lobby".to_string()]);
        assert_eq!(session_a.relays, vec!["wss://mine.example/mqtt".to_string()]);

        store.delete_session(&b.id).expect("delete b");
        assert_eq!(store.list_communities().len(), 1);
        assert!(store.load_session(&b.id).is_none());
        assert!(store.load_session(&a.id).is_some());

        store.clear_session().expect("clear");
        assert!(store.list_communities().is_empty());
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn messages_stay_in_their_community() {
        let (store, path) = temp_store();
        let owner = Identity::generate();
        let a = create_community(&owner, "alpha");
        let b = create_community(&owner, "beta");
        store
            .save_session(&Session {
                community: a.clone(),
                invite_peers: vec![],
                relays: vec![],
                known_peer_urls: HashSet::new(),
                text_channels: vec!["general".into()],
                call_rooms: vec![],
                profiles: HashMap::new(),
            })
            .unwrap();
        store
            .write_session(&Session {
                community: b.clone(),
                invite_peers: vec![],
                relays: vec![],
                known_peer_urls: HashSet::new(),
                text_channels: vec!["general".into()],
                call_rooms: vec!["lobby".into()],
                profiles: HashMap::new(),
            })
            .unwrap();
        store
            .append_message(
                &a.id,
                &UiMessage {
                    sender: "aa".into(),
                    text: "ola alpha".into(),
                    ts: 1,
                    channel: "general".into(),
                    is_self: true,
                    community_id: a.id.clone(),
                },
            )
            .unwrap();
        store
            .append_message(
                &b.id,
                &UiMessage {
                    sender: "bb".into(),
                    text: "ola beta".into(),
                    ts: 2,
                    channel: "general".into(),
                    is_self: false,
                    community_id: b.id.clone(),
                },
            )
            .unwrap();
        let from_a = store.load_messages(&a.id);
        let from_b = store.load_messages(&b.id);
        assert_eq!(from_a.len(), 1);
        assert_eq!(from_a[0].text, "ola alpha");
        assert_eq!(from_b.len(), 1);
        assert_eq!(from_b[0].text, "ola beta");
        assert_eq!(store.active_id().as_deref(), Some(a.id.as_str()));
        drop(store);
        let _ = std::fs::remove_file(path);
    }
}
