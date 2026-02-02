//! GrabNet P2P node implementation

use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use anyhow::{Result, anyhow};
use futures::StreamExt;
use libp2p::{
    identity, noise, tcp, yamux,
    Multiaddr, PeerId, Swarm, SwarmBuilder,
    swarm::SwarmEvent,
    request_response::{self},
    kad::{self, QueryResult, QueryId},
    gossipsub::{self, IdentTopic},
    mdns, identify,
};
use parking_lot::RwLock;
use tokio::sync::{mpsc, oneshot, broadcast};

use super::behaviour::{GrabBehaviour, GrabBehaviourEvent};
use crate::types::{Config, SiteId, WebBundle, GrabRequest, GrabResponse, PeerRecord, ChunkId};
use crate::storage::{ChunkStore, BundleStore};
use crate::crypto::SiteIdExt;

/// Default bootstrap peers for the GrabNet network
pub const DEFAULT_BOOTSTRAP_PEERS: &[&str] = &[
    // These would be official GrabNet bootstrap nodes
    // For now, empty - users can add their own
];

/// Gossipsub topic for site announcements
const SITES_TOPIC: &str = "grabnet/sites/1.0.0";

/// Gossipsub topic for updates
const UPDATES_TOPIC: &str = "grabnet/updates/1.0.0";

/// Message from main thread to swarm event loop
#[derive(Debug)]
enum SwarmCommand {
    Dial(Multiaddr),
    SendRequest(PeerId, GrabRequest, oneshot::Sender<Result<GrabResponse>>),
    Announce(SiteId, u64),
    FindSite(SiteId, oneshot::Sender<Vec<PeerRecord>>),
    GetPeers(oneshot::Sender<Vec<PeerId>>),
    GetAddresses(oneshot::Sender<Vec<String>>),
    Bootstrap,
    Shutdown,
}

/// Network event published to subscribers
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    /// A new peer connected
    PeerConnected(PeerId),
    /// A peer disconnected
    PeerDisconnected(PeerId),
    /// Received a site announcement
    SiteAnnounced { site_id: SiteId, peer_id: PeerId, revision: u64 },
    /// Received a site update
    SiteUpdated { site_id: SiteId, revision: u64 },
    /// Bootstrap complete
    BootstrapComplete { peers: usize },
}

/// GrabNet P2P network node
pub struct GrabNetwork {
    peer_id: PeerId,
    command_tx: mpsc::Sender<SwarmCommand>,
    event_tx: broadcast::Sender<NetworkEvent>,
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    /// Track which sites we're announcing
    announced_sites: Arc<RwLock<HashMap<SiteId, u64>>>,
    /// Connected peers
    connected_peers: Arc<RwLock<HashSet<PeerId>>>,
    /// Background task handle
    _task: tokio::task::JoinHandle<()>,
}

impl GrabNetwork {
    /// Create a new network node
    pub async fn new(
        config: &Config,
        chunk_store: Arc<ChunkStore>,
        bundle_store: Arc<BundleStore>,
    ) -> Result<Self> {
        // Generate identity
        let local_key = identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());
        
        tracing::info!("Local peer ID: {}", local_peer_id);

