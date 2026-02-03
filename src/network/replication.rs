//! Content replication and redundancy management
//!
//! Provides policies and mechanisms for ensuring content availability
//! across multiple peers in the network.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use parking_lot::RwLock;
use tokio::sync::mpsc;

use crate::types::{SiteId, ChunkId};
use crate::crypto::SiteIdExt;

/// Replication policy for a site
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationPolicy {
    /// Minimum number of peers that should host this site
    pub min_replicas: usize,
    /// Maximum number of peers to replicate to
    pub max_replicas: usize,
    /// Priority level (higher = more important)
    pub priority: u8,
    /// Auto-announce interval in seconds
    pub announce_interval_secs: u64,
    /// Whether to actively seek more replicas if below minimum
    pub auto_replicate: bool,
}

impl Default for ReplicationPolicy {
    fn default() -> Self {
        Self {
            min_replicas: 3,
            max_replicas: 10,
            priority: 5,
            announce_interval_secs: 3600, // 1 hour
            auto_replicate: true,
        }
    }
}

/// Health status for a site
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteHealth {
    pub site_id: SiteId,
    pub known_hosts: usize,
    pub verified_hosts: usize,
    pub last_check: u64,
    pub status: HealthStatus,
    pub missing_chunks: Vec<ChunkId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// All chunks available, sufficient replicas
    Healthy,
    /// Some chunks missing or below min replicas
    Degraded,
    /// Critical - very few replicas or missing chunks
    Critical,
    /// Unknown - not yet checked
    Unknown,
}

/// Replication manager tracks site health and manages replication
pub struct ReplicationManager {
    /// Replication policies by site ID
    policies: Arc<RwLock<HashMap<SiteId, ReplicationPolicy>>>,
    /// Known hosts for each site
    site_hosts: Arc<RwLock<HashMap<SiteId, HashSet<String>>>>,
    /// Health status cache
    health_cache: Arc<RwLock<HashMap<SiteId, SiteHealth>>>,
    /// Last announcement time for each site
    last_announce: Arc<RwLock<HashMap<SiteId, Instant>>>,
}

