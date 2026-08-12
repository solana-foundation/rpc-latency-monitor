use std::collections::HashMap;

use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
    TextEncoder,
};

use crate::rpc::methods::RpcMethod;
use crate::rpc::{CallResult, CallStatus};

const LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.002, 0.003, 0.005, 0.01, 0.02, 0.03, 0.05, 0.1, 0.15, 0.2, 0.3, 0.4, 0.5, 1.0, 5.0,
    30.0,
];

#[derive(Clone)]
pub struct Metrics {
    registry: Registry,
    latency: HistogramVec,
    slot_lag: IntGaugeVec,
    requests: IntCounterVec,
    up: IntGaugeVec,
    reference_check: IntCounterVec,
    claim_check: IntCounterVec,
    reference_node_lag: IntGauge,
}

impl Metrics {
    pub fn new(region: &str, geo: &str) -> Result<Self, prometheus::Error> {
        let const_labels = HashMap::from([
            ("region".to_string(), region.to_string()),
            ("geo".to_string(), geo.to_string()),
        ]);
        let registry = Registry::new_custom(None, Some(const_labels))?;

        let latency = HistogramVec::new(
            HistogramOpts::new("rpc_latency_seconds", "RPC request round-trip latency")
                .buckets(LATENCY_BUCKETS.to_vec()),
            &["provider", "method", "status", "target"],
        )?;
        let slot_lag = IntGaugeVec::new(
            Opts::new(
                "rpc_slot_lag",
                "Slots a provider trails the observed chain tip",
            ),
            &["provider", "method"],
        )?;
        let requests = IntCounterVec::new(
            Opts::new("rpc_requests_total", "Total RPC requests by outcome"),
            &["provider", "method", "status", "error_kind", "target"],
        )?;
        let up = IntGaugeVec::new(
            Opts::new("rpc_up", "Whether the provider's last check succeeded"),
            &["provider"],
        )?;
        let reference_check = IntCounterVec::new(
            Opts::new(
                "rpc_reference_check_total",
                "Provider's finalized getBlock blockhash vs the trusted reference, by outcome",
            ),
            &["provider", "result"],
        )?;

        let claim_check = IntCounterVec::new(
            Opts::new(
                "rpc_claim_check_total",
                "Provider (slot, blockhash) claims verified against the reference node, by outcome",
            ),
            &["provider", "method", "target", "result"],
        )?;

        registry.register(Box::new(latency.clone()))?;
        registry.register(Box::new(slot_lag.clone()))?;
        registry.register(Box::new(requests.clone()))?;
        registry.register(Box::new(up.clone()))?;
        registry.register(Box::new(reference_check.clone()))?;
        let reference_node_lag = IntGauge::new(
            "rpc_reference_node_lag",
            "Slots the reference node trails the fleet-observed chain tip",
        )?;

        registry.register(Box::new(claim_check.clone()))?;
        registry.register(Box::new(reference_node_lag.clone()))?;

        Ok(Self {
            registry,
            latency,
            slot_lag,
            requests,
            up,
            reference_check,
            claim_check,
            reference_node_lag,
        })
    }

    pub fn record_claim_check(
        &self,
        provider: &str,
        method: RpcMethod,
        target: &str,
        result: &str,
    ) {
        self.claim_check
            .with_label_values(&[provider, method.label(), target, result])
            .inc();
    }

    pub fn set_reference_node_lag(&self, lag: u64) {
        self.reference_node_lag.set(lag as i64);
    }

    pub fn record_reference_check(&self, provider: &str, result: &str) {
        self.reference_check
            .with_label_values(&[provider, result])
            .inc();
    }

    pub fn record_call(
        &self,
        provider: &str,
        method: RpcMethod,
        result: &CallResult,
        status: CallStatus,
        target: &str,
    ) {
        let method = method.label();
        let status_label = status.label();
        let error_kind = status.error_kind().unwrap_or("none");

        self.latency
            .with_label_values(&[provider, method, status_label, target])
            .observe(result.latency.as_secs_f64());
        self.requests
            .with_label_values(&[provider, method, status_label, error_kind, target])
            .inc();
        self.up
            .with_label_values(&[provider])
            .set(i64::from(!status.is_error()));
    }

    pub fn record_slot_lag(&self, provider: &str, method: RpcMethod, lag: u64) {
        self.slot_lag
            .with_label_values(&[provider, method.label()])
            .set(lag as i64);
    }

    pub fn encode(&self) -> Result<String, prometheus::Error> {
        let mut buffer = Vec::new();
        TextEncoder::new().encode(&self.registry.gather(), &mut buffer)?;
        String::from_utf8(buffer).map_err(|e| prometheus::Error::Msg(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::{CallStatus, ErrorKind};
    use std::time::Duration;

    fn result(status: CallStatus) -> CallResult {
        CallResult {
            latency: Duration::from_millis(12),
            status,
            observed_slot: Some(100),
            signature: None,
            archival_signature: None,
            accounts: Vec::new(),
            claim: None,
        }
    }

    #[test]
    fn claim_check_counter_carries_method_and_result() {
        let metrics = Metrics::new("test", "us-east").unwrap();
        metrics.record_claim_check("helius", RpcMethod::GetLatestBlockhash, "", "match");
        metrics.record_claim_check(
            "helius",
            RpcMethod::GetProgramAccounts,
            "token_owner",
            "match",
        );
        let output = metrics.encode().unwrap();
        assert!(output.contains("rpc_claim_check_total"));
        assert!(output.contains("method=\"getLatestBlockhash\""));
        assert!(output.contains("result=\"match\""));
        assert!(output.contains("target=\"token_owner\""));
    }

    #[test]
    fn encodes_recorded_calls_with_region_label() {
        let metrics = Metrics::new("test-region", "us-east").unwrap();
        metrics.record_call(
            "helius",
            RpcMethod::GetSlot,
            &result(CallStatus::Success),
            CallStatus::Success,
            "",
        );
        metrics.record_call(
            "triton",
            RpcMethod::GetSlot,
            &result(CallStatus::Error(ErrorKind::Timeout)),
            CallStatus::Error(ErrorKind::Timeout),
            "",
        );

        let output = metrics.encode().unwrap();
        assert!(output.contains("rpc_latency_seconds"));
        assert!(output.contains("rpc_requests_total"));
        assert!(output.contains("region=\"test-region\""));
        assert!(output.contains("status=\"success\""));
        assert!(output.contains("error_kind=\"timeout\""));
    }

    #[test]
    fn skipped_slot_keeps_the_provider_up() {
        let metrics = Metrics::new("test", "us-east").unwrap();
        metrics.record_call(
            "triton",
            RpcMethod::GetBlockRecent,
            &result(CallStatus::Skipped),
            CallStatus::Skipped,
            "",
        );
        let output = metrics.encode().unwrap();
        assert!(output.contains("status=\"skipped\""));
        let up_line = output
            .lines()
            .find(|l| l.starts_with("rpc_up{") && l.contains("provider=\"triton\""))
            .unwrap();
        assert!(up_line.ends_with(" 1"));
    }

    #[test]
    fn records_slot_lag_and_up_gauge() {
        let metrics = Metrics::new("test", "us-east").unwrap();
        metrics.record_slot_lag("helius", RpcMethod::GetSlot, 7);
        metrics.record_call(
            "helius",
            RpcMethod::GetSlot,
            &result(CallStatus::Success),
            CallStatus::Success,
            "",
        );

        let output = metrics.encode().unwrap();
        assert!(output.contains("rpc_slot_lag"));
        assert!(output.contains("rpc_up"));
        assert!(output.contains("} 7"));
    }
}
