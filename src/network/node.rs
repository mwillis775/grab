//! GrabNet P2P node implementation

use std::sync::Arc;
use std::collections::HashMap;
use std::time::Duration;
use anyhow::{Result, anyhow};
use futures::StreamExt;
use libp2p::{
    identity, noise, tcp, yamux,
    Multiaddr, PeerId, Swarm, SwarmBuilder,
    swarm::SwarmEvent,
    request_response::{self, ResponseChannel},
    kad,
};
use parking_lot::RwLock;
use tokio::sync::{mpsc, oneshot};

use super::behaviour::{GrabBehaviour, GrabBehaviourEvent};
use crate::types::{Config, SiteId, WebBundle, GrabRequest, GrabResponse, PeerRecord, ChunkId};
use crate::storage::{ChunkStore, BundleStore};
use crate::crypto::SiteIdExt;

/// Message from main thread to swarm event loop
enum SwarmCommand {
    Dial(Multiaddr),
    SendRequest(PeerId, GrabRequest, oneshot::Sender<Result<GrabResponse>>),
    Announce(SiteId, u64),
    GetPeers(oneshot::Sender<Vec<PeerId>>),
    GetAddresses(oneshot::Sender<Vec<String>>),
    Shutdown,
}

/// GrabNet P2P network node
pub struct GrabNetwork {
    peer_id: PeerId,
    command_tx: mpsc::Sender<SwarmCommand>,
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    /// Track which sites we're announcing
    announced_sites: Arc<RwLock<HashMap<SiteId, u64>>>,
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
                cfg.with_idle_connection_timeout(Duration::from_secs(60))
            })
            .build();

        // Command channel
        let (command_tx, command_rx) = mpsc::channel(256);

        // Clone stores for the event loop
        let chunk_store_clone = chunk_store.clone();
        let bundle_store_clone = bundle_store.clone();
        let announced_sites = Arc::new(RwLock::new(HashMap::new()));
        let announced_sites_clone = announced_sites.clone();

        // Start event loop
        let listen_addrs = config.network.listen_addresses.clone();
        let task = tokio::spawn(async move {
            run_swarm(
                swarm,
                command_rx,
                listen_addrs,
                chunk_store_clone,
                bundle_store_clone,
                announced_sites_clone,
            ).await;
        });

        Ok(Self {
            peer_id: local_peer_id,
            command_tx,
            chunk_store,
            bundle_store,
            announced_sites,
            _task: task,
        })
    }

    /// Start the network
    pub async fn start(&self) -> Result<()> {
        // Network starts in the background task
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
        // This would need to be fetched from swarm
        // For now return 0
        0
    }

    /// Get listen addresses
    pub fn listen_addresses(&self) -> Vec<String> {
        // Would need to query swarm
        vec![]
    }

    /// Announce that we're hosting a site
    pub async fn announce_site(&self, site_id: &SiteId, revision: u64) -> Result<()> {
        self.announced_sites.write().insert(*site_id, revision);
        self.command_tx.send(SwarmCommand::Announce(*site_id, revision)).await?;
        Ok(())
    }

    /// Find hosts for a site
    pub async fn find_site(&self, site_id: &SiteId) -> Result<Vec<PeerRecord>> {
        // Query DHT
        // For now return empty
        Ok(vec![])
    }

    /// Fetch a site from the network
    pub async fn fetch_site(&self, site_id: &SiteId) -> Result<Option<WebBundle>> {
        // Find hosts and request manifest
        let hosts = self.find_site(site_id).await?;
        
        if hosts.is_empty() {
            return Ok(None);
        }

        // Try each host
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
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    announced_sites: Arc<RwLock<HashMap<SiteId, u64>>>,
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

    // Pending requests
    let mut pending_requests: HashMap<request_response::OutboundRequestId, oneshot::Sender<Result<GrabResponse>>> = HashMap::new();

    loop {
        tokio::select! {
            // Handle commands
            Some(command) = command_rx.recv() => {
                match command {
                    SwarmCommand::Dial(addr) => {
                        let _ = swarm.dial(addr);
                    }
                    SwarmCommand::SendRequest(peer_id, request, response_tx) => {
                        let request_id = swarm.behaviour_mut().request_response.send_request(&peer_id, request);
                        pending_requests.insert(request_id, response_tx);
                    }
                    SwarmCommand::Announce(site_id, revision) => {
                        // Put in DHT
                        let key = kad::RecordKey::new(&site_id);
                        let value = bincode::serialize(&(swarm.local_peer_id().to_string(), revision)).unwrap_or_default();
                        let record = kad::Record::new(key, value);
                        let _ = swarm.behaviour_mut().kademlia.put_record(record, kad::Quorum::One);
                    }
                    SwarmCommand::GetPeers(tx) => {
                        let peers: Vec<_> = swarm.connected_peers().cloned().collect();
                        let _ = tx.send(peers);
                    }
                    SwarmCommand::GetAddresses(tx) => {
                        let addrs: Vec<_> = swarm.listeners().map(|a| a.to_string()).collect();
                        let _ = tx.send(addrs);
                    }
                    SwarmCommand::Shutdown => {
                        break;
                    }
                }
            }

            // Handle swarm events
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        tracing::info!("Listening on {}", address);
                    }
                    SwarmEvent::Behaviour(GrabBehaviourEvent::RequestResponse(
                        request_response::Event::Message { message, peer }
                    )) => {
                        match message {
                            request_response::Message::Request { request, channel, .. } => {
                                // Handle incoming request
                                let response = handle_request(
                                    request,
                                    &chunk_store,
                                    &bundle_store,
                                    &announced_sites,
                                ).await;
                                let _ = swarm.behaviour_mut().request_response.send_response(channel, response);
                            }
                            request_response::Message::Response { request_id, response } => {
                                // Handle response
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
                    SwarmEvent::Behaviour(GrabBehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
                        for (peer_id, addr) in peers {
                            tracing::debug!("Discovered peer {} at {}", peer_id, addr);
                            swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                        }
                    }
                    SwarmEvent::Behaviour(GrabBehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. })) => {
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
) -> GrabResponse {
    match request {
        GrabRequest::FindSite { site_id } => {
            // Check if we're hosting this site
            if let Some(revision) = announced_sites.read().get(&site_id) {
                GrabResponse::SiteHosts {
                    hosts: vec![PeerRecord {
                        peer_id: "self".to_string(), // Would be actual peer ID
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
            // Record the announcement
            tracing::info!("Peer announced site {} revision {}", site_id.to_base58(), revision);
            GrabResponse::Ack
        }
        GrabRequest::PushUpdate { bundle } => {
            // Store the update
            if let Err(e) = bundle_store.save_bundle(&bundle) {
                return GrabResponse::Error { message: e.to_string() };
            }

            // Store chunks would go here
            tracing::info!("Received update for {} revision {}", bundle.name, bundle.revision);
            GrabResponse::Ack
        }
    }
}

// Need to import mdns and identify for the event handling
use libp2p::{mdns, identify};
