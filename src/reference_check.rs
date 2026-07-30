use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::RngExt;
use serde_json::{json, Value};
use tokio::task::JoinSet;
use tokio::time::sleep;

use crate::config::{GpaTarget, ReferenceCheckConfig};
use crate::metrics::Metrics;
use crate::providers::ProviderEndpoint;
use crate::reference_slot::{ArchivalAnchor, ReferenceSlot};
use crate::rpc::methods::{self, RpcMethod};
use crate::rpc::{
    AccountSample, CallResult, CallStatus, ClaimPayload, ErrorKind, RawResponse, RpcClient,
};

const ARCHIVAL_MIN_DEPTH: u64 = 40_000_000;
const ARCHIVAL_FLOOR: u64 = 20_000_000;
const ARCHIVAL_SLOT_PICK_TRIES: u32 = 32;
const ARCHIVAL_MIN_QUORUM: usize = 2;
const ARCHIVAL_USED_CAP: usize = 200_000;

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

pub fn spawn_archival_check(
    endpoints: &[ProviderEndpoint],
    client: Arc<RpcClient>,
    metrics: Metrics,
    reference: ReferenceSlot,
    interval: Duration,
    anchor: ArchivalAnchor,
) {
    let providers: Vec<ProviderEndpoint> = endpoints.to_vec();
    if providers.len() < ARCHIVAL_MIN_QUORUM {
        return;
    }
    tokio::spawn(async move {
        let mut used = UsedSlots::default();
        loop {
            sleep(interval).await;
            run_archival_round(
                &client, &providers, &metrics, &reference, &mut used, &anchor,
            )
            .await;
        }
    });
}

async fn run_archival_round(
    client: &Arc<RpcClient>,
    providers: &[ProviderEndpoint],
    metrics: &Metrics,
    reference: &ReferenceSlot,
    used: &mut UsedSlots,
    anchor: &ArchivalAnchor,
) {
    let Some(tip) = reference.current() else {
        return;
    };
    let Some(ceil) = tip.checked_sub(ARCHIVAL_MIN_DEPTH) else {
        return;
    };
    if ceil <= ARCHIVAL_FLOOR {
        return;
    }
    let Some(slot) = pick_unused_slot(ARCHIVAL_FLOOR, ceil, used) else {
        return;
    };

    let mut block_set = JoinSet::new();
    for provider in providers {
        let client = client.clone();
        let name = provider.name.clone();
        let url = provider.url.clone();
        block_set.spawn(async move {
            let (result, hash, sig) = timed_archival_block(&client, &url, slot).await;
            (name, result, hash, sig)
        });
    }
    let mut observed: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    while let Some(joined) = block_set.join_next().await {
        let Ok((name, result, hash, sig)) = joined else {
            continue;
        };
        let status = result.status;
        metrics.record_call(&name, RpcMethod::GetBlockArchival, &result, status, "");
        observed.push((name, hash, sig));
    }

    let Some((truth, truth_sig)) = majority_block(&observed) else {
        return;
    };
    for (name, hash, _) in &observed {
        let verdict = match hash {
            Some(h) if *h == truth => "match",
            Some(_) => "mismatch",
            None => "skipped",
        };
        metrics.record_claim_check(name, RpcMethod::GetBlockArchival, "", verdict);
    }

    let Some(sig) = truth_sig else {
        return;
    };
    let mut tx_set = JoinSet::new();
    for provider in providers {
        let client = client.clone();
        let name = provider.name.clone();
        let url = provider.url.clone();
        let sig = sig.clone();
        tx_set.spawn(async move {
            let (result, tx_slot) = timed_archival_tx(&client, &url, &sig).await;
            (name, result, tx_slot)
        });
    }
    let mut sig_confirmations = 0usize;
    while let Some(joined) = tx_set.join_next().await {
        let Ok((name, result, tx_slot)) = joined else {
            continue;
        };
        let status = result.status;
        metrics.record_call(
            &name,
            RpcMethod::GetTransactionArchival,
            &result,
            status,
            "",
        );
        let verdict = match tx_slot {
            Some(s) if s == slot => "match",
            Some(_) => "mismatch",
            None => "skipped",
        };
        if verdict == "match" {
            sig_confirmations += 1;
        }
        metrics.record_claim_check(&name, RpcMethod::GetTransactionArchival, "", verdict);
    }
    // The signature came from ONE provider that matched the majority hash; only
    // anchor gSFA on it once a quorum has independently confirmed it at this
    // slot, so a fabricated signature can't poison every provider's cursor.
    if sig_confirmations >= ARCHIVAL_MIN_QUORUM {
        anchor.set(slot, sig);
    }
}

#[derive(Default)]
struct UsedSlots {
    set: HashSet<u64>,
    order: VecDeque<u64>,
}

impl UsedSlots {
    fn insert(&mut self, slot: u64) -> bool {
        if !self.set.insert(slot) {
            return false;
        }
        self.order.push_back(slot);
        if self.order.len() > ARCHIVAL_USED_CAP {
            if let Some(evicted) = self.order.pop_front() {
                self.set.remove(&evicted);
            }
        }
        true
    }
}