        // Build swarm
        let swarm = SwarmBuilder::with_existing_identity(local_key.clone())
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| {
                GrabBehaviour::new(local_peer_id, key.public())
            })?
            .with_swarm_config(|cfg| {
                cfg.with_idle_connection_timeout(Duration::from_secs(120))
            })
            .build();

        // Command channel
        let (command_tx, command_rx) = mpsc::channel(256);
        
        // Event broadcast channel
        let (event_tx, _) = broadcast::channel(256);

        // Clone stores for the event loop
        let chunk_store_clone = chunk_store.clone();
        let bundle_store_clone = bundle_store.clone();
        let announced_sites = Arc::new(RwLock::new(HashMap::new()));
        let announced_sites_clone = announced_sites.clone();
        let connected_peers = Arc::new(RwLock::new(HashSet::new()));
        let connected_peers_clone = connected_peers.clone();
        let event_tx_clone = event_tx.clone();

        // Start event loop
        let listen_addrs = config.network.listen_addresses.clone();
        let bootstrap_peers = config.network.bootstrap_peers.clone();
        
        let task = tokio::spawn(async move {
            run_swarm(
                swarm,
                command_rx,
                listen_addrs,
                bootstrap_peers,
                chunk_store_clone,
                bundle_store_clone,
                announced_sites_clone,
                connected_peers_clone,
                event_tx_clone,
            ).await;
        });

        Ok(Self {
            peer_id: local_peer_id,
            command_tx,
            event_tx,
            chunk_store,
            bundle_store,
            announced_sites,
            connected_peers,
            _task: task,
        })
    }

    /// Start the network (connects to bootstrap peers)
    pub async fn start(&self) -> Result<()> {
        self.command_tx.send(SwarmCommand::Bootstrap).await?;
        Ok(())
    }

    /// Stop the network
    pub async fn stop(&self) -> Result<()> {
        let _ = self.command_tx.send(SwarmCommand::Shutdown).await;
        Ok(())
    }

    /// Get our peer ID
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    /// Get connected peers count
    pub fn connected_peers(&self) -> usize {
        self.connected_peers.read().len()
    }

    /// Get connected peer IDs
    pub fn connected_peer_ids(&self) -> Vec<PeerId> {
        self.connected_peers.read().iter().cloned().collect()
    }

    /// Get listen addresses
    pub fn listen_addresses(&self) -> Vec<String> {
        vec![]
    }

    /// Subscribe to network events
    pub fn subscribe(&self) -> broadcast::Receiver<NetworkEvent> {
        self.event_tx.subscribe()
    }

    /// Connect to a peer
    pub async fn dial(&self, addr: &str) -> Result<()> {
        let multiaddr: Multiaddr = addr.parse()?;
        self.command_tx.send(SwarmCommand::Dial(multiaddr)).await?;
        Ok(())
    }

    /// Announce that we're hosting a site
    pub async fn announce_site(&self, site_id: &SiteId, revision: u64) -> Result<()> {
        self.announced_sites.write().insert(*site_id, revision);
        self.command_tx.send(SwarmCommand::Announce(*site_id, revision)).await?;
        Ok(())
    }

    /// Find hosts for a site via DHT
    pub async fn find_site(&self, site_id: &SiteId) -> Result<Vec<PeerRecord>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(SwarmCommand::FindSite(*site_id, tx)).await?;
        
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(hosts)) => Ok(hosts),
            Ok(Err(_)) => Ok(vec![]),
            Err(_) => Ok(vec![]),
        }
    }

    /// Fetch a site from the network
    pub async fn fetch_site(&self, site_id: &SiteId) -> Result<Option<WebBundle>> {
        let hosts = self.find_site(site_id).await?;
        
        if hosts.is_empty() {
            return Ok(None);
        }

        for host in hosts {
            if let Ok(peer_id) = host.peer_id.parse::<PeerId>() {
                let (tx, rx) = oneshot::channel();
                self.command_tx.send(SwarmCommand::SendRequest(
                    peer_id,
                    GrabRequest::GetManifest { site_id: *site_id },
                    tx,
                )).await?;

                if let Ok(Ok(GrabResponse::Manifest { bundle })) = rx.await {
                    return Ok(Some(*bundle));
                }
            }
        }

        Ok(None)
    }

    /// Push an update to all hosts
    pub async fn push_update(&self, bundle: &WebBundle) -> Result<usize> {
        let hosts = self.find_site(&bundle.site_id).await?;
        let mut updated = 0;

        for host in hosts {
            if let Ok(peer_id) = host.peer_id.parse::<PeerId>() {
                let (tx, rx) = oneshot::channel();
                self.command_tx.send(SwarmCommand::SendRequest(
                    peer_id,
                    GrabRequest::PushUpdate { bundle: Box::new(bundle.clone()) },
                    tx,
                )).await?;

                if let Ok(Ok(GrabResponse::Ack)) = rx.await {
                    updated += 1;
                }
            }
        }

        Ok(updated)
    }

    /// Get chunks from a peer
    pub async fn get_chunks(&self, peer_id: &PeerId, chunk_ids: &[ChunkId]) -> Result<Vec<(ChunkId, Vec<u8>)>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(SwarmCommand::SendRequest(
            *peer_id,
            GrabRequest::GetChunks { chunk_ids: chunk_ids.to_vec() },
            tx,
        )).await?;

        match rx.await? {
            Ok(GrabResponse::Chunks { chunks }) => Ok(chunks),
            Ok(GrabResponse::Error { message }) => Err(anyhow!(message)),
            _ => Err(anyhow!("Unexpected response")),
        }
    }
}

