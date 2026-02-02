//! User upload management for GrabNet sites

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use anyhow::Result;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::types::{SiteId, ChunkId};
use crate::storage::ChunkStore;
use crate::crypto::{hash, encode_base58};

/// Upload policy for a site
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadPolicy {
    /// Maximum file size in bytes
    pub max_file_size: usize,
    /// Maximum storage per user in bytes
    pub max_storage_per_user: usize,
    /// Allowed MIME types (empty = all)
    pub allowed_types: Vec<String>,
    /// Require authentication
    pub require_auth: bool,
    /// Moderation mode
    pub moderation: ModerationMode,
    /// Rate limit (uploads per hour)
    pub rate_limit: usize,
}

impl Default for UploadPolicy {
    fn default() -> Self {
        Self {
            max_file_size: 10 * 1024 * 1024, // 10 MB
            max_storage_per_user: 100 * 1024 * 1024, // 100 MB
            allowed_types: vec![],
            require_auth: false,
            moderation: ModerationMode::None,
            rate_limit: 60,
        }
    }
}

/// Moderation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModerationMode {
    /// Auto-approve all uploads
    None,
    /// Review after upload
    Post,
    /// Require approval before visible
    Pre,
}

/// Upload status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UploadStatus {
    Pending,
    Approved,
    Rejected,
}

/// A user upload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserUpload {
    /// Unique upload ID
    pub id: String,
    /// Site this belongs to
    pub site_id: SiteId,
    /// Uploader identity
    pub uploader: String,
    /// Original filename
    pub filename: String,
    /// MIME type
    pub mime_type: String,
    /// File size
    pub size: usize,
    /// Content hash
    pub content_hash: String,
    /// Chunk IDs
    pub chunks: Vec<ChunkId>,
    /// Upload timestamp
    pub uploaded_at: u64,
    /// Status
    pub status: UploadStatus,
}

/// Manages user-uploaded content
pub struct UserContentManager {
    chunk_store: Arc<ChunkStore>,
    policies: RwLock<HashMap<SiteId, UploadPolicy>>,
    uploads: RwLock<HashMap<String, UserUpload>>,
    uploads_by_site: RwLock<HashMap<SiteId, Vec<String>>>,
    uploads_by_user: RwLock<HashMap<String, Vec<String>>>,
    rate_limits: RwLock<HashMap<String, Vec<u64>>>,
}

impl UserContentManager {
    /// Create a new content manager
    pub fn new(chunk_store: Arc<ChunkStore>) -> Self {
        Self {
            chunk_store,
            policies: RwLock::new(HashMap::new()),
            uploads: RwLock::new(HashMap::new()),
            uploads_by_site: RwLock::new(HashMap::new()),
            uploads_by_user: RwLock::new(HashMap::new()),
            rate_limits: RwLock::new(HashMap::new()),
        }
    }

    /// Get chunk store reference
    pub fn chunk_store(&self) -> &Arc<ChunkStore> {
        &self.chunk_store
    }

    /// Set upload policy for a site
    pub fn set_policy(&self, site_id: &SiteId, policy: UploadPolicy) {
        self.policies.write().insert(*site_id, policy);
    }

    /// Get policy for a site
    pub fn get_policy(&self, site_id: &SiteId) -> Option<UploadPolicy> {
        self.policies.read().get(site_id).cloned()
    }

    /// Upload content
    pub fn upload(
        &self,
        site_id: &SiteId,
        filename: &str,
        mime_type: &str,
        data: &[u8],
        uploader_id: Option<&str>,
    ) -> Result<Option<UserUpload>> {
        // Get policy
        let policy = match self.policies.read().get(site_id) {
            Some(p) => p.clone(),
            None => return Ok(None), // No policy = uploads disabled
        };

        // Generate uploader ID
        let uploader = uploader_id
            .map(String::from)
            .unwrap_or_else(|| format!("anon_{}", encode_base58(&hash(&rand::random::<[u8; 8]>())[..8])));

        // Check authentication requirement
        if policy.require_auth && uploader_id.is_none() {
            anyhow::bail!("Authentication required");
        }

        // Check file size
        if data.len() > policy.max_file_size {
            anyhow::bail!("File too large (max {} bytes)", policy.max_file_size);
        }

        // Check MIME type
        if !policy.allowed_types.is_empty() {
            let allowed = policy.allowed_types.iter().any(|t| {
                mime_type == t || mime_type.starts_with(&t.replace("*", ""))
            });
            if !allowed {
                anyhow::bail!("File type not allowed: {}", mime_type);
            }
        }

        // Check rate limit
        if !self.check_rate_limit(&uploader, policy.rate_limit) {
            anyhow::bail!("Rate limit exceeded");
        }

        // Check storage quota
        let user_storage = self.get_user_storage(&uploader);
        if user_storage + data.len() > policy.max_storage_per_user {
            anyhow::bail!("Storage quota exceeded");
        }

        // Chunk and store
        let chunk_size = 256 * 1024;
        let mut chunks = Vec::new();

        for chunk in data.chunks(chunk_size) {
            let chunk_id = self.chunk_store.put(chunk)?;
            chunks.push(chunk_id);
        }

        // Create upload record
        let upload_id = encode_base58(&hash(&rand::random::<[u8; 16]>())[..12]);
        let content_hash = encode_base58(&hash(data));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let status = match policy.moderation {
            ModerationMode::Pre => UploadStatus::Pending,
            _ => UploadStatus::Approved,
        };

        let upload = UserUpload {
            id: upload_id.clone(),
            site_id: *site_id,
            uploader: uploader.clone(),
            filename: filename.to_string(),
            mime_type: mime_type.to_string(),
            size: data.len(),
            content_hash,
            chunks,
            uploaded_at: now,
            status,
        };

        // Store
        self.uploads.write().insert(upload_id.clone(), upload.clone());
        self.uploads_by_site.write()
            .entry(*site_id)
            .or_default()
            .push(upload_id.clone());
        self.uploads_by_user.write()
            .entry(uploader.clone())
            .or_default()
            .push(upload_id);

        // Record rate limit
        self.record_upload(&uploader);

        Ok(Some(upload))
    }

