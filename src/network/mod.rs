//! P2P networking layer using libp2p

mod node;
mod protocol;
mod behaviour;
pub mod bootstrap;
pub mod replication;
pub mod health;

pub use node::{GrabNetwork, NetworkEvent, DEFAULT_BOOTSTRAP_PEERS};
pub use protocol::{GrabProtocol, GrabCodec};
pub use behaviour::GrabBehaviour;
pub use bootstrap::{BootstrapConfig, BootstrapNode};
pub use replication::{ReplicationManager, ReplicationPolicy, SiteHealth, HealthStatus, ReplicationStats};
pub use health::{HealthMonitor, HealthSummary, PeerScore, NetworkMetrics, ConnectionHealth};
