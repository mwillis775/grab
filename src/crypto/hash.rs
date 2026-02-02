//! BLAKE3 hashing utilities

use crate::types::{ChunkId, SiteId, PublicKey};

/// Hash data using BLAKE3
#[inline]
pub fn hash(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

/// Hash multiple byte slices
pub fn hash_multi(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part);
    }
    *hasher.finalize().as_bytes()
}

/// Extension trait for SiteId operations
pub trait SiteIdExt {
    /// Generate a stable site ID from publisher key and site name
    fn generate(publisher: &PublicKey, name: &str) -> SiteId;
    
    /// Encode as base58 string
    fn to_base58(&self) -> String;
    
    /// Decode from base58 string
    fn from_base58(s: &str) -> Option<SiteId>;
}

impl SiteIdExt for SiteId {
    fn generate(publisher: &PublicKey, name: &str) -> SiteId {
        hash_multi(&[publisher, name.as_bytes()])
    }
    
    fn to_base58(&self) -> String {
        bs58::encode(self).into_string()
    }
    
    fn from_base58(s: &str) -> Option<SiteId> {
        let bytes = bs58::decode(s).into_vec().ok()?;
        if bytes.len() != 32 {
            return None;
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Some(arr)
    }
}

/// Encode bytes as base58
pub fn encode_base58(data: &[u8]) -> String {
    bs58::encode(data).into_string()
}

/// Decode base58 string to bytes
pub fn decode_base58(s: &str) -> Option<Vec<u8>> {
    bs58::decode(s).into_vec().ok()
}

/// Calculate chunk ID for data
#[inline]
pub fn chunk_id(data: &[u8]) -> ChunkId {
    hash(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash() {
        let data = b"hello world";
        let h = hash(data);
        assert_eq!(h.len(), 32);
        
        // Same input = same output
        assert_eq!(h, hash(data));
        
        // Different input = different output
        assert_ne!(h, hash(b"hello world!"));
    }

    #[test]
    fn test_site_id() {
        let publisher = [1u8; 32];
        let name = "my-site";
        
        let id = SiteId::generate(&publisher, name);
        
        // Stable: same inputs = same ID
        assert_eq!(id, SiteId::generate(&publisher, name));
        
        // Different name = different ID
        assert_ne!(id, SiteId::generate(&publisher, "other-site"));
        
        // Different publisher = different ID
        let other_publisher = [2u8; 32];
        assert_ne!(id, SiteId::generate(&other_publisher, name));
    }

    #[test]
    fn test_base58_roundtrip() {
        let data = [42u8; 32];
        let encoded = data.to_base58();
        let decoded = SiteId::from_base58(&encoded).unwrap();
        assert_eq!(data, decoded);
    }
}
