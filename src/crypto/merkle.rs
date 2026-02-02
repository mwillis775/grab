//! Merkle tree for content verification and delta sync

use crate::types::MerkleProof;
use super::hash;

/// A Merkle tree for content verification
#[derive(Debug, Clone)]
pub struct MerkleTree {
    leaves: Vec<[u8; 32]>,
    nodes: Vec<[u8; 32]>,
}

impl MerkleTree {
    /// Build a Merkle tree from leaf hashes
    pub fn new(leaves: Vec<[u8; 32]>) -> Self {
        if leaves.is_empty() {
            return Self {
                leaves: vec![],
                nodes: vec![[0u8; 32]],
            };
        }

        // Ensure power of 2 by padding with zeros
        let mut padded = leaves.clone();
        let next_pow2 = leaves.len().next_power_of_two();
        padded.resize(next_pow2, [0u8; 32]);

        // Build tree bottom-up
        let mut nodes = Vec::with_capacity(next_pow2 * 2);
        nodes.extend_from_slice(&padded);

        let mut level_size = next_pow2;
        let mut level_start = 0;

        while level_size > 1 {
            for i in (0..level_size).step_by(2) {
                let left = &nodes[level_start + i];
                let right = &nodes[level_start + i + 1];
                let parent = hash_pair(left, right);
                nodes.push(parent);
            }
            level_start += level_size;
            level_size /= 2;
        }

        Self { leaves, nodes }
    }

    /// Build a Merkle tree from raw data chunks
    pub fn from_chunks(chunks: &[&[u8]]) -> Self {
        let leaves: Vec<[u8; 32]> = chunks.iter().map(|c| hash(c)).collect();
        Self::new(leaves)
    }

    /// Get the root hash
    pub fn root(&self) -> [u8; 32] {
        *self.nodes.last().unwrap_or(&[0u8; 32])
    }

    /// Get a proof for a leaf at the given index
    pub fn get_proof(&self, index: usize) -> Option<MerkleProof> {
        if index >= self.leaves.len() {
            return None;
        }

        let padded_size = self.leaves.len().next_power_of_two();
        let mut siblings = Vec::new();
        let mut current_index = index;
        let mut level_start = 0;
        let mut level_size = padded_size;

        while level_size > 1 {
            let sibling_index = if current_index % 2 == 0 {
                current_index + 1
            } else {
                current_index - 1
            };

            if level_start + sibling_index < self.nodes.len() {
                siblings.push(self.nodes[level_start + sibling_index]);
            }

            current_index /= 2;
            level_start += level_size;
            level_size /= 2;
        }

        Some(MerkleProof {
            leaf_index: index,
            leaf_hash: self.leaves[index],
            siblings,
            root: self.root(),
        })
    }

    /// Verify a proof
    pub fn verify_proof(proof: &MerkleProof) -> bool {
        let mut current = proof.leaf_hash;
        let mut index = proof.leaf_index;

        for sibling in &proof.siblings {
            current = if index % 2 == 0 {
                hash_pair(&current, sibling)
            } else {
                hash_pair(sibling, &current)
            };
            index /= 2;
        }

        current == proof.root
    }

    /// Get leaves that differ between two trees
    pub fn diff(&self, other: &MerkleTree) -> Vec<usize> {
        let mut different = Vec::new();
        
        for (i, (a, b)) in self.leaves.iter().zip(other.leaves.iter()).enumerate() {
            if a != b {
                different.push(i);
            }
        }
        
        // Handle size differences
        if self.leaves.len() < other.leaves.len() {
            for i in self.leaves.len()..other.leaves.len() {
                different.push(i);
            }
        }
        
        different
    }

    /// Get number of leaves
    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    /// Check if tree is empty
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }
}

/// Hash two nodes together
#[inline]
fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut combined = [0u8; 64];
    combined[..32].copy_from_slice(left);
    combined[32..].copy_from_slice(right);
    hash(&combined)
}

/// Compute the content hash for a set of chunks
pub fn compute_content_hash(chunks: &[[u8; 32]]) -> [u8; 32] {
    let tree = MerkleTree::new(chunks.to_vec());
    tree.root()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merkle_tree() {
        let leaves = vec![
            hash(b"chunk1"),
            hash(b"chunk2"),
            hash(b"chunk3"),
            hash(b"chunk4"),
        ];

        let tree = MerkleTree::new(leaves.clone());
        assert_eq!(tree.len(), 4);
        
        // Root should be deterministic
        let tree2 = MerkleTree::new(leaves);
        assert_eq!(tree.root(), tree2.root());
    }

    #[test]
    fn test_merkle_proof() {
        let leaves = vec![
            hash(b"a"),
            hash(b"b"),
            hash(b"c"),
            hash(b"d"),
        ];

        let tree = MerkleTree::new(leaves);

        for i in 0..4 {
            let proof = tree.get_proof(i).unwrap();
            assert!(MerkleTree::verify_proof(&proof));
        }
    }

    #[test]
    fn test_merkle_diff() {
        let leaves1 = vec![hash(b"a"), hash(b"b"), hash(b"c")];
        let leaves2 = vec![hash(b"a"), hash(b"X"), hash(b"c")];

        let tree1 = MerkleTree::new(leaves1);
        let tree2 = MerkleTree::new(leaves2);

        let diff = tree1.diff(&tree2);
        assert_eq!(diff, vec![1]); // Only index 1 differs
    }
}
