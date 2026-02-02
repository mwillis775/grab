//! Custom libp2p behaviour for GrabNet

use libp2p::{
    gossipsub, identify, kad, mdns,
    request_response::{self, ProtocolSupport},
    swarm::NetworkBehaviour,
    StreamProtocol,
};
use std::time::Duration;

use super::protocol::{GrabCodec, PROTOCOL_NAME};
use crate::types::{GrabRequest, GrabResponse};

/// Combined network behaviour for GrabNet
#[derive(NetworkBehaviour)]
pub struct GrabBehaviour {
    /// Request/response for direct messaging
    pub request_response: request_response::Behaviour<GrabCodec>,
    /// Kademlia DHT for peer and content discovery
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    /// Gossipsub for pub/sub messaging
    pub gossipsub: gossipsub::Behaviour,
    /// mDNS for local peer discovery
    pub mdns: mdns::tokio::Behaviour,
    /// Identify protocol
    pub identify: identify::Behaviour,
}

impl GrabBehaviour {
    /// Create a new GrabNet behaviour
    pub fn new(local_peer_id: libp2p::PeerId, local_public_key: libp2p::identity::PublicKey) -> Self {
        // Request/response config
        let request_response = request_response::Behaviour::new(
            [(PROTOCOL_NAME, ProtocolSupport::Full)],
            request_response::Config::default()
                .with_request_timeout(Duration::from_secs(60)),
        );

        // Kademlia config
        let store = kad::store::MemoryStore::new(local_peer_id);
        let mut kademlia = kad::Behaviour::new(local_peer_id, store);
        kademlia.set_mode(Some(kad::Mode::Server));

        // Gossipsub config
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(10))
            .validation_mode(gossipsub::ValidationMode::Permissive)
            .build()
            .expect("Valid gossipsub config");

        // Use anonymous message authenticity for now (simpler)
        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Anonymous,
            gossipsub_config,
        )
        .expect("Valid gossipsub behaviour");

        // mDNS for local discovery
        let mdns = mdns::tokio::Behaviour::new(
            mdns::Config::default(),
            local_peer_id,
        )
        .expect("Valid mDNS behaviour");

        // Identify protocol
        let identify = identify::Behaviour::new(identify::Config::new(
            "/grabnet/id/1.0.0".to_string(),
            local_public_key,
        ));

        Self {
            request_response,
            kademlia,
            gossipsub,
            mdns,
            identify,
        }
    }
}
