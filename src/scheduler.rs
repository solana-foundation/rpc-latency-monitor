use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::RngExt;
use tokio::time::sleep;

use crate::config::{CheckConfig, GpaDeriveConfig, GpaTarget};
use crate::gpa_derive::{self, DerivedTargets};
use crate::metrics::Metrics;
use crate::providers::ProviderEndpoint;
use crate::reference_check::ClaimSink;
use crate::reference_slot::{ArchivalAnchor, ReferenceSlot};
use crate::rpc::methods::RpcMethod;
use crate::rpc::{CallResult, CallStatus, ErrorKind, RequestContext, RpcClient};
use crate::sample_log::SampleLogger;

#[allow(clippy::too_many_arguments)]
pub fn spawn_checks(
    endpoints: &[ProviderEndpoint],
    checks: &[CheckConfig],
    client: Arc<RpcClient>,
    metrics: Metrics,
    reference: ReferenceSlot,
    max_slot_lag: u64,
    claims: Option<ClaimSink>,
    samples: Option<SampleLogger>,
    gpa_targets: Vec<GpaTarget>,
    gpa_derive: Option<GpaDeriveConfig>,
    archival_anchor: ArchivalAnchor,
) {
    let shared = SharedState::default();
    let gpa_targets = Arc::new(gpa_targets);
    let derived_gpa = DerivedTargets::default();
    if let Some(config) = gpa_derive {
        tokio::spawn(gpa_derive::run(
            client.clone(),
            config,
            shared.recent_accounts.clone(),
            derived_gpa.clone(),
        ));
    }
    for endpoint in endpoints {
        let tasks: Vec<CheckTask> = checks
            .iter()
            .filter(|check| {
                !matches!(
                    check.method,
                    RpcMethod::GetBlockArchival | RpcMethod::GetTransactionArchival
                )
            })
            .map(|check| CheckTask {
                provider: endpoint.name.clone(),
                url: endpoint.url.clone(),
                check: check.clone(),
                client: client.clone(),
                metrics: metrics.clone(),
                reference: reference.clone(),
                shared: shared.clone(),
                max_slot_lag,
                claims: claims.clone(),
                samples: samples.clone(),
                gpa_targets: gpa_targets.clone(),
                derived_gpa: derived_gpa.clone(),
                archival_anchor: archival_anchor.clone(),
            })
            .collect();
        spawn_provider_runtime(&endpoint.name, tasks);
    }
}

fn spawn_provider_runtime(provider: &str, tasks: Vec<CheckTask>) {
    let thread_name = format!("checks-{provider}");
    std::thread::Builder::new()
        .name(thread_name.clone())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("provider check runtime");
            runtime.block_on(async move {
                for task in tasks {
                    tokio::spawn(task.run());
                }
                std::future::pending::<()>().await;
            });
        })
        .unwrap_or_else(|error| panic!("failed to spawn {thread_name}: {error}"));
}

struct CheckTask {
    provider: String,
    url: String,
    check: CheckConfig,
    client: Arc<RpcClient>,
    metrics: Metrics,
    reference: ReferenceSlot,
    shared: SharedState,
    max_slot_lag: u64,
    claims: Option<ClaimSink>,
    samples: Option<SampleLogger>,
    gpa_targets: Arc<Vec<GpaTarget>>,
    derived_gpa: DerivedTargets,
    archival_anchor: ArchivalAnchor,
}

const GSFA_ANCHOR_MIN_SLOT: u64 = 50_000_000;

