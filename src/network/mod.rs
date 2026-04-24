//! P2P networking layer using libp2p

mod behaviour;
pub mod bootstrap;
pub mod health;
mod node;
mod protocol;
pub mod replication;

pub use behaviour::GrabBehaviour;
pub use bootstrap::{BootstrapConfig, BootstrapNode};
pub use health::{ConnectionHealth, HealthMonitor, HealthSummary, NetworkMetrics, PeerScore};
pub use node::{GrabNetwork, NetworkEvent, DEFAULT_BOOTSTRAP_PEERS};
pub use protocol::{GrabCodec, GrabProtocol};
pub use replication::{
    HealthStatus, ReplicationManager, ReplicationPolicy, ReplicationStats, SiteHealth,
};
