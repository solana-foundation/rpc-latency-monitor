use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::RngExt;
use sha2::{Digest, Sha256};
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
    let seed: Arc<str> = host_seed().into();
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
                host_seed: seed.clone(),
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
    host_seed: Arc<str>,
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
        let interval = self.check.interval.as_millis() as u64;
        let phase = |epoch| self.phase(interval, epoch);
        let mut slot = next_slot_after(unix_millis(), interval, phase);
        loop {
            sleep_until_unix_millis(slot + self.jitter_millis()).await;
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
            slot = next_slot_after(slot.max(unix_millis()), interval, phase);
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

    fn phase(&self, interval: u64, epoch: u64) -> u64 {
        slot_phase(
            &self.host_seed,
            &self.provider,
            self.check.method,
            interval,
            epoch,
        )
    }

    fn jitter_millis(&self) -> u64 {
        let jitter = self.check.jitter.as_millis() as u64;
        if jitter == 0 {
            return 0;
        }
        rand::rng().random_range(0..=jitter)
    }
}

/// Hostname that seeds the check slots, so hosts with the same config use different slots.
fn host_seed() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|h| h.trim().to_owned())
        .unwrap_or_default()
}

const SLOT_EPOCH_MILLIS: u64 = 3_600_000;

/// Slots move once per epoch: the smallest whole number of intervals that covers one hour.
fn epoch_millis(interval: u64) -> u64 {
    interval * SLOT_EPOCH_MILLIS.div_ceil(interval)
}

/// Offset of a provider+method inside its interval for one epoch; the same after a restart.
fn slot_phase(seed: &str, provider: &str, method: RpcMethod, interval: u64, epoch: u64) -> u64 {
    if interval == 0 {
        return 0;
    }
    let digest = Sha256::digest(format!("{seed}|{provider}|{}|{epoch}", method.label()));
    let hash = u64::from_le_bytes(digest[..8].try_into().expect("sha256 is 32 bytes"));
    hash % interval
}

/// First slot after `now`. Each epoch places its slots at its own phase plus whole intervals.
fn next_slot_after(now: u64, interval: u64, phase: impl Fn(u64) -> u64) -> u64 {
    if interval == 0 {
        return now;
    }
    let len = epoch_millis(interval);
    let epoch = now / len;
    let start = epoch * len + phase(epoch) % interval;
    let slot = if now < start {
        start
    } else {
        start + ((now - start) / interval + 1) * interval
    };
    if slot < (epoch + 1) * len {
        slot
    } else {
        (epoch + 1) * len + phase(epoch + 1) % interval
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

async fn sleep_until_unix_millis(at: u64) {
    sleep(Duration::from_millis(at.saturating_sub(unix_millis()))).await;
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
    fn next_slot_is_the_first_phase_point_after_now() {
        let fixed = |_| 7_000;
        assert_eq!(next_slot_after(0, 60_000, fixed), 7_000);
        assert_eq!(next_slot_after(7_000, 60_000, fixed), 67_000);
        assert_eq!(next_slot_after(66_999, 60_000, fixed), 67_000);
        assert_eq!(next_slot_after(120_001, 60_000, |_| 0), 180_000);
        assert_eq!(next_slot_after(5, 0, |_| 3), 5);
    }

    #[test]
    fn next_slot_takes_the_new_phase_at_the_epoch_boundary() {
        let hour = 3_600_000;
        let phase = |epoch| if epoch == 0 { 50_000 } else { 10_000 };
        assert_eq!(next_slot_after(hour - 60_000, 60_000, phase), hour - 10_000);
        assert_eq!(next_slot_after(hour - 10_000, 60_000, phase), hour + 10_000);
        assert_eq!(next_slot_after(hour + 10_000, 60_000, phase), hour + 70_000);
        assert_eq!(epoch_millis(60_000), hour);
        assert_eq!(epoch_millis(7_000), 3_605_000);
    }

    #[test]
    fn slot_phase_is_stable_inside_an_epoch_and_moves_between_epochs() {
        let method = RpcMethod::GetProgramAccounts;
        let a = slot_phase("host", "alpha", method, 60_000, 7);
        assert_eq!(a, slot_phase("host", "alpha", method, 60_000, 7));
        assert!(a < 60_000);
        let mut by_epoch: Vec<u64> = (0..24)
            .map(|epoch| slot_phase("host", "alpha", method, 60_000, epoch))
            .collect();
        by_epoch.sort_unstable();
        by_epoch.dedup();
        assert!(by_epoch.len() > 12);
        assert_eq!(slot_phase("host", "alpha", RpcMethod::GetSlot, 0, 7), 0);
    }

    #[test]
    fn slot_phases_spread_providers_across_the_interval() {
        let providers = ["p0", "p1", "p2", "p3", "p4", "p5", "p6"];
        let mut phases: Vec<u64> = providers
            .iter()
            .map(|p| slot_phase("host", p, RpcMethod::GetProgramAccounts, 60_000, 0))
            .collect();
        phases.sort_unstable();
        phases.dedup();
        assert_eq!(phases.len(), providers.len());
        assert!(phases.last().unwrap() - phases.first().unwrap() > 10_000);
        let other_host: Vec<u64> = providers
            .iter()
            .map(|p| slot_phase("other", p, RpcMethod::GetProgramAccounts, 60_000, 0))
            .collect();
        assert_ne!(phases, {
            let mut o = other_host.clone();
            o.sort_unstable();
            o
        });
    }

    #[test]
    fn shared_state_round_trips_recent_signature() {
        let shared = SharedState::default();
        assert_eq!(shared.recent_signature(), None);
        shared.set_recent_signature("sig".to_string());
        assert_eq!(shared.recent_signature(), Some("sig".to_string()));
    }
}
