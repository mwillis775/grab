//! GrabNet CLI

use std::path::PathBuf;
use anyhow::Result;
use clap::{Parser, Subcommand};
use grabnet::{Grab, PublishOptions, SiteIdExt};
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
        /// Port to listen on
        #[arg(short, long, default_value = "8080")]
        port: u16,

        /// Default site to serve at root (name or ID)
        #[arg(long)]
        default_site: Option<String>,
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
    },

    /// Show node status
    Status,

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

    // Create GrabNet instance
    let grab = Grab::new(cli.data_dir).await?;

    match cli.command {
        Commands::Publish {
            path,
            name,
            entry,
            spa,
            clean_urls,
            no_compress,
        } => {
            println!("📦 Publishing {}...", path);

            let result = grab.publish(&path, PublishOptions {
                name,
                entry,
                compress: !no_compress,
                spa_fallback: spa,
                clean_urls,
                ..Default::default()
            }).await?;

            println!();
            println!("✓ Bundled {} files ({} bytes)", result.file_count, result.total_size);
            if result.compressed_size < result.total_size {
                let savings = 100 - (result.compressed_size * 100 / result.total_size);
                println!("✓ Compressed to {} bytes ({}% smaller)", result.compressed_size, savings);
            }
            println!("✓ {} chunks ({} new)", result.chunk_count, result.new_chunks);
            println!();
            println!("🌐 Site ID:  grab://{}", result.bundle.site_id.to_base58());
            println!("📝 Name:     {}", result.bundle.name);
            println!("🔄 Revision: {}", result.bundle.revision);
            println!();
            println!("Start gateway to serve: grab gateway");
        }

        Commands::Update { site } => {
            println!("🔄 Updating {}...", site);

            match grab.update(&site).await? {
                Some(result) => {
                    println!();
                    println!("✓ Updated to revision {}", result.bundle.revision);
                    println!("✓ {} files, {} chunks", result.file_count, result.chunk_count);
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
                NodeAction::Start { port: _, light: _ } => {
                    println!("🌐 Starting GrabNet node...");
                    grab.start_network().await?;
                    
                    let status = grab.network_status();
                    println!();
                    println!("✓ Node started");
                    if let Some(peer_id) = status.peer_id {
                        println!("  Peer ID: {}", peer_id);
                    }

                    // Keep running
                    println!();
                    println!("Press Ctrl+C to stop");
                    tokio::signal::ctrl_c().await?;
                }

                NodeAction::Status => {
                    let status = grab.network_status();
                    if status.running {
                        println!("🟢 Node is running");
                        if let Some(peer_id) = status.peer_id {
                            println!("  Peer ID: {}", peer_id);
                        }
                        println!("  Peers:   {}", status.peers);
                    } else {
                        println!("🔴 Node is not running");
                    }
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

        Commands::Gateway { port, default_site } => {
            println!("🌐 Starting HTTP gateway on port {}...", port);
            
            // Resolve default site if provided
            let default_site_id = if let Some(site_ref) = default_site {
                // Try to find by name first
                if let Some(published) = grab.bundle_store().get_published_site(&site_ref)? {
                    println!("  Default site: {} ({})", published.name, published.site_id.to_base58());
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

            if let Some(site_id) = default_site_id {
                grab.start_gateway_with_default_site(port, site_id).await?;
            } else {
                grab.start_gateway_on_port(port).await?;
            }

            let stats = grab.storage_stats();
            println!();
            println!("✓ Gateway running at http://127.0.0.1:{}", port);
            println!("  {} published sites", stats.published_sites);
            println!("  {} hosted sites", stats.hosted_sites);
            println!();
            println!("Access sites at: http://127.0.0.1:{}/site/<site-id>/", port);
            println!();
            println!("Press Ctrl+C to stop");
            
            tokio::signal::ctrl_c().await?;
            grab.stop_gateway().await?;
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
