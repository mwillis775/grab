//! Bootstrap node discovery and management

use serde::{Deserialize, Serialize};
use std::path::Path;
use anyhow::Result;

/// Known bootstrap nodes for GrabNet
/// These are the initial nodes new peers connect to for network discovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapConfig {
    /// Official GrabNet bootstrap nodes
    pub official: Vec<BootstrapNode>,
    /// Community-contributed bootstrap nodes
    pub community: Vec<BootstrapNode>,
    /// User-added custom bootstrap nodes
    pub custom: Vec<BootstrapNode>,
    /// Enable local network discovery (mDNS)
    pub mdns_enabled: bool,
    /// Minimum peers before trying more bootstrap nodes
    pub min_peers: usize,
    /// Maximum bootstrap connection attempts
    pub max_attempts: usize,
}

/// A bootstrap node entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapNode {
    /// Human-readable name
    pub name: String,
    /// Multiaddress(es) for this node
    pub addresses: Vec<String>,
    /// Peer ID (for verification)
    pub peer_id: Option<String>,
    /// Geographic region hint
    pub region: Option<String>,
    /// Is this node currently enabled?
    pub enabled: bool,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            official: vec![
                // Primary US bootstrap node
                BootstrapNode {
                    name: "grabnet-us-east".to_string(),
                    addresses: vec![
                        "/dns4/bootstrap-us.grabnet.io/tcp/4001".to_string(),
                    ],
                    peer_id: None, // Will be set when deployed
                    region: Some("us-east".to_string()),
                    enabled: true,
                },
                // Primary EU bootstrap node
                BootstrapNode {
                    name: "grabnet-eu-west".to_string(),
                    addresses: vec![
                        "/dns4/bootstrap-eu.grabnet.io/tcp/4001".to_string(),
                    ],
                    peer_id: None,
                    region: Some("eu-west".to_string()),
                    enabled: true,
                },
            ],
            community: vec![],
            custom: vec![],
            mdns_enabled: true,
            min_peers: 3,
            max_attempts: 10,
        }
    }
}

impl BootstrapConfig {
    /// Load bootstrap config from a file, or create default
    pub fn load_or_default(data_dir: &Path) -> Result<Self> {
        let config_path = data_dir.join("bootstrap.json");
        
        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)?;
            Ok(serde_json::from_str(&contents)?)
        } else {
            let config = Self::default();
            config.save(data_dir)?;
            Ok(config)
        }
    }

    /// Save bootstrap config
    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let config_path = data_dir.join("bootstrap.json");
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(config_path, contents)?;
        Ok(())
    }

    /// Get all enabled bootstrap addresses
    pub fn get_enabled_addresses(&self) -> Vec<String> {
        let mut addresses = Vec::new();
        
        for node in &self.official {
            if node.enabled {
                addresses.extend(node.addresses.clone());
            }
        }
        
        for node in &self.community {
            if node.enabled {
                addresses.extend(node.addresses.clone());
            }
        }
        
        for node in &self.custom {
            if node.enabled {
                addresses.extend(node.addresses.clone());
            }
        }
        
        addresses
    }

    /// Add a custom bootstrap node
    pub fn add_custom(&mut self, name: String, addresses: Vec<String>) {
        self.custom.push(BootstrapNode {
            name,
            addresses,
            peer_id: None,
            region: None,
            enabled: true,
        });
    }

    /// Remove a custom bootstrap node by name
    pub fn remove_custom(&mut self, name: &str) -> bool {
        let initial_len = self.custom.len();
        self.custom.retain(|n| n.name != name);
        self.custom.len() < initial_len
    }

    /// List all bootstrap nodes
    pub fn list_all(&self) -> Vec<&BootstrapNode> {
        let mut all: Vec<&BootstrapNode> = Vec::new();
        all.extend(self.official.iter());
        all.extend(self.community.iter());
        all.extend(self.custom.iter());
        all
    }

    /// Get count of enabled nodes
    pub fn enabled_count(&self) -> usize {
        self.official.iter().filter(|n| n.enabled).count() +
        self.community.iter().filter(|n| n.enabled).count() +
        self.custom.iter().filter(|n| n.enabled).count()
    }
}

/// Well-known peer addresses for development/testing
pub const DEV_BOOTSTRAP_PEERS: &[&str] = &[
    // Local testing
    "/ip4/127.0.0.1/tcp/4001",
];

/// Check if an address is reachable (simple TCP connect test)
pub async fn check_reachable(addr: &str) -> bool {
    use tokio::net::TcpStream;
    use tokio::time::{timeout, Duration};
    
    // Extract host:port from multiaddr (simplified)
    let parts: Vec<&str> = addr.split('/').collect();
    if parts.len() < 5 {
        return false;
    }
    
    let host = parts[2];
    let port: u16 = match parts[4].parse() {
        Ok(p) => p,
        Err(_) => return false,
    };
    
    let connect_addr = format!("{}:{}", host, port);
    
    match timeout(Duration::from_secs(5), TcpStream::connect(&connect_addr)).await {
        Ok(Ok(_)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = BootstrapConfig::default();
        assert!(!config.official.is_empty());
        assert!(config.mdns_enabled);
    }

    #[test]
    fn test_add_remove_custom() {
        let mut config = BootstrapConfig::default();
        config.add_custom("test-node".to_string(), vec!["/ip4/1.2.3.4/tcp/4001".to_string()]);
        assert_eq!(config.custom.len(), 1);
        
        config.remove_custom("test-node");
        assert_eq!(config.custom.len(), 0);
    }
}
