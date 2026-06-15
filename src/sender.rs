use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, IntGaugeVec, Opts, Registry};
use serde::Deserialize;
use serde_json::Value;

const LAND_LATENCY_BUCKETS: &[f64] = &[0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 30.0, 60.0];

#[derive(Debug, Clone, Deserialize)]
pub struct SenderConfig {
    pub providers: Vec<SenderProviderConfig>,
    #[serde(with = "humantime_serde", default = "default_cadence")]
    pub cadence: Duration,
    pub tip_lamports: u64,
    pub max_lamports_per_day: u64,
    #[serde(with = "humantime_serde", default = "default_land_timeout")]
    pub land_timeout: Duration,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SenderProviderConfig {
    pub name: String,
    pub submit_url: String,
}

const fn default_cadence() -> Duration {
    Duration::from_secs(300)
}

const fn default_land_timeout() -> Duration {
    Duration::from_secs(45)
}

#[derive(Debug, Clone)]
pub struct BudgetGuard {
    spent: Arc<AtomicU64>,
    cap: u64,
}

impl BudgetGuard {
    pub fn new(cap_lamports: u64) -> Self {
        Self {
            spent: Arc::new(AtomicU64::new(0)),
            cap: cap_lamports,
        }
    }

    pub fn try_reserve(&self, lamports: u64) -> bool {
        let mut current = self.spent.load(Ordering::Relaxed);
        loop {
            if current.saturating_add(lamports) > self.cap {
                return false;
            }
            match self.spent.compare_exchange_weak(
                current,
                current + lamports,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    pub fn remaining(&self) -> u64 {
        self.cap.saturating_sub(self.spent.load(Ordering::Relaxed))
    }

    pub fn reset(&self) {
        self.spent.store(0, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingStatus {
    Pending,
    Landed { slot: u64 },
    Failed,
}

pub fn classify_status(response: &Value) -> LandingStatus {
    let entry = response
        .get("result")
        .and_then(|result| result.get("value"))
        .and_then(|value| value.get(0));
    let Some(status) = entry else {
        return LandingStatus::Pending;
    };
    if status.is_null() {
        return LandingStatus::Pending;
    }
    let has_error = status.get("err").is_some_and(|err| !err.is_null());
    if has_error {
        return LandingStatus::Failed;
    }
    let confirmation = status
        .get("confirmationStatus")
        .and_then(Value::as_str)
        .unwrap_or("");
    if confirmation == "confirmed" || confirmation == "finalized" {
        let slot = status
            .get("slot")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        LandingStatus::Landed { slot }
    } else {
        LandingStatus::Pending
    }
}

#[derive(Clone)]
pub struct SenderMetrics {
    land_latency: HistogramVec,
    landed: IntCounterVec,
    dropped: IntCounterVec,
    spend: IntCounterVec,
    budget_remaining: IntGaugeVec,
}

impl SenderMetrics {
    pub fn register(registry: &Registry) -> Result<Self, prometheus::Error> {
        let land_latency = HistogramVec::new(
            HistogramOpts::new(
                "sender_land_latency_seconds",
                "Submit-to-confirmed landing latency",
            )
            .buckets(LAND_LATENCY_BUCKETS.to_vec()),
            &["provider", "outcome"],
        )?;
        let landed = IntCounterVec::new(
            Opts::new("sender_landed_total", "Probes that landed on-chain"),
            &["provider"],
        )?;
        let dropped = IntCounterVec::new(
            Opts::new(
                "sender_dropped_total",
                "Probes that never landed before timeout",
            ),
            &["provider"],
        )?;
        let spend = IntCounterVec::new(
            Opts::new(
                "sender_spend_lamports_total",
                "Lamports spent on landing probes",
            ),
            &["provider"],
        )?;
        let budget_remaining = IntGaugeVec::new(
            Opts::new(
                "sender_budget_remaining_lamports",
                "Remaining daily probe budget",
            ),
            &["provider"],
        )?;

        registry.register(Box::new(land_latency.clone()))?;
        registry.register(Box::new(landed.clone()))?;
        registry.register(Box::new(dropped.clone()))?;
        registry.register(Box::new(spend.clone()))?;
        registry.register(Box::new(budget_remaining.clone()))?;

        Ok(Self {
            land_latency,
            landed,
            dropped,
            spend,
            budget_remaining,
        })
    }

    pub fn record_landed(&self, provider: &str, latency: Duration) {
        self.land_latency
            .with_label_values(&[provider, "landed"])
            .observe(latency.as_secs_f64());
        self.landed.with_label_values(&[provider]).inc();
    }

    pub fn record_dropped(&self, provider: &str, latency: Duration) {
        self.land_latency
            .with_label_values(&[provider, "dropped"])
            .observe(latency.as_secs_f64());
        self.dropped.with_label_values(&[provider]).inc();
    }

    pub fn add_spend(&self, provider: &str, lamports: u64) {
        self.spend.with_label_values(&[provider]).inc_by(lamports);
    }

    pub fn set_budget_remaining(&self, provider: &str, lamports: u64) {
        self.budget_remaining
            .with_label_values(&[provider])
            .set(lamports as i64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn budget_guard_reserves_up_to_the_cap() {
        let guard = BudgetGuard::new(100);
        assert!(guard.try_reserve(60));
        assert_eq!(guard.remaining(), 40);
        assert!(!guard.try_reserve(60));
        assert!(guard.try_reserve(40));
        assert_eq!(guard.remaining(), 0);
        guard.reset();
        assert_eq!(guard.remaining(), 100);
    }

    #[test]
    fn classify_pending_when_not_yet_seen() {
        assert_eq!(
            classify_status(&json!({ "result": { "value": [null] } })),
            LandingStatus::Pending
        );
        assert_eq!(
            classify_status(&json!({ "result": { "value": [] } })),
            LandingStatus::Pending
        );
    }

    #[test]
    fn classify_landed_on_confirmed() {
        let response = json!({
            "result": { "value": [{ "slot": 321, "confirmationStatus": "confirmed", "err": null }] }
        });
        assert_eq!(
            classify_status(&response),
            LandingStatus::Landed { slot: 321 }
        );
    }

    #[test]
    fn classify_failed_when_err_present() {
        let response = json!({
            "result": { "value": [{ "slot": 5, "confirmationStatus": "confirmed", "err": { "InstructionError": [] } }] }
        });
        assert_eq!(classify_status(&response), LandingStatus::Failed);
    }
}