fn pick_unused_slot(floor: u64, ceil: u64, used: &mut UsedSlots) -> Option<u64> {
    let mut rng = rand::rng();
    for _ in 0..ARCHIVAL_SLOT_PICK_TRIES {
        let slot = rng.random_range(floor..=ceil);
        if used.insert(slot) {
            return Some(slot);
        }
    }
    None
}

fn majority_block(
    observed: &[(String, Option<String>, Option<String>)],
) -> Option<(String, Option<String>)> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (_, hash, _) in observed {
        if let Some(h) = hash {
            *counts.entry(h.as_str()).or_insert(0) += 1;
        }
    }
    let (winner, votes) = counts.into_iter().max_by_key(|&(_, n)| n)?;
    if votes < ARCHIVAL_MIN_QUORUM {
        return None;
    }
    let winner = winner.to_string();
    let sig = observed
        .iter()
        .find(|(_, h, _)| h.as_deref() == Some(winner.as_str()))
        .and_then(|(_, _, s)| s.clone());
    Some((winner, sig))
}

fn archival_result(latency: Duration, status: CallStatus) -> CallResult {
    CallResult {
        latency,
        status,
        observed_slot: None,
        signature: None,
        archival_signature: None,
        accounts: Vec::new(),
        claim: None,
    }
}

async fn timed_archival_block(
    client: &RpcClient,
    url: &str,
    slot: u64,
) -> (CallResult, Option<String>, Option<String>) {
    let params = json!([slot, {
        "encoding": "json",
        "transactionDetails": "full",
        "rewards": false,
        "commitment": "confirmed",
        "maxSupportedTransactionVersion": 0,
    }]);
    let start = Instant::now();
    let resp = client.raw_call_checked(url, "getBlock", params).await;
    let latency = start.elapsed();
    match resp {
        RawResponse::Result(value) => {
            let hash = value
                .get("blockhash")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let sig = first_signature(&value);
            let status = if hash.is_some() {
                CallStatus::Success
            } else {
                CallStatus::Error(ErrorKind::Empty)
            };
            (archival_result(latency, status), hash, sig)
        }
        RawResponse::RpcError(-32007 | -32009) => {
            (archival_result(latency, CallStatus::Skipped), None, None)
        }
        RawResponse::RpcError(code) => (
            archival_result(latency, CallStatus::Error(ErrorKind::RpcError(code))),
            None,
            None,
        ),
        RawResponse::Unavailable => (
            archival_result(latency, CallStatus::Error(ErrorKind::Transport)),
            None,
            None,
        ),
    }
}

fn first_signature(block: &Value) -> Option<String> {
    block
        .get("transactions")?
        .as_array()?
        .first()?
        .get("transaction")?
        .get("signatures")?
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_owned)
}

async fn timed_archival_tx(
    client: &RpcClient,
    url: &str,
    signature: &str,
) -> (CallResult, Option<u64>) {
    let params = json!([signature, {
        "encoding": "json",
        "commitment": "confirmed",
        "maxSupportedTransactionVersion": 0,
    }]);
    let start = Instant::now();
    let resp = client.raw_call_checked(url, "getTransaction", params).await;
    let latency = start.elapsed();
    match resp {
        RawResponse::Result(value) if value.is_null() => {
            (archival_result(latency, CallStatus::Skipped), None)
        }
        RawResponse::Result(value) => {
            let tx_slot = value.get("slot").and_then(Value::as_u64);
            let status = if tx_slot.is_some() {
                CallStatus::Success
            } else {
                CallStatus::Error(ErrorKind::Empty)
            };
            (archival_result(latency, status), tx_slot)
        }
        RawResponse::RpcError(code) => (
            archival_result(latency, CallStatus::Error(ErrorKind::RpcError(code))),
            None,
        ),
        RawResponse::Unavailable => (
            archival_result(latency, CallStatus::Error(ErrorKind::Transport)),
            None,
        ),
    }
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
        let node_caught_up = self.node_lag().is_some_and(|lag| lag <= self.margin);
        let slot_implausible = node_caught_up
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
        let mut gpa_counts: HashMap<String, (u64, Instant)> = HashMap::new();
        loop {
            sleep(VERIFY_TICK).await;
            run_verify_tick(&client, &metrics, &config, &verifier, &mut gpa_counts).await;
        }
    });
    Some(sink)
}

