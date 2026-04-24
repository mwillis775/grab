//! HTTP Gateway server using axum

use anyhow::Result;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::oneshot;
use tower_http::cors::{Any, CorsLayer};

use crate::content::UserContentManager;
use crate::crypto::SiteIdExt;
use crate::erasure::{ErasureCodec, ShardStore};
use crate::network::GrabNetwork;
use crate::resolver::{DnsResolver, HostAliases};
use crate::storage::{BundleStore, ChunkStore};
use crate::types::{Compression, Config, FileEntry, SiteId};

/// HTTP Gateway for serving GrabNet sites
pub struct Gateway {
    config: Config,
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    shard_store: Option<Arc<ShardStore>>,
    content_manager: Option<UserContentManager>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    default_site: Option<SiteId>,
    network: Option<Arc<RwLock<Option<GrabNetwork>>>>,
    aliases: HostAliases,
    dns_resolver: Option<DnsResolver>,
    start_time: Instant,
}

/// Shared state for handlers
#[derive(Clone)]
struct AppState {
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    shard_store: Option<Arc<ShardStore>>,
    content_manager: Option<Arc<UserContentManager>>,
    default_site: Option<SiteId>,
    network: Option<Arc<RwLock<Option<GrabNetwork>>>>,
    aliases: HostAliases,
    dns_resolver: Option<DnsResolver>,
    start_time: Instant,
}

impl Gateway {
    /// Create a new gateway
    pub fn new(
        config: &Config,
        chunk_store: Arc<ChunkStore>,
        bundle_store: Arc<BundleStore>,
        content_manager: Option<UserContentManager>,
    ) -> Self {
        Self {
            config: config.clone(),
            chunk_store,
            bundle_store,
            shard_store: None,
            content_manager,
            shutdown_tx: None,
            default_site: None,
            network: None,
            aliases: HostAliases::new(),
            dns_resolver: None,
            start_time: Instant::now(),
        }
    }

    /// Create a new gateway with a default site served at root
    pub fn with_default_site(
        config: &Config,
        chunk_store: Arc<ChunkStore>,
        bundle_store: Arc<BundleStore>,
        content_manager: Option<UserContentManager>,
        default_site: SiteId,
    ) -> Self {
        Self {
            config: config.clone(),
            chunk_store,
            bundle_store,
            shard_store: None,
            content_manager,
            shutdown_tx: None,
            default_site: Some(default_site),
            network: None,
            aliases: HostAliases::new(),
            dns_resolver: None,
            start_time: Instant::now(),
        }
    }

    /// Set the shard store for erasure-coded chunk reconstruction
    pub fn with_shard_store(mut self, shard_store: Arc<ShardStore>) -> Self {
        self.shard_store = Some(shard_store);
        self
    }

    /// Set the network reference for peer info endpoints
    pub fn with_network(mut self, network: Arc<RwLock<Option<GrabNetwork>>>) -> Self {
        self.network = Some(network);
        self
    }

    /// Provide a static `Host` → site map. Each request whose `Host` header
    /// matches an alias is routed to that site at root.
    pub fn with_aliases(mut self, aliases: HostAliases) -> Self {
        self.aliases = aliases;
        self
    }

    /// Enable dynamic DNS resolution. When set, requests for hosts not present
    /// in the static alias map fall through to a `_grab.<host>` TXT lookup,
    /// and successful resolutions are cached for the record's TTL.
    pub fn with_dns_resolver(mut self, resolver: DnsResolver) -> Self {
        self.dns_resolver = Some(resolver);
        self
    }

