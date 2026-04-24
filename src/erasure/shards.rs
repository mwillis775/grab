//! Shard storage and identification

use anyhow::Result;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::crypto::hash;
use crate::types::ChunkId;

use super::codec::ErasureConfig;

/// Unique identifier for a shard: (chunk_id, shard_index)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShardId {
    /// The chunk this shard belongs to
    pub chunk_id: ChunkId,
    /// Index within the erasure-coded set (0..total_shards)
    pub shard_index: u8,
}

impl ShardId {
    /// Serialize to a storage key (32-byte chunk_id + 1-byte index)
    pub fn to_key(&self) -> [u8; 33] {
        let mut key = [0u8; 33];
        key[..32].copy_from_slice(&self.chunk_id);
        key[32] = self.shard_index;
        key
    }

    /// Deserialize from a storage key
    pub fn from_key(key: &[u8]) -> Option<Self> {
        if key.len() != 33 {
            return None;
        }
        let mut chunk_id = [0u8; 32];
        chunk_id.copy_from_slice(&key[..32]);
        Some(Self {
            chunk_id,
            shard_index: key[32],
        })
    }
}

/// An erasure-coded shard with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shard {
    /// Shard identifier
    pub id: ShardId,
    /// Shard data
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
    /// BLAKE3 hash of this shard's data for integrity verification
    pub shard_hash: [u8; 32],
    /// Whether this is a parity shard
    pub is_parity: bool,
    /// Original chunk size (needed for reconstruction trimming)
    pub original_chunk_size: u32,
    /// Erasure config used to create this shard
    pub erasure_config: ErasureConfig,
}

/// Persistent shard storage backed by sled
pub struct ShardStore {
    /// Shard data store (key: 33-byte ShardId, value: shard data)
    db: sled::Db,
    /// Shard metadata store (key: 33-byte ShardId, value: ShardMeta)
    meta_db: sled::Tree,
    /// Index: chunk_id -> set of shard indices we have locally
    local_shards: RwLock<HashMap<ChunkId, HashSet<u8>>>,
    /// Statistics
    shard_count: AtomicUsize,
    total_size: AtomicU64,
}

/// Stored shard metadata (compact, without the data itself)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardMeta {
    pub shard_hash: [u8; 32],
    pub is_parity: bool,
    pub original_chunk_size: u32,
    pub erasure_config: ErasureConfig,
    pub data_size: u32,
}

impl ShardStore {
    /// Create a new shard store
    pub fn new(data_dir: &Path) -> Result<Self> {
        let db_path = data_dir.join("shards");
        let db = sled::open(&db_path)?;
        let meta_db = db.open_tree("shard_meta")?;

        // Build in-memory index from existing data
        let mut local_shards: HashMap<ChunkId, HashSet<u8>> = HashMap::new();
        let mut shard_count = 0usize;
        let mut total_size = 0u64;

        for result in db.iter() {
            if let Ok((key, value)) = result {
                if let Some(shard_id) = ShardId::from_key(&key) {
                    local_shards
                        .entry(shard_id.chunk_id)
                        .or_default()
                        .insert(shard_id.shard_index);
                    shard_count += 1;
                    total_size += value.len() as u64;
                }
            }
        }

        Ok(Self {
            db,
            meta_db,
            local_shards: RwLock::new(local_shards),
            shard_count: AtomicUsize::new(shard_count),
            total_size: AtomicU64::new(total_size),
        })
    }

    /// Store a shard
    pub fn put(&self, shard: &Shard) -> Result<()> {
        let key = shard.id.to_key();

        // Verify integrity before storing
        let computed_hash = hash(&shard.data);
        if computed_hash != shard.shard_hash {
            anyhow::bail!("Shard hash mismatch — data corrupted");
        }

        // Store data
        self.db.insert(&key, shard.data.as_slice())?;

        // Store metadata
        let meta = ShardMeta {
            shard_hash: shard.shard_hash,
            is_parity: shard.is_parity,
            original_chunk_size: shard.original_chunk_size,
            erasure_config: shard.erasure_config,
            data_size: shard.data.len() as u32,
        };
        let meta_bytes = bincode::serialize(&meta)?;
        self.meta_db.insert(&key, meta_bytes)?;

        // Update in-memory index
        self.local_shards
            .write()
            .entry(shard.id.chunk_id)
            .or_default()
            .insert(shard.id.shard_index);

        self.shard_count.fetch_add(1, Ordering::Relaxed);
        self.total_size
            .fetch_add(shard.data.len() as u64, Ordering::Relaxed);

        Ok(())
    }

    /// Get a shard's data by ID
    pub fn get(&self, id: &ShardId) -> Result<Option<Vec<u8>>> {
        let key = id.to_key();
        match self.db.get(&key)? {
            Some(data) => Ok(Some(data.to_vec())),
            None => Ok(None),
        }
    }

    /// Get shard metadata
    pub fn get_meta(&self, id: &ShardId) -> Result<Option<ShardMeta>> {
        let key = id.to_key();
        match self.meta_db.get(&key)? {
            Some(data) => {
                let meta: ShardMeta = bincode::deserialize(&data)?;
                Ok(Some(meta))
            }
            None => Ok(None),
        }
    }

