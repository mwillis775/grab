//! Erasure coding for distributed chunk storage
//!
//! Splits content chunks into data + parity shards using Reed-Solomon coding.
//! Any `data_shards` out of `data_shards + parity_shards` total shards can
//! reconstruct the original chunk, reducing per-node storage while maintaining
//! redundancy.
//!
//! # Example
//!
//! With `data_shards = 4` and `parity_shards = 2`:
//! - A 256 KB chunk is split into 6 shards of ~64 KB each
//! - Each node stores 1-2 shards instead of the full 256 KB
//! - Any 4 of the 6 shards can reconstruct the original chunk
//! - Total network storage: 6 × 64 KB = 384 KB (1.5× vs 6× for full replication)

mod codec;
mod shards;

pub use codec::{ErasureCodec, ErasureConfig};
pub use shards::{Shard, ShardId, ShardStore};