    /// Start the gateway
    pub async fn start(&self) -> Result<()> {
        let host = self.config.gateway.host.clone();
        let http_port = self.config.gateway.port;

        let state = AppState {
            chunk_store: self.chunk_store.clone(),
            bundle_store: self.bundle_store.clone(),
            shard_store: self.shard_store.clone(),
            content_manager: self.content_manager.as_ref().map(|m| Arc::new(m.clone())),
            default_site: self.default_site.clone(),
            network: self.network.clone(),
            aliases: self.aliases.clone(),
            dns_resolver: self.dns_resolver.clone(),
            start_time: self.start_time,
        };

        // Build router with standard routes
        let mut app = Router::new()
            // Health check
            .route("/health", get(health_handler))
            // Network/Peer viewer routes
            .route("/api/network", get(network_status_handler))
            .route("/api/network/peers", get(peers_handler))
            .route("/api/network/stats", get(network_stats_handler))
            .route("/peers", get(peer_viewer_handler))
            // API routes
            .route("/api/sites", get(list_sites_handler))
            .route("/api/sites/:site_id", get(get_site_handler))
            .route("/api/sites/:site_id/manifest", get(get_manifest_handler))
            // Upload routes
            .route(
                "/api/sites/:site_id/uploads",
                get(list_uploads_handler).post(upload_handler),
            )
            .route("/uploads/:upload_id", get(serve_upload_handler))
            // Admin routes
            .route("/api/admin/host", post(host_site_handler))
            // Site content
            .route("/site/:site_id", get(redirect_to_index))
            .route("/site/:site_id/", get(serve_site_index))
            .route("/site/:site_id/*path", get(serve_site_handler))
            // ENS resolution → 302 redirect to /site/<resolved-id>/...
            .route("/ens/:name", get(serve_ens_index))
            .route("/ens/:name/", get(serve_ens_index))
            .route("/ens/:name/*path", get(serve_ens_handler));

        // Add root routes if a default site is configured OR host-based routing
        // (static aliases / DNS resolution) is enabled. Without any of these,
        // `/` would shadow nothing useful.
        let host_routing = !self.aliases.is_empty() || self.dns_resolver.is_some();
        if self.default_site.is_some() || host_routing {
            app = app
                .route("/", get(serve_default_index))
                .route("/*path", get(serve_default_handler));
            if self.default_site.is_some() {
                tracing::info!("Default site configured at root");
            }
            if host_routing {
                let alias_count = self.aliases.entries().len();
                tracing::info!(
                    aliases = alias_count,
                    dns = self.dns_resolver.is_some(),
                    "Host-based routing enabled"
                );
            }
        }

        let app = app
            // CORS
            .layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(Any)
                    .allow_headers(Any),
            )
            .with_state(state);

        match self.config.gateway.tls.clone() {
            None => {
                let addr: SocketAddr = format!("{}:{}", host, http_port).parse()?;
                tracing::info!("Gateway listening on http://{}", addr);
                let listener = tokio::net::TcpListener::bind(addr).await?;
                axum::serve(listener, app).await?;
            }
            Some(tls) => {
                use axum_server::tls_rustls::RustlsConfig;

                let rustls_config =
                    RustlsConfig::from_pem_file(&tls.cert_path, &tls.key_path)
                        .await
                        .map_err(|e| {
                            anyhow::anyhow!(
                                "failed to load TLS materials (cert={}, key={}): {}",
                                tls.cert_path.display(),
                                tls.key_path.display(),
                                e
                            )
                        })?;

                let https_port = tls.https_port.unwrap_or(http_port);
                let https_addr: SocketAddr = format!("{}:{}", host, https_port).parse()?;
                tracing::info!("Gateway listening on https://{}", https_addr);

                if let Some(redirect_port) = tls.https_port {
                    // Run HTTP redirect on the original port alongside HTTPS
                    let http_addr: SocketAddr = format!("{}:{}", host, http_port).parse()?;
                    tracing::info!(
                        "HTTP redirect listening on http://{} -> https://*:{}",
                        http_addr,
                        redirect_port
                    );
                    let redirect_app = build_redirect_router(redirect_port);
                    let https_app = app.clone();
                    let https_fut = axum_server::bind_rustls(https_addr, rustls_config)
                        .serve(https_app.into_make_service());
                    let http_fut = async move {
                        let listener = tokio::net::TcpListener::bind(http_addr).await?;
                        axum::serve(listener, redirect_app).await?;
                        Ok::<_, anyhow::Error>(())
                    };
                    tokio::try_join!(
                        async { https_fut.await.map_err(anyhow::Error::from) },
                        http_fut,
                    )?;
                } else {
                    axum_server::bind_rustls(https_addr, rustls_config)
                        .serve(app.into_make_service())
                        .await?;
                }
            }
        }

        Ok(())
    }

    /// Stop the gateway
    pub async fn stop(&self) -> Result<()> {
        // Would send shutdown signal
        Ok(())
    }
}

// Clone implementation for content manager wrapper
impl Clone for UserContentManager {
    fn clone(&self) -> Self {
        // This is a simplified clone - in production would use Arc internally
        UserContentManager::new(self.chunk_store().clone())
    }
}

// ============================================================================
// Handlers
// ============================================================================

/// Router that redirects every request to https://host:https_port/<path>
fn build_redirect_router(https_port: u16) -> Router {
    use axum::extract::Host;
    use axum::http::Uri;
    use axum::response::Redirect;

    Router::new().fallback(move |Host(host): Host, uri: Uri| async move {
        // Strip any existing port from the host header
        let host_only = host.split(':').next().unwrap_or(&host).to_string();
        let path_and_query = uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/");
        let target = if https_port == 443 {
            format!("https://{}{}", host_only, path_and_query)
        } else {
            format!("https://{}:{}{}", host_only, https_port, path_and_query)
        };
        Redirect::permanent(&target)
    })
}

