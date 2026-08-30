use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::rpc::methods::RpcMethod;
use crate::rpc::{RequestContext, RpcClient};

#[derive(Debug, Clone, Default)]
pub struct ReferenceSlot {
    inner: Arc<Mutex<ReferenceState>>,
}

#[derive(Debug, Default)]
struct ReferenceState {
    endpoint_tip: u64,
    provider_tip: u64,
}

impl ReferenceSlot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&self, slot: u64) {
        if let Ok(mut state) = self.inner.lock() {
            state.provider_tip = state.provider_tip.max(slot);
        }
    }

    pub fn observe_endpoint(&self, slot: u64) {
        // Start a fresh provider-observation window whenever the trusted source
        // is observed. This preserves max-observed fallback between successful
        // polls without allowing a value from an older window to poison the tip
        // permanently.
        if let Ok(mut state) = self.inner.lock() {
            // A delayed response from an older poll must not regress the trusted
            // tip or clear observations collected after a newer response.
            if slot > state.endpoint_tip {
                state.endpoint_tip = slot;
                state.provider_tip = 0;
            }
        }
    }

    pub fn current(&self) -> Option<u64> {
        let state = self.inner.lock().ok()?;
        let tip = state.provider_tip.max(state.endpoint_tip);
        match tip {
            0 => None,
            slot => Some(slot),
        }
    }

    pub fn lag_for(&self, observed: u64) -> Option<u64> {
        self.current().map(|tip| tip.saturating_sub(observed))
    }
}

#[derive(Debug, Clone, Default)]
pub struct ArchivalAnchor {
    inner: Arc<std::sync::Mutex<Option<(u64, String)>>>,
}

impl ArchivalAnchor {
    pub fn set(&self, slot: u64, signature: String) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Some((slot, signature));
        }
    }

    pub fn current(&self) -> Option<(u64, String)> {
        self.inner.lock().ok()?.clone()
    }
}

pub async fn poll_reference_endpoint(
    client: RpcClient,
    url: String,
    interval: Duration,
    reference: ReferenceSlot,
) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        let Some(result) = client
            .call(&url, RpcMethod::GetSlot, &RequestContext::default(), None)
            .await
        else {
            continue;
        };
        if let Some(slot) = result.observed_slot {
            reference.observe_endpoint(slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_keeps_the_highest_slot() {
        let reference = ReferenceSlot::new();
        reference.observe(5);
        reference.observe(3);
        assert_eq!(reference.current(), Some(5));
        reference.observe(9);
        assert_eq!(reference.current(), Some(9));
    }

    #[test]
    fn lag_is_zero_when_provider_is_at_or_ahead_of_tip() {
        let reference = ReferenceSlot::new();
        reference.observe(100);
        assert_eq!(reference.lag_for(90), Some(10));
        assert_eq!(reference.lag_for(120), Some(0));
    }

    #[test]
    fn unset_reference_reports_no_lag() {
        let reference = ReferenceSlot::new();
        assert_eq!(reference.current(), None);
        assert_eq!(reference.lag_for(50), None);
    }

    #[test]
    fn provider_observation_cannot_permanently_poison_endpoint_reference() {
        let reference = ReferenceSlot::new();

        // The endpoint poll establishes the trusted chain tip.
        reference.observe_endpoint(100);

        // The scheduler currently feeds a successful provider observation through
        // the same irreversible fetch_max path before claim verification settles.
        reference.observe(1_000_000);

        // A later endpoint poll starts a fresh observation window.
        reference.observe_endpoint(101);

        assert_eq!(reference.current(), Some(101));
        assert_eq!(reference.lag_for(101), Some(0));
    }

    #[test]
    fn provider_max_is_retained_between_endpoint_polls() {
        let reference = ReferenceSlot::new();
        reference.observe_endpoint(100);
        reference.observe(103);
        reference.observe(102);

        assert_eq!(reference.current(), Some(103));
    }

    #[test]
    fn provider_only_mode_keeps_max_observed_fallback() {
        let reference = ReferenceSlot::new();
        reference.observe(100);
        reference.observe(103);
        reference.observe(102);

        assert_eq!(reference.current(), Some(103));
    }

    #[test]
    fn lagging_endpoint_response_cannot_regress_or_clear_provider_tip() {
        let reference = ReferenceSlot::new();
        reference.observe_endpoint(101);
        reference.observe(103);

        reference.observe_endpoint(100);

        assert_eq!(reference.current(), Some(103));
    }
}
