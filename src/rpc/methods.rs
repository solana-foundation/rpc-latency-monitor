use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::rpc::RequestContext;

const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const USDT_MINT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";
const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

const MULTI_ACCOUNTS: [&str; 4] = [USDC_MINT, USDT_MINT, WSOL_MINT, TOKEN_PROGRAM];
const FALLBACK_ADDRESS: &str = USDC_MINT;

const CLOCK_SYSVAR: &str = "SysvarC1ock11111111111111111111111111111111";
const GPA_TOKEN_OWNER: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
const TOKEN_ACCOUNT_LEN: u64 = 165;
const TOKEN_ACCOUNT_OWNER_OFFSET: u64 = 32;

const BLOCK_CONFIRMATION_DEPTH: u64 = 32;
const ARCHIVAL_SLOT_DEPTH: u64 = 40_000_000;

const SIGNATURES_LIMIT: u64 = 1000;
const MULTI_ACCOUNT_BATCH: usize = MULTI_ACCOUNTS.len();
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
                json!([CLOCK_SYSVAR, { "encoding": "base64", "commitment": "processed" }])
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
                json!([FALLBACK_ADDRESS, { "limit": SIGNATURES_LIMIT, "commitment": "confirmed" }])
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

    pub fn is_valid_result(self, result: &Value) -> bool {
        match self {
            Self::GetHealth => result.as_str() == Some("ok"),
            Self::GetSlot => result.as_u64().is_some_and(|slot| slot > 0),
            Self::GetLatestBlockhash => non_empty_str(value_of(result).get("blockhash")),
            Self::GetAccountInfo => clock_slot_from_value(value_of(result)).is_some(),
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
            Self::GetAccountInfo => clock_slot_from_value(value_of(result)),
            Self::GetLatestBlockhash
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

fn clock_slot_from_value(value: &Value) -> Option<u64> {
    let b64 = value.get("data")?.as_array()?.first()?.as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let slot: [u8; 8] = bytes.get(0..8)?.try_into().ok()?;
    Some(u64::from_le_bytes(slot))
}

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
    fn account_read_probes_the_clock_sysvar() {
        let params = RpcMethod::GetAccountInfo
            .build_params(&RequestContext::default())
            .expect("account read always builds");
        assert_eq!(params[0], json!(CLOCK_SYSVAR));
        assert_eq!(params[1]["encoding"], json!("base64"));
    }

    #[test]
    fn account_info_uses_the_data_slot_not_the_envelope_slot() {
        let slot: u64 = 123_456;
        let mut data = slot.to_le_bytes().to_vec();
        data.extend_from_slice(&[0u8; 32]);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
        let result = json!({ "context": { "slot": 999 }, "value": { "data": [b64, "base64"] } });
        assert!(RpcMethod::GetAccountInfo.is_valid_result(&result));
        assert_eq!(RpcMethod::GetAccountInfo.observed_slot(&result), Some(slot));
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
        assert!(!RpcMethod::GetAccountInfo.is_valid_result(&json!({ "value": null })));
        assert!(!RpcMethod::GetAccountInfo
            .is_valid_result(&json!({ "value": { "data": ["", "base64"] } })));
        assert!(!RpcMethod::GetAccountInfo.is_valid_result(&json!({ "context": { "slot": 1 } })));
        assert!(!RpcMethod::GetBlockRecent
            .is_valid_result(&json!({ "blockhash": "abc", "transactions": [] })));
        assert!(RpcMethod::GetBlockRecent
            .is_valid_result(&json!({ "blockhash": "abc", "transactions": [{}] })));
        assert!(!RpcMethod::GetSignaturesForAddress.is_valid_result(&json!([])));
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