/// Run the swarm event loop
async fn run_swarm(
    mut swarm: Swarm<GrabBehaviour>,
    mut command_rx: mpsc::Receiver<SwarmCommand>,
    listen_addrs: Vec<String>,
    bootstrap_peers: Vec<String>,
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    announced_sites: Arc<RwLock<HashMap<SiteId, u64>>>,
    connected_peers: Arc<RwLock<HashSet<PeerId>>>,
    event_tx: broadcast::Sender<NetworkEvent>,
) {
    // Start listening
    for addr in listen_addrs {
        if let Ok(multiaddr) = addr.parse::<Multiaddr>() {
            if let Err(e) = swarm.listen_on(multiaddr.clone()) {
                tracing::warn!("Failed to listen on {}: {}", addr, e);
            } else {
                tracing::info!("Listening on {}", addr);
            }
        }
    }

    // Subscribe to gossipsub topics
    let sites_topic = IdentTopic::new(SITES_TOPIC);
    let updates_topic = IdentTopic::new(UPDATES_TOPIC);
    if let Err(e) = swarm.behaviour_mut().gossipsub.subscribe(&sites_topic) {
        tracing::warn!("Failed to subscribe to sites topic: {}", e);
    }
    if let Err(e) = swarm.behaviour_mut().gossipsub.subscribe(&updates_topic) {
        tracing::warn!("Failed to subscribe to updates topic: {}", e);
    }

    // Pending requests
    let mut pending_requests: HashMap<request_response::OutboundRequestId, oneshot::Sender<Result<GrabResponse>>> = HashMap::new();
    
    // Pending DHT queries
    let mut pending_site_queries: HashMap<QueryId, (SiteId, oneshot::Sender<Vec<PeerRecord>>)> = HashMap::new();
    
    // Discovered providers for sites
    let mut site_providers: HashMap<SiteId, Vec<PeerRecord>> = HashMap::new();

    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {
                match command {
                    SwarmCommand::Dial(addr) => {
                        tracing::debug!("Dialing {}", addr);
                        let _ = swarm.dial(addr);
                    }
                    SwarmCommand::SendRequest(peer_id, request, response_tx) => {
                        let request_id = swarm.behaviour_mut().request_response.send_request(&peer_id, request);
                        pending_requests.insert(request_id, response_tx);
                    }
                    SwarmCommand::Announce(site_id, revision) => {
                        // Put in DHT as provider
                        let key = kad::RecordKey::new(&site_id);
                        swarm.behaviour_mut().kademlia.start_providing(key.clone())
                            .map_err(|e| tracing::warn!("Failed to start providing: {}", e))
                            .ok();
                        
                        // Also put record with revision info
                        let value = bincode::serialize(&(swarm.local_peer_id().to_string(), revision)).unwrap_or_default();
                        let record = kad::Record::new(key, value);
                        let _ = swarm.behaviour_mut().kademlia.put_record(record, kad::Quorum::One);
                        
                        // Broadcast via gossipsub
                        let msg = bincode::serialize(&(site_id, revision)).unwrap_or_default();
                        let _ = swarm.behaviour_mut().gossipsub.publish(sites_topic.clone(), msg);
                        
                        tracing::info!("Announcing site {} revision {}", site_id.to_base58(), revision);
                    }
                    SwarmCommand::FindSite(site_id, tx) => {
                        let key = kad::RecordKey::new(&site_id);
                        let query_id = swarm.behaviour_mut().kademlia.get_providers(key);
                        pending_site_queries.insert(query_id, (site_id, tx));
                        site_providers.insert(site_id, vec![]);
                    }
                    SwarmCommand::GetPeers(tx) => {
                        let peers: Vec<_> = swarm.connected_peers().cloned().collect();
                        let _ = tx.send(peers);
                    }
                    SwarmCommand::GetAddresses(tx) => {
                        let addrs: Vec<_> = swarm.listeners().map(|a| a.to_string()).collect();
                        let _ = tx.send(addrs);
                    }
                    SwarmCommand::Bootstrap => {
                        for addr in &bootstrap_peers {
                            if let Ok(multiaddr) = addr.parse::<Multiaddr>() {
                                tracing::info!("Connecting to bootstrap peer: {}", addr);
                                let _ = swarm.dial(multiaddr);
                            }
                        }
                        for addr in DEFAULT_BOOTSTRAP_PEERS {
                            if let Ok(multiaddr) = addr.parse::<Multiaddr>() {
                                tracing::info!("Connecting to default bootstrap peer: {}", addr);
                                let _ = swarm.dial(multiaddr);
                            }
                        }
                        let _ = swarm.behaviour_mut().kademlia.bootstrap();
                    }
                    SwarmCommand::Shutdown => {
                        tracing::info!("Shutting down network");
                        break;
                    }
                }
            }

            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        tracing::info!("Listening on {}", address);
                    }
                    
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        tracing::debug!("Connected to peer: {}", peer_id);
                        connected_peers.write().insert(peer_id);
                        let _ = event_tx.send(NetworkEvent::PeerConnected(peer_id));
                    }
                    
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        tracing::debug!("Disconnected from peer: {}", peer_id);
                        connected_peers.write().remove(&peer_id);
                        let _ = event_tx.send(NetworkEvent::PeerDisconnected(peer_id));
                    }

                    SwarmEvent::Behaviour(GrabBehaviourEvent::RequestResponse(
                        request_response::Event::Message { message, .. }
                    )) => {
                        match message {
                            request_response::Message::Request { request, channel, .. } => {
                                let response = handle_request(
                                    request,
                                    &chunk_store,
                                    &bundle_store,
                                    &announced_sites,
                                    swarm.local_peer_id(),
                                ).await;
                                let _ = swarm.behaviour_mut().request_response.send_response(channel, response);
                            }
                            request_response::Message::Response { request_id, response } => {
                                if let Some(tx) = pending_requests.remove(&request_id) {
                                    let _ = tx.send(Ok(response));
                                }
                            }
                        }
                    }
                    
                    SwarmEvent::Behaviour(GrabBehaviourEvent::RequestResponse(
                        request_response::Event::OutboundFailure { request_id, error, .. }
                    )) => {
                        if let Some(tx) = pending_requests.remove(&request_id) {
                            let _ = tx.send(Err(anyhow!("Request failed: {:?}", error)));
                        }
                    }
                    
                    SwarmEvent::Behaviour(GrabBehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed { id, result, .. })) => {
                        match result {
                            QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders { providers, .. })) => {
                                if let Some((site_id, _)) = pending_site_queries.get(&id) {
                                    let records = site_providers.entry(*site_id).or_default();
                                    for provider in providers {
                                        records.push(PeerRecord {
                                            peer_id: provider.to_string(),
                                            addresses: vec![],
                                            revision: 0,
                                        });
                                    }
                                }
                            }
                            QueryResult::GetProviders(Ok(kad::GetProvidersOk::FinishedWithNoAdditionalRecord { .. })) => {
                                if let Some((site_id, tx)) = pending_site_queries.remove(&id) {
                                    let records = site_providers.remove(&site_id).unwrap_or_default();
                                    let _ = tx.send(records);
                                }
                            }
                            QueryResult::Bootstrap(Ok(_)) => {
                                let peer_count = connected_peers.read().len();
                                tracing::info!("Kademlia bootstrap complete, {} peers", peer_count);
                                let _ = event_tx.send(NetworkEvent::BootstrapComplete { peers: peer_count });
                            }
                            _ => {}
                        }
                    }

                    SwarmEvent::Behaviour(GrabBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        message,
                        propagation_source,
                        ..
                    })) => {
                        if message.topic == sites_topic.hash() {
                            let result: Result<(SiteId, u64), _> = bincode::deserialize(&message.data);
                            if let Ok((site_id, revision)) = result {
                                tracing::debug!("Received site announcement: {} rev {}", site_id.to_base58(), revision);
                                let _ = event_tx.send(NetworkEvent::SiteAnnounced {
                                    site_id,
                                    peer_id: propagation_source,
                                    revision,
                                });
                            }
                        } else if message.topic == updates_topic.hash() {
                            let result: Result<(SiteId, u64), _> = bincode::deserialize(&message.data);
                            if let Ok((site_id, revision)) = result {
                                tracing::debug!("Received site update: {} rev {}", site_id.to_base58(), revision);
                                let _ = event_tx.send(NetworkEvent::SiteUpdated { site_id, revision });
                            }
                        }
                    }

                    SwarmEvent::Behaviour(GrabBehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
                        for (peer_id, addr) in peers {
                            tracing::debug!("Discovered peer {} at {}", peer_id, addr);
                            swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                        }
                    }
                    
                    SwarmEvent::Behaviour(GrabBehaviourEvent::Mdns(mdns::Event::Expired(peers))) => {
                        for (peer_id, _) in peers {
                            tracing::debug!("Peer expired: {}", peer_id);
                        }
                    }

                    SwarmEvent::Behaviour(GrabBehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. })) => {
                        tracing::debug!("Identified peer {}: {:?}", peer_id, info.protocols);
                        for addr in info.listen_addrs {
                            swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                        }
                    }

                    _ => {}
                }
            }
        }
    }
}

