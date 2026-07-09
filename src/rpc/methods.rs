use serde::Deserialize;
use serde_json::{json, Value};

use crate::rpc::RequestContext;

// Real, permanent mainnet accounts used as fallbacks and for multi-account reads.
// These exist indefinitely, so a `null`/empty response for them is a real failure
// (not a legitimately-absent account) and is scored as such.
const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const USDT_MINT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";
const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

// Fallback single-account target when no live account has been observed yet.
const FALLBACK_ACCOUNT: &str = USDC_MINT;
// Fallback multi-account batch (real accounts, all present).
const MULTI_ACCOUNTS: [&str; 4] = [USDC_MINT, USDT_MINT, WSOL_MINT, TOKEN_PROGRAM];
// Fallback busy address for signature listing.
const FALLBACK_ADDRESS: &str = USDC_MINT;

// A token owner with a large, stable set of token accounts (~thousands), so
// getProgramAccounts and getTokenAccountsByOwner do real, non-trivial work.
const GPA_TOKEN_OWNER: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
const TOKEN_ACCOUNT_LEN: u64 = 165;
const TOKEN_ACCOUNT_OWNER_OFFSET: u64 = 32;

const BLOCK_CONFIRMATION_DEPTH: u64 = 32;
// Slots behind the tip for the archival probe (~months of history at ~2.5 slots/s),
// far past what a non-archival node retains — so this measures archival/cold-storage
// retrieval. Non-archival providers will (correctly) error here.
const ARCHIVAL_SLOT_DEPTH: u64 = 40_000_000;

const SIGNATURES_LIMIT: u64 = 1000;
const MULTI_ACCOUNT_BATCH: usize = 5;
// Cap on how many account keys to retain from a fetched block.
pub const MAX_RECENT_ACCOUNTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcMethod {
    GetHealth,
    GetSlot,
    GetLatestBlockhash,
    GetAccountInfo,
    GetMultipleAccounts,
    GetProgramAccounts,
    GetTokenAccountsByOwner,
    GetBlockRecent,
    GetBlockArchival,
    GetTransactionRecent,
    GetSignaturesForAddress,
}

/// Rotate a target out of a live pool using the tip slot as the seed, so the
/// target changes every cycle (defeats caching / hard-coded-target gaming) while
/// staying identical across providers probed at the same tip.
fn rotate(pool: &[String], seed: Option<u64>) -> Option<&str> {
    if pool.is_empty() {
        return None;
    }
    let idx = (seed.unwrap_or(0) as usize) % pool.len();
    Some(pool[idx].as_str())
}

