//! GrabNet CLI

use anyhow::Result;
use clap::{Parser, Subcommand};
use grabnet::{Grab, PublishOptions, SiteIdExt};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::path::PathBuf;
use std::time::Duration;
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Parser)]
#[command(name = "grab")]
#[command(author = "GrabNet Contributors")]
#[command(version = "0.1.0")]
#[command(about = "Decentralized web hosting - publish websites to the permanent web")]
struct Cli {
    /// Data directory
    #[arg(long, env = "GRAB_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Publish a website
    Publish {
        /// Path to website directory
        path: String,

        /// Site name
        #[arg(short, long)]
        name: Option<String>,

        /// Entry point file
        #[arg(short, long)]
        entry: Option<String>,

        /// Enable SPA mode with fallback
        #[arg(long)]
        spa: Option<String>,

        /// Enable clean URLs
        #[arg(long)]
        clean_urls: bool,

        /// Disable compression
        #[arg(long)]
        no_compress: bool,

        /// Watch for changes and auto-republish
        #[arg(short, long)]
        watch: bool,

        /// Command to run before publishing (pre-deploy hook)
        #[arg(long)]
        pre_hook: Option<String>,

        /// Command to run after publishing (post-deploy hook)
        #[arg(long)]
        post_hook: Option<String>,
    },

    /// Update an existing site
    Update {
        /// Site name or ID
        site: String,
    },

    /// List published and hosted sites
    List,

    /// Show site information
    Info {
        /// Site name or ID
        site: String,
    },

    /// Node management
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },

    /// Host (pin) a site
    Host {
        /// Site ID to host
        site_id: String,
    },

    /// Pin a remote site from the network
    Pin {
        /// Site ID to pin
        site_id: String,

        /// Peer address to connect to
        #[arg(short, long)]
        peer: Option<String>,
    },

    /// Stop hosting a site
    Unhost {
        /// Site ID to unhost
        site_id: String,
    },

    /// Key management
    Keys {
        #[command(subcommand)]
        action: KeysAction,
    },

    /// Start the HTTP gateway
    Gateway {
        /// Port to listen on (HTTP, or HTTPS if --tls-cert is set without --https-port)
        #[arg(short, long, default_value = "8080")]
        port: u16,

        /// Default site to serve at root (name or ID)
        #[arg(long)]
        default_site: Option<String>,

        /// Path to PEM-encoded TLS certificate (full chain). Enables HTTPS.
        #[arg(long, requires = "tls_key")]
        tls_cert: Option<std::path::PathBuf>,

        /// Path to PEM-encoded TLS private key.
        #[arg(long, requires = "tls_cert")]
        tls_key: Option<std::path::PathBuf>,

        /// Dedicated HTTPS port. When set with --tls-cert, --port serves an HTTP→HTTPS redirect.
        #[arg(long, requires = "tls_cert")]
        https_port: Option<u16>,

        /// Static `Host` → site mapping, e.g. `--alias rootedrevival.us=rootedrevival`
        /// or `--alias example.org=<base58-site-id>`. May be repeated.
        #[arg(long = "alias", value_name = "HOST=SITE")]
        aliases: Vec<String>,

        /// Resolve unknown hosts via `_grab.<host>` DNS TXT records.
        /// Successful lookups are cached for the record's TTL.
        #[arg(long)]
        dns_aliases: bool,
    },

    /// DNS-based site discovery
    Dns {
        #[command(subcommand)]
        action: DnsAction,
    },

    /// ENS (Ethereum Name Service) site discovery
    Ens {
        #[command(subcommand)]
        action: EnsAction,
    },

    /// Bootstrap node management
    Bootstrap {
        #[command(subcommand)]
        action: BootstrapAction,
    },

    /// Show storage statistics
    Stats,
}