/// Handle an incoming request
async fn handle_request(
    request: GrabRequest,
    chunk_store: &ChunkStore,
    bundle_store: &BundleStore,
    announced_sites: &RwLock<HashMap<SiteId, u64>>,
    local_peer_id: &PeerId,
) -> GrabResponse {
    match request {
        GrabRequest::FindSite { site_id } => {
            if let Some(revision) = announced_sites.read().get(&site_id) {
                GrabResponse::SiteHosts {
                    hosts: vec![PeerRecord {
                        peer_id: local_peer_id.to_string(),
                        addresses: vec![],
                        revision: *revision,
                    }],
                }
            } else {
                GrabResponse::SiteHosts { hosts: vec![] }
            }
        }
        GrabRequest::GetManifest { site_id } => {
            match bundle_store.get_bundle(&site_id) {
                Ok(Some(bundle)) => GrabResponse::Manifest { bundle: Box::new(bundle) },
                Ok(None) => GrabResponse::Error { message: "Site not found".to_string() },
                Err(e) => GrabResponse::Error { message: e.to_string() },
            }
        }
        GrabRequest::GetChunks { chunk_ids } => {
            let mut chunks = Vec::new();
            for chunk_id in chunk_ids {
                if let Ok(Some(data)) = chunk_store.get(&chunk_id) {
                    chunks.push((chunk_id, data));
                }
            }
            GrabResponse::Chunks { chunks }
        }
        GrabRequest::Announce { site_id, revision } => {
            tracing::info!("Peer announced site {} revision {}", site_id.to_base58(), revision);
            GrabResponse::Ack
        }
        GrabRequest::PushUpdate { bundle } => {
            if let Err(e) = bundle_store.save_bundle(&bundle) {
                return GrabResponse::Error { message: e.to_string() };
            }
            tracing::info!("Received update for {} revision {}", bundle.name, bundle.revision);
            GrabResponse::Ack
        }
    }
}
