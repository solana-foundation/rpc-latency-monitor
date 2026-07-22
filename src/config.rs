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

use crate::rpc::methods::{self, RpcMethod};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub region: String,
    pub server: ServerConfig,
    #[serde(default)]
    pub reference_slot: ReferenceSlotConfig,
    pub providers: Vec<ProviderConfig>,
    pub checks: Vec<CheckConfig>,
    #[serde(default = "default_gpa_targets")]
    pub gpa_targets: Vec<GpaTarget>,
    #[serde(with = "humantime_serde", default = "default_request_timeout")]
    pub request_timeout: Duration,
    #[serde(with = "humantime_serde", default = "default_archival_interval")]
    pub archival_interval: Duration,
    #[serde(with = "humantime_serde", default = "default_archival_timeout")]
    pub archival_timeout: Duration,
    #[serde(default = "default_max_slot_lag")]
    pub max_slot_lag: u64,
    #[serde(default)]
    pub reference_check: ReferenceCheckConfig,
    #[serde(default)]
    pub claim_checks: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReferenceCheckConfig {
    #[serde(default)]
    pub rpc_url: String,
    #[serde(default)]
    pub exclude_provider: Option<String>,
    #[serde(with = "humantime_serde", default = "default_reference_interval")]
    pub interval: Duration,
    #[serde(default = "default_reference_depth")]
    pub depth: u64,
    #[serde(default = "default_claim_delay_slots")]
    pub claim_delay_slots: u64,
    #[serde(default = "default_claim_margin")]
    pub claim_margin: u64,
    #[serde(default = "default_claim_count_tolerance")]
    pub claim_count_tolerance: u64,
    #[serde(default = "default_node_stale_slots")]
    pub node_stale_slots: u64,
}

impl Default for ReferenceCheckConfig {
    fn default() -> Self {
        Self {
            rpc_url: String::new(),
            exclude_provider: None,
            interval: default_reference_interval(),
            depth: default_reference_depth(),
            claim_delay_slots: default_claim_delay_slots(),
            claim_margin: default_claim_margin(),
            claim_count_tolerance: default_claim_count_tolerance(),
            node_stale_slots: default_node_stale_slots(),
        }
    }
}

const fn default_reference_interval() -> Duration {
    Duration::from_secs(300)
}

const fn default_reference_depth() -> u64 {
    64
}

const fn default_claim_delay_slots() -> u64 {
    32
}

const fn default_claim_margin() -> u64 {
    16
}

const fn default_claim_count_tolerance() -> u64 {
    8
}

const fn default_node_stale_slots() -> u64 {
    64
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GpaTarget {
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub data_size: Option<u64>,
    #[serde(default)]
    pub memcmp: Vec<MemcmpFilter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MemcmpFilter {
    pub offset: u64,
    pub bytes: String,
}

fn default_gpa_targets() -> Vec<GpaTarget> {
    vec![methods::builtin_token_owner_target()]
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckConfig {
    pub method: RpcMethod,
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
    #[serde(with = "humantime_serde", default)]
    pub jitter: Duration,
    #[serde(with = "humantime_serde", default)]
    pub timeout: Option<Duration>,
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
        if self
            .checks
            .iter()
            .any(|c| c.method == RpcMethod::GetProgramAccounts)
            && self.gpa_targets.is_empty()
        {
            bail!("config: get_program_accounts check requires at least one gpa_target");
        }
        let mut names = HashSet::with_capacity(self.gpa_targets.len());
        for target in &self.gpa_targets {
            if !names.insert(target.name.as_str()) {
                bail!("config: duplicate gpa_target name '{}'", target.name);
            }
            if target.data_size.is_none() && target.memcmp.is_empty() {
                bail!(
                    "config: gpa_target '{}' needs data_size or memcmp",
                    target.name
                );
            }
            if bs58::decode(&target.program)
                .into_vec()
                .map(|b| b.len())
                .unwrap_or(0)
                != 32
            {
                bail!(
                    "config: gpa_target '{}' program is not a valid pubkey",
                    target.name
                );
            }
            for filter in &target.memcmp {
                if bs58::decode(&filter.bytes)
                    .into_vec()
                    .map(|b| b.is_empty())
                    .unwrap_or(true)
                {
                    bail!(
                        "config: gpa_target '{}' memcmp bytes must be valid, non-empty base58",
                        target.name
                    );
                }
            }
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

const fn default_archival_interval() -> Duration {
    Duration::from_secs(30)
}

const fn default_archival_timeout() -> Duration {
    Duration::from_secs(30)
}

const fn default_max_slot_lag() -> u64 {
    30
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
        assert_eq!(config.providers.len(), 4);
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
    fn gpa_targets_default_to_the_builtin_owner_probe() {
        let config = parse(
            "region: x\n\
             server: { bind: \"0.0.0.0:9464\" }\n\
             providers: [{ name: a, url: \"http://a\" }]\n\
             checks: [{ method: get_program_accounts, interval: 2s }]\n",
        );
        assert!(config.validate().is_ok());
        assert_eq!(
            config.gpa_targets,
            vec![methods::builtin_token_owner_target()]
        );
    }

    #[test]
    fn gpa_targets_are_validated() {
        let base = "region: x\n\
             server: { bind: \"0.0.0.0:9464\" }\n\
             providers: [{ name: a, url: \"http://a\" }]\n\
             checks: [{ method: get_program_accounts, interval: 2s }]\n";

        let empty = parse(&format!("{base}gpa_targets: []\n"));
        assert!(empty.validate().is_err());

        let dup = parse(&format!(
            "{base}gpa_targets:\n\
             \x20 - {{ name: t, program: TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA, data_size: 165 }}\n\
             \x20 - {{ name: t, program: TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA, data_size: 200 }}\n"
        ));
        assert!(dup
            .validate()
            .unwrap_err()
            .to_string()
            .contains("duplicate"));

        let no_filters = parse(&format!(
            "{base}gpa_targets:\n\
             \x20 - {{ name: t, program: TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA }}\n"
        ));
        assert!(no_filters.validate().is_err());

        let bad_program = parse(&format!(
            "{base}gpa_targets:\n\
             \x20 - {{ name: t, program: \"not-a-pubkey!\", data_size: 165 }}\n"
        ));
        assert!(bad_program.validate().is_err());

        let bad_bytes = parse(&format!(
            "{base}gpa_targets:\n\
             \x20 - {{ name: t, program: TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA, memcmp: [{{ offset: 0, bytes: \"0OIl\" }}] }}\n"
        ));
        assert!(bad_bytes.validate().is_err());

        let good = parse(&format!(
            "{base}gpa_targets:\n\
             \x20 - {{ name: pools, program: TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA, data_size: 752 }}\n\
             \x20 - {{ name: owner, program: TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA, memcmp: [{{ offset: 32, bytes: 9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM }}] }}\n"
        ));
        assert!(good.validate().is_ok());
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
