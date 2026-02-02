//! Core types for GrabNet

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 32-byte site identifier: blake3(publisher_key || site_name)
pub type SiteId = [u8; 32];

/// 32-byte chunk identifier: blake3(chunk_data)
pub type ChunkId = [u8; 32];

/// Ed25519 public key
pub type PublicKey = [u8; 32];

/// Ed25519 signature (64 bytes, stored as Vec for serde compatibility)
pub type Signature = Vec<u8>;

/// A website packaged for decentralized hosting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebBundle {
    /// Stable site identifier
    pub site_id: SiteId,
    /// Human-readable name
    pub name: String,
    /// Revision number (auto-increments)
    pub revision: u64,
    /// Merkle root of all content
    pub root_hash: [u8; 32],
    /// Publisher's public key
    pub publisher: PublicKey,
    /// Signature over bundle metadata
    #[serde(with = "serde_bytes")]
    pub signature: Signature,
    /// Site manifest with file listings
    pub manifest: SiteManifest,
    /// Creation timestamp (unix ms)
    pub created_at: u64,
}

/// Site manifest containing file structure and routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteManifest {
    /// All files in the site
    pub files: Vec<FileEntry>,
    /// Entry point (usually "index.html")
    pub entry: String,
    /// Routing configuration
    pub routes: Option<RouteConfig>,
    /// Custom headers
    pub headers: Option<Vec<HeaderRule>>,
}

/// A single file in the site
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relative path from site root
    pub path: String,
    /// Content hash
    pub hash: ChunkId,
    /// Original file size
    pub size: u64,
    /// MIME type
    pub mime_type: String,
    /// Chunk IDs that make up this file
    pub chunks: Vec<ChunkId>,
    /// Compression applied
    pub compression: Option<Compression>,
}

/// Compression method
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    None,
    Gzip,
    Brotli,
}

/// Routing configuration for SPAs and clean URLs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    /// Enable clean URLs (serve /about as /about.html)
    #[serde(default)]
    pub clean_urls: bool,
    /// SPA fallback path
    pub fallback: Option<String>,
    /// Redirect rules
    #[serde(default)]
    pub redirects: Vec<RedirectRule>,
    /// Rewrite rules
    #[serde(default)]
    pub rewrites: Vec<RewriteRule>,
}

/// HTTP redirect rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectRule {
    pub source: String,
    pub destination: String,
    #[serde(default = "default_redirect_status")]
    pub status: u16,
}

fn default_redirect_status() -> u16 {
    301
}

/// URL rewrite rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteRule {
    pub source: String,
    pub destination: String,
}

/// Custom header rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderRule {
    pub source: String,
    pub headers: Vec<(String, String)>,
}

/// A published site (owned by this node)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishedSite {
    pub site_id: SiteId,
    pub name: String,
    pub revision: u64,
    pub root_path: PathBuf,
    pub created_at: u64,
    pub updated_at: u64,
}

/// A hosted site (pinned by this node)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostedSite {
    pub site_id: SiteId,
    pub name: String,
    pub revision: u64,
    pub hosted_at: u64,
    pub last_accessed: u64,
    pub access_count: u64,
}

/// Node configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub network: NetworkConfig,
    pub gateway: GatewayConfig,
    pub storage: StorageConfig,
    pub publisher: PublisherConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// TCP listen port
    #[serde(default = "default_network_port")]
    pub port: u16,
    /// Listen addresses
    #[serde(default = "default_listen_addresses")]
    pub listen_addresses: Vec<String>,
    /// Bootstrap peers
    #[serde(default)]
    pub bootstrap_peers: Vec<String>,
    /// Maximum connections
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
}

fn default_network_port() -> u16 {
    4001
}

fn default_listen_addresses() -> Vec<String> {
    vec![
        "/ip4/0.0.0.0/tcp/4001".to_string(),
        "/ip6/::/tcp/4001".to_string(),
    ]
}

fn default_max_connections() -> usize {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// HTTP port
    #[serde(default = "default_gateway_port")]
    pub port: u16,
    /// Bind address
    #[serde(default = "default_gateway_host")]
    pub host: String,
    /// Enable CORS
    #[serde(default = "default_true")]
    pub cors: bool,
}

fn default_gateway_port() -> u16 {
    8080
}

fn default_gateway_host() -> String {
    "127.0.0.1".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Chunk cache size in MB
    #[serde(default = "default_cache_size")]
    pub cache_size_mb: usize,
    /// Maximum storage in GB (0 = unlimited)
    #[serde(default)]
    pub max_storage_gb: usize,
}

fn default_cache_size() -> usize {
    256
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublisherConfig {
    /// Default chunk size in bytes
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    /// Enable compression
    #[serde(default = "default_true")]
    pub compress: bool,
}

fn default_chunk_size() -> usize {
    256 * 1024 // 256 KB
}

impl Default for Config {
    fn default() -> Self {
        Self {
            network: NetworkConfig {
                port: default_network_port(),
                listen_addresses: default_listen_addresses(),
                bootstrap_peers: vec![],
                max_connections: default_max_connections(),
            },
            gateway: GatewayConfig {
                port: default_gateway_port(),
                host: default_gateway_host(),
                cors: true,
            },
            storage: StorageConfig {
                cache_size_mb: default_cache_size(),
                max_storage_gb: 0,
            },
            publisher: PublisherConfig {
                chunk_size: default_chunk_size(),
                compress: true,
            },
        }
    }
}

impl Config {
    /// Load config from data directory or create default
    pub fn load_or_default(data_dir: &std::path::Path) -> anyhow::Result<Self> {
        let config_path = data_dir.join("config.json");
        
        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)?;
            Ok(serde_json::from_str(&contents)?)
        } else {
            let config = Self::default();
            let contents = serde_json::to_string_pretty(&config)?;
            std::fs::write(&config_path, contents)?;
            Ok(config)
        }
    }
    
    /// Save config to data directory
    pub fn save(&self, data_dir: &std::path::Path) -> anyhow::Result<()> {
        let config_path = data_dir.join("config.json");
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(config_path, contents)?;
        Ok(())
    }
}

/// Protocol message types for P2P communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GrabRequest {
    /// Find nodes hosting a site
    FindSite { site_id: SiteId },
    /// Get site manifest
    GetManifest { site_id: SiteId },
    /// Get chunks by ID
    GetChunks { chunk_ids: Vec<ChunkId> },
    /// Announce we're hosting a site
    Announce { site_id: SiteId, revision: u64 },
    /// Push an update to hosts
    PushUpdate { bundle: Box<WebBundle> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GrabResponse {
    /// List of peers hosting a site
    SiteHosts { hosts: Vec<PeerRecord> },
    /// Site manifest
    Manifest { bundle: Box<WebBundle> },
    /// Requested chunks
    Chunks { chunks: Vec<(ChunkId, Vec<u8>)> },
    /// Acknowledgment
    Ack,
    /// Error
    Error { message: String },
}

/// Information about a peer hosting a site
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRecord {
    pub peer_id: String,
    pub addresses: Vec<String>,
    pub revision: u64,
}

/// Merkle proof for content verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProof {
    pub leaf_index: usize,
    pub leaf_hash: [u8; 32],
    pub siblings: Vec<[u8; 32]>,
    pub root: [u8; 32],
}
