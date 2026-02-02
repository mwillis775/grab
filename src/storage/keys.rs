//! Ed25519 key management

use std::path::Path;
use anyhow::{Result, anyhow};

use crate::types::PublicKey;
use crate::crypto::{generate_keypair, encode_base58, SiteIdExt};

/// Key store for Ed25519 keypairs
pub struct KeyStore {
    db: sled::Db,
    /// Private keys tree
    private_keys: sled::Tree,
    /// Public keys tree (for quick lookup)
    public_keys: sled::Tree,
}

impl KeyStore {
    /// Create a new key store
    pub fn new(data_dir: &Path) -> Result<Self> {
        let db_path = data_dir.join("keys.db");
        let db = sled::open(&db_path)?;
        
        Ok(Self {
            private_keys: db.open_tree("private")?,
            public_keys: db.open_tree("public")?,
            db,
        })
    }

    /// Get or create a keypair by name
    pub fn get_or_create(&self, name: &str) -> Result<(PublicKey, [u8; 32])> {
        // Check if exists
        if let Some(private_key) = self.private_keys.get(name.as_bytes())? {
            let public_key = self.public_keys.get(name.as_bytes())?
                .ok_or_else(|| anyhow!("Corrupted key store: missing public key"))?;
            
            let mut priv_arr = [0u8; 32];
            let mut pub_arr = [0u8; 32];
            priv_arr.copy_from_slice(&private_key);
            pub_arr.copy_from_slice(&public_key);
            
            return Ok((pub_arr, priv_arr));
        }
        
        // Generate new keypair
        let (public_key, private_key) = generate_keypair();
        
        // Store
        self.private_keys.insert(name.as_bytes(), &private_key)?;
        self.public_keys.insert(name.as_bytes(), &public_key)?;
        
        Ok((public_key, private_key))
    }

    /// Get public key by name
    pub fn get_public_key(&self, name: &str) -> Result<Option<PublicKey>> {
        match self.public_keys.get(name.as_bytes())? {
            Some(data) => {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&data);
                Ok(Some(arr))
            }
            None => Ok(None),
        }
    }

    /// Get private key by name (use with caution)
    pub fn get_private_key(&self, name: &str) -> Result<Option<[u8; 32]>> {
        match self.private_keys.get(name.as_bytes())? {
            Some(data) => {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&data);
                Ok(Some(arr))
            }
            None => Ok(None),
        }
    }

    /// List all key names
    pub fn list_keys(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for result in self.public_keys.iter() {
            let (key, _) = result?;
            names.push(String::from_utf8_lossy(&key).to_string());
        }
        Ok(names)
    }

    /// Import a private key
    pub fn import(&self, name: &str, private_key: &[u8; 32]) -> Result<PublicKey> {
        // Derive public key
        let signing_key = ed25519_dalek::SigningKey::from_bytes(private_key);
        let public_key = signing_key.verifying_key().to_bytes();
        
        // Store
        self.private_keys.insert(name.as_bytes(), private_key)?;
        self.public_keys.insert(name.as_bytes(), &public_key)?;
        
        Ok(public_key)
    }

    /// Export a private key (returns base58 encoded)
    pub fn export(&self, name: &str) -> Result<Option<String>> {
        match self.get_private_key(name)? {
            Some(key) => Ok(Some(encode_base58(&key))),
            None => Ok(None),
        }
    }

    /// Delete a key
    pub fn delete(&self, name: &str) -> Result<bool> {
        let existed = self.private_keys.remove(name.as_bytes())?.is_some();
        self.public_keys.remove(name.as_bytes())?;
        Ok(existed)
    }

    /// Flush to disk
    pub fn flush(&self) -> Result<()> {
        self.db.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_key_creation() -> Result<()> {
        let dir = tempdir()?;
        let store = KeyStore::new(dir.path())?;
        
        // Get or create
        let (pub1, priv1) = store.get_or_create("default")?;
        
        // Should return same key
        let (pub2, priv2) = store.get_or_create("default")?;
        assert_eq!(pub1, pub2);
        assert_eq!(priv1, priv2);
        
        // Different name = different key
        let (pub3, _) = store.get_or_create("other")?;
        assert_ne!(pub1, pub3);
        
        Ok(())
    }

    #[test]
    fn test_key_import_export() -> Result<()> {
        let dir = tempdir()?;
        let store = KeyStore::new(dir.path())?;
        
        // Create a key
        let (_, original_private) = store.get_or_create("test")?;
        
        // Export
        let exported = store.export("test")?.unwrap();
        
        // Import to different name
        let dir2 = tempdir()?;
        let store2 = KeyStore::new(dir2.path())?;
        
        let mut private_bytes = [0u8; 32];
        private_bytes.copy_from_slice(&bs58::decode(&exported).into_vec()?);
        
        store2.import("imported", &private_bytes)?;
        
        // Verify
        let retrieved = store2.get_private_key("imported")?.unwrap();
        assert_eq!(retrieved, original_private);
        
        Ok(())
    }

    #[test]
    fn test_list_keys() -> Result<()> {
        let dir = tempdir()?;
        let store = KeyStore::new(dir.path())?;
        
        store.get_or_create("key1")?;
        store.get_or_create("key2")?;
        store.get_or_create("key3")?;
        
        let keys = store.list_keys()?;
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&"key1".to_string()));
        assert!(keys.contains(&"key2".to_string()));
        assert!(keys.contains(&"key3".to_string()));
        
        Ok(())
    }
}