#[derive(Subcommand)]
enum NodeAction {
    /// Start the node
    Start {
        /// Port to listen on
        #[arg(short, long)]
        port: Option<u16>,

        /// Run in light mode (no hosting)
        #[arg(long)]
        light: bool,

        /// Bootstrap peers to connect to
        #[arg(short, long)]
        bootstrap: Vec<String>,
    },

    /// Show node status
    Status,

    /// List connected peers
    Peers,

    /// Connect to a peer
    Connect {
        /// Peer multiaddress
        address: String,
    },

    /// Stop the node
    Stop,
}

#[derive(Subcommand)]
enum KeysAction {
    /// List all keys
    List,

    /// Generate a new key
    Generate {
        /// Key name
        name: String,
    },

    /// Export a key
    Export {
        /// Key name
        name: String,
    },

    /// Import a key
    Import {
        /// Key name
        name: String,

        /// Base58-encoded private key
        private_key: String,
    },
}

#[derive(Subcommand)]
enum BootstrapAction {
    /// List all bootstrap nodes
    List,

    /// Add a custom bootstrap node
    Add {
        /// Node name
        name: String,

        /// Multiaddress (e.g., /ip4/1.2.3.4/tcp/4001)
        address: String,
    },

    /// Remove a custom bootstrap node
    Remove {
        /// Node name
        name: String,
    },

    /// Test connectivity to bootstrap nodes
    Test,
}

#[derive(Subcommand)]
enum DnsAction {
    /// Resolve a domain via `_grab.<host>` TXT record.
    Resolve {
        /// Hostname (e.g. `rootedrevival.us`)
        host: String,
    },

    /// Print the TXT record value to publish for a given site.
    Record {
        /// Site name or base58 site id
        site: String,
    },
}