impl CheckTask {
    async fn run(self) {
        let mut rotation = 0usize;
        let mut used_anchor: Option<String> = None;
        loop {
            let gpa_target = rotated_target(
                &self.gpa_targets,
                &self.derived_gpa.current(),
                self.check.method,
                rotation,
            );
            rotation = rotation.wrapping_add(1);
            let gsfa_anchor = (self.check.method == RpcMethod::GetSignaturesForAddress
                && rotation.is_multiple_of(2))
            .then(|| self.archival_anchor.current())
            .flatten()
            .filter(|(slot, sig)| {
                *slot >= GSFA_ANCHOR_MIN_SLOT && used_anchor.as_ref() != Some(sig)
            });
            if let Some((_, sig)) = &gsfa_anchor {
                used_anchor = Some(sig.clone());
            }
            let ctx = RequestContext {
                tip_slot: self.reference.current(),
                recent_signature: self.shared.recent_signature(),
                archival_signature: self.shared.archival_signature(),
                recent_accounts: self.shared.recent_accounts(),
                gpa_target,
                gsfa_anchor,
            };
            if let Some(result) = self
                .client
                .call(&self.url, self.check.method, &ctx, self.check.timeout)
                .await
            {
                let target = if ctx.gsfa_anchor.is_some() {
                    "archival"
                } else {
                    ctx.gpa_target.as_ref().map_or("", |t| t.name.as_str())
                };
                self.record(&result, target);
            }
            sleep(self.next_delay()).await;
        }
    }

    fn record(&self, result: &CallResult, target: &str) {
        let method = self.check.method;
        let status = if self.is_stale(result) {
            CallStatus::Error(ErrorKind::Stale)
        } else {
            result.status
        };
        self.metrics
            .record_call(&self.provider, method, result, status, target);
        if let Some(samples) = &self.samples {
            samples.log(&self.provider, method, status, target, result);
        }

        if status.is_success() {
            if let Some(sink) = &self.claims {
                sink.submit(&self.provider, method, result);
            }
            if let Some(slot) = result.observed_slot {
                self.reference.observe(slot);
                if let Some(lag) = self.reference.lag_for(slot) {
                    self.metrics.record_slot_lag(&self.provider, method, lag);
                }
            }
            if let Some(signature) = &result.signature {
                self.shared.set_recent_signature(signature.clone());
            }
            if let Some(signature) = &result.archival_signature {
                self.shared.set_archival_signature(signature.clone());
            }
            if !result.accounts.is_empty() {
                self.shared.set_recent_accounts(result.accounts.clone());
            }
        }
    }

    fn is_stale(&self, result: &CallResult) -> bool {
        result.status.is_success()
            && stale_by_lag(
                self.reference.current(),
                result.observed_slot,
                self.max_slot_lag,
            )
    }

    fn next_delay(&self) -> Duration {
        let jitter = self.check.jitter;
        if jitter.is_zero() {
            return self.check.interval;
        }
        let extra = rand::rng().random_range(0..=jitter.as_millis() as u64);
        self.check.interval + Duration::from_millis(extra)
    }
}

#[derive(Clone, Default)]
struct SharedState {
    recent_signature: Arc<Mutex<Option<String>>>,
    archival_signature: Arc<Mutex<Option<String>>>,
    recent_accounts: Arc<Mutex<Vec<String>>>,
}

impl SharedState {
    fn recent_signature(&self) -> Option<String> {
        self.recent_signature.lock().ok()?.clone()
    }

    fn set_recent_signature(&self, signature: String) {
        if let Ok(mut guard) = self.recent_signature.lock() {
            *guard = Some(signature);
        }
    }

    fn archival_signature(&self) -> Option<String> {
        self.archival_signature.lock().ok()?.clone()
    }

    fn set_archival_signature(&self, signature: String) {
        if let Ok(mut guard) = self.archival_signature.lock() {
            *guard = Some(signature);
        }
    }

