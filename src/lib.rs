//! # GrabNet
//!
//! A decentralized web hosting protocol, purpose-built for websites.
//!
//! Unlike IPFS which is a general-purpose content-addressed storage system,
//! GrabNet is specifically designed for hosting websites with:
//!
//! - **Stable site IDs** - Addresses don't change on updates
//! - **Delta sync** - Only transfer changed content
//! - **Built-in HTTP gateway** - Serve sites over standard HTTP
//! - **User uploads** - Support for user-generated content
//! - **Fast name resolution** - <100ms via DHT
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use grabnet::{Grab, PublishOptions};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Create a GrabNet instance
//!     let grab = Grab::new(None).await?;
//!
//!     // Publish a website
//!     let result = grab.publish("./my-website", PublishOptions::default()).await?;
//!     println!("Published to: grab://{}", result.site_id);
//!
//!     // Start the HTTP gateway
//!     grab.start_gateway().await?;
//!
//!     Ok(())
//! }
//! ```

pub mod types;
pub mod crypto;
pub mod storage;
pub mod network;
pub mod gateway;
pub mod content;
pub mod publisher;

// Re-export main types
pub use types::*;
pub use crypto::{hash, sign, verify, generate_keypair, SiteIdExt, encode_base58, decode_base58};
pub use storage::{ChunkStore, BundleStore, KeyStore};
pub use network::GrabNetwork;
pub use gateway::Gateway;
pub use content::UserContentManager;
pub use publisher::{Publisher, PublishOptions, PublishResult};

use std::path::PathBuf;
use std::sync::Arc;
use anyhow::Result;
use parking_lot::RwLock;

/// Main GrabNet SDK
pub struct Grab {
    config: Config,
    data_dir: PathBuf,
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    key_store: Arc<KeyStore>,
    publisher: Publisher,
    network: Arc<RwLock<Option<GrabNetwork>>>,
    gateway: Arc<RwLock<Option<Gateway>>>,
    content_manager: Arc<RwLock<Option<UserContentManager>>>,
}

