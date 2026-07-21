use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;
use tokio::time::sleep;

use crate::config::ReferenceCheckConfig;
use crate::metrics::Metrics;
use crate::providers::ProviderEndpoint;
use crate::reference_slot::ReferenceSlot;
use crate::rpc::methods::{self, RpcMethod};
use crate::rpc::{AccountSample, CallResult, ClaimPayload, RawResponse, RpcClient};

const LATEST_BLOCKHASH_WINDOW: u64 = 8;
const CLAIM_BUFFER_CAP: usize = 1024;
const VERIFY_TICK: Duration = Duration::from_secs(2);
const CLOCK_DRIFT_SECS: i64 = 120;
const GPA_COUNT_TTL: Duration = Duration::from_secs(300);

pub fn spawn_reference_check(
    endpoints: &[ProviderEndpoint],
    client: Arc<RpcClient>,
    metrics: Metrics,
    config: ReferenceCheckConfig,
    enabled: bool,
) {
    if !enabled || config.rpc_url.is_empty() {
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
    payload: Option<ClaimPayload>,
    implausible: bool,
}

const SLOT_MS: u64 = 400;

#[derive(Clone)]
pub struct ClaimSink {
    queue: Arc<Mutex<VecDeque<Claim>>>,
    node_tip: Arc<Mutex<Option<(u64, Instant)>>>,
    fleet: ReferenceSlot,
    margin: u64,
    stale_after: u64,
}

impl ClaimSink {
    fn new(margin: u64, stale_after: u64, fleet: ReferenceSlot) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            node_tip: Arc::new(Mutex::new(None)),
            fleet,
            margin,
            stale_after,
        }
    }

    fn node_tip(&self) -> Option<u64> {
        let guard = self.node_tip.lock().ok()?;
        let (tip, polled_at) = (*guard)?;
        Some(tip + (polled_at.elapsed().as_millis() as u64) / SLOT_MS)
    }

    fn set_node_tip(&self, slot: u64) {
        if let Ok(mut guard) = self.node_tip.lock() {
            *guard = Some((slot, Instant::now()));
        }
    }

    fn node_lag(&self) -> Option<u64> {
        let node = self.node_tip()?;
        let fleet = self.fleet.current()?;
        Some(fleet.saturating_sub(node))
    }

    fn node_stale(&self) -> bool {
        self.node_lag().is_some_and(|lag| lag > self.stale_after)
    }

    fn effective_tip(&self) -> Option<u64> {
        match (self.node_tip(), self.fleet.current()) {
            (Some(n), Some(f)) => Some(n.max(f)),
            (Some(n), None) => Some(n),
            (None, f) => f,
        }
    }

    pub fn submit(&self, provider: &str, method: RpcMethod, result: &CallResult) {
        let claim_slot = result
            .claim
            .as_ref()
            .and_then(ClaimPayload::slot)
            .or(result.observed_slot);
        let slot_implausible = !self.node_stale()
            && matches!(
                (self.node_tip(), claim_slot),
                (Some(tip), Some(slot)) if slot > tip.saturating_add(self.margin)
            );
        let clock_implausible = matches!(
            result.claim,
            Some(ClaimPayload::Clock { unix_timestamp, .. })
                if (unix_now() - unix_timestamp).abs() > CLOCK_DRIFT_SECS
        );
        let implausible = slot_implausible || clock_implausible;
        let verifiable_later = matches!(
            result.claim,
            Some(
                ClaimPayload::Blockhash { .. }
                    | ClaimPayload::Accounts { .. }
                    | ClaimPayload::Transaction { .. }
            )
        );
        if !verifiable_later && !implausible {
            return;
        }
        let Some(slot) = claim_slot.or_else(|| self.effective_tip()) else {
            return;
        };
        let claim = Claim {
            provider: provider.to_owned(),
            method,
            slot,
            payload: result.claim.clone(),
            implausible,
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

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

pub fn spawn_claim_checker(
    client: Arc<RpcClient>,
    metrics: Metrics,
    config: ReferenceCheckConfig,
    fleet: ReferenceSlot,
    enabled: bool,
) -> Option<ClaimSink> {
    if !enabled || config.rpc_url.is_empty() {
        return None;
    }
    let sink = ClaimSink::new(config.claim_margin, config.node_stale_slots, fleet);
    let verifier = sink.clone();
    tokio::spawn(async move {
        let mut gpa_count: Option<(u64, Instant)> = None;
        loop {
            sleep(VERIFY_TICK).await;
            run_verify_tick(&client, &metrics, &config, &verifier, &mut gpa_count).await;
        }
    });
    Some(sink)
}

async fn run_verify_tick(
    client: &RpcClient,
    metrics: &Metrics,
    config: &ReferenceCheckConfig,
    sink: &ClaimSink,
    gpa_count: &mut Option<(u64, Instant)>,
) {
    if let Some(slot) = node_slot(client, &config.rpc_url).await {
        sink.set_node_tip(slot);
    }
    if let Some(lag) = sink.node_lag() {
        metrics.set_reference_node_lag(lag);
    }
    let Some(tip) = sink.effective_tip() else {
        return;
    };
    let due = sink.drain_due(tip, config.claim_delay_slots);
    if due.is_empty() {
        return;
    }
    let node_stale = sink.node_stale();
    let mut blocks: HashMap<u64, NodeBlock> = HashMap::new();
    for claim in due {
        let result = if claim.implausible {
            "implausible"
        } else if node_stale {
            "skipped"
        } else {
            judge_claim(client, config, &claim, &mut blocks, gpa_count).await
        };
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
    config: &ReferenceCheckConfig,
    claim: &Claim,
    blocks: &mut HashMap<u64, NodeBlock>,
    gpa_count: &mut Option<(u64, Instant)>,
) -> &'static str {
    let url = &config.rpc_url;
    match &claim.payload {
        Some(ClaimPayload::Blockhash { blockhash, .. }) => match claim.method {
            RpcMethod::GetBlockRecent => {
                judge_exact_block(client, url, claim.slot, blockhash, blocks).await
            }
            _ => judge_window_block(client, url, claim.slot, blockhash, blocks).await,
        },
        Some(ClaimPayload::Accounts { count, sample, .. }) => {
            let node_count = cached_gpa_count(client, url, gpa_count).await;
            judge_accounts(
                client,
                url,
                *count,
                sample,
                node_count,
                config.claim_count_tolerance,
            )
            .await
        }
        Some(ClaimPayload::Transaction { slot, signature }) => {
            judge_transaction(client, url, *slot, signature).await
        }
        Some(ClaimPayload::Clock { .. }) | None => "skipped",
    }
}

async fn judge_exact_block(
    client: &RpcClient,
    url: &str,
    slot: u64,
    blockhash: &str,
    blocks: &mut HashMap<u64, NodeBlock>,
) -> &'static str {
    match node_block(client, url, slot, blocks).await {
        NodeBlock::Hash(truth) if truth == blockhash => "match",
        NodeBlock::Hash(_) => "mismatch",
        NodeBlock::Skipped => "mismatch",
        NodeBlock::Unavailable => "skipped",
    }
}

async fn judge_window_block(
    client: &RpcClient,
    url: &str,
    slot: u64,
    blockhash: &str,
    blocks: &mut HashMap<u64, NodeBlock>,
) -> &'static str {
    let mut saw_block = false;
    let mut saw_unavailable = false;
    let floor = slot.saturating_sub(LATEST_BLOCKHASH_WINDOW);
    let mut cursor = slot;
    loop {
        match node_block(client, url, cursor, blocks).await {
            NodeBlock::Hash(truth) if truth == blockhash => return "match",
            NodeBlock::Hash(_) => saw_block = true,
            NodeBlock::Skipped => {}
            NodeBlock::Unavailable => saw_unavailable = true,
        }
        if cursor == floor {
            break;
        }
        cursor -= 1;
    }
    match (saw_block, saw_unavailable) {
        (_, true) => "skipped",
        (true, false) => "mismatch",
        (false, false) => "missing",
    }
}