impl ReplicationManager {
    pub fn new() -> Self {
        Self {
            policies: Arc::new(RwLock::new(HashMap::new())),
            site_hosts: Arc::new(RwLock::new(HashMap::new())),
            health_cache: Arc::new(RwLock::new(HashMap::new())),
            last_announce: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set replication policy for a site
    pub fn set_policy(&self, site_id: SiteId, policy: ReplicationPolicy) {
        self.policies.write().insert(site_id, policy);
    }

    /// Get replication policy for a site
    pub fn get_policy(&self, site_id: &SiteId) -> ReplicationPolicy {
        self.policies
            .read()
            .get(site_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Record a host for a site
    pub fn add_host(&self, site_id: SiteId, peer_id: String) {
        self.site_hosts
            .write()
            .entry(site_id)
            .or_default()
            .insert(peer_id);
    }

    /// Remove a host for a site
    pub fn remove_host(&self, site_id: &SiteId, peer_id: &str) {
        if let Some(hosts) = self.site_hosts.write().get_mut(site_id) {
            hosts.remove(peer_id);
        }
    }

    /// Get known hosts for a site
    pub fn get_hosts(&self, site_id: &SiteId) -> Vec<String> {
        self.site_hosts
            .read()
            .get(site_id)
            .map(|h| h.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get health status for a site
    pub fn get_health(&self, site_id: &SiteId) -> Option<SiteHealth> {
        self.health_cache.read().get(site_id).cloned()
    }

    /// Update health status for a site
    pub fn update_health(&self, health: SiteHealth) {
        self.health_cache.write().insert(health.site_id, health);
    }

    /// Check if a site needs replication
    pub fn needs_replication(&self, site_id: &SiteId) -> bool {
        let policy = self.get_policy(site_id);
        if !policy.auto_replicate {
            return false;
        }

        let hosts = self.site_hosts.read();
        let host_count = hosts.get(site_id).map(|h| h.len()).unwrap_or(0);
        
        host_count < policy.min_replicas
    }

    /// Check if a site should be announced
    pub fn should_announce(&self, site_id: &SiteId) -> bool {
        let policy = self.get_policy(site_id);
        let interval = Duration::from_secs(policy.announce_interval_secs);
        
        let last = self.last_announce.read().get(site_id).copied();
        match last {
            Some(t) => t.elapsed() >= interval,
            None => true,
        }
    }

    /// Record that we announced a site
    pub fn record_announce(&self, site_id: SiteId) {
        self.last_announce.write().insert(site_id, Instant::now());
    }

    /// Get sites that need attention (below min replicas or unhealthy)
    pub fn get_sites_needing_attention(&self) -> Vec<(SiteId, SiteHealth)> {
        let mut result = Vec::new();
        
        for (site_id, health) in self.health_cache.read().iter() {
            if health.status != HealthStatus::Healthy {
                result.push((*site_id, health.clone()));
            }
        }
        
        // Also check sites below min replicas
        let policies = self.policies.read();
        let hosts = self.site_hosts.read();
        
        for (site_id, policy) in policies.iter() {
            let host_count = hosts.get(site_id).map(|h| h.len()).unwrap_or(0);
            if host_count < policy.min_replicas {
                // Add if not already in result
                if !result.iter().any(|(id, _)| id == site_id) {
                    result.push((*site_id, SiteHealth {
                        site_id: *site_id,
                        known_hosts: host_count,
                        verified_hosts: 0,
                        last_check: 0,
                        status: HealthStatus::Degraded,
                        missing_chunks: vec![],
                    }));
                }
            }
        }
        
        result
    }

    /// Get replication stats
    pub fn get_stats(&self) -> ReplicationStats {
        let policies = self.policies.read();
        let hosts = self.site_hosts.read();
        let health = self.health_cache.read();
        
        let total_sites = policies.len();
        let mut healthy = 0;
        let mut degraded = 0;
        let mut critical = 0;
        let mut total_replicas = 0;
        
        for site_id in policies.keys() {
            let host_count = hosts.get(site_id).map(|h| h.len()).unwrap_or(0);
            total_replicas += host_count;
            
            if let Some(h) = health.get(site_id) {
                match h.status {
                    HealthStatus::Healthy => healthy += 1,
                    HealthStatus::Degraded => degraded += 1,
                    HealthStatus::Critical => critical += 1,
                    HealthStatus::Unknown => {}
                }
            }
        }
        
        ReplicationStats {
            total_sites,
            healthy_sites: healthy,
            degraded_sites: degraded,
            critical_sites: critical,
            total_replicas,
            average_replicas: if total_sites > 0 {
                total_replicas as f64 / total_sites as f64
            } else {
                0.0
            },
        }
    }
}

impl Default for ReplicationManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Replication statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationStats {
    pub total_sites: usize,
    pub healthy_sites: usize,
    pub degraded_sites: usize,
    pub critical_sites: usize,
    pub total_replicas: usize,
    pub average_replicas: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replication_policy() {
        let manager = ReplicationManager::new();
        let site_id = [0u8; 32];
        
        // Default policy
        let policy = manager.get_policy(&site_id);
        assert_eq!(policy.min_replicas, 3);
        
        // Custom policy
        manager.set_policy(site_id, ReplicationPolicy {
            min_replicas: 5,
            max_replicas: 20,
            ..Default::default()
        });
        
        let policy = manager.get_policy(&site_id);
        assert_eq!(policy.min_replicas, 5);
    }

    #[test]
    fn test_host_tracking() {
        let manager = ReplicationManager::new();
        let site_id = [1u8; 32];
        
        manager.add_host(site_id, "peer1".to_string());
        manager.add_host(site_id, "peer2".to_string());
        
        let hosts = manager.get_hosts(&site_id);
        assert_eq!(hosts.len(), 2);
        
        manager.remove_host(&site_id, "peer1");
        let hosts = manager.get_hosts(&site_id);
        assert_eq!(hosts.len(), 1);
    }

    #[test]
    fn test_needs_replication() {
        let manager = ReplicationManager::new();
        let site_id = [2u8; 32];
        
        manager.set_policy(site_id, ReplicationPolicy {
            min_replicas: 3,
            auto_replicate: true,
            ..Default::default()
        });
        
        // No hosts - needs replication
        assert!(manager.needs_replication(&site_id));
        
        // Add hosts
        manager.add_host(site_id, "peer1".to_string());
        manager.add_host(site_id, "peer2".to_string());
        assert!(manager.needs_replication(&site_id)); // Still need 1 more
        
        manager.add_host(site_id, "peer3".to_string());
        assert!(!manager.needs_replication(&site_id)); // Met minimum
    }
}
