//! Reed-Solomon erasure encoding/decoding

use anyhow::{anyhow, Context, Result};
use reed_solomon_erasure::galois_8::ReedSolomon;
use serde::{Deserialize, Serialize};

use crate::crypto::hash;
use crate::types::ChunkId;

use super::shards::{Shard, ShardId};

/// Configuration for erasure coding
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErasureConfig {
    /// Number of data shards (minimum needed to reconstruct)
    pub data_shards: usize,
    /// Number of parity shards (extra redundancy)
    pub parity_shards: usize,
}

impl Default for ErasureConfig {
    fn default() -> Self {
        Self {
            // 4 data + 2 parity = 6 total shards
            // Any 4 can reconstruct. Storage overhead: 1.5× vs N× for full replication.
            data_shards: 4,
            parity_shards: 2,
        }
    }
}

impl ErasureConfig {
    /// Total number of shards
    pub fn total_shards(&self) -> usize {
        self.data_shards + self.parity_shards
    }

    /// Storage overhead ratio (total / data)
    pub fn overhead_ratio(&self) -> f64 {
        self.total_shards() as f64 / self.data_shards as f64
    }

    /// Create a config with custom shard counts
    pub fn new(data_shards: usize, parity_shards: usize) -> Result<Self> {
        if data_shards == 0 {
            return Err(anyhow!("data_shards must be > 0"));
        }
        if parity_shards == 0 {
            return Err(anyhow!("parity_shards must be > 0"));
        }
        if data_shards + parity_shards > 256 {
            return Err(anyhow!("total shards must be <= 256 for GF(2^8)"));
        }
        Ok(Self {
            data_shards,
            parity_shards,
        })
    }
}

/// Erasure encoder/decoder using Reed-Solomon
pub struct ErasureCodec {
    config: ErasureConfig,
    rs: ReedSolomon,
}

impl ErasureCodec {
    /// Create a new codec with the given configuration
    pub fn new(config: ErasureConfig) -> Result<Self> {
        let rs = ReedSolomon::new(config.data_shards, config.parity_shards)
            .map_err(|e| anyhow!("Failed to create Reed-Solomon codec: {:?}", e))?;
        Ok(Self { config, rs })
    }

    /// Create a codec with default parameters (4+2)
    pub fn default_codec() -> Result<Self> {
        Self::new(ErasureConfig::default())
    }

    /// Get the erasure config
    pub fn config(&self) -> &ErasureConfig {
        &self.config
    }

    /// Encode a chunk into data + parity shards.
    ///
    /// The input chunk is split into `data_shards` equal-sized pieces,
    /// padded to align, then `parity_shards` parity pieces are computed.
    ///
    /// Returns a Vec of `Shard` structs, each identified by (chunk_id, shard_index).
    pub fn encode(&self, chunk_id: &ChunkId, data: &[u8]) -> Result<Vec<Shard>> {
        let shard_size = self.shard_size(data.len());
        let total = self.config.total_shards();

        // Build shard buffers: pad data to fill exactly data_shards × shard_size
        let mut shard_data: Vec<Vec<u8>> = Vec::with_capacity(total);

        // Split original data into data shards (with zero-padding on last)
        for i in 0..self.config.data_shards {
            let start = i * shard_size;
            let end = std::cmp::min(start + shard_size, data.len());
            let mut shard = vec![0u8; shard_size];
            if start < data.len() {
                let copy_len = end - start;
                shard[..copy_len].copy_from_slice(&data[start..end]);
            }
            shard_data.push(shard);
        }

        // Add empty parity shards
        for _ in 0..self.config.parity_shards {
            shard_data.push(vec![0u8; shard_size]);
        }

        // Compute parity
        self.rs
            .encode(&mut shard_data)
            .map_err(|e| anyhow!("Erasure encode failed: {:?}", e))?;

        // Package as Shard structs
        let original_size = data.len() as u32;
        let shards: Vec<Shard> = shard_data
            .into_iter()
            .enumerate()
            .map(|(index, sdata)| {
                let shard_hash = hash(&sdata);
                Shard {
                    id: ShardId {
                        chunk_id: *chunk_id,
                        shard_index: index as u8,
                    },
                    data: sdata,
                    shard_hash,
                    is_parity: index >= self.config.data_shards,
                    original_chunk_size: original_size,
                    erasure_config: self.config,
                }
            })
            .collect();

        Ok(shards)
    }

    /// Decode (reconstruct) the original chunk from a subset of shards.
    ///
    /// Requires at least `data_shards` shards (any mix of data + parity).
    /// Missing shards should be represented as None in the input.
    pub fn decode(
        &self,
        shards: &mut Vec<Option<Vec<u8>>>,
        original_size: usize,
    ) -> Result<Vec<u8>> {
        if shards.len() != self.config.total_shards() {
            return Err(anyhow!(
                "Expected {} shards, got {}",
                self.config.total_shards(),
                shards.len()
            ));
        }

        // Count available shards
        let available = shards.iter().filter(|s| s.is_some()).count();
        if available < self.config.data_shards {
            return Err(anyhow!(
                "Need at least {} shards to reconstruct, only have {}",
                self.config.data_shards,
                available
            ));
        }

        // Reconstruct missing shards
        self.rs
            .reconstruct(shards)
            .map_err(|e| anyhow!("Erasure decode failed: {:?}", e))?;

        // Concatenate data shards and trim to original size
        let mut result = Vec::with_capacity(original_size);
        for shard in shards.iter().take(self.config.data_shards) {
            if let Some(data) = shard {
                result.extend_from_slice(data);
            } else {
                return Err(anyhow!("Reconstruction failed: data shard still missing"));
            }
        }

        // Trim padding
        result.truncate(original_size);

        Ok(result)
    }