async fn judge_accounts(
    client: &RpcClient,
    url: &str,
    count: u64,
    sample: &[AccountSample],
    node_count: Option<u64>,
    tolerance: u64,
) -> &'static str {
    if let Some(node_count) = node_count {
        if count.abs_diff(node_count) > tolerance {
            return "mismatch";
        }
    }
    if sample.is_empty() {
        return "skipped";
    }
    let pubkeys: Vec<&str> = sample.iter().map(|s| s.pubkey.as_str()).collect();
    let params = json!([pubkeys, { "encoding": "base64", "commitment": "confirmed" }]);
    let response = client
        .raw_call_checked(url, "getMultipleAccounts", params)
        .await;
    let RawResponse::Result(result) = response else {
        return "skipped";
    };
    let Some(entries) = result.get("value").and_then(serde_json::Value::as_array) else {
        return "skipped";
    };
    if entries.len() != sample.len() {
        return "skipped";
    }
    let owner = methods::probe_owner_bytes();
    let mut drift = false;
    for (claimed, entry) in sample.iter().zip(entries) {
        if entry.is_null() || !methods::gpa_account_matches(entry, &owner) {
            return "mismatch";
        }
        let node_data = entry
            .get("data")
            .and_then(serde_json::Value::as_array)
            .and_then(|a| a.first())
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if node_data != claimed.data {
            drift = true;
        }
    }
    if drift {
        "drift"
    } else {
        "match"
    }
}