async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "gateway": "grabnet"
    }))
}

#[derive(Serialize)]
struct SitesResponse {
    published: Vec<SiteInfo>,
    hosted: Vec<SiteInfo>,
}

#[derive(Serialize)]
struct SiteInfo {
    site_id: String,
    name: String,
    revision: u64,
}

async fn list_sites_handler(State(state): State<AppState>) -> impl IntoResponse {
    let published = state
        .bundle_store
        .get_all_published_sites()
        .unwrap_or_default()
        .into_iter()
        .map(|s| SiteInfo {
            site_id: s.site_id.to_base58(),
            name: s.name,
            revision: s.revision,
        })
        .collect();

    let hosted = state
        .bundle_store
        .get_all_hosted_sites()
        .unwrap_or_default()
        .into_iter()
        .map(|s| SiteInfo {
            site_id: s.site_id.to_base58(),
            name: s.name,
            revision: s.revision,
        })
        .collect();

    Json(SitesResponse { published, hosted })
}

/// Host (pin) a site by name or ID via the running gateway.
async fn host_site_handler(
    State(state): State<AppState>,
    Json(req): Json<HostRequest>,
) -> impl IntoResponse {
    let site_id_or_name = req.site.trim().to_string();
    if site_id_or_name.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "site is required"}))).into_response();
    }

    // Resolve name to SiteId
    let site_id = match state.bundle_store.resolve_site_id(&site_id_or_name) {
        Ok(Some(id)) => id,
        Ok(None) => {
            match SiteId::from_base58(&site_id_or_name) {
                Some(id) => id,
                None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Unknown site"}))).into_response(),
            }
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    };

    // Try local bundle
    let bundle = match state.bundle_store.get_bundle(&site_id) {
        Ok(Some(b)) => b,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Site not found locally. Publish or fetch it first."}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    };

    // Save as hosted
    if let Err(e) = state.bundle_store.save_hosted_site(&bundle) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response();
    }

    // Announce to network if available
    if let Some(ref net_lock) = state.network {
        let guard = net_lock.read();
        if let Some(ref network) = *guard {
            let peer_id = network.peer_id().to_string();
            drop(guard);
            tracing::info!("Announced hosting from peer {}", peer_id);
        }
    }

    tracing::info!("Now hosting site: {} ({})", bundle.name, site_id.to_base58());

    Json(serde_json::json!({
        "success": true,
        "site_id": site_id.to_base58(),
        "name": bundle.name,
        "revision": bundle.revision,
    })).into_response()
}

#[derive(Deserialize)]
struct HostRequest {
    site: String,
}