impl Grab {
    /// Create a new GrabNet instance
    pub async fn new(data_dir: Option<PathBuf>) -> Result<Self> {
        let data_dir = data_dir.unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".grab")
        });

        std::fs::create_dir_all(&data_dir)?;

        let config = Config::load_or_default(&data_dir)?;
        let chunk_store = Arc::new(ChunkStore::new(&data_dir)?);
        let bundle_store = Arc::new(BundleStore::new(&data_dir)?);
        let key_store = Arc::new(KeyStore::new(&data_dir)?);

        let publisher = Publisher::new(
            chunk_store.clone(),
            bundle_store.clone(),
            key_store.clone(),
        );

        Ok(Self {
            config,
            data_dir,
            chunk_store,
            bundle_store,
            key_store,
            publisher,
            network: Arc::new(RwLock::new(None)),
            gateway: Arc::new(RwLock::new(None)),
            content_manager: Arc::new(RwLock::new(None)),
        })
    }

    /// Create with user uploads enabled
    pub async fn with_uploads(data_dir: Option<PathBuf>) -> Result<Self> {
        let mut grab = Self::new(data_dir).await?;
        let manager = UserContentManager::new(grab.chunk_store.clone());
        *grab.content_manager.write() = Some(manager);
        Ok(grab)
    }

    // =========================================================================
    // Publishing
    // =========================================================================

    /// Publish a website directory
    pub async fn publish(&self, path: &str, options: PublishOptions) -> Result<PublishResult> {
        let result = self.publisher.publish(path, options).await?;

        // Announce to network if running
        if let Some(network) = self.network.read().as_ref() {
            network.announce_site(&result.bundle.site_id, result.bundle.revision).await?;
        }

        Ok(result)
    }

    /// Update an existing site
    pub async fn update(&self, site_id_or_name: &str) -> Result<Option<PublishResult>> {
        let site = match self.bundle_store.get_published_site(site_id_or_name)? {
            Some(s) => s,
            None => return Ok(None),
        };

        let result = self.publisher.publish(
            site.root_path.to_str().unwrap_or(""),
            PublishOptions {
                name: Some(site.name.clone()),
                ..Default::default()
            },
        ).await?;

        // Push update to network
        if let Some(network) = self.network.read().as_ref() {
            let hosts = network.push_update(&result.bundle).await?;
            tracing::info!("Update pushed to {} hosts", hosts);
        }

        Ok(Some(result))
    }

    /// List published sites
    pub fn list_published(&self) -> Result<Vec<PublishedSite>> {
        self.bundle_store.get_all_published_sites()
    }

    // =========================================================================
    // Hosting
    // =========================================================================

    /// Host (pin) a site locally
    pub async fn host(&self, site_id: &SiteId) -> Result<bool> {
        let bundle = match self.bundle_store.get_bundle(site_id)? {
            Some(b) => b,
            None => {
                // Try to fetch from network
                if let Some(network) = self.network.read().as_ref() {
                    match network.fetch_site(site_id).await? {
                        Some(b) => b,
                        None => return Ok(false),
                    }
                } else {
                    return Ok(false);
                }
            }
        };

        self.bundle_store.save_hosted_site(&bundle)?;

        // Announce to network
        if let Some(network) = self.network.read().as_ref() {
            network.announce_site(site_id, bundle.revision).await?;
        }

        Ok(true)
    }

    /// List hosted sites
    pub fn list_hosted(&self) -> Result<Vec<HostedSite>> {
        self.bundle_store.get_all_hosted_sites()
    }

    // =========================================================================
    // Network
    // =========================================================================

    /// Start the P2P network
    pub async fn start_network(&self) -> Result<()> {
        if self.network.read().is_some() {
            return Ok(());
        }

        let network = GrabNetwork::new(
            &self.config,
            self.chunk_store.clone(),
            self.bundle_store.clone(),
        ).await?;

        network.start().await?;

        // Announce all our sites
        let published = self.bundle_store.get_all_published_sites()?;
        let hosted = self.bundle_store.get_all_hosted_sites()?;

        for site in published {
            network.announce_site(&site.site_id, site.revision).await?;
        }
        for site in hosted {
            network.announce_site(&site.site_id, site.revision).await?;
        }

        *self.network.write() = Some(network);
        Ok(())
    }

    /// Stop the network
    pub async fn stop_network(&self) -> Result<()> {
        if let Some(network) = self.network.write().take() {
            network.stop().await?;
        }
        Ok(())
    }

    /// Get network status
    pub fn network_status(&self) -> NetworkStatus {
        match self.network.read().as_ref() {
            Some(network) => NetworkStatus {
                running: true,
                peer_id: Some(network.peer_id().to_string()),
                peers: network.connected_peers(),
                addresses: network.listen_addresses(),
            },
            None => NetworkStatus::default(),
        }
    }

    // =========================================================================
    // Gateway
    // =========================================================================

    /// Start the HTTP gateway
    pub async fn start_gateway(&self) -> Result<()> {
        self.start_gateway_on_port(self.config.gateway.port).await
    }

    /// Start the HTTP gateway on a specific port
    pub async fn start_gateway_on_port(&self, port: u16) -> Result<()> {
        if self.gateway.read().is_some() {
            return Ok(());
        }

        let mut config = self.config.clone();
        config.gateway.port = port;

        let gateway = Gateway::new(
            &config,
            self.chunk_store.clone(),
            self.bundle_store.clone(),
            self.content_manager.read().clone(),
        );

        gateway.start().await?;
        *self.gateway.write() = Some(gateway);
        Ok(())
    }

    /// Stop the gateway
    pub async fn stop_gateway(&self) -> Result<()> {
        if let Some(gateway) = self.gateway.write().take() {
            gateway.stop().await?;
        }
        Ok(())
    }

    // =========================================================================
    // User Content
    // =========================================================================

    /// Enable user uploads for a site
    pub fn enable_uploads(&self, site_id: &SiteId, policy: content::UploadPolicy) -> Result<()> {
        let mut manager_lock = self.content_manager.write();
        if manager_lock.is_none() {
            *manager_lock = Some(UserContentManager::new(self.chunk_store.clone()));
        }
        if let Some(manager) = manager_lock.as_mut() {
            manager.set_policy(site_id, policy);
        }
        Ok(())
    }

    /// Upload content to a site
    pub async fn upload_content(
        &self,
        site_id: &SiteId,
        filename: &str,
        mime_type: &str,
        data: &[u8],
    ) -> Result<Option<content::UserUpload>> {
        let manager_lock = self.content_manager.read();
        match manager_lock.as_ref() {
            Some(manager) => manager.upload(site_id, filename, mime_type, data, None),
            None => Ok(None),
        }
    }

    /// List uploads for a site
    pub fn list_uploads(&self, site_id: &SiteId) -> Vec<content::UserUpload> {
        self.content_manager
            .read()
            .as_ref()
            .map(|m| m.list_site_uploads(site_id))
            .unwrap_or_default()
    }

    // =========================================================================
    // Keys
    // =========================================================================

    /// List all key names
    pub fn list_keys(&self) -> Result<Vec<String>> {
        self.key_store.list_keys()
    }

    /// Get public key for a key name
    pub fn get_public_key(&self, name: &str) -> Result<Option<PublicKey>> {
        self.key_store.get_public_key(name)
    }

    // =========================================================================
    // Storage Stats
    // =========================================================================

    /// Get storage statistics
    pub fn storage_stats(&self) -> StorageStats {
        StorageStats {
            chunks: self.chunk_store.count(),
            total_size: self.chunk_store.total_size(),
            published_sites: self.bundle_store.get_all_published_sites().unwrap_or_default().len(),
            hosted_sites: self.bundle_store.get_all_hosted_sites().unwrap_or_default().len(),
        }
    }

    /// Get the config
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get chunk store reference
    pub fn chunk_store(&self) -> &Arc<ChunkStore> {
        &self.chunk_store
    }

    /// Get bundle store reference
    pub fn bundle_store(&self) -> &Arc<BundleStore> {
        &self.bundle_store
    }
}

