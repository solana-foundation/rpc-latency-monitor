use std::sync::Arc;

use serde_json::json;
use tokio::time::sleep;

use crate::config::ReferenceCheckConfig;
use crate::metrics::Metrics;
use crate::providers::ProviderEndpoint;
use crate::rpc::RpcClient;

pub fn spawn_reference_check(
    endpoints: &[ProviderEndpoint],
    client: Arc<RpcClient>,
    metrics: Metrics,
    config: ReferenceCheckConfig,
) {
    if config.rpc_url.is_empty() {
        return;
    }
    let judged: Vec<ProviderEndpoint> = endpoints
        .iter()
        .filter(|e| config.exclude_provider.as_deref() != Some(e.name.as_str()))
        .cloned()
        .collect();
    if judged.is_empty() {
        return;
    }
    tokio::spawn(async move {
        loop {
            sleep(config.interval).await;
            run_round(&client, &config, &judged, &metrics).await;
        }
    });
}

async fn run_round(
    client: &RpcClient,
    config: &ReferenceCheckConfig,
    judged: &[ProviderEndpoint],
    metrics: &Metrics,
) {
    let Some(tip) = finalized_slot(client, &config.rpc_url).await else {
        return;
    };
    let Some(slot) = tip.checked_sub(config.depth) else {
        return;
    };
    let Some(truth) = block_hash(client, &config.rpc_url, slot).await else {
        return;
    };
    for endpoint in judged {
        let observed = block_hash(client, &endpoint.url, slot).await;
        metrics.record_reference_check(&endpoint.name, classify(observed.as_deref(), &truth));
    }
}

fn classify(observed: Option<&str>, truth: &str) -> &'static str {
    match observed {
        None => "missing",
        Some(h) if h == truth => "match",
        Some(_) => "mismatch",
    }
}

async fn finalized_slot(client: &RpcClient, url: &str) -> Option<u64> {
    client
        .raw_call(url, "getSlot", json!([{ "commitment": "finalized" }]))
        .await?
        .as_u64()
}

async fn block_hash(client: &RpcClient, url: &str, slot: u64) -> Option<String> {
    let params = json!([slot, {
        "encoding": "json",
        "transactionDetails": "none",
        "rewards": false,
        "commitment": "finalized",
        "maxSupportedTransactionVersion": 0,
    }]);
    client
        .raw_call(url, "getBlock", params)
        .await?
        .get("blockhash")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_against_reference_truth() {
        assert_eq!(classify(Some("abc"), "abc"), "match");
        assert_eq!(classify(Some("xyz"), "abc"), "mismatch");
        assert_eq!(classify(None, "abc"), "missing");
    }
}
