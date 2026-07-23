use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::{GpaTarget, MemcmpFilter};
use crate::rpc::{AccountSample, ClaimPayload, RequestContext};

const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const USDT_MINT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";
pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

const MULTI_ACCOUNTS: [&str; 4] = [USDC_MINT, USDT_MINT, WSOL_MINT, TOKEN_PROGRAM];
const FALLBACK_ADDRESS: &str = USDC_MINT;

pub const HIGH_VOLUME_MINTS: [&str; 3] = [USDC_MINT, USDT_MINT, WSOL_MINT];

const CLOCK_SYSVAR: &str = "SysvarC1ock11111111111111111111111111111111";
const VOTE_PROGRAM: &str = "Vote111111111111111111111111111111111111111";
const GPA_TOKEN_OWNER: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
pub const TOKEN_ACCOUNT_LEN: u64 = 165;
pub const TOKEN_ACCOUNT_MINT_OFFSET: u64 = 0;
pub const TOKEN_ACCOUNT_OWNER_OFFSET: u64 = 32;

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
    GetTransactionArchival,
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
            Self::GetTransactionArchival => "getTransaction_archival",
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
            Self::GetTransactionRecent | Self::GetTransactionArchival => "getTransaction",
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
            Self::GetProgramAccounts => {
                let target = ctx.gpa_target.as_ref()?;
                json!([target.program, {
                    "encoding": "base64",
                    "commitment": "processed",
                    "withContext": true,
                    "filters": gpa_filters(target),
                }])
            }
            Self::GetTokenAccountsByOwner => json!([
                GPA_TOKEN_OWNER,
                { "programId": TOKEN_PROGRAM },
                { "encoding": "base64", "commitment": "processed" },
            ]),
            Self::GetSignaturesForAddress => {
                json!([FALLBACK_ADDRESS, { "limit": SIGNATURES_LIMIT, "commitment": "confirmed" }])
            }
            Self::GetBlockRecent | Self::GetBlockArchival => block_params(self.probed_slot(ctx)?),
            Self::GetTransactionRecent => transaction_params(ctx.recent_signature.clone()?),
            Self::GetTransactionArchival => transaction_params(ctx.archival_signature.clone()?),
        };
        Some(params)
    }

    pub fn is_valid_result(self, result: &Value, ctx: &RequestContext) -> bool {
        match self {
            Self::GetHealth => result.as_str() == Some("ok"),
            Self::GetSlot => result.as_u64().is_some_and(|slot| slot > 0),
            Self::GetLatestBlockhash => non_empty_str(value_of(result).get("blockhash")),
            Self::GetAccountInfo => clock_slot_from_value(value_of(result)).is_some(),
            Self::GetMultipleAccounts => value_of(result)
                .as_array()
                .is_some_and(|a| a.iter().any(|entry| !entry.is_null())),
            Self::GetProgramAccounts => match ctx.gpa_target.as_ref() {
                Some(target) => entries_match(result, target),
                None => false,
            },
            Self::GetTokenAccountsByOwner => entries_match(result, &builtin_token_owner_target()),
            Self::GetBlockRecent | Self::GetBlockArchival => {
                non_empty_str(result.get("blockhash"))
                    && result
                        .get("transactions")
                        .and_then(Value::as_array)
                        .is_some_and(|a| !a.is_empty())
            }
            Self::GetTransactionRecent | Self::GetTransactionArchival => {
                !result.is_null()
                    && result.get("slot").is_some()
                    && result.get("transaction").is_some()
            }
            Self::GetSignaturesForAddress => result.as_array().is_some_and(|a| !a.is_empty()),
        }
    }

    pub fn probed_slot(self, ctx: &RequestContext) -> Option<u64> {
        match self {
            Self::GetBlockRecent => Some(ctx.tip_slot?.saturating_sub(BLOCK_CONFIRMATION_DEPTH)),
            Self::GetBlockArchival => ctx.tip_slot?.checked_sub(ARCHIVAL_SLOT_DEPTH),
            _ => None,
        }
    }

    pub fn claim_payload(self, result: &Value, ctx: &RequestContext) -> Option<ClaimPayload> {
        match self {
            Self::GetLatestBlockhash => Some(ClaimPayload::Blockhash {
                slot: self.observed_slot(result)?,
                blockhash: non_empty_string(value_of(result).get("blockhash"))?,
            }),
            Self::GetBlockRecent => Some(ClaimPayload::Blockhash {
                slot: self.probed_slot(ctx)?,
                blockhash: non_empty_string(result.get("blockhash"))?,
            }),
            Self::GetProgramAccounts | Self::GetTokenAccountsByOwner => {
                let target = match self {
                    Self::GetProgramAccounts => ctx.gpa_target.clone()?,
                    _ => builtin_token_owner_target(),
                };
                let entries = value_of(result).as_array()?;
                Some(ClaimPayload::Accounts {
                    slot: self.observed_slot(result),
                    count: entries.len() as u64,
                    sample: sample_accounts(entries),
                    target,
                })
            }
            Self::GetTransactionRecent => Some(ClaimPayload::Transaction {
                slot: result.get("slot")?.as_u64()?,
                signature: ctx.recent_signature.clone()?,
            }),
            Self::GetSignaturesForAddress => {
                let first = result.as_array()?.first()?;
                Some(ClaimPayload::Transaction {
                    slot: first.get("slot")?.as_u64()?,
                    signature: non_empty_string(first.get("signature"))?,
                })
            }
            Self::GetAccountInfo => {
                let b64 = value_of(result)
                    .get("data")?
                    .as_array()?
                    .first()?
                    .as_str()?;
                let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
                Some(ClaimPayload::Clock {
                    slot: u64::from_le_bytes(bytes.get(0..8)?.try_into().ok()?),
                    unix_timestamp: i64::from_le_bytes(bytes.get(32..40)?.try_into().ok()?),
                })
            }
            _ => None,
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
            | Self::GetTransactionArchival
            | Self::GetSignaturesForAddress => None,
        }
    }

    #[inline]
    pub const fn probes_fixed_slot(self) -> bool {
        matches!(self, Self::GetBlockRecent | Self::GetBlockArchival)
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

    pub fn archival_signature(self, result: &Value) -> Option<String> {
        if !matches!(self, Self::GetBlockArchival) {
            return None;
        }
        result
            .get("transactions")?
            .as_array()?
            .iter()
            .filter(|tx| !is_vote_transaction(tx))
            .find_map(|tx| {
                tx.get("transaction")?
                    .get("signatures")?
                    .as_array()?
                    .first()?
                    .as_str()
                    .map(str::to_owned)
            })
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

fn is_vote_transaction(tx: &Value) -> bool {
    tx.get("transaction")
        .and_then(|t| t.get("message"))
        .and_then(|m| m.get("accountKeys"))
        .and_then(Value::as_array)
        .is_some_and(|keys| {
            keys.iter()
                .filter_map(Value::as_str)
                .any(|k| k == VOTE_PROGRAM)
        })
}

fn transaction_params(signature: String) -> Value {
    json!([signature, {
        "encoding": "json",
        "commitment": "confirmed",
        "maxSupportedTransactionVersion": 0,
    }])
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
    let rotating = MULTI_ACCOUNT_BATCH - 1;
    if pool.len() >= rotating {
        let start = (seed.unwrap_or(0) as usize) % pool.len();
        let mut out = vec![FALLBACK_ADDRESS.to_owned()];
        out.extend((0..rotating).map(|k| pool[(start + k) % pool.len()].clone()));
        out
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

pub fn builtin_token_owner_target() -> GpaTarget {
    GpaTarget {
        name: "token_owner".to_owned(),
        program: TOKEN_PROGRAM.to_owned(),
        data_size: Some(TOKEN_ACCOUNT_LEN),
        memcmp: vec![MemcmpFilter {
            offset: TOKEN_ACCOUNT_OWNER_OFFSET,
            bytes: GPA_TOKEN_OWNER.to_owned(),
        }],
    }
}

pub fn gpa_filters(target: &GpaTarget) -> Vec<Value> {
    let mut filters = Vec::new();
    if let Some(size) = target.data_size {
        filters.push(json!({ "dataSize": size }));
    }
    for filter in &target.memcmp {
        filters.push(json!({ "memcmp": { "offset": filter.offset, "bytes": filter.bytes } }));
    }
    filters
}

pub fn gpa_count_params(target: &GpaTarget) -> Value {
    json!([target.program, {
        "encoding": "base64",
        "commitment": "confirmed",
        "dataSlice": { "offset": 0, "length": 0 },
        "filters": gpa_filters(target),
    }])
}

pub struct GpaMatcher {
    program: String,
    data_size: Option<usize>,
    memcmp: Vec<(usize, Vec<u8>)>,
}

impl GpaMatcher {
    pub fn new(target: &GpaTarget) -> Self {
        Self {
            program: target.program.clone(),
            data_size: target.data_size.map(|n| n as usize),
            memcmp: target
                .memcmp
                .iter()
                .map(|f| {
                    (
                        f.offset as usize,
                        bs58::decode(&f.bytes).into_vec().unwrap_or_default(),
                    )
                })
                .collect(),
        }
    }

    pub fn matches(&self, entry: &Value) -> bool {
        let account = entry.get("account").unwrap_or(entry);
        if account.get("owner").and_then(Value::as_str) != Some(self.program.as_str()) {
            return false;
        }
        let Some(b64) = account
            .get("data")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
        else {
            return false;
        };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            return false;
        };
        if self.data_size.is_some_and(|n| bytes.len() != n) {
            return false;
        }
        self.memcmp
            .iter()
            .all(|(off, want)| bytes.get(*off..off + want.len()) == Some(want.as_slice()))
    }
}

fn entries_match(result: &Value, target: &GpaTarget) -> bool {
    let matcher = GpaMatcher::new(target);
    value_of(result)
        .as_array()
        .is_some_and(|a| !a.is_empty() && a.iter().all(|e| matcher.matches(e)))
}

fn value_of(result: &Value) -> &Value {
    result.get("value").unwrap_or(result)
}

fn non_empty_str(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|s| !s.is_empty())
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

const CLAIM_SAMPLE: usize = 3;

fn sample_accounts(entries: &[Value]) -> Vec<AccountSample> {
    if entries.is_empty() {
        return Vec::new();
    }
    let picks = [0, entries.len() / 2, entries.len() - 1];
    let mut out: Vec<AccountSample> = Vec::with_capacity(CLAIM_SAMPLE);
    for &i in picks.iter() {
        let entry = &entries[i];
        let Some(pubkey) = non_empty_string(entry.get("pubkey")) else {
            continue;
        };
        if out.iter().any(|s| s.pubkey == pubkey) {
            continue;
        }
        let account = entry.get("account").unwrap_or(entry);
        let Some(data) = account
            .get("data")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
        else {
            continue;
        };
        out.push(AccountSample {
            pubkey,
            data: data.to_owned(),
        });
    }
    out
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

    fn gpa_ctx() -> RequestContext {
        RequestContext {
            gpa_target: Some(builtin_token_owner_target()),
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
        assert!(RpcMethod::GetAccountInfo.is_valid_result(&result, &RequestContext::default()));
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
        assert!(RpcMethod::GetProgramAccounts
            .build_params(&RequestContext::default())
            .is_none());
        let params = RpcMethod::GetProgramAccounts
            .build_params(&gpa_ctx())
            .unwrap();
        assert_eq!(params[0], json!(TOKEN_PROGRAM));
        assert!(params[1].get("dataSlice").is_none());
        assert_eq!(
            params[1]["filters"][0],
            json!({ "dataSize": TOKEN_ACCOUNT_LEN })
        );
        assert_eq!(
            params[1]["filters"][1],
            json!({ "memcmp": { "offset": TOKEN_ACCOUNT_OWNER_OFFSET, "bytes": GPA_TOKEN_OWNER } })
        );
    }

    #[test]
    fn gpa_params_follow_the_configured_target() {
        let target = GpaTarget {
            name: "dex_pools".into(),
            program: VOTE_PROGRAM.into(),
            data_size: Some(752),
            memcmp: Vec::new(),
        };
        let ctx = RequestContext {
            gpa_target: Some(target.clone()),
            ..RequestContext::default()
        };
        let params = RpcMethod::GetProgramAccounts.build_params(&ctx).unwrap();
        assert_eq!(params[0], json!(VOTE_PROGRAM));
        assert_eq!(params[1]["filters"], json!([{ "dataSize": 752 }]));

        let count_params = gpa_count_params(&target);
        assert_eq!(count_params[0], json!(VOTE_PROGRAM));
        assert_eq!(
            count_params[1]["dataSlice"],
            json!({ "offset": 0, "length": 0 })
        );

        let data = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 752]);
        let entry = json!({ "account": { "owner": VOTE_PROGRAM, "data": [data, "base64"] } });
        let ok = json!({ "value": [entry] });
        assert!(RpcMethod::GetProgramAccounts.is_valid_result(&ok, &ctx));
        assert!(!RpcMethod::GetProgramAccounts.is_valid_result(&ok, &gpa_ctx()));
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
        let batch = params[0].as_array().unwrap();
        assert_eq!(batch.len(), MULTI_ACCOUNT_BATCH);
        assert_eq!(batch[0], json!(FALLBACK_ADDRESS));
        assert!(batch[1..]
            .iter()
            .all(|k| k.as_str().unwrap().starts_with("acct")));
    }

    #[test]
    fn gpa_rejects_garbage_that_doesnt_match_the_filter() {
        let owner = bs58::decode(GPA_TOKEN_OWNER).into_vec().unwrap();
        let off = TOKEN_ACCOUNT_OWNER_OFFSET as usize;
        let mut data = vec![0u8; TOKEN_ACCOUNT_LEN as usize];
        data[off..off + 32].copy_from_slice(&owner);
        let good = base64::engine::general_purpose::STANDARD.encode(&data);
        let ok = json!({ "value": [ { "account": { "owner": TOKEN_PROGRAM, "data": [good, "base64"] } } ] });
        assert!(RpcMethod::GetProgramAccounts.is_valid_result(&ok, &gpa_ctx()));

        let junk =
            base64::engine::general_purpose::STANDARD.encode(vec![7u8; TOKEN_ACCOUNT_LEN as usize]);
        let bad = json!({ "value": [ { "account": { "owner": TOKEN_PROGRAM, "data": [junk, "base64"] } } ] });
        assert!(!RpcMethod::GetProgramAccounts.is_valid_result(&bad, &gpa_ctx()));

        assert!(!RpcMethod::GetProgramAccounts.is_valid_result(&json!({ "value": [] }), &gpa_ctx()));
    }

    #[test]
    fn empty_or_truncated_responses_are_invalid() {
        let ctx = RequestContext::default();
        assert!(!RpcMethod::GetAccountInfo.is_valid_result(&json!({ "value": null }), &ctx));
        assert!(!RpcMethod::GetAccountInfo
            .is_valid_result(&json!({ "value": { "data": ["", "base64"] } }), &ctx));
        assert!(
            !RpcMethod::GetAccountInfo.is_valid_result(&json!({ "context": { "slot": 1 } }), &ctx)
        );
        assert!(!RpcMethod::GetBlockRecent
            .is_valid_result(&json!({ "blockhash": "abc", "transactions": [] }), &ctx));
        assert!(RpcMethod::GetBlockRecent
            .is_valid_result(&json!({ "blockhash": "abc", "transactions": [{}] }), &ctx));
        assert!(!RpcMethod::GetSignaturesForAddress.is_valid_result(&json!([]), &ctx));
        assert!(!RpcMethod::GetProgramAccounts.is_valid_result(&json!({ "value": [] }), &gpa_ctx()));
        assert!(RpcMethod::GetHealth.is_valid_result(&json!("ok"), &ctx));
        assert!(!RpcMethod::GetHealth.is_valid_result(&json!("behind"), &ctx));
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
    fn archival_transaction_needs_a_harvested_archival_signature() {
        assert!(RpcMethod::GetTransactionArchival
            .build_params(&RequestContext::default())
            .is_none());
        let ctx = RequestContext {
            archival_signature: Some("oldsig".to_string()),
            ..RequestContext::default()
        };
        let params = RpcMethod::GetTransactionArchival
            .build_params(&ctx)
            .unwrap();
        assert_eq!(params[0], json!("oldsig"));
        assert_eq!(
            RpcMethod::GetTransactionArchival.rpc_name(),
            "getTransaction"
        );
    }

    #[test]
    fn archival_signature_is_harvested_only_from_archival_blocks() {
        let block = json!({
            "blockhash": "abc",
            "transactions": [
                { "transaction": { "signatures": ["sigA"], "message": {} } },
                { "transaction": { "signatures": ["sigB"], "message": {} } },
            ],
        });
        assert_eq!(
            RpcMethod::GetBlockArchival.archival_signature(&block),
            Some("sigA".to_string())
        );
        assert_eq!(RpcMethod::GetBlockRecent.archival_signature(&block), None);
    }

    #[test]
    fn archival_signature_skips_vote_transactions() {
        let block = json!({
            "blockhash": "abc",
            "transactions": [
                { "transaction": { "signatures": ["voteSig"], "message": {
                    "accountKeys": ["somevoter", VOTE_PROGRAM] } } },
                { "transaction": { "signatures": ["userSig"], "message": {
                    "accountKeys": ["payer", TOKEN_PROGRAM] } } },
            ],
        });
        assert_eq!(
            RpcMethod::GetBlockArchival.archival_signature(&block),
            Some("userSig".to_string())
        );

        let only_votes = json!({
            "blockhash": "abc",
            "transactions": [
                { "transaction": { "signatures": ["voteSig"], "message": {
                    "accountKeys": [VOTE_PROGRAM] } } },
            ],
        });
        assert_eq!(
            RpcMethod::GetBlockArchival.archival_signature(&only_votes),
            None
        );
    }

    #[test]
    fn archival_transaction_rejects_a_response_missing_the_transaction() {
        let ctx = RequestContext::default();
        assert!(!RpcMethod::GetTransactionArchival.is_valid_result(&json!(null), &ctx));
        assert!(!RpcMethod::GetTransactionArchival.is_valid_result(&json!({ "slot": 1 }), &ctx));
        assert!(RpcMethod::GetTransactionArchival
            .is_valid_result(&json!({ "slot": 1, "transaction": {} }), &ctx));
    }

    #[test]
    fn claim_payload_extracts_blockhash_claims() {
        let ctx = ctx_with_tip(1_000);
        let latest = json!({ "context": { "slot": 999 }, "value": { "blockhash": "abc" } });
        assert_eq!(
            RpcMethod::GetLatestBlockhash.claim_payload(&latest, &ctx),
            Some(ClaimPayload::Blockhash {
                slot: 999,
                blockhash: "abc".into()
            })
        );
        let block = json!({ "blockhash": "xyz", "transactions": [{}] });
        assert_eq!(
            RpcMethod::GetBlockRecent.claim_payload(&block, &ctx),
            Some(ClaimPayload::Blockhash {
                slot: 1_000 - BLOCK_CONFIRMATION_DEPTH,
                blockhash: "xyz".into()
            })
        );
    }

    #[test]
    fn claim_payload_samples_accounts_with_count() {
        let entries: Vec<Value> = (0..10)
            .map(|n| {
                json!({ "pubkey": format!("k{n}"), "account": { "data": [format!("d{n}"), "base64"] } })
            })
            .collect();
        let result = json!({ "context": { "slot": 500 }, "value": entries });
        let Some(ClaimPayload::Accounts {
            slot,
            count,
            sample,
            target,
        }) = RpcMethod::GetProgramAccounts.claim_payload(&result, &gpa_ctx())
        else {
            panic!("expected accounts payload");
        };
        assert_eq!(target.name, "token_owner");
        assert_eq!(slot, Some(500));
        assert_eq!(count, 10);
        let keys: Vec<&str> = sample.iter().map(|s| s.pubkey.as_str()).collect();
        assert_eq!(keys, vec!["k0", "k5", "k9"]);
        assert_eq!(sample[0].data, "d0");
    }

    #[test]
    fn claim_payload_extracts_transaction_claims() {
        let ctx = RequestContext {
            recent_signature: Some("sig".into()),
            ..RequestContext::default()
        };
        let tx = json!({ "slot": 777, "transaction": {} });
        assert_eq!(
            RpcMethod::GetTransactionRecent.claim_payload(&tx, &ctx),
            Some(ClaimPayload::Transaction {
                slot: 777,
                signature: "sig".into()
            })
        );
        let sigs =
            json!([{ "signature": "first", "slot": 888 }, { "signature": "second", "slot": 800 }]);
        assert_eq!(
            RpcMethod::GetSignaturesForAddress.claim_payload(&sigs, &ctx),
            Some(ClaimPayload::Transaction {
                slot: 888,
                signature: "first".into()
            })
        );
    }

    #[test]
    fn claim_payload_parses_the_clock_sysvar() {
        let slot: u64 = 123;
        let ts: i64 = 1_700_000_000;
        let mut data = slot.to_le_bytes().to_vec();
        data.extend_from_slice(&[0u8; 24]);
        data.extend_from_slice(&ts.to_le_bytes());
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
        let result = json!({ "value": { "data": [b64, "base64"] } });
        assert_eq!(
            RpcMethod::GetAccountInfo.claim_payload(&result, &RequestContext::default()),
            Some(ClaimPayload::Clock {
                slot: 123,
                unix_timestamp: ts
            })
        );
    }

    #[test]
    fn methods_without_verifiable_content_yield_no_payload() {
        let ctx = RequestContext::default();
        assert_eq!(RpcMethod::GetSlot.claim_payload(&json!(42), &ctx), None);
        assert_eq!(RpcMethod::GetHealth.claim_payload(&json!("ok"), &ctx), None);
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
