use serde::Deserialize;
use serde_json::{json, Value};

use crate::rpc::RequestContext;

const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
const HIGH_TRAFFIC_ADDRESS: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const BLOCK_CONFIRMATION_DEPTH: u64 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcMethod {
    GetHealth,
    GetSlot,
    GetLatestBlockhash,
    GetAccountInfo,
    GetBlockRecent,
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
            Self::GetBlockRecent => "getBlock_recent",
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
            Self::GetBlockRecent => "getBlock",
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
                json!([SYSTEM_PROGRAM, { "encoding": "base64", "commitment": "processed" }])
            }
            Self::GetSignaturesForAddress => json!([HIGH_TRAFFIC_ADDRESS, { "limit": 10 }]),
            Self::GetBlockRecent => {
                let slot = ctx.tip_slot?.saturating_sub(BLOCK_CONFIRMATION_DEPTH);
                json!([slot, {
                    "encoding": "json",
                    "transactionDetails": "none",
                    "rewards": false,
                    "commitment": "confirmed",
                    "maxSupportedTransactionVersion": 0,
                }])
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

    pub fn observed_slot(self, result: &Value) -> Option<u64> {
        match self {
            Self::GetSlot => result.as_u64(),
            Self::GetLatestBlockhash | Self::GetAccountInfo => {
                result.get("context")?.get("slot")?.as_u64()
            }
            Self::GetHealth
            | Self::GetBlockRecent
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_slot_builds_processed_commitment_params() {
        let params = RpcMethod::GetSlot
            .build_params(&RequestContext::default())
            .expect("get_slot always builds");
        assert_eq!(params, json!([{ "commitment": "processed" }]));
    }

    #[test]
    fn get_block_recent_needs_a_tip_slot() {
        assert!(RpcMethod::GetBlockRecent
            .build_params(&RequestContext::default())
            .is_none());

        let ctx = RequestContext {
            tip_slot: Some(1_000),
            recent_signature: None,
        };
        let params = RpcMethod::GetBlockRecent.build_params(&ctx).unwrap();
        assert_eq!(params[0], json!(1_000 - BLOCK_CONFIRMATION_DEPTH));
    }

    #[test]
    fn get_transaction_recent_needs_a_signature() {
        assert!(RpcMethod::GetTransactionRecent
            .build_params(&RequestContext::default())
            .is_none());

        let ctx = RequestContext {
            tip_slot: None,
            recent_signature: Some("sig123".to_string()),
        };
        let params = RpcMethod::GetTransactionRecent.build_params(&ctx).unwrap();
        assert_eq!(params[0], json!("sig123"));
    }

    #[test]
    fn observed_slot_reads_the_right_field_per_method() {
        assert_eq!(RpcMethod::GetSlot.observed_slot(&json!(42)), Some(42));
        assert_eq!(
            RpcMethod::GetAccountInfo.observed_slot(&json!({ "context": { "slot": 7 } })),
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
