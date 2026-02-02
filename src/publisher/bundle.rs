//! Website bundling and publishing

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use std::io::Read;
use anyhow::{Result, Context};
use walkdir::WalkDir;
use flate2::read::GzEncoder;
use flate2::Compression as GzCompression;

use crate::types::{
    WebBundle, SiteManifest, FileEntry, PublishedSite,
    RouteConfig, Compression, SiteId, ChunkId,
};
use crate::storage::{ChunkStore, BundleStore, KeyStore};
use crate::crypto::{hash, sign_bundle, SiteIdExt, MerkleTree, encode_base58};

/// Options for publishing a website
#[derive(Debug, Clone, Default)]
pub struct PublishOptions {
    /// Site name (defaults to directory name)
    pub name: Option<String>,
    /// Entry point (defaults to index.html)
    pub entry: Option<String>,
    /// Key name to use for signing
    pub key_name: Option<String>,
    /// Enable gzip compression
    pub compress: bool,
    /// Chunk size in bytes
    pub chunk_size: Option<usize>,
    /// SPA fallback path
    pub spa_fallback: Option<String>,
    /// Enable clean URLs
    pub clean_urls: bool,
    /// Command to run before publishing (pre-deploy hook)
    pub pre_hook: Option<String>,
    /// Command to run after publishing (post-deploy hook)
    pub post_hook: Option<String>,
}

/// Result of publishing a website
#[derive(Debug)]
pub struct PublishResult {
    /// The published bundle
    pub bundle: WebBundle,
    /// Files included
    pub file_count: usize,
    /// Total size before compression
    pub total_size: u64,
    /// Size after compression
    pub compressed_size: u64,
    /// Chunks created
    pub chunk_count: usize,
    /// New chunks (not deduplicated)
    pub new_chunks: usize,
}

/// Website publisher
pub struct Publisher {
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    key_store: Arc<KeyStore>,
}

impl Publisher {
    /// Create a new publisher
    pub fn new(
        chunk_store: Arc<ChunkStore>,
        bundle_store: Arc<BundleStore>,
        key_store: Arc<KeyStore>,
    ) -> Self {
        Self {
            chunk_store,
            bundle_store,
            key_store,
        }
    }

    /// Publish a website directory
    pub async fn publish(&self, path: &str, options: PublishOptions) -> Result<PublishResult> {
        let root_path = PathBuf::from(path).canonicalize()
            .context("Failed to resolve path")?;

        if !root_path.is_dir() {
            anyhow::bail!("Path is not a directory: {}", path);
        }

        // Determine site name
        let name = options.name.clone()
            .or_else(|| root_path.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "unnamed".to_string());

        // Get or create signing key
        let key_name = options.key_name.as_deref().unwrap_or("default");
        let (public_key, private_key) = self.key_store.get_or_create(key_name)?;

        // Generate stable site ID
        let site_id = SiteId::generate(&public_key, &name);

        // Check for existing revision
        let previous_revision = self.bundle_store
            .get_published_site(&site_id.to_base58())?
            .map(|s| s.revision)
            .unwrap_or(0);

        let revision = previous_revision + 1;

        // Scan and bundle files
        let chunk_size = options.chunk_size.unwrap_or(256 * 1024);
        let compress = options.compress;

        let (files, stats) = self.bundle_directory(&root_path, chunk_size, compress).await?;

        // Determine entry point
        let entry = options.entry.clone()
            .or_else(|| {
                // Auto-detect
                for candidate in ["index.html", "index.htm", "main.html"] {
                    if files.iter().any(|f| f.path == candidate) {
                        return Some(candidate.to_string());
                    }
                }
                None
            })
            .unwrap_or_else(|| "index.html".to_string());

        // Build route config
        let routes = if options.spa_fallback.is_some() || options.clean_urls {
            Some(RouteConfig {
                clean_urls: options.clean_urls,
                fallback: options.spa_fallback.clone(),
                redirects: vec![],
                rewrites: vec![],
            })
        } else {
            None
        };

        // Build manifest
        let manifest = SiteManifest {
            files: files.clone(),
            entry,
            routes,
            headers: None,
        };

        // Compute root hash from file hashes
        let file_hashes: Vec<[u8; 32]> = files.iter().map(|f| f.hash).collect();
        let tree = MerkleTree::new(file_hashes);
        let root_hash = tree.root();

        // Sign the bundle
        let signature = sign_bundle(&site_id, revision, &root_hash, &private_key);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        // Create bundle
        let bundle = WebBundle {
            site_id,
            name: name.clone(),
            revision,
            root_hash,
            publisher: public_key,
            signature,
            manifest,
            created_at: now,
        };

        // Save bundle
        self.bundle_store.save_bundle(&bundle)?;

        // Save as published site
        let published = PublishedSite {
            site_id,
            name,
            revision,
            root_path,
            created_at: if revision == 1 { now } else { 0 }, // Would load from previous
            updated_at: now,
        };
        self.bundle_store.save_published_site(&published)?;

        Ok(PublishResult {
            bundle,
            file_count: files.len(),
            total_size: stats.total_size,
            compressed_size: stats.compressed_size,
            chunk_count: stats.chunk_count,
            new_chunks: stats.new_chunks,
        })
    }