    /// Compute shard size for a given chunk size.
    /// Each shard holds ceil(chunk_size / data_shards) bytes.
    pub fn shard_size(&self, chunk_size: usize) -> usize {
        (chunk_size + self.config.data_shards - 1) / self.config.data_shards
    }

    /// Verify a shard's integrity against its stored hash
    pub fn verify_shard(shard: &Shard) -> bool {
        let computed = hash(&shard.data);
        computed == shard.shard_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_full() {
        let codec = ErasureCodec::default_codec().unwrap();
        let chunk_id = [42u8; 32];
        let data = b"Hello, this is test data for erasure coding in GrabNet!";

        let shards = codec.encode(&chunk_id, data).unwrap();
        assert_eq!(shards.len(), 6); // 4 data + 2 parity

        // Reconstruct with all shards present
        let mut shard_data: Vec<Option<Vec<u8>>> =
            shards.iter().map(|s| Some(s.data.clone())).collect();

        let recovered = codec.decode(&mut shard_data, data.len()).unwrap();
        assert_eq!(&recovered, data);
    }

    #[test]
    fn test_decode_with_missing_shards() {
        let codec = ErasureCodec::default_codec().unwrap();
        let chunk_id = [7u8; 32];
        let data = vec![0xAB; 1024]; // 1 KB chunk

        let shards = codec.encode(&chunk_id, &data).unwrap();

        // Remove 2 shards (the parity budget) — should still reconstruct
        let mut shard_data: Vec<Option<Vec<u8>>> =
            shards.iter().map(|s| Some(s.data.clone())).collect();
        shard_data[1] = None; // Remove data shard 1
        shard_data[4] = None; // Remove parity shard 0

        let recovered = codec.decode(&mut shard_data, data.len()).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn test_decode_insufficient_shards() {
        let codec = ErasureCodec::default_codec().unwrap();
        let chunk_id = [99u8; 32];
        let data = b"short";

        let shards = codec.encode(&chunk_id, data).unwrap();

        // Remove 3 shards — only 3 remain, need 4
        let mut shard_data: Vec<Option<Vec<u8>>> =
            shards.iter().map(|s| Some(s.data.clone())).collect();
        shard_data[0] = None;
        shard_data[2] = None;
        shard_data[5] = None;

        let result = codec.decode(&mut shard_data, data.len());
        assert!(result.is_err());
    }

    #[test]
    fn test_shard_verification() {
        let codec = ErasureCodec::default_codec().unwrap();
        let chunk_id = [1u8; 32];
        let data = b"verify me";

        let shards = codec.encode(&chunk_id, data).unwrap();

        // All shards should verify
        for shard in &shards {
            assert!(ErasureCodec::verify_shard(shard));
        }

        // Tampered shard should not verify
        let mut bad_shard = shards[0].clone();
        bad_shard.data[0] ^= 0xFF;
        assert!(!ErasureCodec::verify_shard(&bad_shard));
    }

    #[test]
    fn test_large_chunk() {
        let codec = ErasureCodec::default_codec().unwrap();
        let chunk_id = [0u8; 32];
        let data = vec![0xCD; 256 * 1024]; // 256 KB (typical chunk size)

        let shards = codec.encode(&chunk_id, &data).unwrap();

        // Each shard should be 64 KB
        for shard in &shards {
            assert_eq!(shard.data.len(), 64 * 1024);
        }

        // Reconstruct with 2 missing
        let mut shard_data: Vec<Option<Vec<u8>>> =
            shards.iter().map(|s| Some(s.data.clone())).collect();
        shard_data[0] = None;
        shard_data[3] = None;

        let recovered = codec.decode(&mut shard_data, data.len()).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn test_custom_config() {
        let config = ErasureConfig::new(3, 3).unwrap();
        let codec = ErasureCodec::new(config).unwrap();
        let chunk_id = [5u8; 32];
        let data = vec![0xEF; 900];

        let shards = codec.encode(&chunk_id, &data).unwrap();
        assert_eq!(shards.len(), 6); // 3+3

        // Can tolerate losing 3 shards
        let mut shard_data: Vec<Option<Vec<u8>>> =
            shards.iter().map(|s| Some(s.data.clone())).collect();
        shard_data[0] = None;
        shard_data[2] = None;
        shard_data[4] = None;

        let recovered = codec.decode(&mut shard_data, data.len()).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn test_overhead_ratio() {
        let config = ErasureConfig::default();
        assert!((config.overhead_ratio() - 1.5).abs() < f64::EPSILON);

        let config2 = ErasureConfig::new(4, 4).unwrap();
        assert!((config2.overhead_ratio() - 2.0).abs() < f64::EPSILON);
    }
}
