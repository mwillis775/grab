//! Bundle and site metadata storage using sled

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use anyhow::{Result, anyhow};

use crate::types::{SiteId, WebBundle, PublishedSite, HostedSite, SiteManifest};
use crate::crypto::{encode_base58, SiteIdExt};

/// Site metadata store backed by sled
pub struct BundleStore {
    /// Published sites (owned by us)
    published: sled::Tree,
    /// Hosted sites (pinned by us)
    hosted: sled::Tree,
    /// Full bundle data
    bundles: sled::Tree,
    /// Site manifests
    manifests: sled::Tree,
    /// Site name -> site ID mapping
    names: sled::Tree,
    /// Database handle
    _db: sled::Db,
}

impl BundleStore {
    /// Create a new bundle store
    pub fn new(data_dir: &Path) -> Result<Self> {
        let db_path = data_dir.join("sites.db");
        let db = sled::open(&db_path)?;
        
        Ok(Self {
            published: db.open_tree("published")?,
            hosted: db.open_tree("hosted")?,
            bundles: db.open_tree("bundles")?,
            manifests: db.open_tree("manifests")?,
            names: db.open_tree("names")?,
            _db: db,
        })
    }

    // =========================================================================
    // Published Sites (owned by us)
    // =========================================================================

    /// Save a published site
    pub fn save_published_site(&self, site: &PublishedSite) -> Result<()> {
        let key = &site.site_id;
        let value = bincode::serialize(site)?;
        self.published.insert(key, value)?;
        
        // Index by name
        self.names.insert(site.name.as_bytes(), key)?;
        
        Ok(())
    }

    /// Get a published site by ID or name
    pub fn get_published_site(&self, id_or_name: &str) -> Result<Option<PublishedSite>> {
        // Try as site ID first
        if let Some(site_id) = SiteId::from_base58(id_or_name) {
            if let Some(data) = self.published.get(&site_id)? {
                return Ok(Some(bincode::deserialize(&data)?));
            }
        }
        
        // Try as name
        if let Some(site_id) = self.names.get(id_or_name.as_bytes())? {
            if let Some(data) = self.published.get(&*site_id)? {
                return Ok(Some(bincode::deserialize(&data)?));
            }
        }
        
        Ok(None)
    }

    /// Get all published sites
    pub fn get_all_published_sites(&self) -> Result<Vec<PublishedSite>> {
        let mut sites = Vec::new();
        for result in self.published.iter() {
            let (_, value) = result?;
            sites.push(bincode::deserialize(&value)?);
        }
        Ok(sites)
    }