    /// Get a full Shard struct (data + metadata)
    pub fn get_full(&self, id: &ShardId) -> Result<Option<Shard>> {
        let data = match self.get(id)? {
            Some(d) => d,
            None => return Ok(None),
        };
        let meta = match self.get_meta(id)? {
            Some(m) => m,
            None => return Ok(None),
        };

        Ok(Some(Shard {
            id: *id,
            data,
            shard_hash: meta.shard_hash,
            is_parity: meta.is_parity,
            original_chunk_size: meta.original_chunk_size,
            erasure_config: meta.erasure_config,
        }))
    }

    /// Check if we have a specific shard
    pub fn contains(&self, id: &ShardId) -> bool {
        self.local_shards
            .read()
            .get(&id.chunk_id)
            .map(|s| s.contains(&id.shard_index))
            .unwrap_or(false)
    }

    /// Get the set of shard indices we hold for a given chunk
    pub fn local_shard_indices(&self, chunk_id: &ChunkId) -> Vec<u8> {
        self.local_shards
            .read()
            .get(chunk_id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Check if we have enough shards to reconstruct a chunk
    pub fn can_reconstruct(&self, chunk_id: &ChunkId, data_shards_needed: usize) -> bool {
        self.local_shards
            .read()
            .get(chunk_id)
            .map(|s| s.len() >= data_shards_needed)
            .unwrap_or(false)
    }

    /// Delete all shards for a chunk
    pub fn delete_chunk_shards(&self, chunk_id: &ChunkId) -> Result<usize> {
        let indices: Vec<u8> = self.local_shard_indices(chunk_id);
        let mut deleted = 0;

        for idx in &indices {
            let id = ShardId {
                chunk_id: *chunk_id,
                shard_index: *idx,
            };
            let key = id.to_key();
            if let Some(data) = self.db.remove(&key)? {
                self.meta_db.remove(&key)?;
                self.shard_count.fetch_sub(1, Ordering::Relaxed);
                self.total_size
                    .fetch_sub(data.len() as u64, Ordering::Relaxed);
                deleted += 1;
            }
        }

        self.local_shards.write().remove(chunk_id);

        Ok(deleted)
    }

    /// Get total shard count
    pub fn count(&self) -> usize {
        self.shard_count.load(Ordering::Relaxed)
    }

    /// Get total storage used by shards
    pub fn total_size(&self) -> u64 {
        self.total_size.load(Ordering::Relaxed)
    }

    /// Get number of chunks that have shards stored
    pub fn chunk_count(&self) -> usize {
        self.local_shards.read().len()
    }

    /// Flush to disk
    pub fn flush(&self) -> Result<()> {
        self.db.flush()?;
        self.meta_db.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::erasure::ErasureCodec;
    use tempfile::tempdir;

    #[test]
    fn test_shard_id_key_roundtrip() {
        let id = ShardId {
            chunk_id: [42u8; 32],
            shard_index: 3,
        };
        let key = id.to_key();
        let recovered = ShardId::from_key(&key).unwrap();
        assert_eq!(id, recovered);
    }

    #[test]
    fn test_shard_store_put_get() {
        let dir = tempdir().unwrap();
        let store = ShardStore::new(dir.path()).unwrap();
        let codec = ErasureCodec::default_codec().unwrap();

        let chunk_id = [1u8; 32];
        let data = b"test data for shard storage";
        let shards = codec.encode(&chunk_id, data).unwrap();

        // Store first 3 shards
        for shard in &shards[..3] {
            store.put(shard).unwrap();
        }

        assert_eq!(store.count(), 3);
        assert_eq!(store.local_shard_indices(&chunk_id).len(), 3);

        // Verify retrieval
        let retrieved = store.get_full(&shards[0].id).unwrap().unwrap();
        assert_eq!(retrieved.data, shards[0].data);
        assert_eq!(retrieved.shard_hash, shards[0].shard_hash);

        // Not-stored shard returns None
        assert!(store.get(&shards[4].id).unwrap().is_none());
    }

    #[test]
    fn test_shard_store_can_reconstruct() {
        let dir = tempdir().unwrap();
        let store = ShardStore::new(dir.path()).unwrap();
        let codec = ErasureCodec::default_codec().unwrap();

        let chunk_id = [2u8; 32];
        let data = vec![0xAB; 1024];
        let shards = codec.encode(&chunk_id, &data).unwrap();

        // Store 3 shards — not enough (need 4)
        for shard in &shards[..3] {
            store.put(shard).unwrap();
        }
        assert!(!store.can_reconstruct(&chunk_id, 4));

        // Store one more — now enough
        store.put(&shards[3]).unwrap();
        assert!(store.can_reconstruct(&chunk_id, 4));
    }

    #[test]
    fn test_shard_store_delete() {
        let dir = tempdir().unwrap();
        let store = ShardStore::new(dir.path()).unwrap();
        let codec = ErasureCodec::default_codec().unwrap();

        let chunk_id = [3u8; 32];
        let data = b"delete test";
        let shards = codec.encode(&chunk_id, data).unwrap();

        for shard in &shards {
            store.put(shard).unwrap();
        }
        assert_eq!(store.count(), 6);

        let deleted = store.delete_chunk_shards(&chunk_id).unwrap();
        assert_eq!(deleted, 6);
        assert_eq!(store.count(), 0);
        assert!(store.local_shard_indices(&chunk_id).is_empty());
    }
}