impl RpcMethod {
    #[inline]
    pub const fn label(self) -> &'static str {
        match self {
            Self::GetHealth => "getHealth",
            Self::GetSlot => "getSlot",
            Self::GetLatestBlockhash => "getLatestBlockhash",
            Self::GetAccountInfo => "getAccountInfo",
            Self::GetMultipleAccounts => "getMultipleAccounts",
            Self::GetProgramAccounts => "getProgramAccounts",
            Self::GetTokenAccountsByOwner => "getTokenAccountsByOwner",
            Self::GetBlockRecent => "getBlock_recent",
            Self::GetBlockArchival => "getBlock_archival",
            Self::GetTransactionRecent => "getTransaction_recent",
            Self::GetSignaturesForAddress => "getSignaturesForAddress",
        }
    }

    #[inline]
    pub const fn rpc_name(self) -> &'static str {
        match self {
            Self::GetHealth => "getHealth",
            Self::GetSlot => "getSlot",
            Self::GetLatestBlockhash => "getLatestBlockhash",
            Self::GetAccountInfo => "getAccountInfo",
            Self::GetMultipleAccounts => "getMultipleAccounts",
            Self::GetProgramAccounts => "getProgramAccounts",
            Self::GetTokenAccountsByOwner => "getTokenAccountsByOwner",
            Self::GetBlockRecent | Self::GetBlockArchival => "getBlock",
            Self::GetTransactionRecent => "getTransaction",
            Self::GetSignaturesForAddress => "getSignaturesForAddress",
        }
    }

    pub fn build_params(self, ctx: &RequestContext) -> Option<Value> {
        let params = match self {
            Self::GetHealth => json!([]),
            Self::GetSlot => json!([{ "commitment": "processed" }]),
            Self::GetLatestBlockhash => json!([{ "commitment": "processed" }]),
            Self::GetAccountInfo => {
                // Rotate over accounts seen in recent blocks (real, varying, hard to
                // pre-cache); fall back to a known-present account before any block lands.
                let account =
                    rotate(&ctx.recent_accounts, ctx.tip_slot).unwrap_or(FALLBACK_ACCOUNT);
                json!([account, { "encoding": "base64", "commitment": "processed" }])
            }
            Self::GetMultipleAccounts => {
                let accounts = pick_batch(&ctx.recent_accounts, ctx.tip_slot);
                json!([accounts, { "encoding": "base64", "commitment": "processed" }])
            }
            Self::GetProgramAccounts => json!([TOKEN_PROGRAM, {
                "encoding": "base64",
                "commitment": "processed",
                "withContext": true,
                "filters": [
                    { "dataSize": TOKEN_ACCOUNT_LEN },
                    { "memcmp": { "offset": TOKEN_ACCOUNT_OWNER_OFFSET, "bytes": GPA_TOKEN_OWNER } },
                ],
            }]),
            Self::GetTokenAccountsByOwner => json!([
                GPA_TOKEN_OWNER,
                { "programId": TOKEN_PROGRAM },
                { "encoding": "base64", "commitment": "processed" },
            ]),
            Self::GetSignaturesForAddress => {
                let address =
                    rotate(&ctx.recent_accounts, ctx.tip_slot).unwrap_or(FALLBACK_ADDRESS);
                json!([address, { "limit": SIGNATURES_LIMIT }])
            }
            Self::GetBlockRecent => {
                let slot = ctx.tip_slot?.saturating_sub(BLOCK_CONFIRMATION_DEPTH);
                block_params(slot)
            }
            Self::GetBlockArchival => {
                let slot = ctx.tip_slot?.checked_sub(ARCHIVAL_SLOT_DEPTH)?;
                block_params(slot)
            }
            Self::GetTransactionRecent => {
                let signature = ctx.recent_signature.clone()?;
                json!([signature, {
                    "encoding": "json",
                    "commitment": "confirmed",
                    "maxSupportedTransactionVersion": 0,
                }])
            }
        };
        Some(params)
    }

    /// Whether a 200/`result` response is actually complete, not empty or truncated.
    /// A fast-but-empty answer (null account, zero transactions, empty page) must not
    /// score as a success.
    pub fn is_valid_result(self, result: &Value) -> bool {
        match self {
            Self::GetHealth => result.as_str() == Some("ok"),
            Self::GetSlot => result.as_u64().is_some_and(|slot| slot > 0),
            Self::GetLatestBlockhash => non_empty_str(value_of(result).get("blockhash")),
            Self::GetAccountInfo => result.get("value").is_some_and(|v| !v.is_null()),
            Self::GetMultipleAccounts => value_of(result)
                .as_array()
                .is_some_and(|a| a.iter().any(|entry| !entry.is_null())),
            Self::GetProgramAccounts | Self::GetTokenAccountsByOwner => {
                value_of(result).as_array().is_some_and(|a| !a.is_empty())
            }
            Self::GetBlockRecent | Self::GetBlockArchival => {
                non_empty_str(result.get("blockhash"))
                    && result
                        .get("transactions")
                        .and_then(Value::as_array)
                        .is_some_and(|a| !a.is_empty())
            }
            Self::GetTransactionRecent => !result.is_null() && result.get("slot").is_some(),
            Self::GetSignaturesForAddress => result.as_array().is_some_and(|a| !a.is_empty()),
        }
    }

    pub fn observed_slot(self, result: &Value) -> Option<u64> {
        match self {
            Self::GetSlot => result.as_u64(),
            Self::GetLatestBlockhash
            | Self::GetAccountInfo
            | Self::GetMultipleAccounts
            | Self::GetProgramAccounts
            | Self::GetTokenAccountsByOwner => result.get("context")?.get("slot")?.as_u64(),
            Self::GetHealth
            | Self::GetBlockRecent
            | Self::GetBlockArchival
            | Self::GetTransactionRecent
            | Self::GetSignaturesForAddress => None,
        }
    }

    pub fn recent_signature(self, result: &Value) -> Option<String> {
        if !matches!(self, Self::GetSignaturesForAddress) {
            return None;
        }
        result
            .as_array()?
            .first()?
            .get("signature")?
            .as_str()
            .map(str::to_owned)
    }

    /// Account keys observed in a freshly fetched block, used as live probe targets
    /// for account reads. Only the recent-block fetch yields these.
    pub fn recent_accounts(self, result: &Value) -> Vec<String> {
        if !matches!(self, Self::GetBlockRecent) {
            return Vec::new();
        }
        let Some(txs) = result.get("transactions").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut out: Vec<String> = Vec::new();
        for tx in txs {
            let keys = tx
                .get("transaction")
                .and_then(|t| t.get("message"))
                .and_then(|m| m.get("accountKeys"))
                .and_then(Value::as_array);
            if let Some(keys) = keys {
                for key in keys.iter().filter_map(Value::as_str) {
                    let key = key.to_owned();
                    if !out.contains(&key) {
                        out.push(key);
                        if out.len() >= MAX_RECENT_ACCOUNTS {
                            return out;
                        }
                    }
                }
            }
        }
        out
    }
}