    fn recent_accounts(&self) -> Vec<String> {
        self.recent_accounts
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn set_recent_accounts(&self, accounts: Vec<String>) {
        if let Ok(mut guard) = self.recent_accounts.lock() {
            *guard = accounts;
        }
    }
}

fn stale_by_lag(tip: Option<u64>, observed: Option<u64>, max_lag: u64) -> bool {
    matches!((tip, observed), (Some(t), Some(s)) if t.saturating_sub(s) > max_lag)
}

fn rotated_target(
    targets: &[GpaTarget],
    derived: &[GpaTarget],
    method: RpcMethod,
    rotation: usize,
) -> Option<GpaTarget> {
    match method {
        RpcMethod::GetProgramAccounts => {
            let total = targets.len() + derived.len();
            if total == 0 {
                return None;
            }
            let index = rotation % total;
            Some(if index < targets.len() {
                targets[index].clone()
            } else {
                derived[index - targets.len()].clone()
            })
        }
        RpcMethod::GetTokenAccountsByOwner => {
            let owner = derived
                .iter()
                .find(|t| t.name == gpa_derive::BY_OWNER_NAME)?;
            (rotation % 2 == 1).then(|| owner.clone())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_only_when_lag_exceeds_budget() {
        assert!(!stale_by_lag(None, Some(100), 150));
        assert!(!stale_by_lag(Some(1000), None, 150));
        assert!(!stale_by_lag(Some(1000), Some(900), 150));
        assert!(!stale_by_lag(Some(1000), Some(850), 150));
        assert!(stale_by_lag(Some(1000), Some(700), 150));
        assert!(!stale_by_lag(Some(500), Some(900), 150));
    }

    fn named_targets(names: &[&str]) -> Vec<GpaTarget> {
        names
            .iter()
            .map(|n| GpaTarget {
                name: (*n).to_owned(),
                program: "prog".to_owned(),
                data_size: Some(1),
                memcmp: Vec::new(),
            })
            .collect()
    }

    #[test]
    fn gpa_targets_rotate_round_robin_only_for_gpa() {
        let targets = named_targets(&["a", "b", "c"]);
        let picked: Vec<String> = (0..4)
            .map(|i| {
                rotated_target(&targets, &[], RpcMethod::GetProgramAccounts, i)
                    .unwrap()
                    .name
            })
            .collect();
        assert_eq!(picked, vec!["a", "b", "c", "a"]);
        assert!(rotated_target(&targets, &[], RpcMethod::GetSlot, 0).is_none());
        assert!(rotated_target(&[], &[], RpcMethod::GetProgramAccounts, 0).is_none());
    }

    #[test]
    fn derived_targets_join_the_rotation_after_configured_ones() {
        let targets = named_targets(&["a"]);
        let derived = named_targets(&["derived_x", "derived_y"]);
        let picked: Vec<String> = (0..4)
            .map(|i| {
                rotated_target(&targets, &derived, RpcMethod::GetProgramAccounts, i)
                    .unwrap()
                    .name
            })
            .collect();
        assert_eq!(picked, vec!["a", "derived_x", "derived_y", "a"]);

        let only_derived: Vec<String> = (0..2)
            .map(|i| {
                rotated_target(&[], &derived, RpcMethod::GetProgramAccounts, i)
                    .unwrap()
                    .name
            })
            .collect();
        assert_eq!(only_derived, vec!["derived_x", "derived_y"]);
    }

    #[test]
    fn gtabo_alternates_builtin_and_derived_owner() {
        let derived = vec![
            GpaTarget {
                name: gpa_derive::BY_MINT_NAME.to_owned(),
                program: "prog".to_owned(),
                data_size: Some(1),
                memcmp: Vec::new(),
            },
            GpaTarget {
                name: gpa_derive::BY_OWNER_NAME.to_owned(),
                program: "prog".to_owned(),
                data_size: Some(1),
                memcmp: Vec::new(),
            },
        ];
        let method = RpcMethod::GetTokenAccountsByOwner;
        assert!(rotated_target(&[], &derived, method, 0).is_none());
        assert_eq!(
            rotated_target(&[], &derived, method, 1).unwrap().name,
            gpa_derive::BY_OWNER_NAME
        );
        assert!(rotated_target(&[], &derived, method, 2).is_none());
        assert!(rotated_target(&[], &[], method, 1).is_none());
        assert!(rotated_target(&[], &derived[..1], method, 1).is_none());
    }

    #[test]
    fn shared_state_round_trips_recent_signature() {
        let shared = SharedState::default();
        assert_eq!(shared.recent_signature(), None);
        shared.set_recent_signature("sig".to_string());
        assert_eq!(shared.recent_signature(), Some("sig".to_string()));
    }
}