async fn judge_transaction(
    client: &RpcClient,
    url: &str,
    slot: u64,
    signature: &str,
) -> &'static str {
    let params = json!([signature, {
        "encoding": "json",
        "commitment": "confirmed",
        "maxSupportedTransactionVersion": 0,
    }]);
    match client.raw_call_checked(url, "getTransaction", params).await {
        RawResponse::Result(value) if value.is_null() => "missing",
        RawResponse::Result(value) => match value.get("slot").and_then(|s| s.as_u64()) {
            Some(node_slot) if node_slot == slot => "match",
            Some(_) => "mismatch",
            None => "skipped",
        },
        RawResponse::RpcError(_) | RawResponse::Unavailable => "skipped",
    }
}

async fn cached_gpa_count(
    client: &RpcClient,
    url: &str,
    cache: &mut Option<(u64, Instant)>,
) -> Option<u64> {
    if let Some((count, fetched)) = cache {
        if fetched.elapsed() < GPA_COUNT_TTL {
            return Some(*count);
        }
    }
    let result = client
        .raw_call(url, "getProgramAccounts", methods::gpa_count_params())
        .await?;
    let count = result.as_array()?.len() as u64;
    *cache = Some((count, Instant::now()));
    Some(count)
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
    use crate::rpc::CallStatus;

    #[test]
    fn classify_against_reference_truth() {
        assert_eq!(classify(Some("abc"), "abc"), "match");
        assert_eq!(classify(Some("xyz"), "abc"), "mismatch");
        assert_eq!(classify(None, "abc"), "missing");
    }

    fn success_result(claim: Option<ClaimPayload>, observed_slot: Option<u64>) -> CallResult {
        CallResult {
            latency: Duration::from_millis(5),
            status: CallStatus::Success,
            observed_slot,
            signature: None,
            archival_signature: None,
            accounts: Vec::new(),
            claim,
        }
    }

    fn sink_with_tip(tip: u64) -> ClaimSink {
        let fleet = ReferenceSlot::new();
        fleet.observe(tip);
        let sink = ClaimSink::new(16, 64, fleet);
        sink.set_node_tip(tip);
        sink
    }

    fn blockhash(slot: u64, hash: &str) -> ClaimPayload {
        ClaimPayload::Blockhash {
            slot,
            blockhash: hash.into(),
        }
    }

    #[test]
    fn sink_buffers_payload_claims_and_ignores_plain_results() {
        let sink = sink_with_tip(1000);
        sink.submit(
            "helius",
            RpcMethod::GetLatestBlockhash,
            &success_result(Some(blockhash(990, "B")), Some(990)),
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
        let sink = sink_with_tip(1000);
        sink.submit(
            "liar",
            RpcMethod::GetSlot,
            &success_result(None, Some(1100)),
        );
        {
            let queue = sink.queue.lock().unwrap();
            assert_eq!(queue.len(), 1);
            assert!(queue[0].implausible);
        }
        sink.submit(
            "honest",
            RpcMethod::GetSlot,
            &success_result(None, Some(1010)),
        );
        assert_eq!(sink.queue.lock().unwrap().len(), 1);
    }

    #[test]
    fn stale_node_suppresses_implausibility() {
        let fleet = ReferenceSlot::new();
        fleet.observe(2000);
        let sink = ClaimSink::new(16, 64, fleet);
        sink.set_node_tip(1000);
        assert!(sink.node_stale());
        sink.submit(
            "helius",
            RpcMethod::GetSlot,
            &success_result(None, Some(1990)),
        );
        assert!(sink.queue.lock().unwrap().is_empty());
    }

    #[test]
    fn clock_drift_is_implausible_even_with_a_fresh_slot() {
        let sink = sink_with_tip(1000);
        sink.submit(
            "liar",
            RpcMethod::GetAccountInfo,
            &success_result(
                Some(ClaimPayload::Clock {
                    slot: 990,
                    unix_timestamp: unix_now() - 3600,
                }),
                Some(990),
            ),
        );
        let queue = sink.queue.lock().unwrap();
        assert_eq!(queue.len(), 1);
        assert!(queue[0].implausible);
    }

    #[test]
    fn fresh_clock_is_not_buffered() {
        let sink = sink_with_tip(1000);
        sink.submit(
            "honest",
            RpcMethod::GetAccountInfo,
            &success_result(
                Some(ClaimPayload::Clock {
                    slot: 990,
                    unix_timestamp: unix_now(),
                }),
                Some(990),
            ),
        );
        assert!(sink.queue.lock().unwrap().is_empty());
    }

    #[test]
    fn node_tip_is_projected_forward_between_polls() {
        let sink = sink_with_tip(1000);
        assert!(sink.node_tip().unwrap() >= 1000);
        sink.submit(
            "honest",
            RpcMethod::GetSlot,
            &success_result(None, Some(1015)),
        );
        assert!(sink.queue.lock().unwrap().is_empty());
    }

    #[test]
    fn drain_returns_only_settled_claims() {
        let sink = sink_with_tip(1000);
        for slot in [960, 980] {
            sink.submit(
                "helius",
                RpcMethod::GetLatestBlockhash,
                &success_result(Some(blockhash(slot, "B")), Some(slot)),
            );
        }
        let due = sink.drain_due(1000, 32);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].slot, 960);
        assert_eq!(sink.queue.lock().unwrap().len(), 1);
    }

    #[test]
    fn effective_tip_takes_the_max_of_node_and_fleet() {
        let fleet = ReferenceSlot::new();
        fleet.observe(2000);
        let sink = ClaimSink::new(16, 64, fleet);
        sink.set_node_tip(1000);
        assert_eq!(sink.effective_tip(), Some(2000));
    }

    #[test]
    fn buffer_is_bounded() {
        let sink = sink_with_tip(10);
        for _ in 0..(CLAIM_BUFFER_CAP + 10) {
            sink.submit(
                "helius",
                RpcMethod::GetBlockRecent,
                &success_result(Some(blockhash(5, "B")), None),
            );
        }
        assert_eq!(sink.queue.lock().unwrap().len(), CLAIM_BUFFER_CAP);
    }

    async fn judge_block_with(
        map: &[(u64, NodeBlock)],
        method: RpcMethod,
        slot: u64,
        hash: &str,
    ) -> &'static str {
        let mut cache: HashMap<u64, NodeBlock> = map.iter().cloned().collect();
        for s in slot.saturating_sub(LATEST_BLOCKHASH_WINDOW)..=slot {
            cache.entry(s).or_insert(NodeBlock::Skipped);
        }
        let client = RpcClient::new(Duration::from_millis(1)).unwrap();
        match method {
            RpcMethod::GetBlockRecent => {
                judge_exact_block(&client, "http://127.0.0.1:1", slot, hash, &mut cache).await
            }
            _ => judge_window_block(&client, "http://127.0.0.1:1", slot, hash, &mut cache).await,
        }
    }

    #[tokio::test]
    async fn block_recent_claim_matches_the_node_hash() {
        let m = RpcMethod::GetBlockRecent;
        assert_eq!(
            judge_block_with(&[(100, NodeBlock::Hash("AAA".into()))], m, 100, "AAA").await,
            "match"
        );
        assert_eq!(
            judge_block_with(&[(100, NodeBlock::Hash("BBB".into()))], m, 100, "AAA").await,
            "mismatch"
        );
        assert_eq!(
            judge_block_with(&[(100, NodeBlock::Skipped)], m, 100, "AAA").await,
            "mismatch"
        );
        assert_eq!(
            judge_block_with(&[(100, NodeBlock::Unavailable)], m, 100, "AAA").await,
            "skipped"
        );
    }

    #[tokio::test]
    async fn latest_blockhash_matches_anywhere_in_the_window() {
        let m = RpcMethod::GetLatestBlockhash;
        assert_eq!(
            judge_block_with(&[(98, NodeBlock::Hash("AAA".into()))], m, 100, "AAA").await,
            "match"
        );
        assert_eq!(
            judge_block_with(&[(97, NodeBlock::Hash("ZZZ".into()))], m, 100, "AAA").await,
            "mismatch"
        );
        assert_eq!(judge_block_with(&[], m, 100, "AAA").await, "missing");
        assert_eq!(
            judge_block_with(
                &[
                    (100, NodeBlock::Unavailable),
                    (99, NodeBlock::Hash("ZZZ".into())),
                ],
                m,
                100,
                "AAA"
            )
            .await,
            "skipped"
        );
    }

    #[tokio::test]
    async fn account_count_outside_tolerance_is_a_mismatch() {
        let client = RpcClient::new(Duration::from_millis(1)).unwrap();
        let sample = vec![AccountSample {
            pubkey: "k1".into(),
            data: "d1".into(),
        }];
        assert_eq!(
            judge_accounts(&client, "http://127.0.0.1:1", 100, &sample, Some(50), 8).await,
            "mismatch"
        );
        assert_eq!(
            judge_accounts(&client, "http://127.0.0.1:1", 100, &sample, Some(104), 8).await,
            "skipped"
        );
    }
}
