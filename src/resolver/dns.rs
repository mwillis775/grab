//! DNS-based site discovery.
//!
//! Looks up `_grab.<host>` TXT records and parses the first entry of the form
//! `grab-site=<base58 site id>`. Results are cached in-memory with the TTL
//! returned by the upstream resolver (clamped to a sane range so a hostile
//! NS can't pin a stale mapping forever).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::TokioAsyncResolver;
use parking_lot::Mutex;
use thiserror::Error;

use crate::crypto::SiteIdExt;
use crate::types::SiteId;

const TXT_PREFIX: &str = "_grab.";
const SITE_KEY: &str = "grab-site=";

/// Minimum and maximum TTL we'll honor for cached entries.
const MIN_TTL: Duration = Duration::from_secs(30);
const MAX_TTL: Duration = Duration::from_secs(60 * 60); // 1h

#[derive(Debug, Error)]
pub enum DnsResolverError {
    #[error("dns lookup failed: {0}")]
    Lookup(#[from] hickory_resolver::error::ResolveError),
    #[error("no grab-site TXT record at {0}")]
    NotFound(String),
    #[error("invalid base58 site id in TXT record at {host}: {raw:?}")]
    InvalidSiteId { host: String, raw: String },
}

#[derive(Clone)]
struct CacheEntry {
    site: Option<SiteId>,
    expires_at: Instant,
}

/// Async DNS resolver scoped to grabnet's TXT-record convention.
#[derive(Clone)]
pub struct DnsResolver {
    inner: TokioAsyncResolver,
    cache: Arc<Mutex<HashMap<String, CacheEntry>>>,
}

impl DnsResolver {
    /// Build a resolver from the system's DNS configuration (`/etc/resolv.conf`
    /// on Unix, registry settings on Windows). Falls back to Cloudflare 1.1.1.1
    /// if the system config can't be read.
    pub fn from_system() -> Self {
        let inner = match TokioAsyncResolver::tokio_from_system_conf() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "could not read system resolver config; falling back to Cloudflare");
                TokioAsyncResolver::tokio(ResolverConfig::cloudflare(), ResolverOpts::default())
            }
        };
        Self {
            inner,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Resolve `<host>` to a site id by querying `_grab.<host>` TXT.
    ///
    /// Returns `Ok(None)` for a successful lookup that yielded no matching
    /// TXT record (cached negatively for `MIN_TTL`).
    pub async fn resolve(&self, host: &str) -> Result<Option<SiteId>, DnsResolverError> {
        let key = host.split(':').next().unwrap_or(host).to_ascii_lowercase();

        // Cache check
        if let Some(entry) = self.cache.lock().get(&key).cloned() {
            if entry.expires_at > Instant::now() {
                return Ok(entry.site);
            }
        }

        let qname = format!("{TXT_PREFIX}{key}");
        let lookup = match self.inner.txt_lookup(&qname).await {
            Ok(l) => l,
            Err(e) => {
                use hickory_resolver::error::ResolveErrorKind;
                if matches!(
                    e.kind(),
                    ResolveErrorKind::NoRecordsFound { .. }
                ) {
                    // Negative-cache for MIN_TTL so we don't hammer DNS.
                    self.cache.lock().insert(
                        key,
                        CacheEntry {
                            site: None,
                            expires_at: Instant::now() + MIN_TTL,
                        },
                    );
                    return Ok(None);
                }
                return Err(e.into());
            }
        };

        let mut chosen: Option<(SiteId, Duration)> = None;
        for record in lookup.iter() {
            for chunk in record.iter() {
                let s = String::from_utf8_lossy(chunk);
                let s = s.trim();
                if let Some(value) = s.strip_prefix(SITE_KEY) {
                    let value = value.trim();
                    let id = SiteId::from_base58(value).ok_or_else(|| {
                        DnsResolverError::InvalidSiteId {
                            host: qname.clone(),
                            raw: value.to_string(),
                        }
                    })?;
                    // Use the lookup's TTL (Hickory exposes valid_until as Instant)
                    let ttl = lookup
                        .valid_until()
                        .saturating_duration_since(Instant::now())
                        .clamp(MIN_TTL, MAX_TTL);
                    chosen = Some((id, ttl));
                    break;
                }
            }
            if chosen.is_some() {
                break;
            }
        }

        let (site, ttl) = match chosen {
            Some((id, ttl)) => (Some(id), ttl),
            None => (None, MIN_TTL),
        };

        self.cache.lock().insert(
            key,
            CacheEntry {
                site,
                expires_at: Instant::now() + ttl,
            },
        );

        Ok(site)
    }

    /// Drop a cached entry for `host` (e.g. after a publish).
    pub fn invalidate(&self, host: &str) {
        let key = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
        self.cache.lock().remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_parsing_round_trip() {
        // SiteId::from_base58 accepts a 32-byte payload encoded in base58.
        let id: SiteId = [7u8; 32];
        let encoded = id.to_base58();
        let parsed = SiteId::from_base58(&encoded).unwrap();
        assert_eq!(parsed, id);
    }
}