    /// Bundle a directory into chunks
    async fn bundle_directory(
        &self,
        root: &Path,
        chunk_size: usize,
        compress: bool,
    ) -> Result<(Vec<FileEntry>, BundleStats)> {
        let mut files = Vec::new();
        let mut stats = BundleStats::default();

        for entry in WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path();
            let relative_path = path.strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");

            // Skip hidden files and common ignores
            if relative_path.starts_with('.') 
                || relative_path.contains("/.")
                || relative_path.starts_with("node_modules/")
                || relative_path.starts_with("target/")
            {
                continue;
            }

            // Read file
            let content = std::fs::read(path)
                .with_context(|| format!("Failed to read: {}", path.display()))?;

            stats.total_size += content.len() as u64;

            // Determine MIME type
            let mime_type = mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string();

            // Compress if enabled and beneficial
            let should_compress = compress && is_compressible(&mime_type);
            let (data, compression) = if should_compress {
                let mut encoder = GzEncoder::new(&content[..], GzCompression::default());
                let mut compressed = Vec::new();
                encoder.read_to_end(&mut compressed)?;
                
                // Only use if smaller
                if compressed.len() < content.len() {
                    (compressed, Some(Compression::Gzip))
                } else {
                    (content.clone(), None)
                }
            } else {
                (content.clone(), None)
            };

            stats.compressed_size += data.len() as u64;

            // Chunk the data
            let mut chunks = Vec::new();
            for chunk_data in data.chunks(chunk_size) {
                let chunk_id = self.chunk_store.put(chunk_data)?;
                
                // Track if this is a new chunk
                if chunks.iter().all(|id| id != &chunk_id) {
                    stats.new_chunks += 1;
                }
                
                chunks.push(chunk_id);
                stats.chunk_count += 1;
            }

            // Content hash
            let file_hash = hash(&content);

            files.push(FileEntry {
                path: relative_path,
                hash: file_hash,
                size: content.len() as u64,
                mime_type,
                chunks,
                compression,
            });
        }

        // Sort for deterministic ordering
        files.sort_by(|a, b| a.path.cmp(&b.path));

        Ok((files, stats))
    }
}

#[derive(Default)]
struct BundleStats {
    total_size: u64,
    compressed_size: u64,
    chunk_count: usize,
    new_chunks: usize,
}

/// Check if a MIME type benefits from compression
fn is_compressible(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime.contains("javascript")
        || mime.contains("json")
        || mime.contains("xml")
        || mime.contains("svg")
        || mime == "application/wasm"
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[tokio::test]
    async fn test_publish() -> Result<()> {
        // Create test site
        let site_dir = tempdir()?;
        fs::write(site_dir.path().join("index.html"), "<h1>Hello</h1>")?;
        fs::write(site_dir.path().join("style.css"), "body { color: red; }")?;

        // Create stores
        let data_dir = tempdir()?;
        let chunk_store = Arc::new(ChunkStore::new(data_dir.path())?);
        let bundle_store = Arc::new(BundleStore::new(data_dir.path())?);
        let key_store = Arc::new(KeyStore::new(data_dir.path())?);

        let publisher = Publisher::new(chunk_store, bundle_store, key_store);

        let result = publisher.publish(
            site_dir.path().to_str().unwrap(),
            PublishOptions {
                compress: true,
                ..Default::default()
            },
        ).await?;

        assert_eq!(result.file_count, 2);
        assert_eq!(result.bundle.revision, 1);
        assert_eq!(result.bundle.manifest.entry, "index.html");

        Ok(())
    }
}
