//! HTTP Gateway server using axum

use std::sync::Arc;
use std::net::SocketAddr;
use std::time::Instant;
use anyhow::Result;
use axum::{
    Router,
    routing::get,
    extract::{Path, State, Query},
    response::{IntoResponse, Response, Html},
    http::{StatusCode, header, HeaderMap, HeaderValue},
    body::Body,
    Json,
};
use tower_http::cors::{CorsLayer, Any};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use parking_lot::RwLock;

use crate::types::{Config, SiteId, FileEntry, Compression};
use crate::storage::{ChunkStore, BundleStore};
use crate::content::UserContentManager;
use crate::crypto::SiteIdExt;
use crate::network::GrabNetwork;

/// HTTP Gateway for serving GrabNet sites
pub struct Gateway {
    config: Config,
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    content_manager: Option<UserContentManager>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    default_site: Option<SiteId>,
    network: Option<Arc<RwLock<Option<GrabNetwork>>>>,
    start_time: Instant,
}

/// Shared state for handlers
#[derive(Clone)]
struct AppState {
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    content_manager: Option<Arc<UserContentManager>>,
    default_site: Option<SiteId>,
    network: Option<Arc<RwLock<Option<GrabNetwork>>>>,
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
            content_manager,
            shutdown_tx: None,
            default_site: None,
            network: None,
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
            content_manager,
            shutdown_tx: None,
            default_site: Some(default_site),
            network: None,
            start_time: Instant::now(),
        }
    }

    /// Set the network reference for peer info endpoints
    pub fn with_network(mut self, network: Arc<RwLock<Option<GrabNetwork>>>) -> Self {
        self.network = Some(network);
        self
    }

    /// Start the gateway
    pub async fn start(&self) -> Result<()> {
        let addr: SocketAddr = format!("{}:{}", self.config.gateway.host, self.config.gateway.port)
            .parse()?;

        let state = AppState {
            chunk_store: self.chunk_store.clone(),
            bundle_store: self.bundle_store.clone(),
            content_manager: self.content_manager.as_ref().map(|m| Arc::new(m.clone())),
            default_site: self.default_site.clone(),
            network: self.network.clone(),
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
            .route("/api/sites/:site_id/uploads", get(list_uploads_handler).post(upload_handler))
            .route("/uploads/:upload_id", get(serve_upload_handler))
            // Site content
            .route("/site/:site_id", get(redirect_to_index))
            .route("/site/:site_id/", get(serve_site_index))
            .route("/site/:site_id/*path", get(serve_site_handler));

        // Add root routes if default site is set
        if self.default_site.is_some() {
            app = app
                .route("/", get(serve_default_index))
                .route("/*path", get(serve_default_handler));
            tracing::info!("Default site configured at root");
        }

        let app = app
            // CORS
            .layer(CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any))
            .with_state(state);

        tracing::info!("Gateway listening on http://{}", addr);

        // Start server
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

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
    let published = state.bundle_store.get_all_published_sites()
        .unwrap_or_default()
        .into_iter()
        .map(|s| SiteInfo {
            site_id: s.site_id.to_base58(),
            name: s.name,
            revision: s.revision,
        })
        .collect();

    let hosted = state.bundle_store.get_all_hosted_sites()
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
        })).into_response(),
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
// Default Site Handlers (serve at root when configured)
// ============================================================================

async fn serve_default_index(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let site_id = match &state.default_site {
        Some(id) => id.to_base58(),
        None => return (StatusCode::NOT_FOUND, "No default site configured").into_response(),
    };
    serve_site_path(site_id, "".to_string(), headers, state).await
}

async fn serve_default_handler(
    Path(path): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    // Skip API and site routes
    if path.starts_with("api/") || path.starts_with("site/") || 
       path.starts_with("uploads/") || path == "health" {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    
    let site_id = match &state.default_site {
        Some(id) => id.to_base58(),
        None => return (StatusCode::NOT_FOUND, "No default site configured").into_response(),
    };
    serve_site_path(site_id, path, headers, state).await
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
                return serve_file(f, &state.chunk_store, &headers, StatusCode::NOT_FOUND).await;
            }
            return (StatusCode::NOT_FOUND, "File not found").into_response();
        }
    };

    // Record access
    let _ = state.bundle_store.record_access(&site_id);

    serve_file(file, &state.chunk_store, &headers, StatusCode::OK).await
}

fn find_file<'a>(files: &'a [FileEntry], path: &str, routes: Option<&crate::types::RouteConfig>) -> Option<&'a FileEntry> {
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
    request_headers: &HeaderMap,
    status: StatusCode,
) -> Response {
    // Check ETag
    let etag = format!("\"{}\"", crate::crypto::encode_base58(&file.hash[..8]));
    if let Some(if_none_match) = request_headers.get(header::IF_NONE_MATCH) {
        if if_none_match.as_bytes() == etag.as_bytes() {
            return StatusCode::NOT_MODIFIED.into_response();
        }
    }

    // Collect chunks
    let mut content = Vec::with_capacity(file.size as usize);
    for chunk_id in &file.chunks {
        match chunk_store.get(chunk_id) {
            Ok(Some(data)) => content.extend_from_slice(&data),
            _ => return (StatusCode::INTERNAL_SERVER_ERROR, "Missing chunk").into_response(),
        }
    }

    // Handle compression
    let accept_encoding = request_headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let (body, content_encoding) = match file.compression {
        Some(Compression::Gzip) if accept_encoding.contains("gzip") => {
            (content, Some("gzip"))
        }
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

    // Build response
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, &file.mime_type)
        .header(header::CONTENT_LENGTH, body.len())
        .header(header::ETAG, &etag)
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable");

    if let Some(encoding) = content_encoding {
        response = response.header(header::CONTENT_ENCODING, encoding);
    }

    response.body(Body::from(body)).unwrap()
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
        Ok(Some(upload)) => {
            Json(serde_json::json!({
                "upload": upload,
                "url": format!("/uploads/{}", upload.id),
            })).into_response()
        }
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

    let published = state.bundle_store.get_all_published_sites().unwrap_or_default().len();
    let hosted = state.bundle_store.get_all_hosted_sites().unwrap_or_default().len();

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
            network.connected_peer_ids()
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
        published_sites: state.bundle_store.get_all_published_sites().unwrap_or_default().len(),
        hosted_sites: state.bundle_store.get_all_hosted_sites().unwrap_or_default().len(),
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

    let published = state.bundle_store.get_all_published_sites().unwrap_or_default();
    let hosted = state.bundle_store.get_all_hosted_sites().unwrap_or_default();
    let chunks = state.chunk_store.count();
    let storage = state.chunk_store.total_size();

    Html(format!(r#"<!DOCTYPE html>
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
            addresses.iter().map(|a| format!("<div class='address-item'>{}</div>", a)).collect::<Vec<_>>().join("")
        },
        peers.len(),
        if peers.is_empty() {
            "<div class='empty-state'>No peers connected</div>".to_string()
        } else {
            peers.iter().map(|p| format!("<div class='peer-item'><span class='peer-dot'></span>{}</div>", p)).collect::<Vec<_>>().join("")
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