/// Network status information
#[derive(Debug, Default)]
pub struct NetworkStatus {
    pub running: bool,
    pub peer_id: Option<String>,
    pub peers: usize,
    pub addresses: Vec<String>,
}

/// Storage statistics
#[derive(Debug, Default)]
pub struct StorageStats {
    pub chunks: usize,
    pub total_size: u64,
    pub published_sites: usize,
    pub hosted_sites: usize,
}

#[cfg(test)]
mod serialization_tests {
    use super::*;
    use types::{SiteManifest, FileEntry, Compression};
    
    #[test]
    fn test_manifest_roundtrip() {
        let manifest = SiteManifest {
            files: vec![FileEntry {
                path: "index.html".to_string(),
                hash: [0u8; 32],
                size: 100,
                mime_type: "text/html".to_string(),
                chunks: vec![[1u8; 32]],
                compression: Some(Compression::Gzip),
            }],
            entry: "index.html".to_string(),
            routes: None,
            headers: None,
        };
        
        let encoded = bincode::serialize(&manifest).unwrap();
        println!("Encoded manifest: {} bytes", encoded.len());
        println!("Raw bytes: {:?}", &encoded[..encoded.len().min(50)]);
        
        let decoded: SiteManifest = bincode::deserialize(&encoded).unwrap();
        assert_eq!(decoded.entry, manifest.entry);
        assert_eq!(decoded.files.len(), manifest.files.len());
    }
}
