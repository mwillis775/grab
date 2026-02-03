//! Network health monitoring and peer scoring
//!
//! Tracks connection health, peer reliability, and network metrics.

use std::collections::HashMap;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use parking_lot::RwLock;

/// Peer score tracking for reputation-based prioritization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerScore {
    /// Peer ID
    pub peer_id: String,
    /// Total requests made to this peer
    pub total_requests: u64,
    /// Successful responses
    pub successful_responses: u64,
    /// Failed requests
    pub failed_requests: u64,
    /// Average response time in milliseconds
    pub avg_response_time_ms: u64,
    /// Last seen timestamp (unix millis)
    pub last_seen: u64,
    /// Bytes received from this peer
    pub bytes_received: u64,
    /// Bytes sent to this peer
    pub bytes_sent: u64,
    /// Current score (0-100)
    pub score: u8,
}

impl PeerScore {
    pub fn new(peer_id: String) -> Self {
        Self {
            peer_id,
            total_requests: 0,
            successful_responses: 0,
            failed_requests: 0,
            avg_response_time_ms: 0,
            last_seen: 0,
            bytes_received: 0,
            bytes_sent: 0,
            score: 50, // Start neutral
        }
    }

    /// Record a successful response
    pub fn record_success(&mut self, response_time_ms: u64, bytes: u64) {
        self.total_requests += 1;
        self.successful_responses += 1;
        self.bytes_received += bytes;
        
        // Update rolling average response time
        self.avg_response_time_ms = (self.avg_response_time_ms * (self.total_requests - 1) 
            + response_time_ms) / self.total_requests;
        
        self.last_seen = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        
        self.recalculate_score();
    }

    /// Record a failed request
    pub fn record_failure(&mut self) {
        self.total_requests += 1;
        self.failed_requests += 1;
        self.recalculate_score();
    }

    /// Recalculate the peer score
    fn recalculate_score(&mut self) {
        if self.total_requests == 0 {
            self.score = 50;
            return;
        }

        // Success rate contributes 60% of score
        let success_rate = self.successful_responses as f64 / self.total_requests as f64;
        let success_score = (success_rate * 60.0) as u8;

        // Response time contributes 30% (faster = better)
        // < 100ms = 30, > 5000ms = 0
        let time_score = if self.avg_response_time_ms < 100 {
            30
        } else if self.avg_response_time_ms > 5000 {
            0
        } else {
            let normalized = (5000 - self.avg_response_time_ms) as f64 / 4900.0;
            (normalized * 30.0) as u8
        };

        // Longevity contributes 10%
        let longevity_score = if self.total_requests > 100 { 10 } else {
            (self.total_requests / 10) as u8
        };

        self.score = success_score + time_score + longevity_score;
    }

    /// Get reliability percentage
    pub fn reliability(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        (self.successful_responses as f64 / self.total_requests as f64) * 100.0
    }
}

/// Network metrics aggregation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMetrics {
    /// Total peers ever connected
    pub total_peers_seen: u64,
    /// Currently connected peers
    pub connected_peers: usize,
    /// Total bytes transferred
    pub total_bytes_transferred: u64,
    /// Total requests made
    pub total_requests: u64,
    /// Total successful requests
    pub successful_requests: u64,
    /// Average network latency in ms
    pub avg_latency_ms: u64,
    /// Uptime in seconds
    pub uptime_secs: u64,
    /// Last updated timestamp
    pub last_updated: u64,
}

impl Default for NetworkMetrics {
    fn default() -> Self {
        Self {
            total_peers_seen: 0,
            connected_peers: 0,
            total_bytes_transferred: 0,
            total_requests: 0,
            successful_requests: 0,
            avg_latency_ms: 0,
            uptime_secs: 0,
            last_updated: 0,
        }
    }
}

/// Connection health check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionHealth {
    pub peer_id: String,
    pub latency_ms: Option<u64>,
    pub reachable: bool,
    pub last_check: u64,
}

/// Health monitor for the network
pub struct HealthMonitor {
    /// Peer scores
    scores: RwLock<HashMap<String, PeerScore>>,
    /// Network metrics
    metrics: RwLock<NetworkMetrics>,
    /// Connection health cache
    health_cache: RwLock<HashMap<String, ConnectionHealth>>,
    /// Start time
    start_time: Instant,
}

impl HealthMonitor {
    pub fn new() -> Self {
        Self {
            scores: RwLock::new(HashMap::new()),
            metrics: RwLock::new(NetworkMetrics::default()),
            health_cache: RwLock::new(HashMap::new()),
            start_time: Instant::now(),
        }
    }

    /// Record a peer connection
    pub fn peer_connected(&self, peer_id: &str) {
        // Update or create peer score
        let mut scores = self.scores.write();
        scores.entry(peer_id.to_string())
            .or_insert_with(|| PeerScore::new(peer_id.to_string()));
        
        // Update metrics
        let mut metrics = self.metrics.write();
        metrics.connected_peers += 1;
        metrics.total_peers_seen += 1;
    }

    /// Record a peer disconnection
    pub fn peer_disconnected(&self, peer_id: &str) {
        let mut metrics = self.metrics.write();
        if metrics.connected_peers > 0 {
            metrics.connected_peers -= 1;
        }
    }