async fn get_site_handler(
    Path(site_id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let site_id = match SiteId::from_base58(&site_id) {
        Some(id) => id,
        None => return (StatusCode::BAD_REQUEST, "Invalid site ID").into_response(),
    };

    match state.bundle_store.get_bundle(&site_id) {
        Ok(Some(bundle)) => Json(serde_json::json!({
            "site_id": bundle.site_id.to_base58(),
            "name": bundle.name,
            "revision": bundle.revision,
            "files": bundle.manifest.files.len(),
            "entry": bundle.manifest.entry,
        }))
        .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Site not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_manifest_handler(
    Path(site_id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let site_id = match SiteId::from_base58(&site_id) {
        Some(id) => id,
        None => return (StatusCode::BAD_REQUEST, "Invalid site ID").into_response(),
    };

    match state.bundle_store.get_manifest(&site_id) {
        Ok(Some(manifest)) => Json(manifest).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Site not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn redirect_to_index(Path(site_id): Path<String>) -> impl IntoResponse {
    axum::response::Redirect::permanent(&format!("/site/{}/", site_id))
}

async fn serve_site_index(
    Path(site_id): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Serve the index.html for trailing slash requests
    serve_site_path(site_id, "".to_string(), headers, state).await
}

async fn serve_site_handler(
    Path((site_id, path)): Path<(String, String)>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    serve_site_path(site_id, path, headers, state).await
}

// ============================================================================
// Default / Host-routed Site Handlers
// ============================================================================

/// Resolve the site to serve at root for this request.
///
/// Order: static `Host` alias → DNS TXT lookup (cached) → configured default.
async fn resolve_root_site(headers: &HeaderMap, state: &AppState) -> Option<SiteId> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !host.is_empty() {
        if let Some(id) = state.aliases.get(host) {
            return Some(id);
        }
        if let Some(resolver) = state.dns_resolver.as_ref() {
            match resolver.resolve(host).await {
                Ok(Some(id)) => return Some(id),
                Ok(None) => {}
                Err(e) => tracing::debug!(host, error = %e, "dns resolve failed"),
            }
        }
    }

    state.default_site.clone()
}

async fn serve_default_index(headers: HeaderMap, State(state): State<AppState>) -> Response {
    let site_id = match resolve_root_site(&headers, &state).await {
        Some(id) => id.to_base58(),
        None => return (StatusCode::NOT_FOUND, "No site for this host").into_response(),
    };
    serve_site_path(site_id, "".to_string(), headers, state).await
}

async fn serve_default_handler(
    Path(path): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    // Skip API and site routes
    if path.starts_with("api/")
        || path.starts_with("site/")
        || path.starts_with("uploads/")
        || path == "health"
    {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    let site_id = match resolve_root_site(&headers, &state).await {
        Some(id) => id.to_base58(),
        None => return (StatusCode::NOT_FOUND, "No site for this host").into_response(),
    };
    serve_site_path(site_id, path, headers, state).await
}

// ============================================================================
// ENS Handlers — `/ens/<name>[/path]` resolves via the configured ENS gateway
// and 302-redirects to `/site/<resolved-id>[/path]`.
// ============================================================================

async fn ens_resolve_redirect(name: &str, suffix: &str) -> Response {
    use crate::resolver::ens::EnsResolver;
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".eth") {
        return (StatusCode::BAD_REQUEST, "ENS names must end in .eth").into_response();
    }
    let resolver = EnsResolver::default();
    match resolver.resolve(&lower).await {
        Ok(id) => {
            let location = format!("/site/{}{}", id.to_base58(), suffix);
            Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, location)
                .header(header::CACHE_CONTROL, "public, max-age=300")
                .body(axum::body::Body::empty())
                .unwrap()
        }
        Err(e) => {
            tracing::warn!(name = %lower, error = %e, "ens resolve failed");
            (StatusCode::NOT_FOUND, format!("ENS resolve failed: {e}")).into_response()
        }
    }
}

async fn serve_ens_index(Path(name): Path<String>) -> Response {
    ens_resolve_redirect(&name, "/").await
}

async fn serve_ens_handler(Path((name, path)): Path<(String, String)>) -> Response {
    let suffix = if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    };
    ens_resolve_redirect(&name, &suffix).await
}

async fn serve_site_path(
    site_id: String,
    path: String,
    headers: HeaderMap,
    state: AppState,
) -> Response {
    tracing::debug!("serve_site_path: site_id={}, path={}", site_id, path);

    let site_id = match SiteId::from_base58(&site_id) {
        Some(id) => id,
        None => return (StatusCode::BAD_REQUEST, "Invalid site ID").into_response(),
    };

    // Get manifest
    let manifest = match state.bundle_store.get_manifest(&site_id) {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "Site not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Normalize path
    let mut path = path.trim_start_matches('/').to_string();
    if path.is_empty() || path.ends_with('/') {
        path.push_str(&manifest.entry);
    }
    tracing::debug!("Resolved path: {}", path);

    // Find file
    let file = find_file(&manifest.files, &path, manifest.routes.as_ref());

    let file = match file {
        Some(f) => f,
        None => {
            // Try 404.html
            if let Some(f) = manifest.files.iter().find(|f| f.path == "404.html") {
                return serve_file(
                    f,
                    &state.chunk_store,
                    state.shard_store.as_ref(),
                    &headers,
                    StatusCode::NOT_FOUND,
                )
                .await;
            }
            return (StatusCode::NOT_FOUND, "File not found").into_response();
        }
    };

    // Record access
    let _ = state.bundle_store.record_access(&site_id);

    serve_file(
        file,
        &state.chunk_store,
        state.shard_store.as_ref(),
        &headers,
        StatusCode::OK,
    )
    .await
}

fn find_file<'a>(
    files: &'a [FileEntry],
    path: &str,
    routes: Option<&crate::types::RouteConfig>,
) -> Option<&'a FileEntry> {
    // Exact match
    if let Some(f) = files.iter().find(|f| f.path == path) {
        return Some(f);
    }

    // Clean URLs
    if let Some(routes) = routes {
        if routes.clean_urls {
            let html_path = format!("{}.html", path);
            if let Some(f) = files.iter().find(|f| f.path == html_path) {
                return Some(f);
            }
        }
    }

    // Directory index
    let index_path = format!("{}/index.html", path.trim_end_matches('/'));
    if let Some(f) = files.iter().find(|f| f.path == index_path) {
        return Some(f);
    }

    // SPA fallback
    if let Some(routes) = routes {
        if let Some(fallback) = &routes.fallback {
            return files.iter().find(|f| &f.path == fallback);
        }
    }

    None
}

async fn serve_file(
    file: &FileEntry,
    chunk_store: &ChunkStore,
    shard_store: Option<&Arc<ShardStore>>,
    request_headers: &HeaderMap,
    status: StatusCode,
) -> Response {
    // ETag is the full BLAKE3 of the file content (base58, quoted per RFC 7232).
    // Clients/CDNs treat the file as immutable when its hash hasn't changed —
    // a new revision yields a different hash, so cached entries are still safe.
    let etag = format!("\"{}\"", crate::crypto::encode_base58(&file.hash));
    if let Some(if_none_match) = request_headers.get(header::IF_NONE_MATCH) {
        // Tolerate weak ETag prefix and comma-separated lists.
        let raw = if_none_match.to_str().unwrap_or("");
        let any_match = raw.split(',').map(|s| s.trim().trim_start_matches("W/")).any(|s| s == etag);
        if any_match {
            return Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, &etag)
                .header(header::CACHE_CONTROL, "public, max-age=0, must-revalidate")
                .body(Body::empty())
                .unwrap();
        }
    }

    // Collect chunks, falling back to shard reconstruction when a chunk is missing
    let mut content = Vec::with_capacity(file.size as usize);
    for chunk_id in &file.chunks {
        match chunk_store.get(chunk_id) {
            Ok(Some(data)) => content.extend_from_slice(&data),
            _ => {
                // Try reconstructing from erasure-coded shards
                if let Some(ss) = shard_store {
                    match reconstruct_chunk_from_shards(chunk_id, ss) {
                        Ok(data) => {
                            // Cache the reconstructed chunk back into chunk store
                            let _ = chunk_store.put(&data);
                            content.extend_from_slice(&data);
                        }
                        Err(e) => {
                            tracing::warn!("Shard reconstruction failed for chunk: {}", e);
                            return (StatusCode::INTERNAL_SERVER_ERROR, "Missing chunk")
                                .into_response();
                        }
                    }
                } else {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "Missing chunk").into_response();
                }
            }
        }
    }

    // Handle compression
    let accept_encoding = request_headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let (body, content_encoding) = match file.compression {
        Some(Compression::Gzip) if accept_encoding.contains("gzip") => (content, Some("gzip")),
        Some(Compression::Gzip) => {
            // Decompress for client
            use flate2::read::GzDecoder;
            use std::io::Read;
            let mut decoder = GzDecoder::new(&content[..]);
            let mut decompressed = Vec::new();
            if decoder.read_to_end(&mut decompressed).is_ok() {
                (decompressed, None)
            } else {
                (content, None)
            }
        }
        _ => (content, None),
    };

    // Build response — use short cache for mutable site files (HTML, JS, CSS),
    // long cache only for truly immutable content-addressed assets
    let cache_control = if file.mime_type.starts_with("text/html")
        || file.mime_type.contains("javascript")
        || file.mime_type.contains("css")
    {
        "public, max-age=0, must-revalidate"
    } else {
        "public, max-age=31536000, immutable"
    };

    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, &file.mime_type)
        .header(header::CONTENT_LENGTH, body.len())
        .header(header::ETAG, &etag)
        .header(header::CACHE_CONTROL, cache_control)
        // Vary on Accept-Encoding so a CDN doesn't serve gzipped bytes to a
        // client that didn't ask for them.
        .header(header::VARY, "Accept-Encoding");

    if let Some(encoding) = content_encoding {
        response = response.header(header::CONTENT_ENCODING, encoding);
    }

    response.body(Body::from(body)).unwrap()
}