fn block_params(slot: u64) -> Value {
    json!([slot, {
        "encoding": "json",
        "transactionDetails": "full",
        "rewards": false,
        "commitment": "confirmed",
        "maxSupportedTransactionVersion": 0,
    }])
}

/// A rotating window of `MULTI_ACCOUNT_BATCH` accounts from the live pool, or the
/// static fallback set before any block has been observed.
fn pick_batch(pool: &[String], seed: Option<u64>) -> Vec<String> {
    if pool.len() >= MULTI_ACCOUNT_BATCH {
        let start = (seed.unwrap_or(0) as usize) % pool.len();
        (0..MULTI_ACCOUNT_BATCH)
            .map(|k| pool[(start + k) % pool.len()].clone())
            .collect()
    } else {
        MULTI_ACCOUNTS.iter().map(|s| (*s).to_owned()).collect()
    }
}

/// `result.value` for methods that wrap their payload in a context envelope,
/// falling back to the result itself.
fn value_of(result: &Value) -> &Value {
    result.get("value").unwrap_or(result)
}

fn non_empty_str(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with_tip(tip: u64) -> RequestContext {
        RequestContext {
            tip_slot: Some(tip),
            ..RequestContext::default()
        }
    }

    #[test]
    fn get_slot_builds_processed_commitment_params() {
        let params = RpcMethod::GetSlot
            .build_params(&RequestContext::default())
            .expect("get_slot always builds");
        assert_eq!(params, json!([{ "commitment": "processed" }]));
    }

    #[test]
    fn account_read_falls_back_then_rotates_over_live_accounts() {
        // No observed accounts yet -> fallback account, still runs.
        let params = RpcMethod::GetAccountInfo
            .build_params(&RequestContext::default())
            .expect("account read always builds");
        assert_eq!(params[0], json!(FALLBACK_ACCOUNT));

        // With a live pool, the tip slot selects (and rotates) the target.
        let ctx = RequestContext {
            tip_slot: Some(3),
            recent_accounts: vec!["A".into(), "B".into(), "C".into()],
            ..RequestContext::default()
        };
        let params = RpcMethod::GetAccountInfo.build_params(&ctx).unwrap();
        assert_eq!(params[0], json!("A")); // 3 % 3 == 0
    }

    #[test]
    fn block_recent_uses_full_transaction_details_and_needs_tip() {
        assert!(RpcMethod::GetBlockRecent
            .build_params(&RequestContext::default())
            .is_none());
        let params = RpcMethod::GetBlockRecent
            .build_params(&ctx_with_tip(1_000))
            .unwrap();
        assert_eq!(params[0], json!(1_000 - BLOCK_CONFIRMATION_DEPTH));
        assert_eq!(params[1]["transactionDetails"], json!("full"));
    }

    #[test]
    fn archival_block_reaches_deep_into_history() {
        // Not enough history yet -> skipped.
        assert!(RpcMethod::GetBlockArchival
            .build_params(&ctx_with_tip(10))
            .is_none());
        let tip = ARCHIVAL_SLOT_DEPTH + 500;
        let params = RpcMethod::GetBlockArchival
            .build_params(&ctx_with_tip(tip))
            .unwrap();
        assert_eq!(params[0], json!(500));
    }

    #[test]
    fn gpa_returns_account_data_and_filters_by_owner() {
        let params = RpcMethod::GetProgramAccounts
            .build_params(&RequestContext::default())
            .unwrap();
        assert_eq!(params[0], json!(TOKEN_PROGRAM));
        // No dataSlice: the scan returns real account data, not zero bytes.
        assert!(params[1].get("dataSlice").is_none());
        assert_eq!(
            params[1]["filters"][0],
            json!({ "dataSize": TOKEN_ACCOUNT_LEN })
        );
    }

    #[test]
    fn signatures_query_pulls_a_full_page() {
        let params = RpcMethod::GetSignaturesForAddress
            .build_params(&RequestContext::default())
            .unwrap();
        assert_eq!(params[1]["limit"], json!(SIGNATURES_LIMIT));
    }

    #[test]
    fn multiple_accounts_uses_static_fallback_then_live_batch() {
        let params = RpcMethod::GetMultipleAccounts
            .build_params(&RequestContext::default())
            .unwrap();
        assert_eq!(params[0].as_array().unwrap().len(), MULTI_ACCOUNTS.len());

        let pool: Vec<String> = (0..10).map(|n| format!("acct{n}")).collect();
        let ctx = RequestContext {
            tip_slot: Some(0),
            recent_accounts: pool,
            ..RequestContext::default()
        };
        let params = RpcMethod::GetMultipleAccounts.build_params(&ctx).unwrap();
        assert_eq!(params[0].as_array().unwrap().len(), MULTI_ACCOUNT_BATCH);
    }

    #[test]
    fn empty_or_truncated_responses_are_invalid() {
        // Null account = failure.
        assert!(!RpcMethod::GetAccountInfo.is_valid_result(&json!({ "value": null })));
        assert!(RpcMethod::GetAccountInfo
            .is_valid_result(&json!({ "value": { "data": ["", "base64"] } })));
        // Block with no transactions = truncated.
        assert!(!RpcMethod::GetBlockRecent
            .is_valid_result(&json!({ "blockhash": "abc", "transactions": [] })));
        assert!(RpcMethod::GetBlockRecent
            .is_valid_result(&json!({ "blockhash": "abc", "transactions": [{}] })));
        // Empty signature page = failure for a busy address.
        assert!(!RpcMethod::GetSignaturesForAddress.is_valid_result(&json!([])));
        // Empty gPA result = failure (owner has many accounts).
        assert!(!RpcMethod::GetProgramAccounts.is_valid_result(&json!({ "value": [] })));
        assert!(RpcMethod::GetHealth.is_valid_result(&json!("ok")));
        assert!(!RpcMethod::GetHealth.is_valid_result(&json!("behind")));
    }

    #[test]
    fn recent_accounts_extracted_from_block_transactions() {
        let block = json!({
            "blockhash": "abc",
            "transactions": [
                { "transaction": { "message": { "accountKeys": ["k1", "k2"] } } },
                { "transaction": { "message": { "accountKeys": ["k2", "k3"] } } },
            ],
        });
        let accounts = RpcMethod::GetBlockRecent.recent_accounts(&block);
        assert_eq!(accounts, vec!["k1", "k2", "k3"]);
        // Only the recent-block fetch yields targets.
        assert!(RpcMethod::GetSlot.recent_accounts(&block).is_empty());
    }

    #[test]
    fn observed_slot_reads_context_for_envelope_methods() {
        assert_eq!(RpcMethod::GetSlot.observed_slot(&json!(42)), Some(42));
        assert_eq!(
            RpcMethod::GetMultipleAccounts
                .observed_slot(&json!({ "context": { "slot": 7 }, "value": [] })),
            Some(7)
        );
        assert_eq!(RpcMethod::GetHealth.observed_slot(&json!("ok")), None);
    }

    #[test]
    fn recent_signature_is_extracted_only_for_signature_queries() {
        let result = json!([{ "signature": "abc" }, { "signature": "def" }]);
        assert_eq!(
            RpcMethod::GetSignaturesForAddress.recent_signature(&result),
            Some("abc".to_string())
        );
        assert_eq!(RpcMethod::GetSlot.recent_signature(&result), None);
    }
}