async fn run_verify_tick(
    client: &RpcClient,
    metrics: &Metrics,
    config: &ReferenceCheckConfig,
    sink: &ClaimSink,
    gpa_counts: &mut HashMap<String, (u64, Instant)>,
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
            judge_claim(client, config, &claim, &mut blocks, gpa_counts).await
        };
        let target = match &claim.payload {
            Some(ClaimPayload::Accounts { target, .. }) => target.name.as_str(),
            _ => "",
        };
        metrics.record_claim_check(&claim.provider, claim.method, target, result);
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
    gpa_counts: &mut HashMap<String, (u64, Instant)>,
) -> &'static str {
    let url = &config.rpc_url;
    match &claim.payload {
        Some(ClaimPayload::Blockhash { blockhash, .. }) => match claim.method {
            RpcMethod::GetBlockRecent => {
                judge_exact_block(client, url, claim.slot, blockhash, blocks).await
            }
            _ => judge_window_block(client, url, claim.slot, blockhash, blocks).await,
        },
        Some(ClaimPayload::Accounts {
            count,
            sample,
            target,
            ..
        }) => {
            let node_count = cached_gpa_count(client, url, target, gpa_counts).await;
            judge_accounts(
                client,
                url,
                *count,
                sample,
                target,
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

#[allow(clippy::too_many_arguments)]
async fn judge_accounts(
    client: &RpcClient,
    url: &str,
    count: u64,
    sample: &[AccountSample],
    target: &GpaTarget,
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
    let matcher = methods::GpaMatcher::new(target);
    let mut drift = false;
    for (claimed, entry) in sample.iter().zip(entries) {
        if entry.is_null() || !matcher.matches(entry) {
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
    target: &GpaTarget,
    cache: &mut HashMap<String, (u64, Instant)>,
) -> Option<u64> {
    let params = methods::gpa_count_params(target);
    let key = params.to_string();
    if let Some((count, fetched)) = cache.get(&key) {
        if fetched.elapsed() < GPA_COUNT_TTL {
            return Some(*count);
        }
    }
    let result = client.raw_call(url, "getProgramAccounts", params).await?;
    let count = result.as_array()?.len() as u64;
    cache.insert(key, (count, Instant::now()));
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
    fn catching_up_node_suppresses_implausibility() {
        let fleet = ReferenceSlot::new();
        fleet.observe(2000);
        let sink = ClaimSink::new(16, 64, fleet);
        sink.set_node_tip(1950);
        assert!(!sink.node_stale());
        sink.submit(
            "honest",
            RpcMethod::GetSlot,
            &success_result(None, Some(2000)),
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

    fn obs(
        rows: &[(&str, Option<&str>, Option<&str>)],
    ) -> Vec<(String, Option<String>, Option<String>)> {
        rows.iter()
            .map(|(n, h, s)| ((*n).to_string(), h.map(str::to_owned), s.map(str::to_owned)))
            .collect()
    }

    #[test]
    fn majority_block_picks_agreed_hash_and_a_matching_sig() {
        let observed = obs(&[
            ("triton", Some("AAA"), Some("sigA")),
            ("helius", Some("AAA"), Some("sigA")),
            ("alchemy", Some("BBB"), Some("sigB")),
        ]);
        assert_eq!(
            majority_block(&observed),
            Some(("AAA".to_string(), Some("sigA".to_string())))
        );
    }

    #[test]
    fn majority_block_needs_a_quorum() {
        assert_eq!(majority_block(&obs(&[("triton", Some("AAA"), None)])), None);
        assert_eq!(
            majority_block(&obs(&[
                ("triton", Some("AAA"), None),
                ("helius", Some("BBB"), None),
                ("alchemy", None, None),
            ])),
            None
        );
    }

    #[test]
    fn pick_unused_slot_stays_in_range_and_never_repeats() {
        let mut used = UsedSlots::default();
        let mut seen = Vec::new();
        for _ in 0..50 {
            let s = pick_unused_slot(20_000_000, 20_100_000, &mut used).unwrap();
            assert!((20_000_000..=20_100_000).contains(&s));
            assert!(!seen.contains(&s), "repeated slot {s}");
            seen.push(s);
        }
        let mut small = UsedSlots::default();
        pick_unused_slot(7, 9, &mut small);
        pick_unused_slot(7, 9, &mut small);
        pick_unused_slot(7, 9, &mut small);
        assert!(pick_unused_slot(7, 9, &mut small).is_none());
    }

    #[test]
    fn used_slots_evicts_oldest_past_the_cap() {
        let mut used = UsedSlots::default();
        for s in 0..(ARCHIVAL_USED_CAP as u64 + 10) {
            assert!(used.insert(s));
        }
        assert_eq!(used.set.len(), ARCHIVAL_USED_CAP);
        assert!(!used.set.contains(&0));
        assert!(used.set.contains(&(ARCHIVAL_USED_CAP as u64 + 9)));
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
        let target = methods::builtin_token_owner_target();
        let sample = vec![AccountSample {
            pubkey: "k1".into(),
            data: "d1".into(),
        }];
        assert_eq!(
            judge_accounts(
                &client,
                "http://127.0.0.1:1",
                100,
                &sample,
                &target,
                Some(50),
                8
            )
            .await,
            "mismatch"
        );
        assert_eq!(
            judge_accounts(
                &client,
                "http://127.0.0.1:1",
                100,
                &sample,
                &target,
                Some(104),
                8
            )
            .await,
            "skipped"
        );
    }
}
