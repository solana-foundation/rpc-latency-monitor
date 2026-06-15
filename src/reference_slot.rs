use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::rpc::methods::RpcMethod;
use crate::rpc::{RequestContext, RpcClient};

#[derive(Debug, Clone, Default)]
pub struct ReferenceSlot {
    tip: Arc<AtomicU64>,
}

impl ReferenceSlot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&self, slot: u64) {
        self.tip.fetch_max(slot, Ordering::Relaxed);
    }

    pub fn current(&self) -> Option<u64> {
        match self.tip.load(Ordering::Relaxed) {
            0 => None,
            slot => Some(slot),
        }
    }

    pub fn lag_for(&self, observed: u64) -> Option<u64> {
        self.current().map(|tip| tip.saturating_sub(observed))
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
        let Some(result) = client.call(&url, RpcMethod::GetSlot, &RequestContext::default()).await
        else {
            continue;
        };
        if let Some(slot) = result.observed_slot {
            reference.observe(slot);
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
}