    /// Record a successful request to a peer
    pub fn record_request_success(&self, peer_id: &str, response_time_ms: u64, bytes: u64) {
        {
            let mut scores = self.scores.write();
            let score = scores.entry(peer_id.to_string())
                .or_insert_with(|| PeerScore::new(peer_id.to_string()));
            score.record_success(response_time_ms, bytes);
        }
        
        {
            let mut metrics = self.metrics.write();
            metrics.total_requests += 1;
            metrics.successful_requests += 1;
            metrics.total_bytes_transferred += bytes;
            
            // Update rolling average latency
            if metrics.total_requests > 0 {
                metrics.avg_latency_ms = (metrics.avg_latency_ms * (metrics.total_requests - 1) 
                    + response_time_ms) / metrics.total_requests;
            }
        }
    }

    /// Record a failed request to a peer
    pub fn record_request_failure(&self, peer_id: &str) {
        {
            let mut scores = self.scores.write();
            let score = scores.entry(peer_id.to_string())
                .or_insert_with(|| PeerScore::new(peer_id.to_string()));
            score.record_failure();
        }
        
        {
            let mut metrics = self.metrics.write();
            metrics.total_requests += 1;
        }
    }

    /// Update connection health for a peer
    pub fn update_health(&self, health: ConnectionHealth) {
        self.health_cache.write().insert(health.peer_id.clone(), health);
    }

    /// Get peer score
    pub fn get_peer_score(&self, peer_id: &str) -> Option<PeerScore> {
        self.scores.read().get(peer_id).cloned()
    }

    /// Get all peer scores
    pub fn get_all_scores(&self) -> Vec<PeerScore> {
        self.scores.read().values().cloned().collect()
    }

    /// Get top peers by score
    pub fn get_top_peers(&self, limit: usize) -> Vec<PeerScore> {
        let mut scores: Vec<_> = self.scores.read().values().cloned().collect();
        scores.sort_by(|a, b| b.score.cmp(&a.score));
        scores.truncate(limit);
        scores
    }

    /// Get network metrics
    pub fn get_metrics(&self) -> NetworkMetrics {
        let mut metrics = self.metrics.read().clone();
        metrics.uptime_secs = self.start_time.elapsed().as_secs();
        metrics.last_updated = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        metrics
    }

    /// Get connection health for a peer
    pub fn get_health(&self, peer_id: &str) -> Option<ConnectionHealth> {
        self.health_cache.read().get(peer_id).cloned()
    }

    /// Get all connection health entries
    pub fn get_all_health(&self) -> Vec<ConnectionHealth> {
        self.health_cache.read().values().cloned().collect()
    }

    /// Get summary health status
    pub fn get_health_summary(&self) -> HealthSummary {
        let metrics = self.get_metrics();
        let scores = self.get_all_scores();
        
        let avg_score = if scores.is_empty() {
            0.0
        } else {
            scores.iter().map(|s| s.score as f64).sum::<f64>() / scores.len() as f64
        };

        let reliability = if metrics.total_requests > 0 {
            (metrics.successful_requests as f64 / metrics.total_requests as f64) * 100.0
        } else {
            0.0
        };

        HealthSummary {
            status: if reliability > 95.0 { "healthy" } else if reliability > 80.0 { "good" } else { "degraded" }.to_string(),
            connected_peers: metrics.connected_peers,
            avg_peer_score: avg_score,
            network_reliability: reliability,
            avg_latency_ms: metrics.avg_latency_ms,
            uptime_secs: metrics.uptime_secs,
            total_bytes_transferred: metrics.total_bytes_transferred,
        }
    }
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// High-level health summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthSummary {
    pub status: String,
    pub connected_peers: usize,
    pub avg_peer_score: f64,
    pub network_reliability: f64,
    pub avg_latency_ms: u64,
    pub uptime_secs: u64,
    pub total_bytes_transferred: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_score() {
        let mut score = PeerScore::new("test-peer".to_string());
        
        // Initial score
        assert_eq!(score.score, 50);
        
        // Record successes
        score.record_success(50, 1000);
        score.record_success(60, 2000);
        score.record_success(70, 1500);
        
        // Score should improve
        assert!(score.score > 50);
        assert_eq!(score.successful_responses, 3);
        
        // Record failure
        score.record_failure();
        
        // Score should drop slightly
        assert!(score.score < 100);
        assert_eq!(score.failed_requests, 1);
    }

    #[test]
    fn test_health_monitor() {
        let monitor = HealthMonitor::new();
        
        // Connect peers
        monitor.peer_connected("peer1");
        monitor.peer_connected("peer2");
        
        let metrics = monitor.get_metrics();
        assert_eq!(metrics.connected_peers, 2);
        assert_eq!(metrics.total_peers_seen, 2);
        
        // Record requests
        monitor.record_request_success("peer1", 100, 1000);
        monitor.record_request_success("peer1", 50, 500);
        monitor.record_request_failure("peer2");
        
        let metrics = monitor.get_metrics();
        assert_eq!(metrics.total_requests, 3);
        assert_eq!(metrics.successful_requests, 2);
        
        // Check peer scores
        let score1 = monitor.get_peer_score("peer1").unwrap();
        assert!(score1.score > 50);
        
        let score2 = monitor.get_peer_score("peer2").unwrap();
        assert!(score2.score < 50);
    }

    #[test]
    fn test_top_peers() {
        let monitor = HealthMonitor::new();
        
        monitor.peer_connected("good-peer");
        monitor.record_request_success("good-peer", 50, 1000);
        monitor.record_request_success("good-peer", 60, 1000);
        
        monitor.peer_connected("bad-peer");
        monitor.record_request_failure("bad-peer");
        monitor.record_request_failure("bad-peer");
        
        let top = monitor.get_top_peers(5);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].peer_id, "good-peer");
    }
}
