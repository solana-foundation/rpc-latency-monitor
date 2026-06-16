use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context};
use figment::{
    providers::{Env, Format, Yaml},
    Figment,
};
use serde::Deserialize;

use crate::rpc::methods::RpcMethod;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub region: String,
    pub server: ServerConfig,
    #[serde(default)]
    pub reference_slot: ReferenceSlotConfig,
    pub providers: Vec<ProviderConfig>,
    pub checks: Vec<CheckConfig>,
    #[serde(with = "humantime_serde", default = "default_request_timeout")]
    pub request_timeout: Duration,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub bind: SocketAddr,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReferenceSlotConfig {
    pub source: ReferenceSource,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(with = "humantime_serde", default = "default_poll_interval")]
    pub poll_interval: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceSource {
    MaxObserved,
    Endpoint,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckConfig {
    pub method: RpcMethod,
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
    #[serde(with = "humantime_serde", default)]
    pub jitter: Duration,
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let config: Config = Figment::new()
            .merge(Yaml::file(path))
            .merge(Env::prefixed("MONITOR_"))
            .extract()
            .with_context(|| format!("loading config from {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.providers.is_empty() {
            bail!("config: at least one provider is required");
        }
        if self.checks.is_empty() {
            bail!("config: at least one check is required");
        }
        let mut seen = HashSet::with_capacity(self.providers.len());
        for provider in &self.providers {
            if !seen.insert(provider.name.as_str()) {
                bail!("config: duplicate provider name '{}'", provider.name);
            }
        }
        if self.reference_slot.source == ReferenceSource::Endpoint
            && self.reference_slot.endpoint.is_none()
        {
            bail!("config: reference_slot.source is 'endpoint' but no endpoint is set");
        }
        Ok(())
    }
}

impl Default for ReferenceSlotConfig {
    fn default() -> Self {
        Self {
            source: ReferenceSource::MaxObserved,
            endpoint: None,
            poll_interval: default_poll_interval(),
        }
    }
}

const fn default_request_timeout() -> Duration {
    Duration::from_secs(10)
}

const fn default_poll_interval() -> Duration {
    Duration::from_secs(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Config {
        Figment::from(Yaml::string(yaml))
            .extract()
            .expect("config should parse")
    }

    #[test]
    fn parses_the_example_config() {
        let config = parse(include_str!("../config.example.yaml"));

        assert_eq!(config.region, "local");
        assert_eq!(config.server.bind.port(), 9464);
        assert_eq!(config.reference_slot.source, ReferenceSource::MaxObserved);
        assert_eq!(config.request_timeout, Duration::from_secs(10));
        assert_eq!(config.providers.len(), 3);
        assert!(config.checks.iter().any(|c| c.method == RpcMethod::GetSlot));
    }

    #[test]
    fn jitter_defaults_to_zero_when_omitted() {
        let config = parse(
            "region: x\n\
             server: { bind: \"0.0.0.0:9464\" }\n\
             providers: [{ name: a, url: \"http://a\" }]\n\
             checks: [{ method: get_slot, interval: 2s }]\n",
        );
        assert_eq!(config.checks[0].jitter, Duration::ZERO);
        assert_eq!(config.reference_slot.source, ReferenceSource::MaxObserved);
    }

    #[test]
    fn rejects_duplicate_provider_names() {
        let config = parse(
            "region: x\n\
             server: { bind: \"0.0.0.0:9464\" }\n\
             providers: [{ name: dup, url: \"http://a\" }, { name: dup, url: \"http://b\" }]\n\
             checks: [{ method: get_slot, interval: 2s }]\n",
        );
        let err = config
            .validate()
            .expect_err("duplicate names must be rejected");
        assert!(err.to_string().contains("duplicate provider name"));
    }

    #[test]
    fn endpoint_source_requires_an_endpoint() {
        let config = parse(
            "region: x\n\
             server: { bind: \"0.0.0.0:9464\" }\n\
             reference_slot: { source: endpoint }\n\
             providers: [{ name: a, url: \"http://a\" }]\n\
             checks: [{ method: get_slot, interval: 2s }]\n",
        );
        assert!(config.validate().is_err());
    }
}