    /// Delete a published site
    pub fn delete_published_site(&self, site_id: &SiteId) -> Result<bool> {
        if let Some(data) = self.published.remove(site_id)? {
            let site: PublishedSite = bincode::deserialize(&data)?;
            self.names.remove(site.name.as_bytes())?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // =========================================================================
    // Hosted Sites (pinned by us)
    // =========================================================================

    /// Save a hosted site
    pub fn save_hosted_site(&self, bundle: &WebBundle) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let site = HostedSite {
            site_id: bundle.site_id,
            name: bundle.name.clone(),
            revision: bundle.revision,
            hosted_at: now,
            last_accessed: now,
            access_count: 0,
        };

        let value = bincode::serialize(&site)?;
        self.hosted.insert(&bundle.site_id, value)?;
        
        // Also save the bundle
        self.save_bundle(bundle)?;
        
        Ok(())
    }

    /// Get a hosted site
    pub fn get_hosted_site(&self, site_id: &SiteId) -> Result<Option<HostedSite>> {
        match self.hosted.get(site_id)? {
            Some(data) => Ok(Some(bincode::deserialize(&data)?)),
            None => Ok(None),
        }
    }

    /// Get all hosted sites
    pub fn get_all_hosted_sites(&self) -> Result<Vec<HostedSite>> {
        let mut sites = Vec::new();
        for result in self.hosted.iter() {
            let (_, value) = result?;
            sites.push(bincode::deserialize(&value)?);
        }
        Ok(sites)
    }

    /// Delete a hosted site
    pub fn delete_hosted_site(&self, site_id: &SiteId) -> Result<bool> {
        Ok(self.hosted.remove(site_id)?.is_some())
    }

    /// Record an access to a hosted site
    pub fn record_access(&self, site_id: &SiteId) -> Result<()> {
        if let Some(data) = self.hosted.get(site_id)? {
            let mut site: HostedSite = bincode::deserialize(&data)?;
            site.last_accessed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            site.access_count += 1;
            self.hosted.insert(site_id, bincode::serialize(&site)?)?;
        }
        Ok(())
    }

    // =========================================================================
    // Bundles
    // =========================================================================

    /// Save a bundle
    pub fn save_bundle(&self, bundle: &WebBundle) -> Result<()> {
        let value = bincode::serialize(bundle)?;
        tracing::debug!("Saving bundle: {} bytes", value.len());
        self.bundles.insert(&bundle.site_id, value)?;
        
        // Save manifest separately for quick access
        let manifest = bincode::serialize(&bundle.manifest)?;
        tracing::debug!("Saving manifest: {} bytes", manifest.len());
        self.manifests.insert(&bundle.site_id, manifest)?;
        
        // Ensure data is flushed to disk
        self.flush()?;
        
        // Verify saved correctly
        if let Some(saved) = self.manifests.get(&bundle.site_id)? {
            tracing::debug!("Verified manifest saved: {} bytes", saved.len());
        } else {
            tracing::error!("Manifest not found after save!");
        }
        
        Ok(())
    }

    /// Get a bundle by site ID
    pub fn get_bundle(&self, site_id: &SiteId) -> Result<Option<WebBundle>> {
        match self.bundles.get(site_id)? {
            Some(data) => Ok(Some(bincode::deserialize(&data)?)),
            None => Ok(None),
        }
    }

    /// Get just the manifest (faster than full bundle)
    pub fn get_manifest(&self, site_id: &SiteId) -> Result<Option<SiteManifest>> {
        match self.manifests.get(site_id)? {
            Some(data) => {
                tracing::debug!("Reading manifest: {} bytes", data.len());
                let manifest = bincode::deserialize(&data)?;
                Ok(Some(manifest))
            }
            None => {
                tracing::debug!("Manifest not found for site");
                Ok(None)
            }
        }
    }

    /// Resolve a site ID from name or ID string
    pub fn resolve_site_id(&self, id_or_name: &str) -> Result<Option<SiteId>> {
        // Try as base58 site ID
        if let Some(site_id) = SiteId::from_base58(id_or_name) {
            return Ok(Some(site_id));
        }
        
        // Try as name
        if let Some(site_id_bytes) = self.names.get(id_or_name.as_bytes())? {
            let mut site_id = [0u8; 32];
            site_id.copy_from_slice(&site_id_bytes);
            return Ok(Some(site_id));
        }
        
        Ok(None)
    }

    /// Flush all data to disk
    pub fn flush(&self) -> Result<()> {
        self.published.flush()?;
        self.hosted.flush()?;
        self.bundles.flush()?;
        self.manifests.flush()?;
        self.names.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::path::PathBuf;

    fn create_test_bundle() -> WebBundle {
        WebBundle {
            site_id: [1u8; 32],
            name: "test-site".to_string(),
            revision: 1,
            root_hash: [2u8; 32],
            publisher: [3u8; 32],
            signature: [4u8; 64].to_vec(),
            manifest: SiteManifest {
                files: vec![],
                entry: "index.html".to_string(),
                routes: None,
                headers: None,
            },
            created_at: 1234567890,
        }
    }

    #[test]
    fn test_published_sites() -> Result<()> {
        let dir = tempdir()?;
        let store = BundleStore::new(dir.path())?;
        
        let site = PublishedSite {
            site_id: [1u8; 32],
            name: "my-site".to_string(),
            revision: 1,
            root_path: PathBuf::from("/tmp/site"),
            created_at: 123,
            updated_at: 456,
        };
        
        store.save_published_site(&site)?;
        
        // Get by ID
        let site_id_b58 = site.site_id.to_base58();
        let retrieved = store.get_published_site(&site_id_b58)?.unwrap();
        assert_eq!(retrieved.name, "my-site");
        
        // Get by name
        let by_name = store.get_published_site("my-site")?.unwrap();
        assert_eq!(by_name.site_id, site.site_id);
        
        // List all
        let all = store.get_all_published_sites()?;
        assert_eq!(all.len(), 1);
        
        Ok(())
    }

    #[test]
    fn test_hosted_sites() -> Result<()> {
        let dir = tempdir()?;
        let store = BundleStore::new(dir.path())?;
        
        let bundle = create_test_bundle();
        store.save_hosted_site(&bundle)?;
        
        let hosted = store.get_hosted_site(&bundle.site_id)?.unwrap();
        assert_eq!(hosted.name, "test-site");
        assert_eq!(hosted.revision, 1);
        
        Ok(())
    }
}
