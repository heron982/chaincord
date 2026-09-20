//! Per-community erasure archive (def. 17) + leave handoff helpers (def. 20).
//!
//! Live chat still uses the plaintext SQLite cache. This module packs that
//! cache into an encrypted blob, splits it into Reed-Solomon shards, and
//! tracks which member should hold which shard.

use reed_solomon_erasure::galois_8::ReedSolomon;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DATA_SHARDS: usize = 4;
pub const PARITY_SHARDS: usize = 2;
pub const TOTAL_SHARDS: usize = DATA_SHARDS + PARITY_SHARDS;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveStatus {
    /// Local plaintext cache is enough for the UI.
    Live,
    /// Ciphertext/shards exist but fewer than k online (or none held here).
    PendingK,
    /// Not enough shards anywhere we know about.
    Lost,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivePlan {
    pub blob_id: String,
    pub shards: Vec<Vec<u8>>,
    /// Holder public key for each shard index (length == TOTAL_SHARDS).
    pub holders: Vec<String>,
}

/// Stable assignment: sorted member keys, shard i → members[i % n].
/// Solo member holds every shard.
pub fn assign_holders(members: &[String]) -> Vec<String> {
    let mut keys: Vec<String> = members
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    keys.sort();
    keys.dedup();
    if keys.is_empty() {
        return vec![String::new(); TOTAL_SHARDS];
    }
    (0..TOTAL_SHARDS)
        .map(|i| keys[i % keys.len()].clone())
        .collect()
}

pub fn blob_id_for(ciphertext: &[u8]) -> String {
    hex::encode(Sha256::digest(ciphertext))[..32].to_string()
}

fn pad_shards(data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let len = data.len();
    let shard_payload = (len + DATA_SHARDS - 1) / DATA_SHARDS;
    let shard_len = shard_payload
        .checked_add(4)
        .ok_or_else(|| "archive too large".to_string())?;
    let mut shards = Vec::with_capacity(TOTAL_SHARDS);
    for i in 0..DATA_SHARDS {
        let mut shard = vec![0u8; shard_len];
        shard[0..4].copy_from_slice(&(len as u32).to_be_bytes());
        let start = i * shard_payload;
        if start < len {
            let end = (start + shard_payload).min(len);
            let dst = 4..4 + (end - start);
            shard[dst].copy_from_slice(&data[start..end]);
        }
        shards.push(shard);
    }
    for _ in 0..PARITY_SHARDS {
        shards.push(vec![0u8; shard_len]);
    }
    Ok(shards)
}

fn unpad_data_shards(shards: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if shards.len() < DATA_SHARDS {
        return Err("not enough data shards".into());
    }
    let first = &shards[0];
    if first.len() < 4 {
        return Err("corrupt shard".into());
    }
    let mut len_buf = [0u8; 4];
    len_buf.copy_from_slice(&first[0..4]);
    let len = u32::from_be_bytes(len_buf) as usize;
    let shard_payload = first.len() - 4;
    let mut out = Vec::with_capacity(len);
    for shard in shards.iter().take(DATA_SHARDS) {
        if shard.len() != first.len() {
            return Err("uneven shards".into());
        }
        if shard[0..4] != first[0..4] {
            return Err("corrupt shard header".into());
        }
        let take = (len - out.len()).min(shard_payload);
        out.extend_from_slice(&shard[4..4 + take]);
        if out.len() >= len {
            break;
        }
    }
    if out.len() != len {
        return Err("truncated archive".into());
    }
    Ok(out)
}

pub fn encode(data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if data.is_empty() {
        return Err("empty archive".into());
    }
    let mut shards = pad_shards(data)?;
    let rs = ReedSolomon::new(DATA_SHARDS, PARITY_SHARDS).map_err(|e| e.to_string())?;
    rs.encode(&mut shards).map_err(|e| e.to_string())?;
    Ok(shards)
}

/// `pieces[i] = Some(shard)` or `None` if missing. Needs any DATA_SHARDS pieces.
pub fn reconstruct(pieces: &mut [Option<Vec<u8>>]) -> Result<Vec<u8>, String> {
    if pieces.len() != TOTAL_SHARDS {
        return Err("wrong shard count".into());
    }
    let present = pieces.iter().filter(|p| p.is_some()).count();
    if present < DATA_SHARDS {
        return Err("need more shards".into());
    }
    let rs = ReedSolomon::new(DATA_SHARDS, PARITY_SHARDS).map_err(|e| e.to_string())?;
    rs.reconstruct(pieces).map_err(|e| e.to_string())?;
    let data_shards: Vec<Vec<u8>> = pieces
        .iter()
        .take(DATA_SHARDS)
        .map(|p| p.clone().ok_or_else(|| "reconstruct incomplete".to_string()))
        .collect::<Result<_, _>>()?;
    unpad_data_shards(&data_shards)
}

pub fn plan_archive(ciphertext: &[u8], members: &[String]) -> Result<ArchivePlan, String> {
    let shards = encode(ciphertext)?;
    let holders = assign_holders(members);
    Ok(ArchivePlan {
        blob_id: blob_id_for(ciphertext),
        shards,
        holders,
    })
}

/// Shard indices this member is assigned to hold.
pub fn my_shard_indices(me: &str, holders: &[String]) -> Vec<usize> {
    holders
        .iter()
        .enumerate()
        .filter(|(_, pk)| pk.as_str() == me)
        .map(|(i, _)| i)
        .collect()
}

/// Shards I must push before leave (every index assigned to me, when peers are online).
pub fn handoff_indices(me: &str, holders: &[String], online_others: &[String]) -> Vec<usize> {
    if online_others.iter().all(|pk| pk == me) || online_others.is_empty() {
        return Vec::new();
    }
    my_shard_indices(me, holders)
}

/// Pick who receives handoff: prefer non-owner among online, else first online.
pub fn pick_handoff_target(me: &str, owner: &str, online: &[String]) -> Option<String> {
    let mut others: Vec<&String> = online.iter().filter(|pk| pk.as_str() != me).collect();
    if others.is_empty() {
        return None;
    }
    others.sort();
    others
        .iter()
        .find(|pk| pk.as_str() != owner)
        .or_else(|| others.first())
        .map(|s| (*s).clone())
}

pub const MIN_ONLINE_REPLICAS: usize = 2;

/// Shard indices not present in `have` (0..TOTAL_SHARDS).
pub fn missing_indices(have: &[u8]) -> Vec<usize> {
    let set: std::collections::HashSet<u8> = have.iter().copied().collect();
    (0..TOTAL_SHARDS as u8)
        .filter(|i| !set.contains(i))
        .map(|i| i as usize)
        .collect()
}

/// Indices I should ask `remote` for (they have, I don't).
pub fn indices_to_pull(mine: &[u8], remote: &[u8]) -> Vec<u8> {
    let mine_set: std::collections::HashSet<u8> = mine.iter().copied().collect();
    let mut out: Vec<u8> = remote
        .iter()
        .copied()
        .filter(|i| (*i as usize) < TOTAL_SHARDS && !mine_set.contains(i))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// `copies[i]` = how many online peers (including me) hold shard i.
/// Returns indices below `min_replicas` (clamped to at least 1).
pub fn under_replicated(copies: &[usize], online_peers: usize, min_replicas: usize) -> Vec<usize> {
    let target = if online_peers <= 1 {
        1
    } else {
        min_replicas.max(1).min(online_peers)
    };
    copies
        .iter()
        .enumerate()
        .take(TOTAL_SHARDS)
        .filter(|(_, &n)| n < target)
        .map(|(i, _)| i)
        .collect()
}

/// Pick who should receive a spare copy: online peer with fewest of these shards.
pub fn pick_repair_target(
    me: &str,
    online: &[String],
    // pk -> indices they hold for this blob
    inventory: &std::collections::HashMap<String, Vec<u8>>,
    shard_index: usize,
) -> Option<String> {
    let idx = shard_index as u8;
    let mut candidates: Vec<&String> = online.iter().filter(|pk| pk.as_str() != me).collect();
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by_key(|pk| {
        let has = inventory
            .get(pk.as_str())
            .map(|v| v.contains(&idx))
            .unwrap_or(false);
        (has, pk.as_str())
    });
    candidates.first().map(|s| (*s).clone())
}

pub fn swarm_archive_status(
    local_messages: usize,
    distinct_shards_known: usize,
    online_holders: usize,
) -> ArchiveStatus {
    if local_messages > 0 {
        return ArchiveStatus::Live;
    }
    if distinct_shards_known >= DATA_SHARDS {
        return ArchiveStatus::PendingK;
    }
    if distinct_shards_known > 0 || online_holders > 0 {
        return ArchiveStatus::PendingK;
    }
    ArchiveStatus::Lost
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn encode_roundtrip_and_loss() {
        let data = b"hello community archive payload for erasure coding";
        let shards = encode(data).expect("encode");
        assert_eq!(shards.len(), TOTAL_SHARDS);

        let mut pieces: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
        // Drop two shards (parity budget).
        pieces[1] = None;
        pieces[5] = None;
        let out = reconstruct(&mut pieces).expect("reconstruct");
        assert_eq!(out, data);
    }

    #[test]
    fn assign_is_stable_and_covers_all() {
        let holders = assign_holders(&["bb".into(), "aa".into(), "cc".into()]);
        assert_eq!(holders.len(), TOTAL_SHARDS);
        assert_eq!(holders[0], "aa");
        assert_eq!(holders[1], "bb");
        assert_eq!(holders[2], "cc");
        assert_eq!(holders[3], "aa");
        let solo = assign_holders(&["only".into()]);
        assert!(solo.iter().all(|h| h == "only"));
    }

    #[test]
    fn handoff_lists_my_shards_when_peers_online() {
        let holders = assign_holders(&["aa".into(), "bb".into()]);
        let mine = handoff_indices("aa", &holders, &["bb".into()]);
        assert!(!mine.is_empty());
        assert!(mine.iter().all(|&i| holders[i] == "aa"));
        assert!(handoff_indices("aa", &holders, &[]).is_empty());
    }

    #[test]
    fn pick_target_skips_self() {
        assert_eq!(
            pick_handoff_target("aa", "aa", &["aa".into(), "bb".into(), "cc".into()]),
            Some("bb".into())
        );
        assert_eq!(pick_handoff_target("aa", "aa", &["aa".into()]), None);
    }

    #[test]
    fn pull_and_under_replicated() {
        assert_eq!(indices_to_pull(&[0, 1], &[1, 2, 3]), vec![2, 3]);
        assert_eq!(missing_indices(&[0, 2]), vec![1, 3, 4, 5]);
        let copies = [2, 1, 0, 2, 1, 1];
        assert_eq!(under_replicated(&copies, 3, 2), vec![1, 2, 4, 5]);
        assert!(under_replicated(&[1, 1, 1, 1, 1, 1], 1, 2).is_empty());
    }

    #[test]
    fn repair_target_prefers_peer_without_shard() {
        let mut inv = HashMap::new();
        inv.insert("bb".into(), vec![0u8, 1]);
        inv.insert("cc".into(), vec![2u8]);
        assert_eq!(
            pick_repair_target("aa", &["bb".into(), "cc".into()], &inv, 0),
            Some("cc".into())
        );
    }

    #[test]
    fn swarm_status_pending_vs_lost() {
        assert_eq!(swarm_archive_status(3, 0, 0), ArchiveStatus::Live);
        assert_eq!(swarm_archive_status(0, 4, 1), ArchiveStatus::PendingK);
        assert_eq!(swarm_archive_status(0, 1, 0), ArchiveStatus::PendingK);
        assert_eq!(swarm_archive_status(0, 0, 0), ArchiveStatus::Lost);
    }
}
