//! HTTP Gateway server using axum

use std::sync::Arc;
use std::net::SocketAddr;
use anyhow::Result;
use axum::{
    Router,
    routing::get,
    extract::{Path, State, Query},
    response::{IntoResponse, Response},
    http::{StatusCode, header, HeaderMap, HeaderValue},
    body::Body,
    Json,
};
use tower_http::cors::{CorsLayer, Any};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::types::{Config, SiteId, FileEntry, Compression};
use crate::storage::{ChunkStore, BundleStore};
use crate::content::UserContentManager;
use crate::crypto::SiteIdExt;

/// HTTP Gateway for serving GrabNet sites
pub struct Gateway {
    config: Config,
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    content_manager: Option<UserContentManager>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    default_site: Option<SiteId>,
}

/// Shared state for handlers
#[derive(Clone)]
struct AppState {
    chunk_store: Arc<ChunkStore>,
    bundle_store: Arc<BundleStore>,
    content_manager: Option<Arc<UserContentManager>>,
    default_site: Option<SiteId>,
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
        }
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
        };

        // Build router with standard routes
        let mut app = Router::new()
            // Health check
            .route("/health", get(health_handler))
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
