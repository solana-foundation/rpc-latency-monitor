use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::time::sleep;

use crate::config::ReferenceCheckConfig;
use crate::metrics::Metrics;
use crate::providers::ProviderEndpoint;
use crate::rpc::methods::RpcMethod;
use crate::rpc::{CallResult, RawResponse, RpcClient};

const LATEST_BLOCKHASH_WINDOW: u64 = 8;
const CLAIM_BUFFER_CAP: usize = 1024;
const VERIFY_TICK: Duration = Duration::from_secs(2);

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

#[derive(Debug, Clone)]
struct Claim {
    provider: String,
    method: RpcMethod,
    slot: u64,
    blockhash: Option<String>,
    implausible: bool,
}

#[derive(Clone)]
pub struct ClaimSink {
    queue: Arc<Mutex<VecDeque<Claim>>>,
    node_tip: Arc<AtomicU64>,
    margin: u64,
}

impl ClaimSink {
    fn new(margin: u64) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            node_tip: Arc::new(AtomicU64::new(0)),
            margin,
        }
    }

    fn node_tip(&self) -> Option<u64> {
        match self.node_tip.load(Ordering::Relaxed) {
            0 => None,
            tip => Some(tip),
        }
    }

    fn set_node_tip(&self, slot: u64) {
        self.node_tip.fetch_max(slot, Ordering::Relaxed);
    }

    pub fn submit(&self, provider: &str, method: RpcMethod, result: &CallResult) {
        let implausible = matches!(
            (self.node_tip(), result.observed_slot),
            (Some(tip), Some(slot)) if slot > tip.saturating_add(self.margin)
        );
        let claim = match (&result.blockhash_claim, implausible) {
            (Some(c), _) => Claim {
                provider: provider.to_owned(),
                method,
                slot: c.slot,
                blockhash: Some(c.blockhash.clone()),
                implausible,
            },
            (None, true) => Claim {
                provider: provider.to_owned(),
                method,
                slot: result.observed_slot.unwrap_or_default(),
                blockhash: None,
                implausible: true,
            },
            (None, false) => return,
        };
        if let Ok(mut queue) = self.queue.lock() {
            if queue.len() >= CLAIM_BUFFER_CAP {
                queue.pop_front();
            }
            queue.push_back(claim);
        }
    }

    fn drain_due(&self, tip: u64, delay: u64) -> Vec<Claim> {
        let Ok(mut queue) = self.queue.lock() else {
            return Vec::new();
        };
        let mut due = Vec::new();
        queue.retain(|claim| {
            if claim.implausible || claim.slot.saturating_add(delay) <= tip {
                due.push(claim.clone());
                false
            } else {
                true
            }
        });
        due
    }
}

pub fn spawn_claim_checker(
    client: Arc<RpcClient>,
    metrics: Metrics,
    config: ReferenceCheckConfig,
) -> Option<ClaimSink> {
    if config.rpc_url.is_empty() {
        return None;
    }
    let sink = ClaimSink::new(config.claim_margin);
    let verifier = sink.clone();
    tokio::spawn(async move {
        loop {
            sleep(VERIFY_TICK).await;
            run_verify_tick(&client, &metrics, &config, &verifier).await;
        }
    });
    Some(sink)
}

