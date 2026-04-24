//! Domain → SiteId resolution
//!
//! Two strategies are supported:
//!
//! - **Static aliases** populated at startup (`HostAliases::insert`). The
//!   gateway consults this map first.
//! - **DNS TXT records** of the form `_grab.<host> IN TXT "grab-site=<base58>"`.
//!   Lookups are cached by TTL and shared across requests.
//!
//! ENS resolution lives in a sibling module (`ens.rs`) and is invoked by the
//! `grab ens` CLI subcommand. Gateway routing currently relies on DNS / static
//! aliases only.

pub mod dns;
pub mod ens;

pub use dns::{DnsResolver, DnsResolverError};

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::crypto::SiteIdExt;
use crate::types::SiteId;

/// Map of `Host` header → site id.
///
/// Cheap to clone (`Arc<RwLock<...>>` inside). Lookup is case-insensitive on
/// the host portion only; ports stripped by the caller.
#[derive(Clone, Default)]
pub struct HostAliases {
    inner: Arc<RwLock<HashMap<String, SiteId>>>,
}

impl HostAliases {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, host: impl Into<String>, site: SiteId) {
        self.inner.write().insert(host.into().to_ascii_lowercase(), site);
    }

    pub fn get(&self, host: &str) -> Option<SiteId> {
        let key = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
        self.inner.read().get(&key).copied()
    }

    pub fn entries(&self) -> Vec<(String, SiteId)> {
        self.inner
            .read()
            .iter()
            .map(|(h, s)| (h.clone(), *s))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }
}

/// Parse a CLI-style `host=site_id_or_base58` alias string.
///
/// Returns the lowercased host and the parsed site id. Rejects anything that
/// isn't valid base58 — name-to-id resolution must happen before this is
/// called (the CLI does it with `BundleStore::get_published_site`).
pub fn parse_alias(spec: &str) -> Result<(String, SiteId), String> {
    let (host, site) = spec
        .split_once('=')
        .ok_or_else(|| format!("alias must be host=site_id, got {spec:?}"))?;
    let host = host.trim();
    let site = site.trim();
    if host.is_empty() {
        return Err("alias host is empty".into());
    }
    let id = SiteId::from_base58(site)
        .ok_or_else(|| format!("alias site id is not valid base58: {site:?}"))?;
    Ok((host.to_ascii_lowercase(), id))
}
