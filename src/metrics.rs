use std::collections::HashMap;

use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGaugeVec, Opts, Registry, TextEncoder,
};

use crate::rpc::methods::RpcMethod;
use crate::rpc::CallResult;

const LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.002, 0.003, 0.004, 0.005, 0.006, 0.007, 0.008, 0.009, 0.01, 0.0125, 0.015, 0.0175,
    0.02, 0.025, 0.03, 0.04, 0.05, 0.06, 0.07, 0.08, 0.09, 0.1, 0.15, 0.2, 0.3, 0.5, 1.0, 2.5, 5.0,
    10.0,
];

#[derive(Clone)]
pub struct Metrics {
    registry: Registry,
    latency: HistogramVec,
    slot_lag: IntGaugeVec,
    requests: IntCounterVec,
    up: IntGaugeVec,
}

impl Metrics {
    pub fn new(region: &str) -> Result<Self, prometheus::Error> {
        let const_labels = HashMap::from([("region".to_string(), region.to_string())]);
        let registry = Registry::new_custom(None, Some(const_labels))?;

        let latency = HistogramVec::new(
            HistogramOpts::new("rpc_latency_seconds", "RPC request round-trip latency")
                .buckets(LATENCY_BUCKETS.to_vec()),
            &["provider", "method", "status"],
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
            &["provider", "method", "status", "error_kind"],
        )?;
        let up = IntGaugeVec::new(
            Opts::new("rpc_up", "Whether the provider's last check succeeded"),
            &["provider"],
        )?;

        registry.register(Box::new(latency.clone()))?;
        registry.register(Box::new(slot_lag.clone()))?;
        registry.register(Box::new(requests.clone()))?;
        registry.register(Box::new(up.clone()))?;

        Ok(Self {
            registry,
            latency,
            slot_lag,
            requests,
            up,
        })
    }

    pub fn record_call(&self, provider: &str, method: RpcMethod, result: &CallResult) {
        let method = method.label();
        let status = result.status.label();
        let error_kind = result.status.error_kind().unwrap_or("none");

        self.latency
            .with_label_values(&[provider, method, status])
            .observe(result.latency.as_secs_f64());
        self.requests
            .with_label_values(&[provider, method, status, error_kind])
            .inc();
        self.up
            .with_label_values(&[provider])
            .set(i64::from(result.status.is_success()));
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
            accounts: Vec::new(),
        }
    }

    #[test]
    fn encodes_recorded_calls_with_region_label() {
        let metrics = Metrics::new("test-region").unwrap();
        metrics.record_call("helius", RpcMethod::GetSlot, &result(CallStatus::Success));
        metrics.record_call(
            "triton",
            RpcMethod::GetSlot,
            &result(CallStatus::Error(ErrorKind::Timeout)),
        );

        let output = metrics.encode().unwrap();
        assert!(output.contains("rpc_latency_seconds"));
        assert!(output.contains("rpc_requests_total"));
        assert!(output.contains("region=\"test-region\""));
        assert!(output.contains("status=\"success\""));
        assert!(output.contains("error_kind=\"timeout\""));
    }

    #[test]
    fn records_slot_lag_and_up_gauge() {
        let metrics = Metrics::new("test").unwrap();
        metrics.record_slot_lag("helius", RpcMethod::GetSlot, 7);
        metrics.record_call("helius", RpcMethod::GetSlot, &result(CallStatus::Success));

        let output = metrics.encode().unwrap();
        assert!(output.contains("rpc_slot_lag"));
        assert!(output.contains("rpc_up"));
        assert!(output.contains("} 7"));
    }
}