async fn run_verify_tick(
    client: &RpcClient,
    metrics: &Metrics,
    config: &ReferenceCheckConfig,
    sink: &ClaimSink,
) {
    if let Some(slot) = node_slot(client, &config.rpc_url).await {
        sink.set_node_tip(slot);
    }
    let Some(tip) = sink.node_tip() else {
        return;
    };
    let due = sink.drain_due(tip, config.claim_delay_slots);
    if due.is_empty() {
        return;
    }
    let mut cache: HashMap<u64, NodeBlock> = HashMap::new();
    for claim in due {
        let result = judge_claim(client, &config.rpc_url, &claim, &mut cache).await;
        metrics.record_claim_check(&claim.provider, claim.method, result);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NodeBlock {
    Hash(String),
    Skipped,
    Unavailable,
}

async fn judge_claim(
    client: &RpcClient,
    url: &str,
    claim: &Claim,
    cache: &mut HashMap<u64, NodeBlock>,
) -> &'static str {
    if claim.implausible {
        return "implausible";
    }
    let Some(blockhash) = &claim.blockhash else {
        return "skipped";
    };
    match claim.method {
        RpcMethod::GetBlockRecent => {
            match node_block(client, url, claim.slot, cache).await {
                NodeBlock::Hash(truth) if &truth == blockhash => "match",
                NodeBlock::Hash(_) => "mismatch",
                NodeBlock::Skipped => "mismatch",
                NodeBlock::Unavailable => "skipped",
            }
        }
        RpcMethod::GetLatestBlockhash => {
            let mut saw_block = false;
            let mut saw_unavailable = false;
            let floor = claim.slot.saturating_sub(LATEST_BLOCKHASH_WINDOW);
            let mut slot = claim.slot;
            loop {
                match node_block(client, url, slot, cache).await {
                    NodeBlock::Hash(truth) if &truth == blockhash => return "match",
                    NodeBlock::Hash(_) => saw_block = true,
                    NodeBlock::Skipped => {}
                    NodeBlock::Unavailable => saw_unavailable = true,
                }
                if slot == floor {
                    break;
                }
                slot -= 1;
            }
            match (saw_block, saw_unavailable) {
                (_, true) => "skipped",
                (true, false) => "mismatch",
                (false, false) => "missing",
            }
        }
        _ => "skipped",
    }
}

async fn node_block(
    client: &RpcClient,
    url: &str,
    slot: u64,
    cache: &mut HashMap<u64, NodeBlock>,
) -> NodeBlock {
    if let Some(cached) = cache.get(&slot) {
        return cached.clone();
    }
    let params = json!([slot, {
        "encoding": "json",
        "transactionDetails": "none",
        "rewards": false,
        "commitment": "confirmed",
        "maxSupportedTransactionVersion": 0,
    }]);
    let answer = match client.raw_call_checked(url, "getBlock", params).await {
        RawResponse::Result(block) => match block.get("blockhash").and_then(|h| h.as_str()) {
            Some(hash) if !hash.is_empty() => NodeBlock::Hash(hash.to_owned()),
            _ => NodeBlock::Unavailable,
        },
        RawResponse::RpcError(-32007 | -32009) => NodeBlock::Skipped,
        RawResponse::RpcError(_) | RawResponse::Unavailable => NodeBlock::Unavailable,
    };
    cache.insert(slot, answer.clone());
    answer
}

async fn node_slot(client: &RpcClient, url: &str) -> Option<u64> {
    client
        .raw_call(url, "getSlot", json!([{ "commitment": "processed" }]))
        .await?
        .as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::{BlockhashClaim, CallStatus};
    use std::time::Duration as StdDuration;

    #[test]
    fn classify_against_reference_truth() {
        assert_eq!(classify(Some("abc"), "abc"), "match");
        assert_eq!(classify(Some("xyz"), "abc"), "mismatch");
        assert_eq!(classify(None, "abc"), "missing");
    }

    fn success_result(claim: Option<BlockhashClaim>, observed_slot: Option<u64>) -> CallResult {
        CallResult {
            latency: StdDuration::from_millis(5),
            status: CallStatus::Success,
            observed_slot,
            signature: None,
            archival_signature: None,
            accounts: Vec::new(),
            blockhash_claim: claim,
        }
    }

    #[test]
    fn sink_buffers_blockhash_claims_and_ignores_plain_results() {
        let sink = ClaimSink::new(16);
        sink.set_node_tip(1000);

        sink.submit(
            "helius",
            RpcMethod::GetLatestBlockhash,
            &success_result(
                Some(BlockhashClaim {
                    slot: 990,
                    blockhash: "B".into(),
                }),
                Some(990),
            ),
        );
        sink.submit(
            "helius",
            RpcMethod::GetSlot,
            &success_result(None, Some(995)),
        );
        assert_eq!(sink.queue.lock().unwrap().len(), 1);
    }

    #[test]
    fn sink_flags_slots_ahead_of_the_node_tip() {
        let sink = ClaimSink::new(16);
        sink.set_node_tip(1000);

        sink.submit(
            "liar",
            RpcMethod::GetSlot,
            &success_result(None, Some(1100)),
        );
        let queue = sink.queue.lock().unwrap();
        assert_eq!(queue.len(), 1);
        assert!(queue[0].implausible);

        drop(queue);
        sink.submit(
            "honest",
            RpcMethod::GetSlot,
            &success_result(None, Some(1010)),
        );
        assert_eq!(sink.queue.lock().unwrap().len(), 1);
    }

    #[test]
    fn sink_without_a_node_tip_never_flags() {
        let sink = ClaimSink::new(16);
        sink.submit(
            "helius",
            RpcMethod::GetSlot,
            &success_result(None, Some(1_000_000)),
        );
        assert!(sink.queue.lock().unwrap().is_empty());
    }

    #[test]
    fn drain_returns_only_settled_claims() {
        let sink = ClaimSink::new(16);
        sink.set_node_tip(1000);
        for slot in [960, 980] {
            sink.submit(
                "helius",
                RpcMethod::GetLatestBlockhash,
                &success_result(
                    Some(BlockhashClaim {
                        slot,
                        blockhash: "B".into(),
                    }),
                    Some(slot),
                ),
            );
        }
        let due = sink.drain_due(1000, 32);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].slot, 960);
        assert_eq!(sink.queue.lock().unwrap().len(), 1);
    }

    #[test]
    fn buffer_is_bounded() {
        let sink = ClaimSink::new(16);
        sink.set_node_tip(10);
        for _ in 0..(CLAIM_BUFFER_CAP + 10) {
            sink.submit(
                "helius",
                RpcMethod::GetBlockRecent,
                &success_result(
                    Some(BlockhashClaim {
                        slot: 5,
                        blockhash: "B".into(),
                    }),
                    None,
                ),
            );
        }
        assert_eq!(sink.queue.lock().unwrap().len(), CLAIM_BUFFER_CAP);
    }

    fn claim(method: RpcMethod, slot: u64, blockhash: &str) -> Claim {
        Claim {
            provider: "p".into(),
            method,
            slot,
            blockhash: Some(blockhash.into()),
            implausible: false,
        }
    }

    async fn judge_with(map: &[(u64, NodeBlock)], c: &Claim) -> &'static str {
        let mut cache: HashMap<u64, NodeBlock> = map.iter().cloned().collect();
        for slot in c.slot.saturating_sub(LATEST_BLOCKHASH_WINDOW)..=c.slot {
            cache.entry(slot).or_insert(NodeBlock::Skipped);
        }
        let client = RpcClient::new(StdDuration::from_millis(1)).unwrap();
        judge_claim(&client, "http://127.0.0.1:1", c, &mut cache).await
    }

    #[tokio::test]
    async fn block_recent_claim_matches_the_node_hash() {
        let c = claim(RpcMethod::GetBlockRecent, 100, "AAA");
        assert_eq!(
            judge_with(&[(100, NodeBlock::Hash("AAA".into()))], &c).await,
            "match"
        );
        assert_eq!(
            judge_with(&[(100, NodeBlock::Hash("BBB".into()))], &c).await,
            "mismatch"
        );
        assert_eq!(judge_with(&[(100, NodeBlock::Skipped)], &c).await, "mismatch");
        assert_eq!(
            judge_with(&[(100, NodeBlock::Unavailable)], &c).await,
            "skipped"
        );
    }

    #[tokio::test]
    async fn latest_blockhash_matches_anywhere_in_the_window() {
        let c = claim(RpcMethod::GetLatestBlockhash, 100, "AAA");
        assert_eq!(
            judge_with(&[(100, NodeBlock::Hash("AAA".into()))], &c).await,
            "match"
        );
        assert_eq!(
            judge_with(
                &[
                    (100, NodeBlock::Skipped),
                    (98, NodeBlock::Hash("AAA".into())),
                ],
                &c
            )
            .await,
            "match"
        );
        assert_eq!(
            judge_with(&[(97, NodeBlock::Hash("ZZZ".into()))], &c).await,
            "mismatch"
        );
        assert_eq!(judge_with(&[], &c).await, "missing");
        assert_eq!(
            judge_with(
                &[
                    (100, NodeBlock::Unavailable),
                    (99, NodeBlock::Hash("ZZZ".into())),
                ],
                &c
            )
            .await,
            "skipped"
        );
    }

    #[tokio::test]
    async fn implausible_claims_are_reported_as_such() {
        let c = Claim {
            provider: "p".into(),
            method: RpcMethod::GetSlot,
            slot: 100,
            blockhash: None,
            implausible: true,
        };
        assert_eq!(judge_with(&[], &c).await, "implausible");
    }
}