/// Attempt to reconstruct a chunk from locally stored erasure-coded shards
fn reconstruct_chunk_from_shards(
    chunk_id: &crate::types::ChunkId,
    shard_store: &ShardStore,
) -> anyhow::Result<Vec<u8>> {
    use crate::erasure::ShardId;

    // Get the first available shard to read its erasure config
    let local_indices = shard_store.local_shard_indices(chunk_id);
    if local_indices.is_empty() {
        anyhow::bail!("No shards available for chunk");
    }

    let first_id = ShardId {
        chunk_id: *chunk_id,
        shard_index: local_indices[0],
    };
    let first_shard = shard_store
        .get_full(&first_id)?
        .ok_or_else(|| anyhow::anyhow!("Shard metadata missing"))?;

    let config = first_shard.erasure_config;
    let original_size = first_shard.original_chunk_size as usize;

    if local_indices.len() < config.data_shards {
        anyhow::bail!(
            "Not enough shards to reconstruct: have {}, need {}",
            local_indices.len(),
            config.data_shards
        );
    }

    let codec = ErasureCodec::new(config)?;

    // Build the shard vector expected by the codec (None for missing)
    let total = config.total_shards();
    let mut shards: Vec<Option<Vec<u8>>> = vec![None; total];

    for idx in &local_indices {
        let id = ShardId {
            chunk_id: *chunk_id,
            shard_index: *idx,
        };
        if let Some(data) = shard_store.get(&id)? {
            shards[*idx as usize] = Some(data);
        }
    }

    codec.decode(&mut shards, original_size)
}