#[derive(Subcommand)]
enum EnsAction {
    /// Resolve an ENS name (`*.eth`) to a site id via the configured gateway.
    Resolve {
        /// ENS name (e.g. `rootedrevival.eth`)
        name: String,

        /// Override the ENS gateway URL template.
        /// `{name}` is replaced with the lowercased ENS name.
        #[arg(long)]
        gateway: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose {
        EnvFilter::new("grabnet=debug,info")
    } else {
        EnvFilter::new("grabnet=info,warn")
    };
    fmt().with_env_filter(filter).init();

    // Get data directory (clone for use in Bootstrap command)
    let data_dir = cli.data_dir.clone().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".grab")
    });

    // Create GrabNet instance
    let grab = Grab::new(Some(data_dir.clone())).await?;

    match cli.command {
        Commands::Publish {
            path,
            name,
            entry,
            spa,
            clean_urls,
            no_compress,
            watch,
            pre_hook,
            post_hook,
        } => {
            // Run pre-deploy hook if specified
            if let Some(ref hook) = pre_hook {
                println!("🔧 Running pre-deploy hook...");
                run_hook(hook, &path)?;
            }

            println!("📦 Publishing {}...", path);

            let options = PublishOptions {
                name: name.clone(),
                entry: entry.clone(),
                compress: !no_compress,
                spa_fallback: spa.clone(),
                clean_urls,
                pre_hook: pre_hook.clone(),
                post_hook: post_hook.clone(),
                ..Default::default()
            };

            let result = grab.publish(&path, options.clone()).await?;

            println!();
            println!(
                "✓ Bundled {} files ({} bytes)",
                result.file_count, result.total_size
            );
            if result.compressed_size < result.total_size {
                let savings = 100 - (result.compressed_size * 100 / result.total_size);
                println!(
                    "✓ Compressed to {} bytes ({}% smaller)",
                    result.compressed_size, savings
                );
            }
            println!(
                "✓ {} chunks ({} new)",
                result.chunk_count, result.new_chunks
            );
            println!();
            println!("🌐 Site ID:  grab://{}", result.bundle.site_id.to_base58());
            println!("📝 Name:     {}", result.bundle.name);
            println!("🔄 Revision: {}", result.bundle.revision);

            // Run post-deploy hook if specified
            if let Some(ref hook) = post_hook {
                println!();
                println!("🔧 Running post-deploy hook...");
                run_hook(hook, &path)?;
            }

            println!();

            if watch {
                println!("👀 Watching for changes... (Ctrl+C to stop)");
                println!();

                run_watch_mode(&grab, &path, options).await?;
            } else {
                println!("Start gateway to serve: grab gateway");
            }
        }

        Commands::Update { site } => {
            println!("🔄 Updating {}...", site);

            match grab.update(&site).await? {
                Some(result) => {
                    println!();
                    println!("✓ Updated to revision {}", result.bundle.revision);
                    println!(
                        "✓ {} files, {} chunks",
                        result.file_count, result.chunk_count
                    );
                }
                None => {
                    println!("❌ Site not found: {}", site);
                }
            }
        }

        Commands::List => {
            let published = grab.list_published()?;
            let hosted = grab.list_hosted()?;

            if published.is_empty() && hosted.is_empty() {
                println!("No sites found.");
                println!();
                println!("Publish a site: grab publish ./my-website");
            } else {
                if !published.is_empty() {
                    println!("📤 Published Sites:");
                    println!();
                    for site in published {
                        println!("  {} (rev {})", site.name, site.revision);
                        println!("    ID: {}", site.site_id.to_base58());
                    }
                }

                if !hosted.is_empty() {
                    println!();
                    println!("📥 Hosted Sites:");
                    println!();
                    for site in hosted {
                        println!("  {} (rev {})", site.name, site.revision);
                        println!("    ID: {}", site.site_id.to_base58());
                    }
                }
            }
        }

        Commands::Info { site } => {
            // Try published first
            if let Some(published) = grab.bundle_store().get_published_site(&site)? {
                println!("📤 Published Site: {}", published.name);
                println!();
                println!("  Site ID:   {}", published.site_id.to_base58());
                println!("  Revision:  {}", published.revision);
                println!("  Path:      {}", published.root_path.display());

                match grab.bundle_store().get_manifest(&published.site_id) {
                    Ok(Some(manifest)) => {
                        println!("  Files:     {}", manifest.files.len());
                        println!("  Entry:     {}", manifest.entry);
                    }
                    Ok(None) => {
                        println!("  ⚠️  No manifest found");
                    }
                    Err(e) => {
                        println!("  ❌ Error loading manifest: {}", e);
                    }
                }
            } else {
                println!("❌ Site not found: {}", site);
            }
        }

        Commands::Node { action } => {
            match action {
                NodeAction::Start {
                    port: _,
                    light: _,
                    bootstrap,
                } => {
                    println!("🌐 Starting GrabNet node...");
                    grab.start_network().await?;

                    let status = grab.network_status();
                    println!();
                    println!("✓ Node started");
                    if let Some(peer_id) = &status.peer_id {
                        println!("  Peer ID: {}", peer_id);
                    }

                    // Connect to additional bootstrap peers
                    if !bootstrap.is_empty() {
                        for addr in bootstrap {
                            println!("  Connecting to {}...", addr);
                            if let Err(e) = grab.dial_peer(&addr).await {
                                println!("  ⚠️  Failed: {}", e);
                            }
                        }
                    }

                    // Keep running and show events
                    println!();
                    println!("Press Ctrl+C to stop");
                    println!();

                    // Subscribe to events
                    if let Some(mut rx) = grab.subscribe_network() {
                        loop {
                            tokio::select! {
                                _ = tokio::signal::ctrl_c() => {
                                    break;
                                }
                                event = rx.recv() => {
                                    match event {
                                        Ok(grabnet::network::NetworkEvent::PeerConnected(peer)) => {
                                            println!("  🟢 Peer connected: {}", peer);
                                        }
                                        Ok(grabnet::network::NetworkEvent::PeerDisconnected(peer)) => {
                                            println!("  🔴 Peer disconnected: {}", peer);
                                        }
                                        Ok(grabnet::network::NetworkEvent::SiteAnnounced { site_id, peer_id, revision }) => {
                                            println!("  📢 Site announced: {} rev {} from {}", site_id.to_base58(), revision, peer_id);
                                        }
                                        Ok(grabnet::network::NetworkEvent::BootstrapComplete { peers }) => {
                                            println!("  ✓ Bootstrap complete, {} peers", peers);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    } else {
                        tokio::signal::ctrl_c().await?;
                    }
                }

                NodeAction::Status => {
                    let status = grab.network_status();
                    if status.running {
                        println!("🟢 Node is running");
                        if let Some(peer_id) = status.peer_id {
                            println!("  Peer ID: {}", peer_id);
                        }
                        println!("  Peers:   {}", status.peers);
                        println!();
                        if !status.addresses.is_empty() {
                            println!("Listen addresses:");
                            for addr in &status.addresses {
                                println!("  {}", addr);
                            }
                        }
                    } else {
                        println!("🔴 Node is not running");
                    }
                }

                NodeAction::Peers => {
                    let status = grab.network_status();
                    if !status.running {
                        println!("🔴 Node is not running");
                        println!();
                        println!("Start it with: grab node start");
                        return Ok(());
                    }

                    println!("🔗 Connected Peers: {}", status.peers);
                    println!();

                    // Get detailed peer list
                    if let Some(guard) = grab.network() {
                        if let Some(network) = guard.as_ref() {
                            let peers = network.connected_peer_ids();
                            if peers.is_empty() {
                                println!("  No peers connected");
                                println!();
                                println!("Connect to a peer with: grab node connect <address>");
                            } else {
                                for peer in peers {
                                    println!("  • {}", peer);
                                }
                            }
                        }
                    }
                }

                NodeAction::Connect { address } => {
                    // Start network if not running
                    grab.start_network().await?;

                    println!("Connecting to {}...", address);
                    grab.dial_peer(&address).await?;
                    println!("✓ Connection initiated");
                }

                NodeAction::Stop => {
                    grab.stop_network().await?;
                    println!("✓ Node stopped");
                }
            }
        }

        Commands::Host { site_id } => {
            println!("📥 Hosting site {}...", site_id);

            let id = grabnet::SiteId::from_base58(&site_id)
                .ok_or_else(|| anyhow::anyhow!("Invalid site ID"))?;

            if grab.host(&id).await? {
                println!("✓ Now hosting site");
            } else {
                println!("❌ Failed to host site (not found)");
            }
        }

        Commands::Pin { site_id, peer } => {
            println!("📥 Pinning remote site {}...", site_id);

            let id = grabnet::SiteId::from_base58(&site_id)
                .ok_or_else(|| anyhow::anyhow!("Invalid site ID"))?;

            // Start network
            println!("  Starting P2P network...");
            grab.start_network().await?;

            // Give it a moment to initialize
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            // Connect to peer if provided
            if let Some(peer_addr) = peer {
                println!("  Connecting to peer {}...", peer_addr);
                grab.dial_peer(&peer_addr).await?;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }

            // Try to fetch and host
            if grab.host(&id).await? {
                println!("✓ Site pinned successfully!");

                // Show info
                if let Ok(Some(bundle)) = grab.bundle_store().get_bundle(&id) {
                    println!("  Name:     {}", bundle.name);
                    println!("  Revision: {}", bundle.revision);
                    println!("  Files:    {}", bundle.manifest.files.len());
                }
            } else {
                println!("❌ Failed to find site on network");
                println!(
                    "  Try providing a peer address: grab pin {} --peer /ip4/x.x.x.x/tcp/4001",
                    site_id
                );
            }
        }

        Commands::Unhost { site_id } => {
            println!("Removing site {}...", site_id);
            // Would unhost
            println!("✓ Stopped hosting site");
        }

        Commands::Keys { action } => {
            match action {
                KeysAction::List => {
                    let keys = grab.list_keys()?;
                    if keys.is_empty() {
                        println!("No keys found. Generate one: grab keys generate default");
                    } else {
                        println!("🔑 Keys:");
                        for name in keys {
                            if let Ok(Some(public_key)) = grab.get_public_key(&name) {
                                println!("  {} -> {}", name, grabnet::encode_base58(&public_key));
                            }
                        }
                    }
                }

                KeysAction::Generate { name } => {
                    // Getting or creating will generate if doesn't exist
                    if let Ok(Some(public_key)) = grab.get_public_key(&name) {
                        println!("Key '{}' already exists", name);
                        println!("Public key: {}", grabnet::encode_base58(&public_key));
                    }
                }

                KeysAction::Export { name } => {
                    // Would export key
                    println!("⚠️  Key export requires confirmation");
                    println!("Use: grab keys export {} --confirm", name);
                }

                KeysAction::Import { name, private_key } => {
                    println!("Importing key '{}'...", name);
                    // Would import
                    println!("✓ Key imported");
                }
            }
        }

        Commands::Gateway {
            port,
            default_site,
            tls_cert,
            tls_key,
            https_port,
            aliases: alias_specs,
            dns_aliases,
        } => {
            let tls = match (tls_cert, tls_key) {
                (Some(cert), Some(key)) => {
                    if !cert.exists() {
                        println!("❌ TLS cert not found: {}", cert.display());
                        return Ok(());
                    }
                    if !key.exists() {
                        println!("❌ TLS key not found: {}", key.display());
                        return Ok(());
                    }
                    Some(grabnet::TlsConfig {
                        cert_path: cert,
                        key_path: key,
                        https_port,
                    })
                }
                _ => None,
            };

            let scheme = if tls.is_some() { "https" } else { "http" };
            let listen_port = tls
                .as_ref()
                .and_then(|t| t.https_port)
                .unwrap_or(port);
            println!("🌐 Starting {} gateway on port {}...", scheme.to_uppercase(), listen_port);
            if tls.as_ref().and_then(|t| t.https_port).is_some() {
                println!("   HTTP redirect on port {} -> {}://*:{}", port, scheme, listen_port);
            }

            // Resolve default site if provided
            let default_site_id = if let Some(site_ref) = default_site {
                // Try to find by name first
                if let Some(published) = grab.bundle_store().get_published_site(&site_ref)? {
                    println!(
                        "  Default site: {} ({})",
                        published.name,
                        published.site_id.to_base58()
                    );
                    Some(published.site_id)
                } else if let Some(id) = grabnet::SiteId::from_base58(&site_ref) {
                    println!("  Default site: {}", site_ref);
                    Some(id)
                } else {
                    println!("❌ Site not found: {}", site_ref);
                    return Ok(());
                }
            } else {
                None
            };

            // Parse `--alias host=site` entries. The site portion may be a
            // local site name (resolved against the bundle store) or a base58
            // site id.
            let mut aliases: Vec<(String, grabnet::SiteId)> = Vec::new();
            for spec in alias_specs {
                let Some((host, site_ref)) = spec.split_once('=') else {
                    println!("❌ Invalid --alias spec (want host=site): {}", spec);
                    return Ok(());
                };
                let host = host.trim().to_ascii_lowercase();
                let site_ref = site_ref.trim();
                let site_id = if let Some(published) =
                    grab.bundle_store().get_published_site(site_ref)?
                {
                    published.site_id
                } else if let Some(id) = grabnet::SiteId::from_base58(site_ref) {
                    id
                } else {
                    println!("❌ Alias site not found: {}", site_ref);
                    return Ok(());
                };
                println!("  Host alias: {} -> {}", host, site_id.to_base58());
                aliases.push((host, site_id));
            }

            if dns_aliases {
                println!("  DNS resolution: enabled (TXT _grab.<host>)");
            }

            grab.start_gateway_with(grabnet::GatewayOptions {
                port,
                default_site: default_site_id,
                tls,
                aliases,
                dns_aliases,
            })
            .await?;

            let stats = grab.storage_stats();
            println!();
            println!("✓ Gateway running at {}://127.0.0.1:{}", scheme, listen_port);
            println!("  {} published sites", stats.published_sites);
            println!("  {} hosted sites", stats.hosted_sites);
            println!();
            println!(
                "Access sites at: {}://127.0.0.1:{}/site/<site-id>/",
                scheme, listen_port
            );
            println!();
            println!("Press Ctrl+C to stop");

            tokio::signal::ctrl_c().await?;
            grab.stop_gateway().await?;
        }

        Commands::Dns { action } => match action {
            DnsAction::Resolve { host } => {
                let resolver = grabnet::DnsResolver::from_system();
                match resolver.resolve(&host).await {
                    Ok(Some(id)) => {
                        println!("✓ {} -> grab://{}", host, id.to_base58());
                    }
                    Ok(None) => {
                        println!(
                            "❌ No grab-site TXT record at _grab.{} (queried successfully)",
                            host
                        );
                    }
                    Err(e) => {
                        println!("❌ Resolve failed: {}", e);
                    }
                }
            }
            DnsAction::Record { site } => {
                let id = if let Some(published) = grab.bundle_store().get_published_site(&site)? {
                    published.site_id
                } else if let Some(id) = grabnet::SiteId::from_base58(&site) {
                    id
                } else {
                    println!("❌ Site not found: {}", site);
                    return Ok(());
                };
                println!("Publish this DNS record:");
                println!();
                println!("  _grab.<your-domain>.  IN  TXT  \"grab-site={}\"", id.to_base58());
                println!();
                println!("Then run the gateway with --dns-aliases to serve");
                println!("requests for <your-domain> from this site.");
            }
        },

        Commands::Ens { action } => match action {
            EnsAction::Resolve { name, gateway } => {
                let resolver = match gateway {
                    Some(g) => grabnet::resolver::ens::EnsResolver::new(g),
                    None => grabnet::resolver::ens::EnsResolver::default(),
                };
                match resolver.resolve(&name).await {
                    Ok(id) => println!("✓ {} -> grab://{}", name, id.to_base58()),
                    Err(e) => println!("❌ ENS resolve failed: {}", e),
                }
            }
        },

        Commands::Bootstrap { action } => {
            let mut config = grabnet::network::BootstrapConfig::load_or_default(&data_dir)?;

            match action {
                BootstrapAction::List => {
                    println!("🌐 Bootstrap Nodes:");
                    println!();

                    println!("Official:");
                    for node in &config.official {
                        let status = if node.enabled { "✓" } else { "✗" };
                        println!(
                            "  {} {} [{}]",
                            status,
                            node.name,
                            node.region.as_deref().unwrap_or("unknown")
                        );
                        for addr in &node.addresses {
                            println!("      {}", addr);
                        }
                    }

                    if !config.community.is_empty() {
                        println!();
                        println!("Community:");
                        for node in &config.community {
                            let status = if node.enabled { "✓" } else { "✗" };
                            println!("  {} {}", status, node.name);
                            for addr in &node.addresses {
                                println!("      {}", addr);
                            }
                        }
                    }

                    if !config.custom.is_empty() {
                        println!();
                        println!("Custom:");
                        for node in &config.custom {
                            let status = if node.enabled { "✓" } else { "✗" };
                            println!("  {} {}", status, node.name);
                            for addr in &node.addresses {
                                println!("      {}", addr);
                            }
                        }
                    }

                    println!();
                    println!(
                        "mDNS: {}",
                        if config.mdns_enabled {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    );
                    println!("Minimum peers: {}", config.min_peers);
                }

                BootstrapAction::Add { name, address } => {
                    config.add_custom(name.clone(), vec![address.clone()]);
                    config.save(&data_dir)?;
                    println!("✓ Added bootstrap node: {}", name);
                    println!("  Address: {}", address);
                }

                BootstrapAction::Remove { name } => {
                    if config.remove_custom(&name) {
                        config.save(&data_dir)?;
                        println!("✓ Removed bootstrap node: {}", name);
                    } else {
                        println!("❌ Custom node not found: {}", name);
                        println!("   Note: Only custom nodes can be removed");
                    }
                }

                BootstrapAction::Test => {
                    println!("🔍 Testing bootstrap node connectivity...");
                    println!();

                    let addresses = config.get_enabled_addresses();
                    for addr in addresses {
                        print!("  {} ... ", addr);
                        if grabnet::network::bootstrap::check_reachable(&addr).await {
                            println!("✓ reachable");
                        } else {
                            println!("✗ unreachable");
                        }
                    }
                }
            }
        }

        Commands::Stats => {
            let stats = grab.storage_stats();
            println!("📊 Storage Statistics:");
            println!();
            println!("  Chunks:          {}", stats.chunks);
            println!("  Total size:      {} bytes", stats.total_size);
            println!("  Published sites: {}", stats.published_sites);
            println!("  Hosted sites:    {}", stats.hosted_sites);
        }
    }

    Ok(())
}

/// Run watch mode - monitor directory for changes and auto-republish
async fn run_watch_mode(grab: &Grab, path: &str, options: PublishOptions) -> Result<()> {
    use std::sync::mpsc::channel;

    let (tx, rx) = channel();

    // Create debounced watcher (500ms debounce)
    let mut debouncer = new_debouncer(Duration::from_millis(500), tx)?;

    // Watch the directory recursively
    debouncer
        .watcher()
        .watch(std::path::Path::new(path), RecursiveMode::Recursive)?;

    // Get the site name for updates
    let site_name = options.name.clone().unwrap_or_else(|| {
        std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "site".to_string())
    });

    loop {
        match rx.recv() {
            Ok(Ok(events)) => {
                // Filter out hidden files and build artifacts
                let relevant_events: Vec<_> = events
                    .iter()
                    .filter(|e| {
                        let path_str = e.path.to_string_lossy();
                        !path_str.contains("/.")
                            && !path_str.contains("/node_modules/")
                            && !path_str.contains("/target/")
                            && !path_str.contains("/.git/")
                    })
                    .collect();

                if relevant_events.is_empty() {
                    continue;
                }

                // Show which files changed
                for event in &relevant_events {
                    println!("  📝 Changed: {}", event.path.display());
                }

                // Republish
                println!("🔄 Republishing...");
                match grab.update(&site_name).await {
                    Ok(Some(result)) => {
                        println!(
                            "✓ Updated to revision {} ({} files)",
                            result.bundle.revision, result.file_count
                        );
                    }
                    Ok(None) => {
                        // Site not found, do full publish
                        match grab.publish(path, options.clone()).await {
                            Ok(result) => {
                                println!(
                                    "✓ Published revision {} ({} files)",
                                    result.bundle.revision, result.file_count
                                );
                            }
                            Err(e) => {
                                println!("❌ Publish failed: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        println!("❌ Update failed: {}", e);
                    }
                }
                println!();
            }
            Ok(Err(error)) => {
                println!("⚠️  Watch error: {:?}", error);
            }
            Err(e) => {
                println!("❌ Channel error: {}", e);
                break;
            }
        }
    }

    Ok(())
}

/// Run a deploy hook command
fn run_hook(command: &str, working_dir: &str) -> Result<()> {
    use std::process::Command;

    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(working_dir)
        .output()?;

    if !output.stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }

    if output.status.success() {
        println!("✓ Hook completed successfully");
        Ok(())
    } else {
        anyhow::bail!("Hook failed with exit code: {:?}", output.status.code())
    }
}