    /// Get an upload by ID
    pub fn get_upload(&self, upload_id: &str) -> Option<UserUpload> {
        self.uploads.read().get(upload_id).cloned()
    }

    /// Get upload content
    pub fn get_upload_content(&self, upload_id: &str) -> Option<Vec<u8>> {
        let upload = self.uploads.read().get(upload_id)?.clone();

        if upload.status != UploadStatus::Approved {
            return None;
        }

        let mut content = Vec::with_capacity(upload.size);
        for chunk_id in &upload.chunks {
            let data = self.chunk_store.get(chunk_id).ok()??;
            content.extend_from_slice(&data);
        }

        Some(content)
    }

    /// List uploads for a site
    pub fn list_site_uploads(&self, site_id: &SiteId) -> Vec<UserUpload> {
        let ids = self.uploads_by_site.read()
            .get(site_id)
            .cloned()
            .unwrap_or_default();

        let uploads = self.uploads.read();
        ids.iter()
            .filter_map(|id| uploads.get(id).cloned())
            .collect()
    }

    /// Approve an upload
    pub fn approve(&self, upload_id: &str) -> bool {
        if let Some(upload) = self.uploads.write().get_mut(upload_id) {
            upload.status = UploadStatus::Approved;
            true
        } else {
            false
        }
    }

    /// Reject an upload
    pub fn reject(&self, upload_id: &str) -> bool {
        if let Some(upload) = self.uploads.write().get_mut(upload_id) {
            upload.status = UploadStatus::Rejected;
            true
        } else {
            false
        }
    }

    /// Delete an upload
    pub fn delete(&self, upload_id: &str) -> bool {
        let upload = match self.uploads.write().remove(upload_id) {
            Some(u) => u,
            None => return false,
        };

        // Remove from indexes
        if let Some(list) = self.uploads_by_site.write().get_mut(&upload.site_id) {
            list.retain(|id| id != upload_id);
        }
        if let Some(list) = self.uploads_by_user.write().get_mut(&upload.uploader) {
            list.retain(|id| id != upload_id);
        }

        true
    }

    // =========================================================================
    // Helpers
    // =========================================================================

    fn check_rate_limit(&self, user_id: &str, limit: usize) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let hour_ago = now - 3600 * 1000;

        let rates = self.rate_limits.read();
        let recent = rates.get(user_id)
            .map(|times| times.iter().filter(|&&t| t > hour_ago).count())
            .unwrap_or(0);

        recent < limit
    }

    fn record_upload(&self, user_id: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let mut rates = self.rate_limits.write();
        let times = rates.entry(user_id.to_string()).or_default();
        times.push(now);

        // Clean old entries
        let hour_ago = now - 3600 * 1000;
        times.retain(|&t| t > hour_ago);
    }

    fn get_user_storage(&self, user_id: &str) -> usize {
        let ids = self.uploads_by_user.read()
            .get(user_id)
            .cloned()
            .unwrap_or_default();

        let uploads = self.uploads.read();
        ids.iter()
            .filter_map(|id| uploads.get(id))
            .map(|u| u.size)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_upload() -> Result<()> {
        let dir = tempdir()?;
        let chunk_store = Arc::new(ChunkStore::new(dir.path())?);
        let manager = UserContentManager::new(chunk_store);

        let site_id = [1u8; 32];
        manager.set_policy(&site_id, UploadPolicy::default());

        let upload = manager.upload(
            &site_id,
            "test.txt",
            "text/plain",
            b"hello world",
            Some("user1"),
        )?.unwrap();

        assert_eq!(upload.filename, "test.txt");
        assert_eq!(upload.status, UploadStatus::Approved);

        // Retrieve content
        let content = manager.get_upload_content(&upload.id).unwrap();
        assert_eq!(content, b"hello world");

        Ok(())
    }

    #[test]
    fn test_moderation() -> Result<()> {
        let dir = tempdir()?;
        let chunk_store = Arc::new(ChunkStore::new(dir.path())?);
        let manager = UserContentManager::new(chunk_store);

        let site_id = [1u8; 32];
        manager.set_policy(&site_id, UploadPolicy {
            moderation: ModerationMode::Pre,
            ..Default::default()
        });

        let upload = manager.upload(
            &site_id,
            "test.txt",
            "text/plain",
            b"needs review",
            None,
        )?.unwrap();

        assert_eq!(upload.status, UploadStatus::Pending);

        // Content not accessible while pending
        assert!(manager.get_upload_content(&upload.id).is_none());

        // Approve
        manager.approve(&upload.id);
        let upload = manager.get_upload(&upload.id).unwrap();
        assert_eq!(upload.status, UploadStatus::Approved);

        // Now content is accessible
        assert!(manager.get_upload_content(&upload.id).is_some());

        Ok(())
    }
}