// ============================================================================
// Upload Handlers
// ============================================================================

async fn list_uploads_handler(
    Path(site_id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let Some(manager) = &state.content_manager else {
        return (StatusCode::NOT_IMPLEMENTED, "Uploads not enabled").into_response();
    };

    let site_id = match SiteId::from_base58(&site_id) {
        Some(id) => id,
        None => return (StatusCode::BAD_REQUEST, "Invalid site ID").into_response(),
    };

    let uploads = manager.list_site_uploads(&site_id);
    Json(serde_json::json!({ "uploads": uploads })).into_response()
}

async fn upload_handler(
    Path(site_id): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let Some(manager) = &state.content_manager else {
        return (StatusCode::NOT_IMPLEMENTED, "Uploads not enabled").into_response();
    };

    let site_id = match SiteId::from_base58(&site_id) {
        Some(id) => id,
        None => return (StatusCode::BAD_REQUEST, "Invalid site ID").into_response(),
    };

    let filename = headers
        .get("x-upload-filename")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unnamed")
        .to_string();

    let mime_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    match manager.upload(&site_id, &filename, &mime_type, &body, None) {
        Ok(Some(upload)) => Json(serde_json::json!({
            "upload": upload,
            "url": format!("/uploads/{}", upload.id),
        }))
        .into_response(),
        Ok(None) => (StatusCode::BAD_REQUEST, "Upload failed").into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn serve_upload_handler(
    Path(upload_id): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let Some(manager) = &state.content_manager else {
        return (StatusCode::NOT_IMPLEMENTED, "Uploads not enabled").into_response();
    };

    let upload = match manager.get_upload(&upload_id) {
        Some(u) => u,
        None => return (StatusCode::NOT_FOUND, "Upload not found").into_response(),
    };

    if upload.status != crate::content::UploadStatus::Approved {
        return (StatusCode::FORBIDDEN, "Content not approved").into_response();
    }

    let content = match manager.get_upload_content(&upload_id) {
        Some(c) => c,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, "Content unavailable").into_response(),
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &upload.mime_type)
        .header(header::CONTENT_LENGTH, content.len())
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .body(Body::from(content))
        .unwrap()
}

// ============================================================================
// Network & Peer Viewer Handlers
// ============================================================================

/// Network status response
#[derive(Serialize)]
struct NetworkStatusResponse {
    running: bool,
    peer_id: Option<String>,
    connected_peers: usize,
    listen_addresses: Vec<String>,
    uptime_seconds: u64,
    published_sites: usize,
    hosted_sites: usize,
}

/// Peer info response
#[derive(Serialize)]
struct PeerInfo {
    peer_id: String,
    connected: bool,
    addresses: Vec<String>,
}

/// Network stats response
#[derive(Serialize)]
struct NetworkStatsResponse {
    total_chunks: usize,
    total_storage_bytes: u64,
    published_sites: usize,
    hosted_sites: usize,
    connected_peers: usize,
    uptime_seconds: u64,
}

async fn network_status_handler(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs();

    let (running, peer_id, peers, addresses) = if let Some(net_lock) = &state.network {
        let guard = net_lock.read();
        if let Some(network) = guard.as_ref() {
            (
                true,
                Some(network.peer_id().to_string()),
                network.connected_peers(),
                network.listen_addresses(),
            )
        } else {
            (false, None, 0, vec![])
        }
    } else {
        (false, None, 0, vec![])
    };

    let published = state
        .bundle_store
        .get_all_published_sites()
        .unwrap_or_default()
        .len();
    let hosted = state
        .bundle_store
        .get_all_hosted_sites()
        .unwrap_or_default()
        .len();

    Json(NetworkStatusResponse {
        running,
        peer_id,
        connected_peers: peers,
        listen_addresses: addresses,
        uptime_seconds: uptime,
        published_sites: published,
        hosted_sites: hosted,
    })
}

async fn peers_handler(State(state): State<AppState>) -> impl IntoResponse {
    let peers: Vec<PeerInfo> = if let Some(net_lock) = &state.network {
        let guard = net_lock.read();
        if let Some(network) = guard.as_ref() {
            network
                .connected_peer_ids()
                .into_iter()
                .map(|pid| PeerInfo {
                    peer_id: pid.to_string(),
                    connected: true,
                    addresses: vec![],
                })
                .collect()
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    Json(serde_json::json!({
        "peers": peers,
        "count": peers.len(),
    }))
}

async fn network_stats_handler(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs();

    let peers = if let Some(net_lock) = &state.network {
        let guard = net_lock.read();
        guard.as_ref().map(|n| n.connected_peers()).unwrap_or(0)
    } else {
        0
    };

    Json(NetworkStatsResponse {
        total_chunks: state.chunk_store.count(),
        total_storage_bytes: state.chunk_store.total_size(),
        published_sites: state
            .bundle_store
            .get_all_published_sites()
            .unwrap_or_default()
            .len(),
        hosted_sites: state
            .bundle_store
            .get_all_hosted_sites()
            .unwrap_or_default()
            .len(),
        connected_peers: peers,
        uptime_seconds: uptime,
    })
}

async fn peer_viewer_handler(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs();

    let (running, peer_id, peers, addresses) = if let Some(net_lock) = &state.network {
        let guard = net_lock.read();
        if let Some(network) = guard.as_ref() {
            (
                true,
                network.peer_id().to_string(),
                network.connected_peer_ids(),
                network.listen_addresses(),
            )
        } else {
            (false, String::new(), vec![], vec![])
        }
    } else {
        (false, String::new(), vec![], vec![])
    };

    let published = state
        .bundle_store
        .get_all_published_sites()
        .unwrap_or_default();
    let hosted = state
        .bundle_store
        .get_all_hosted_sites()
        .unwrap_or_default();
    let chunks = state.chunk_store.count();
    let storage = state.chunk_store.total_size();

    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>GrabNet Peer Viewer</title>
    <style>
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            color: #e4e4e7;
            min-height: 100vh;
            padding: 20px;
        }}
        .container {{ max-width: 1200px; margin: 0 auto; }}
        h1 {{
            font-size: 2.5rem;
            margin-bottom: 10px;
            background: linear-gradient(90deg, #4ade80, #22d3ee);
            -webkit-background-clip: text;
            -webkit-text-fill-color: transparent;
        }}
        .subtitle {{ color: #a1a1aa; margin-bottom: 30px; }}
        .grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 20px; margin-bottom: 30px; }}
        .card {{
            background: rgba(255,255,255,0.05);
            border: 1px solid rgba(255,255,255,0.1);
            border-radius: 16px;
            padding: 24px;
            backdrop-filter: blur(10px);
        }}
        .card h2 {{
            font-size: 1.1rem;
            color: #a1a1aa;
            margin-bottom: 12px;
            text-transform: uppercase;
            letter-spacing: 0.05em;
        }}
        .stat {{
            font-size: 2.5rem;
            font-weight: 700;
            color: #fff;
        }}
        .stat.green {{ color: #4ade80; }}
        .stat.blue {{ color: #22d3ee; }}
        .stat.purple {{ color: #a78bfa; }}
        .stat.orange {{ color: #fb923c; }}
        .status-badge {{
            display: inline-flex;
            align-items: center;
            gap: 8px;
            padding: 6px 12px;
            border-radius: 20px;
            font-size: 0.875rem;
            font-weight: 500;
        }}
        .status-badge.online {{ background: rgba(74, 222, 128, 0.2); color: #4ade80; }}
        .status-badge.offline {{ background: rgba(239, 68, 68, 0.2); color: #ef4444; }}
        .status-dot {{
            width: 8px;
            height: 8px;
            border-radius: 50%;
            animation: pulse 2s infinite;
        }}
        .status-badge.online .status-dot {{ background: #4ade80; }}
        .status-badge.offline .status-dot {{ background: #ef4444; }}
        @keyframes pulse {{
            0%, 100% {{ opacity: 1; }}
            50% {{ opacity: 0.5; }}
        }}
        .peer-list {{ margin-top: 20px; }}
        .peer-item {{
            display: flex;
            align-items: center;
            gap: 12px;
            padding: 12px 16px;
            background: rgba(255,255,255,0.03);
            border-radius: 8px;
            margin-bottom: 8px;
            font-family: 'Monaco', 'Menlo', monospace;
            font-size: 0.85rem;
            word-break: break-all;
        }}
        .peer-dot {{
            width: 10px;
            height: 10px;
            border-radius: 50%;
            background: #4ade80;
            flex-shrink: 0;
        }}
        .section {{ margin-bottom: 30px; }}
        .section-title {{
            font-size: 1.25rem;
            margin-bottom: 16px;
            display: flex;
            align-items: center;
            gap: 10px;
        }}
        .site-item {{
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 12px 16px;
            background: rgba(255,255,255,0.03);
            border-radius: 8px;
            margin-bottom: 8px;
        }}
        .site-name {{ font-weight: 500; }}
        .site-id {{ color: #71717a; font-size: 0.8rem; font-family: monospace; }}
        .site-rev {{ color: #a1a1aa; font-size: 0.875rem; }}
        .empty-state {{ color: #71717a; text-align: center; padding: 40px; }}
        .address-item {{
            padding: 8px 12px;
            background: rgba(34, 211, 238, 0.1);
            border-radius: 6px;
            margin-bottom: 6px;
            font-family: monospace;
            font-size: 0.8rem;
            color: #22d3ee;
        }}
        .refresh-btn {{
            position: fixed;
            bottom: 30px;
            right: 30px;
            padding: 14px 24px;
            background: linear-gradient(135deg, #4ade80, #22d3ee);
            color: #1a1a2e;
            border: none;
            border-radius: 30px;
            font-weight: 600;
            cursor: pointer;
            box-shadow: 0 4px 20px rgba(74, 222, 128, 0.3);
            transition: transform 0.2s;
        }}
        .refresh-btn:hover {{ transform: scale(1.05); }}
        .peer-id-box {{
            background: rgba(0,0,0,0.3);
            padding: 12px 16px;
            border-radius: 8px;
            font-family: monospace;
            font-size: 0.75rem;
            word-break: break-all;
            margin-top: 10px;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>🌐 GrabNet Network</h1>
        <p class="subtitle">Real-time P2P network status and peer connections</p>

        <div class="grid">
            <div class="card">
                <h2>Network Status</h2>
                <div class="status-badge {}">
                    <span class="status-dot"></span>
                    {}
                </div>
                <div class="peer-id-box">
                    <strong>Peer ID:</strong><br>{}
                </div>
            </div>
            <div class="card">
                <h2>Connected Peers</h2>
                <div class="stat green">{}</div>
            </div>
            <div class="card">
                <h2>Published Sites</h2>
                <div class="stat blue">{}</div>
            </div>
            <div class="card">
                <h2>Hosted Sites</h2>
                <div class="stat purple">{}</div>
            </div>
            <div class="card">
                <h2>Storage Chunks</h2>
                <div class="stat orange">{}</div>
            </div>
            <div class="card">
                <h2>Total Storage</h2>
                <div class="stat">{}</div>
            </div>
        </div>

        <div class="section">
            <h2 class="section-title">📡 Listen Addresses</h2>
            <div class="card">
                {}
            </div>
        </div>

        <div class="section">
            <h2 class="section-title">🔗 Connected Peers ({})</h2>
            <div class="card">
                {}
            </div>
        </div>

        <div class="section">
            <h2 class="section-title">📤 Published Sites</h2>
            <div class="card">
                {}
            </div>
        </div>

        <div class="section">
            <h2 class="section-title">📥 Hosted Sites</h2>
            <div class="card">
                {}
            </div>
        </div>
    </div>

    <button class="refresh-btn" onclick="location.reload()">🔄 Refresh</button>

    <script>
        // Auto-refresh every 10 seconds
        setTimeout(() => location.reload(), 10000);
    </script>
</body>
</html>"#,
        if running { "online" } else { "offline" },
        if running { "Online" } else { "Offline" },
        if running { &peer_id } else { "Not connected" },
        peers.len(),
        published.len(),
        hosted.len(),
        chunks,
        format_bytes(storage),
        if addresses.is_empty() {
            "<div class='empty-state'>No listen addresses</div>".to_string()
        } else {
            addresses
                .iter()
                .map(|a| format!("<div class='address-item'>{}</div>", a))
                .collect::<Vec<_>>()
                .join("")
        },
        peers.len(),
        if peers.is_empty() {
            "<div class='empty-state'>No peers connected</div>".to_string()
        } else {
            peers
                .iter()
                .map(|p| {
                    format!(
                        "<div class='peer-item'><span class='peer-dot'></span>{}</div>",
                        p
                    )
                })
                .collect::<Vec<_>>()
                .join("")
        },
        if published.is_empty() {
            "<div class='empty-state'>No published sites</div>".to_string()
        } else {
            published.iter().map(|s| format!(
                "<div class='site-item'><div><div class='site-name'>{}</div><div class='site-id'>{}</div></div><div class='site-rev'>rev {}</div></div>",
                s.name, crate::crypto::SiteIdExt::to_base58(&s.site_id), s.revision
            )).collect::<Vec<_>>().join("")
        },
        if hosted.is_empty() {
            "<div class='empty-state'>No hosted sites</div>".to_string()
        } else {
            hosted.iter().map(|s| format!(
                "<div class='site-item'><div><div class='site-name'>{}</div><div class='site-id'>{}</div></div><div class='site-rev'>rev {}</div></div>",
                s.name, crate::crypto::SiteIdExt::to_base58(&s.site_id), s.revision
            )).collect::<Vec<_>>().join("")
        },
    ))
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
