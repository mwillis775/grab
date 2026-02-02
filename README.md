# GrabNet 🌐

> A decentralized web hosting protocol written in Rust. Purpose-built for websites.

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)

GrabNet is a peer-to-peer protocol for publishing and hosting websites on a decentralized network. Unlike traditional hosting, your site gets a permanent, stable address that never changes—even when you update the content. Think of it as "git for websites" meets "BitTorrent for hosting."

## Table of Contents

- [Why GrabNet?](#why-grabnet)
- [Live Sites](#live-sites)
- [Features](#features)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [CLI Reference](#cli-reference)
- [P2P Networking](#p2p-networking)
- [Architecture](#architecture)
- [Protocol Specification](#protocol-specification)
- [User Uploads](#user-uploads)
- [Configuration](#configuration)
- [SDK Usage](#sdk-usage)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)
- [License](#license)

---

## Why GrabNet?

### The Problem with Current Solutions

**Traditional Hosting**: Centralized servers mean single points of failure, censorship vulnerability, and ongoing costs.

**IPFS**: Great for content-addressed storage, but:
- Addresses change on every update (content-addressed)
- IPNS is slow (30+ seconds to resolve)
- No first-class website concepts
- Requires pinning services or your own infrastructure
- No delta updates (re-upload everything)
- Depends on HTTP gateways for web access

### How GrabNet Solves These

| Problem | Traditional | IPFS | GrabNet |
|---------|-------------|------|---------|
| Address stability | ✅ Fixed | ❌ Changes on update | ✅ Stable forever |
| Name resolution | ✅ Fast DNS | ❌ IPNS is slow | ✅ <100ms via DHT |
| Update efficiency | ✅ Incremental | ❌ Full re-upload | ✅ Delta sync |
| Censorship resistance | ❌ Single server | ✅ Distributed | ✅ Distributed |
| Native HTTP serving | ✅ Yes | ❌ Needs gateway | ✅ Built-in |
| User-generated content | ✅ Yes | ❌ Complex | ✅ Built-in |
| Cost | ❌ Ongoing | ✅ Free (if pinned) | ✅ Free |
| Setup complexity | ❌ Complex | ❌ Complex | ✅ Single binary |

---

## Live Sites

Sites currently hosted on GrabNet:

| Site | Description |
|------|-------------|
| [rootedrevival.us](https://rootedrevival.us) | Regenerative landscaping business + Open Scholar knowledge repository |

*Running your own GrabNet site? Open a PR to add it here!*

---

## Features

### Core Features

- **🔒 Stable Addresses** - Your site ID is `blake3(publisher_key || site_name)`. It never changes, even after thousands of updates.

- **⚡ Delta Sync** - Merkle tree diffing means only changed chunks are transferred. Update a 10MB site by sending just 50KB.

- **🌐 Native HTTP Gateway** - Serve sites directly over HTTP from any node. No external gateway required.

- **📦 Single Binary** - No Node.js, Python, or runtime dependencies. Just one Rust binary.

- **🔐 Cryptographic Ownership** - Ed25519 signatures prove you own your content. No one can impersonate your site.

- **📤 User Uploads** - Built-in support for user-generated content with configurable policies and moderation.

- **🗜️ Automatic Compression** - Gzip compression reduces bandwidth by 60-80% for text-based content.

### P2P Features

- **🔄 Persistent Identity** - Your peer ID is stable across restarts, making you discoverable on the network.

- **📌 Site Pinning** - Pin remote sites to host them locally and contribute to their availability.

- **🔁 Auto-Replication** - Sites you host automatically sync when publishers push updates.

- **👀 Watch Mode** - Develop with `--watch` flag for automatic republishing on file changes.

- **🔧 Deploy Hooks** - Run custom commands before and after publishing (build scripts, notifications, etc.).

### Technical Highlights

- **BLAKE3** for blazing-fast content hashing
- **Ed25519** for secure cryptographic signatures
- **libp2p** for proven P2P networking (Kademlia DHT, Gossipsub, Noise encryption)
- **sled** for embedded, zero-config database storage
- **axum** for high-performance HTTP serving

---

## Installation

### From Source (Recommended)

```bash
# Clone the repository
git clone https://github.com/mwillis775/grab.git
cd grab

# Build release binary
cargo build --release

# Binary is at ./target/release/grab
# Optionally, install to your PATH:
sudo cp target/release/grab /usr/local/bin/
```

### Requirements

- **Rust 1.70+** - Install via [rustup](https://rustup.rs/)
- **Linux/macOS** - Windows support coming soon

### Verify Installation

```bash
grab --version
# grab 0.1.0

grab --help
# Shows all available commands
```

---

## Quick Start

### 1. Publish Your First Website

```bash
# Create a simple website
mkdir my-site
echo '<h1>Hello from GrabNet!</h1>' > my-site/index.html

# Publish it
grab publish my-site --name hello-world

# Output:
# 📦 Publishing my-site...
# 
# ✓ Bundled 1 files (32 bytes)
# ✓ Compressed to 52 bytes
# ✓ 1 chunks (1 new)
# 
# 🌐 Site ID:  grab://7xK9pL2nMqR5tY8vW3bN6cF4hJ1dS9aE2gZ...
# 📝 Name:     hello-world
# 🔄 Revision: 1
# 
# Start gateway to serve: grab gateway
```

### 2. Start the Gateway

```bash
grab gateway --port 8080

# Output:
# 🌐 Starting HTTP gateway on port 8080...
# ✓ Gateway running at http://127.0.0.1:8080
#   1 published sites
#   0 hosted sites
# 
# Access sites at: http://127.0.0.1:8080/site/<site-id>/
```

### 3. View Your Site

Open your browser to:
```
http://127.0.0.1:8080/site/7xK9pL2nMqR5tY8vW3bN6cF4hJ1dS9aE2gZ.../
```

Or use curl:
```bash
curl http://127.0.0.1:8080/site/7xK9pL2nMqR5tY8vW3bN6cF4hJ1dS9aE2gZ.../
# <h1>Hello from GrabNet!</h1>
```

### 4. Update Your Site

```bash
# Make changes
echo '<h1>Updated!</h1><p>Version 2</p>' > my-site/index.html

# Update (only changed chunks are processed)
grab update hello-world

# Output:
# 🔄 Updating hello-world...
# ✓ Updated to revision 2
# ✓ 1 files, 1 chunks
```

The site ID stays the same! Refresh your browser to see the update.

---

## CLI Reference

### Publishing Commands

```bash
# Publish a website directory
grab publish <path> [options]

Options:
  --name <name>       Site name (defaults to directory name)
  --entry <file>      Entry point file (defaults to index.html)
  --spa <fallback>    Enable SPA mode with fallback file
  --clean-urls        Enable clean URLs (/about → /about.html)
  --no-compress       Disable gzip compression
  --watch             Watch for changes and auto-republish
  --pre-hook <cmd>    Command to run before publishing
  --post-hook <cmd>   Command to run after publishing

# Examples:
grab publish ./my-blog --name personal-blog
grab publish ./react-app --spa index.html
grab publish ./docs --clean-urls

# Watch mode - auto-republish on file changes
grab publish ./my-site --name dev-site --watch

# With build hooks
grab publish ./gatsby-site --pre-hook "npm run build" --post-hook "curl -X POST https://hooks.example.com/deployed"
```

```bash
# Update an existing site
grab update <name-or-id>

# Examples:
grab update personal-blog
grab update 7xK9pL2nMqR5tY8vW3bN6cF4hJ1dS9aE2gZ
```

### Site Management

```bash
# List all your sites
grab list

# Output:
# 📤 Published Sites:
#   personal-blog (rev 5)
#     ID: 7xK9pL2nMqR5tY8vW3bN6cF4hJ1dS9aE2gZ
#   
#   portfolio (rev 12)
#     ID: 9aB3cD5eF7gH1iJ3kL5mN7oP9qR1sT3uV
```

```bash
# Show detailed site information
grab info <name-or-id>

# Output:
# 📤 Published Site: personal-blog
#   Site ID:   7xK9pL2nMqR5tY8vW3bN6cF4hJ1dS9aE2gZ
#   Revision:  5
#   Path:      /home/user/sites/personal-blog
#   Files:     23
#   Entry:     index.html
```

### Gateway Commands

```bash
# Start HTTP gateway (default port 8080)
grab gateway

# Custom port
grab gateway --port 3000

# The gateway serves all your published and hosted sites
```

**Gateway Endpoints:**

| Endpoint | Description |
|----------|-------------|
| `GET /health` | Health check |
| `GET /api/sites` | List all sites |
| `GET /api/sites/:id` | Get site info |
| `GET /api/sites/:id/manifest` | Get site manifest |
| `GET /site/:id/` | Serve site index |
| `GET /site/:id/*path` | Serve site files |

### Network Commands

```bash
# Start P2P node (full node - hosts content)
grab node start

# Connect to bootstrap peers on start
grab node start --bootstrap /ip4/1.2.3.4/tcp/4001/p2p/12D3KooW...

# Light mode (no hosting, just publishing)
grab node start --light

# Custom port
grab node start --port 4002

# Connect to a specific peer
grab node connect /ip4/1.2.3.4/tcp/4001/p2p/12D3KooW...

# Check node status
grab node status

# Stop the node
grab node stop
```

### Pinning Remote Sites

```bash
# Pin a site from the network
grab pin <site-id>

# Pin with a specific peer address
grab pin <site-id> --peer /ip4/1.2.3.4/tcp/4001/p2p/12D3KooW...

# Example:
grab pin AtnArdZARzYJ7sTKYdrn4HHYsofuSe9gonDNsrqwFFa1 --peer /ip4/192.168.1.100/tcp/4001
```

This fetches the site from the network and hosts it locally. Your node will then serve the site to other peers and through the HTTP gateway.

### Hosting Commands

```bash
# Host (pin) a local site
grab host <site-id>

# Stop hosting a site
grab unhost <site-id>
```

### Key Management

```bash
# List all signing keys
grab keys list

# Output:
# 🔑 Keys:
#   default -> 5Ht7YkMnB3xR9pL2qW4sT6vU8wX0zA1cD...

# Generate a new key
grab keys generate my-key

# Export a key (for backup)
grab keys export my-key

# Import a key
grab keys import my-key <base58-private-key>
```

### Statistics

```bash
grab stats

# Output:
# 📊 Storage Statistics:
#   Chunks:          1,247
#   Total size:      45,234,567 bytes
#   Published sites: 3
#   Hosted sites:    12
```

### Global Options

```bash
--data-dir <path>   # Custom data directory (default: ~/.grab)
-v, --verbose       # Enable debug logging
-h, --help          # Show help
-V, --version       # Show version
```

---

## P2P Networking

GrabNet uses [libp2p](https://libp2p.io/) for peer-to-peer networking with the following protocols:

### Protocols Used

| Protocol | Purpose |
|----------|---------|
| **Kademlia DHT** | Peer discovery and content routing |
| **Gossipsub** | Real-time site announcements and updates |
| **mDNS** | Local network peer discovery |
| **Identify** | Peer identification and address exchange |
| **Noise** | Encrypted connections |

### Network Events

When running a node, you'll see events like:

```
🌐 Starting GrabNet node...
✓ Node started
  Peer ID: 12D3KooWMrzk1N4k9sqP4vxmii8rbqNymzJKDRYxtH5ZTuuqG21w

  🟢 Peer connected: 12D3KooWHK5mD...
  📢 Site announced: AtnArdZA... rev 3 from 12D3KooWHK5mD...
  ✓ Bootstrap complete, 5 peers
```

### Hosting Content

When you run a node, you:
1. **Announce** your published sites to the DHT
2. **Respond** to requests from other peers
3. **Replicate** content to help the network

Other nodes can find your sites by querying the DHT and request content directly.

### Auto-Replication

GrabNet automatically replicates content you're hosting:

1. **Site Announcements**: When a publisher announces a new revision via Gossipsub
2. **Revision Check**: If you're hosting that site and your revision is older
3. **Fetch Update**: Automatically request the new manifest and chunks
4. **Store Locally**: Save the updated content to serve to others

This ensures hosted sites stay up-to-date without manual intervention.

### Persistent Identity

Your peer ID is derived from an Ed25519 keypair stored in `~/.grab/keys.db`. This means:
- Your peer ID stays the same across restarts
- Other peers can reliably find you on the network
- Sites you announce remain discoverable

### Firewall Configuration

For best connectivity, open port 4001 (TCP) on your firewall. GrabNet also works behind NAT using the libp2p hole-punching and relay protocols.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                            CLI                                   │
│                  grab publish | update | gateway                 │
├─────────────────────────────────────────────────────────────────┤
│                         Grab SDK                                 │
│               High-level API for all operations                  │
├─────────────────────────────────────────────────────────────────┤
│                       HTTP Gateway                               │
│               axum server for HTTP/HTTPS access                  │
├───────────────────────────┬─────────────────────────────────────┤
│       User Content        │           Publisher                  │
│   Uploads & Moderation    │      Bundle, Chunk, Sign             │
├───────────────────────────┴─────────────────────────────────────┤
│                        P2P Network                               │
│          libp2p: Kademlia DHT + Gossipsub + Noise               │
├─────────────────────────────────────────────────────────────────┤
│                         Storage                                  │
│              sled: Chunks, Bundles, Keys, Metadata              │
├─────────────────────────────────────────────────────────────────┤
│                          Crypto                                  │
│               BLAKE3 hashing + Ed25519 signing                   │
└─────────────────────────────────────────────────────────────────┘
```

### Data Flow

**Publishing:**
1. Scan directory for files
2. Chunk files (256KB default)
3. Hash each chunk with BLAKE3
4. Compress chunks with gzip
5. Build Merkle tree
6. Create manifest with file metadata
7. Sign bundle with Ed25519 private key
8. Store in local sled database
9. Announce to DHT (if network running)

**Serving:**
1. HTTP request comes in
2. Parse site ID from URL
3. Load manifest from storage
4. Resolve file path (clean URLs, SPA fallback)
5. Fetch chunks from storage
6. Decompress if needed
7. Return with proper Content-Type and caching headers

---

## Protocol Specification

### Site ID Generation

Site IDs are stable, deterministic identifiers:

```
site_id = BLAKE3(publisher_public_key || site_name)
```

This means:
- Same publisher + same name = same ID forever
- Different publishers can't claim your site names
- ID doesn't change when content updates

### WebBundle Format

```rust
struct WebBundle {
    site_id: [u8; 32],       // Stable identifier
    name: String,             // Human-readable name
    revision: u64,            // Auto-incrementing version
    root_hash: [u8; 32],      // Merkle root of all content
    publisher: [u8; 32],      // Ed25519 public key
    signature: Vec<u8>,       // Ed25519 signature (64 bytes)
    manifest: SiteManifest,   // File listings and routing
    created_at: u64,          // Unix timestamp (milliseconds)
}

struct SiteManifest {
    files: Vec<FileEntry>,
    entry: String,            // Default: "index.html"
    routes: Option<RouteConfig>,
    headers: Option<Vec<HeaderRule>>,
}

struct FileEntry {
    path: String,             // Relative path
    hash: [u8; 32],           // Content hash
    size: u64,                // Original size
    mime_type: String,        // MIME type
    chunks: Vec<[u8; 32]>,    // Chunk IDs
    compression: Option<Compression>,
}
```

### Signature Verification

Bundles are signed to prove authenticity:

```
message = site_id || revision (as u64 LE bytes) || root_hash
signature = Ed25519_Sign(private_key, message)
```

Anyone can verify:
```
Ed25519_Verify(publisher_public_key, message, signature)
```

### Network Messages

| Message | Direction | Description |
|---------|-----------|-------------|
| `FindSite(site_id)` | Request | Find nodes hosting a site |
| `SiteHosts(peers)` | Response | List of hosting peers |
| `GetManifest(site_id, rev)` | Request | Fetch site manifest |
| `Manifest(data)` | Response | Serialized manifest |
| `GetChunks(chunk_ids)` | Request | Fetch content chunks |
| `Chunks(data)` | Response | Chunk data |
| `Announce(site_id, rev)` | Broadcast | Declare hosting a site |
| `PushUpdate(bundle)` | Unicast | Push update to hosts |

---

## User Uploads

GrabNet supports user-generated content for dynamic websites.

### Enable Uploads (SDK)

```rust
use grabnet::{Grab, UploadPolicy, ModerationMode};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let grab = Grab::with_uploads(None).await?;
    
    grab.enable_uploads(&site_id, UploadPolicy {
        max_file_size: 10 * 1024 * 1024,      // 10 MB per file
        max_storage_per_user: 100 * 1024 * 1024, // 100 MB total per user
        allowed_types: vec![
            "image/*".into(),
            "video/mp4".into(),
            "application/pdf".into(),
        ],
        moderation: ModerationMode::Pre,       // Review before visible
        rate_limit: 60,                         // 60 uploads per hour
        ..Default::default()
    })?;
    
    grab.start_gateway().await?;
    Ok(())
}
```

### Upload via HTTP

```bash
# Upload a file
curl -X POST http://localhost:8080/api/sites/{site_id}/uploads \
  -H "Content-Type: image/png" \
  -H "X-Upload-Filename: avatar.png" \
  -H "X-User-Id: user123" \
  --data-binary @avatar.png

# Response:
# {
#   "upload_id": "upl_7xK9pL2nMq...",
#   "url": "/uploads/upl_7xK9pL2nMq...",
#   "status": "pending_review"
# }
```

### Moderation Modes

- `None` - No moderation, uploads immediately visible
- `Pre` - Uploads must be approved before visible
- `Post` - Uploads visible immediately, can be removed later

---

## Configuration

GrabNet stores configuration at `~/.grab/config.json`:

```json
{
  "network": {
    "port": 4001,
    "listen_addresses": ["/ip4/0.0.0.0/tcp/4001"],
    "bootstrap_peers": [
      "/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMG..."
    ],
    "max_connections": 100
  },
  "gateway": {
    "port": 8080,
    "host": "127.0.0.1",
    "cors": true
  },
  "storage": {
    "cache_size_mb": 256,
    "max_storage_gb": 0
  },
  "publisher": {
    "chunk_size": 262144,
    "compress": true
  }
}
```

### Configuration Options

| Section | Option | Default | Description |
|---------|--------|---------|-------------|
| `network.port` | `4001` | P2P listening port |
| `network.bootstrap_peers` | `[]` | Initial peers to connect to |
| `network.max_connections` | `100` | Maximum peer connections |
| `gateway.port` | `8080` | HTTP gateway port |
| `gateway.host` | `127.0.0.1` | Gateway bind address |
| `gateway.cors` | `true` | Enable CORS headers |
| `storage.cache_size_mb` | `256` | In-memory chunk cache |
| `storage.max_storage_gb` | `0` | Max storage (0 = unlimited) |
| `publisher.chunk_size` | `262144` | Chunk size in bytes (256KB) |
| `publisher.compress` | `true` | Enable gzip compression |

### Environment Variables

```bash
GRAB_DATA_DIR=/custom/path   # Override data directory
RUST_LOG=grabnet=debug       # Enable debug logging
```

---

## SDK Usage

### Basic Publishing

```rust
use grabnet::{Grab, PublishOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create instance (uses ~/.grab by default)
    let grab = Grab::new(None).await?;
    
    // Publish a website
    let result = grab.publish("./my-website", PublishOptions {
        name: Some("my-site".into()),
        compress: true,
        ..Default::default()
    }).await?;
    
    println!("Published: grab://{}", result.bundle.site_id);
    println!("Revision: {}", result.bundle.revision);
    
    Ok(())
}
```

### Running the Gateway

```rust
use grabnet::Grab;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let grab = Grab::new(None).await?;
    
    // Start gateway on custom port
    grab.start_gateway_on_port(3000).await?;
    
    println!("Gateway running on http://127.0.0.1:3000");
    
    // Keep running until Ctrl+C
    tokio::signal::ctrl_c().await?;
    grab.stop_gateway().await?;
    
    Ok(())
}
```

### Hosting Remote Sites

```rust
use grabnet::{Grab, SiteId, SiteIdExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let grab = Grab::new(None).await?;
    
    // Start network to discover sites
    grab.start_network().await?;
    
    // Host someone else's site
    let site_id = SiteId::from_base58("7xK9pL2nMqR5tY8vW3bN6cF4hJ1dS9aE2gZ")
        .expect("Invalid site ID");
    
    if grab.host(&site_id).await? {
        println!("Now hosting site!");
    } else {
        println!("Site not found on network");
    }
    
    Ok(())
}
```

---

## Troubleshooting

### "Address already in use"

Another process is using the port:

```bash
# Find what's using port 8080
lsof -i :8080

# Kill the process or use a different port
grab gateway --port 8081
```

### "Could not acquire lock"

Another grab process has the database open:

```bash
# Kill any running grab processes
pkill -9 grab

# Try again
grab gateway
```

### Site not loading in browser

1. Check the gateway is running:
   ```bash
   curl http://127.0.0.1:8080/health
   # Should return: {"status":"ok","gateway":"grabnet"}
   ```

2. Verify the site exists:
   ```bash
   curl http://127.0.0.1:8080/api/sites
   ```

3. Check you're using the full site ID with trailing slash:
   ```
   http://127.0.0.1:8080/site/FULL_SITE_ID_HERE/
   ```

### Manifest deserialization errors

This usually means the database is corrupted or from an incompatible version:

```bash
# Backup and reset
mv ~/.grab ~/.grab.backup
grab publish ./my-site --name my-site
```

### Debug logging

```bash
# Enable verbose logging
grab -v gateway

# Or set environment variable
RUST_LOG=grabnet=debug grab gateway
```

---

## Contributing

Contributions are welcome! Here's how to get started:

### Development Setup

```bash
# Clone the repo
git clone https://github.com/mwillis775/grab.git
cd grab

# Build in debug mode
cargo build

# Run tests
cargo test

# Run with logging
RUST_LOG=grabnet=debug cargo run -- publish ./test-site
```

### Project Structure

```
grab/
├── Cargo.toml           # Dependencies
├── src/
│   ├── lib.rs           # Main SDK
│   ├── types.rs         # Core data types
│   ├── crypto/          # Hashing & signing
│   │   ├── mod.rs
│   │   ├── hash.rs      # BLAKE3 hashing
│   │   └── sign.rs      # Ed25519 signing
│   ├── storage/         # Persistence
│   │   ├── mod.rs
│   │   ├── chunks.rs    # Content chunks
│   │   ├── bundles.rs   # Site bundles
│   │   └── keys.rs      # Key management
│   ├── network/         # P2P networking
│   │   ├── mod.rs
│   │   ├── node.rs      # Network node
│   │   ├── behaviour.rs # libp2p behaviour
│   │   └── protocol.rs  # Wire protocol
│   ├── gateway/         # HTTP server
│   │   ├── mod.rs
│   │   └── server.rs    # axum handlers
│   ├── content/         # User uploads
│   │   ├── mod.rs
│   │   └── uploads.rs
│   ├── publisher/       # Site bundling
│   │   ├── mod.rs
│   │   └── bundle.rs
│   └── bin/
│       └── grab.rs      # CLI binary
```

### Coding Guidelines

- Use `rustfmt` for formatting
- Add tests for new functionality
- Update documentation for public APIs
- Keep commits focused and well-described

### Submitting Changes

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/my-feature`)
3. Make your changes
4. Run tests (`cargo test`)
5. Commit (`git commit -am 'Add my feature'`)
6. Push (`git push origin feature/my-feature`)
7. Open a Pull Request

---

## Roadmap

- [ ] **Browser Extension** - Resolve `grab://` URLs in the browser
- [ ] **HTTPS/TLS Support** - Built-in TLS termination
- [ ] **ENS Integration** - Map ENS names to site IDs
- [ ] **DNS Integration** - TXT record based resolution
- [ ] **Economic Incentives** - Token-based hosting rewards
- [ ] **CDN Integration** - Edge caching for popular sites
- [ ] **WASM Browser Nodes** - Run nodes directly in browsers
- [ ] **Mobile Apps** - iOS/Android gateway apps

---

## License

GrabNet is dual-licensed under MIT and Apache 2.0. You may choose either license.

- [MIT License](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)

---

## Credits

Built by [Michael Willis](https://github.com/mwillis775) for [Rooted Revival](https://rootedrevival.us).

Powered by:
- [libp2p](https://libp2p.io/) - P2P networking
- [sled](https://sled.rs/) - Embedded database
- [axum](https://github.com/tokio-rs/axum) - Web framework
- [BLAKE3](https://github.com/BLAKE3-team/BLAKE3) - Cryptographic hashing
- [ed25519-dalek](https://github.com/dalek-cryptography/ed25519-dalek) - Signatures
