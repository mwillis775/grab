//! Content-addressed chunk storage using sled

use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use anyhow::Result;
use parking_lot::RwLock;
use std::collections::HashMap;

use crate::types::ChunkId;
use crate::crypto::hash;

/// Content-addressed chunk store backed by sled
pub struct ChunkStore {
    db: sled::Db,
    /// In-memory LRU cache
    cache: RwLock<HashMap<ChunkId, Vec<u8>>>,
    cache_max_size: usize,
    /// Statistics
    chunk_count: AtomicUsize,
    total_size: AtomicU64,
}

impl ChunkStore {
    /// Create a new chunk store
    pub fn new(data_dir: &Path) -> Result<Self> {
        let db_path = data_dir.join("chunks");
        let db = sled::open(&db_path)?;
        
        // Count existing chunks
        let chunk_count = db.len();
        let mut total_size = 0u64;
        for result in db.iter() {
            if let Ok((_, value)) = result {
                total_size += value.len() as u64;
            }
        }
        
        Ok(Self {
            db,
            cache: RwLock::new(HashMap::new()),
            cache_max_size: 1000, // Max cached chunks
            chunk_count: AtomicUsize::new(chunk_count),
            total_size: AtomicU64::new(total_size),
        })
    }

    /// Store a chunk, returns its content-addressed ID
    pub fn put(&self, data: &[u8]) -> Result<ChunkId> {
        let chunk_id = hash(data);
        
        // Check if already exists
        if self.db.contains_key(&chunk_id)? {
            return Ok(chunk_id);
        }
        
        // Store in database
        self.db.insert(&chunk_id, data)?;
        
        // Update stats
        self.chunk_count.fetch_add(1, Ordering::Relaxed);
        self.total_size.fetch_add(data.len() as u64, Ordering::Relaxed);
        
        // Add to cache
        self.cache_put(chunk_id, data.to_vec());
        
        Ok(chunk_id)
    }

    /// Get a chunk by ID
    pub fn get(&self, chunk_id: &ChunkId) -> Result<Option<Vec<u8>>> {
        // Check cache first
        if let Some(data) = self.cache.read().get(chunk_id) {
            return Ok(Some(data.clone()));
        }
        
        // Load from database
        match self.db.get(chunk_id)? {
            Some(data) => {
                let data = data.to_vec();
                self.cache_put(*chunk_id, data.clone());
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }

    /// Check if a chunk exists
    pub fn contains(&self, chunk_id: &ChunkId) -> Result<bool> {
        if self.cache.read().contains_key(chunk_id) {
            return Ok(true);
        }
        Ok(self.db.contains_key(chunk_id)?)
    }

    /// Get multiple chunks, returning found and missing
    pub fn get_many(&self, chunk_ids: &[ChunkId]) -> Result<(Vec<(ChunkId, Vec<u8>)>, Vec<ChunkId>)> {
        let mut found = Vec::new();
        let mut missing = Vec::new();
        
        for chunk_id in chunk_ids {
            match self.get(chunk_id)? {
                Some(data) => found.push((*chunk_id, data)),
                None => missing.push(*chunk_id),
            }
        }
        
        Ok((found, missing))
    }

    /// Get chunks we're missing from a list
    pub fn get_missing(&self, chunk_ids: &[ChunkId]) -> Result<Vec<ChunkId>> {
        let mut missing = Vec::new();
        for chunk_id in chunk_ids {
            if !self.contains(chunk_id)? {
                missing.push(*chunk_id);
            }
        }
        Ok(missing)
    }

    /// Delete a chunk
    pub fn delete(&self, chunk_id: &ChunkId) -> Result<bool> {
        if let Some(data) = self.db.remove(chunk_id)? {
            self.chunk_count.fetch_sub(1, Ordering::Relaxed);
            self.total_size.fetch_sub(data.len() as u64, Ordering::Relaxed);
            self.cache.write().remove(chunk_id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get chunk count
    pub fn count(&self) -> usize {
        self.chunk_count.load(Ordering::Relaxed)
    }

    /// Get total size in bytes
    pub fn total_size(&self) -> u64 {
        self.total_size.load(Ordering::Relaxed)
    }

    /// Flush to disk
    pub fn flush(&self) -> Result<()> {
        self.db.flush()?;
        Ok(())
    }

    /// Add to cache with simple eviction
    fn cache_put(&self, chunk_id: ChunkId, data: Vec<u8>) {
        let mut cache = self.cache.write();
        
        // Simple eviction: clear half when full
        if cache.len() >= self.cache_max_size {
            let to_remove: Vec<_> = cache.keys().take(self.cache_max_size / 2).cloned().collect();
            for key in to_remove {
                cache.remove(&key);
            }
        }
        
        cache.insert(chunk_id, data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_chunk_store() -> Result<()> {
        let dir = tempdir()?;
        let store = ChunkStore::new(dir.path())?;
        
        let data = b"hello grabnet";
        let chunk_id = store.put(data)?;
        
        // Verify content addressing
        assert_eq!(chunk_id, hash(data));
        
        // Retrieve
        let retrieved = store.get(&chunk_id)?.unwrap();
        assert_eq!(retrieved, data);
        
        // Contains
        assert!(store.contains(&chunk_id)?);
        assert!(!store.contains(&[0u8; 32])?);
        
        // Stats
        assert_eq!(store.count(), 1);
        assert_eq!(store.total_size(), data.len() as u64);
        
        Ok(())
    }

    #[test]
    fn test_deduplication() -> Result<()> {
        let dir = tempdir()?;
        let store = ChunkStore::new(dir.path())?;
        
        let data = b"duplicate content";
        let id1 = store.put(data)?;
        let id2 = store.put(data)?;
        
        // Same content = same ID
        assert_eq!(id1, id2);
        
        // Only stored once
        assert_eq!(store.count(), 1);
        
        Ok(())
    }
}
