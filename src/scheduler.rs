use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::RngExt;
use tokio::time::sleep;

use crate::config::CheckConfig;
use crate::metrics::Metrics;
use crate::providers::ProviderEndpoint;
use crate::reference_slot::ReferenceSlot;
use crate::rpc::{CallResult, RequestContext, RpcClient};

pub fn spawn_checks(
    endpoints: &[ProviderEndpoint],
    checks: &[CheckConfig],
    client: Arc<RpcClient>,
    metrics: Metrics,
    reference: ReferenceSlot,
) {
    let shared = SharedState::default();
    for endpoint in endpoints {
        for check in checks {
            let task = CheckTask {
                provider: endpoint.name.clone(),
                url: endpoint.url.clone(),
                check: check.clone(),
                client: client.clone(),
                metrics: metrics.clone(),
                reference: reference.clone(),
                shared: shared.clone(),
            };
            tokio::spawn(task.run());
        }
    }
}

struct CheckTask {
    provider: String,
    url: String,
    check: CheckConfig,
    client: Arc<RpcClient>,
    metrics: Metrics,
    reference: ReferenceSlot,
    shared: SharedState,
}

impl CheckTask {
    async fn run(self) {
        loop {
            let ctx = RequestContext {
                tip_slot: self.reference.current(),
                recent_signature: self.shared.recent_signature(),
                recent_accounts: self.shared.recent_accounts(),
            };
            if let Some(result) = self.client.call(&self.url, self.check.method, &ctx).await {
                self.record(&result);
            }
            sleep(self.next_delay()).await;
        }
    }

    fn record(&self, result: &CallResult) {
        let method = self.check.method;
        self.metrics.record_call(&self.provider, method, result);
        if let Some(slot) = result.observed_slot {
            self.reference.observe(slot);
            if let Some(lag) = self.reference.lag_for(slot) {
                self.metrics.record_slot_lag(&self.provider, method, lag);
            }
        }
        if let Some(signature) = &result.signature {
            self.shared.set_recent_signature(signature.clone());
        }
        if !result.accounts.is_empty() {
            self.shared.set_recent_accounts(result.accounts.clone());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_state_round_trips_recent_signature() {
        let shared = SharedState::default();
        assert_eq!(shared.recent_signature(), None);
        shared.set_recent_signature("sig".to_string());
        assert_eq!(shared.recent_signature(), Some("sig".to_string()));
    }
}
