//! ENS (Ethereum Name Service) text-record resolver.
//!
//! Convention: an ENS name's `text(name, "grab-site")` record holds the base58
//! site id. The MVP resolver issues a single HTTP GET against a public ENS
//! gateway rather than pulling in a full Ethereum client; users who need
//! sovereignty can point `--ens-gateway` at their own resolver service.
//!
//! Default gateway: `https://api.ensideas.com/ens/text/{name}/grab-site`
//! (returns plain text body). The endpoint is configurable.

use thiserror::Error;

use crate::crypto::SiteIdExt;
use crate::types::SiteId;

pub const DEFAULT_ENS_GATEWAY: &str =
    "https://api.ensideas.com/ens/text/{name}/grab-site";

#[derive(Debug, Error)]
pub enum EnsResolverError {
    #[error("name {0:?} is not an ENS name (must end in .eth)")]
    NotEns(String),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("ens gateway returned empty body for {0}")]
    Empty(String),
    #[error("ens gateway returned status {status} for {name}: {body}")]
    Status {
        name: String,
        status: u16,
        body: String,
    },
    #[error("invalid base58 site id from ens gateway for {name}: {raw:?}")]
    InvalidSiteId { name: String, raw: String },
}

#[derive(Clone)]
pub struct EnsResolver {
    gateway_template: String,
    client: reqwest::Client,
}

impl EnsResolver {
    pub fn new(gateway_template: impl Into<String>) -> Self {
        Self {
            gateway_template: gateway_template.into(),
            client: reqwest::Client::builder()
                .user_agent(concat!("grabnet/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
        }
    }

    pub fn default() -> Self {
        Self::new(DEFAULT_ENS_GATEWAY)
    }

    /// Resolve `<name>.eth` to a `SiteId` via the configured ENS gateway.
    pub async fn resolve(&self, name: &str) -> Result<SiteId, EnsResolverError> {
        let name = name.trim().to_ascii_lowercase();
        if !name.ends_with(".eth") {
            return Err(EnsResolverError::NotEns(name));
        }

        let url = self.gateway_template.replace("{name}", &name);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        let body = resp.text().await?.trim().to_string();

        if !status.is_success() {
            return Err(EnsResolverError::Status {
                name,
                status: status.as_u16(),
                body,
            });
        }
        if body.is_empty() {
            return Err(EnsResolverError::Empty(name));
        }

        SiteId::from_base58(&body)
            .ok_or(EnsResolverError::InvalidSiteId { name, raw: body })
    }
}
