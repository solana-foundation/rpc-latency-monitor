pub mod methods;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderValue, CACHE_CONTROL, PRAGMA};
use reqwest::StatusCode;
use serde_json::Value;

use crate::config::GpaTarget;
use crate::rpc::methods::RpcMethod;

#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub tip_slot: Option<u64>,
    pub recent_signature: Option<String>,
    pub archival_signature: Option<String>,
    pub recent_accounts: Vec<String>,
    pub gpa_target: Option<GpaTarget>,
    pub gsfa_anchor: Option<(u64, String)>,
}

#[derive(Debug, Clone)]
pub struct CallResult {
    pub latency: Duration,
    pub status: CallStatus,
    pub observed_slot: Option<u64>,
    pub signature: Option<String>,
    pub archival_signature: Option<String>,
    pub accounts: Vec<String>,
    pub claim: Option<ClaimPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimPayload {
    Blockhash {
        slot: u64,
        blockhash: String,
    },
    Accounts {
        slot: Option<u64>,
        count: u64,
        sample: Vec<AccountSample>,
        target: GpaTarget,
    },
    Transaction {
        slot: u64,
        signature: String,
    },
    Clock {
        slot: u64,
        unix_timestamp: i64,
    },
}

impl ClaimPayload {
    pub fn slot(&self) -> Option<u64> {
        match self {
            Self::Blockhash { slot, .. }
            | Self::Transaction { slot, .. }
            | Self::Clock { slot, .. } => Some(*slot),
            Self::Accounts { slot, .. } => *slot,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSample {
    pub pubkey: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawResponse {
    Result(Value),
    RpcError(i64),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    Success,
    Skipped,
    Error(ErrorKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Timeout,
    Transport,
    HttpStatus(u16),
    RpcError(i64),
    Decode,
    Empty,
    Stale,
}

impl CallStatus {
    #[inline]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    const fn is_client_side(self) -> bool {
        matches!(self, Self::Error(ErrorKind::HttpStatus(400..=499)))
    }

    #[inline]
    pub const fn label(self) -> &'static str {
        if self.is_client_side() {
            return "skipped";
        }
        match self {
            Self::Success => "success",
            Self::Skipped => "skipped",
            Self::Error(_) => "error",
        }
    }

    #[inline]
    pub const fn is_error(self) -> bool {
        matches!(self, Self::Error(_)) && !self.is_client_side()
    }

    #[inline]
    pub const fn error_kind(self) -> Option<&'static str> {
        match self {
            Self::Success | Self::Skipped => None,
            Self::Error(kind) => Some(kind.as_str()),
        }
    }
}

impl ErrorKind {
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::HttpStatus(code) => match code {
                429 => "http_429",
                400..=499 => "http_4xx",
                500..=599 => "http_5xx",
                _ => "http_status",
            },
            Self::RpcError(code) => match code {
                -32004 => "rpc_block_unavailable",
                -32005 => "rpc_node_unhealthy",
                -32007 | -32009 => "rpc_slot_skipped",
                -32011 => "rpc_tx_history_unavailable",
                -32601 => "rpc_method_not_found",
                -32602 => "rpc_invalid_params",
                _ => "rpc_error",
            },
            Self::Decode => "decode",
            Self::Empty => "empty",
            Self::Stale => "stale",
        }
    }
}

pub struct RpcClient {
    http: reqwest::Client,
    next_id: AtomicU64,
}

impl RpcClient {
    pub fn new(request_timeout: Duration) -> reqwest::Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
        let http = reqwest::Client::builder()
            .timeout(request_timeout)
            .default_headers(headers)
            .build()?;
        Ok(Self {
            http,
            next_id: AtomicU64::new(1),
        })
    }

    pub async fn raw_call(&self, url: &str, method: &str, params: Value) -> Option<Value> {
        match self.raw_call_checked(url, method, params).await {
            RawResponse::Result(value) => Some(value),
            _ => None,
        }
    }

    pub async fn raw_call_checked(&self, url: &str, method: &str, params: Value) -> RawResponse {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id.fetch_add(1, Ordering::Relaxed),
            "method": method,
            "params": params,
        });
        let Ok(response) = self.http.post(url).json(&body).send().await else {
            return RawResponse::Unavailable;
        };
        if !response.status().is_success() {
            return RawResponse::Unavailable;
        }
        let Ok(json) = response.json::<Value>().await else {
            return RawResponse::Unavailable;
        };
        if let Some(error) = json.get("error") {
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
            return RawResponse::RpcError(code);
        }
        match json.get("result") {
            Some(value) => RawResponse::Result(value.clone()),
            None => RawResponse::Unavailable,
        }
    }

    pub async fn call(
        &self,
        url: &str,
        method: RpcMethod,
        ctx: &RequestContext,
        timeout: Option<Duration>,
    ) -> Option<CallResult> {
        let params = method.build_params(ctx)?;
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id.fetch_add(1, Ordering::Relaxed),
            "method": method.rpc_name(),
            "params": params,
        });

        let mut request = self.http.post(url).json(&body);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let start = Instant::now();
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return Some(CallResult {
                    latency: start.elapsed(),
                    status: CallStatus::Error(send_error_kind(&error)),
                    observed_slot: None,
                    signature: None,
                    archival_signature: None,
                    accounts: Vec::new(),
                    claim: None,
                });
            }
        };

        let http_status = response.status();
        let body = response.bytes().await;
        let latency = start.elapsed();

        let parsed = match body {
            Ok(bytes) => classify(http_status, &bytes, method, ctx),
            Err(_) => Parsed::error(ErrorKind::Transport),
        };
        Some(CallResult {
            latency,
            status: parsed.status,
            observed_slot: parsed.observed_slot,
            signature: parsed.signature,
            archival_signature: parsed.archival_signature,
            accounts: parsed.accounts,
            claim: parsed.claim,
        })
    }
}

struct Parsed {
    status: CallStatus,
    observed_slot: Option<u64>,
    signature: Option<String>,
    archival_signature: Option<String>,
    accounts: Vec<String>,
    claim: Option<ClaimPayload>,
}

impl Parsed {
    fn error(kind: ErrorKind) -> Self {
        Self {
            status: CallStatus::Error(kind),
            observed_slot: None,
            signature: None,
            archival_signature: None,
            accounts: Vec::new(),
            claim: None,
        }
    }
}

const SLOT_SKIPPED: i64 = -32007;
const SLOT_SKIPPED_LONG_TERM: i64 = -32009;

fn send_error_kind(error: &reqwest::Error) -> ErrorKind {
    if error.is_timeout() {
        ErrorKind::Timeout
    } else {
        ErrorKind::Transport
    }
}

fn classify(status: StatusCode, body: &[u8], method: RpcMethod, ctx: &RequestContext) -> Parsed {
    if !status.is_success() {
        return Parsed::error(ErrorKind::HttpStatus(status.as_u16()));
    }
    let Ok(json) = serde_json::from_slice::<Value>(body) else {
        return Parsed::error(ErrorKind::Decode);
    };
    if let Some(error) = json.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        if matches!(code, SLOT_SKIPPED | SLOT_SKIPPED_LONG_TERM) && method.probes_fixed_slot() {
            return Parsed {
                status: CallStatus::Skipped,
                observed_slot: None,
                signature: None,
                archival_signature: None,
                accounts: Vec::new(),
                claim: None,
            };
        }
        return Parsed::error(ErrorKind::RpcError(code));
    }
    let Some(result) = json.get("result") else {
        return Parsed::error(ErrorKind::Decode);
    };
    if !method.is_valid_result(result, ctx) {
        return Parsed::error(ErrorKind::Empty);
    }
    let anchored_gsfa =
        matches!(method, RpcMethod::GetSignaturesForAddress) && ctx.gsfa_anchor.is_some();
    Parsed {
        status: CallStatus::Success,
        observed_slot: (!anchored_gsfa)
            .then(|| method.observed_slot(result))
            .flatten(),
        signature: (!anchored_gsfa)
            .then(|| method.recent_signature(result))
            .flatten(),
        archival_signature: method.archival_signature(result),
        accounts: method.recent_accounts(result),
        claim: (!anchored_gsfa)
            .then(|| method.claim_payload(result, ctx))
            .flatten(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    #[test]
    fn success_extracts_slot_for_get_slot() {
        let parsed = classify(
            StatusCode::OK,
            &body(r#"{"jsonrpc":"2.0","result":12345,"id":1}"#),
            RpcMethod::GetSlot,
            &RequestContext::default(),
        );
        assert!(parsed.status.is_success());
        assert_eq!(parsed.observed_slot, Some(12345));
    }

    #[test]
    fn success_extracts_context_slot_for_latest_blockhash() {
        let parsed = classify(
            StatusCode::OK,
            &body(
                r#"{"jsonrpc":"2.0","result":{"context":{"slot":999},"value":{"blockhash":"abc"}},"id":1}"#,
            ),
            RpcMethod::GetLatestBlockhash,
            &RequestContext::default(),
        );
        assert!(parsed.status.is_success());
        assert_eq!(parsed.observed_slot, Some(999));
    }

    #[test]
    fn empty_result_on_200_is_an_empty_error_not_success() {
        let parsed = classify(
            StatusCode::OK,
            &body(r#"{"jsonrpc":"2.0","result":{"context":{"slot":1},"value":[]},"id":1}"#),
            RpcMethod::GetProgramAccounts,
            &RequestContext::default(),
        );
        assert_eq!(parsed.status, CallStatus::Error(ErrorKind::Empty));
        assert_eq!(parsed.observed_slot, None);
    }

    #[test]
    fn success_extracts_signature_for_signature_query() {
        let parsed = classify(
            StatusCode::OK,
            &body(r#"{"jsonrpc":"2.0","result":[{"signature":"abc","slot":5}],"id":1}"#),
            RpcMethod::GetSignaturesForAddress,
            &RequestContext::default(),
        );
        assert_eq!(parsed.signature, Some("abc".to_string()));
    }

    #[test]
    fn anchored_signature_query_yields_no_claim_slot_or_harvest() {
        let ctx = RequestContext {
            gsfa_anchor: Some((100, "anchor".to_string())),
            ..RequestContext::default()
        };
        let parsed = classify(
            StatusCode::OK,
            &body(r#"{"jsonrpc":"2.0","result":[{"signature":"old","slot":50}],"id":1}"#),
            RpcMethod::GetSignaturesForAddress,
            &ctx,
        );
        assert_eq!(parsed.status, CallStatus::Success);
        assert_eq!(parsed.signature, None);
        assert_eq!(parsed.observed_slot, None);
        assert!(parsed.claim.is_none());
    }

    #[test]
    fn anchored_signature_query_rejects_slots_at_or_past_the_anchor() {
        let ctx = RequestContext {
            gsfa_anchor: Some((100, "anchor".to_string())),
            ..RequestContext::default()
        };
        let parsed = classify(
            StatusCode::OK,
            &body(r#"{"jsonrpc":"2.0","result":[{"signature":"new","slot":100}],"id":1}"#),
            RpcMethod::GetSignaturesForAddress,
            &ctx,
        );
        assert_eq!(parsed.status, CallStatus::Error(ErrorKind::Empty));
    }

    #[test]
    fn skipped_slot_rpc_errors_are_not_provider_errors() {
        for code in [-32007i64, -32009] {
            let parsed = classify(
                StatusCode::OK,
                &body(&format!(
                    r#"{{"jsonrpc":"2.0","error":{{"code":{code},"message":"Slot 1 was skipped"}},"id":1}}"#
                )),
                RpcMethod::GetBlockRecent,
                &RequestContext::default(),
            );
            assert_eq!(parsed.status, CallStatus::Skipped);
        }
        assert_eq!(CallStatus::Skipped.label(), "skipped");
        assert_eq!(CallStatus::Skipped.error_kind(), None);
        assert!(!CallStatus::Skipped.is_success());
        assert!(!CallStatus::Skipped.is_error());
    }

    #[test]
    fn skipped_slot_codes_stay_errors_on_non_fixed_slot_methods() {
        let parsed = classify(
            StatusCode::OK,
            &body(
                r#"{"jsonrpc":"2.0","error":{"code":-32007,"message":"Slot 1 was skipped"},"id":1}"#,
            ),
            RpcMethod::GetTransactionRecent,
            &RequestContext::default(),
        );
        assert_eq!(
            parsed.status,
            CallStatus::Error(ErrorKind::RpcError(-32007))
        );
    }

    #[test]
    fn json_rpc_error_is_classified_with_code() {
        let parsed = classify(
            StatusCode::OK,
            &body(r#"{"jsonrpc":"2.0","error":{"code":-32005,"message":"unhealthy"},"id":1}"#),
            RpcMethod::GetHealth,
            &RequestContext::default(),
        );
        assert_eq!(
            parsed.status,
            CallStatus::Error(ErrorKind::RpcError(-32005))
        );
        assert_eq!(parsed.observed_slot, None);
    }

    #[test]
    fn non_success_http_status_is_an_error() {
        let parsed = classify(
            StatusCode::INTERNAL_SERVER_ERROR,
            &body("upstream down"),
            RpcMethod::GetSlot,
            &RequestContext::default(),
        );
        assert_eq!(parsed.status, CallStatus::Error(ErrorKind::HttpStatus(500)));
    }

    #[test]
    fn http_error_kind_buckets_by_code() {
        assert_eq!(ErrorKind::HttpStatus(429).as_str(), "http_429");
        assert_eq!(ErrorKind::HttpStatus(400).as_str(), "http_4xx");
        assert_eq!(ErrorKind::HttpStatus(403).as_str(), "http_4xx");
        assert_eq!(ErrorKind::HttpStatus(503).as_str(), "http_5xx");
        assert_eq!(ErrorKind::Stale.as_str(), "stale");
    }

    #[test]
    fn known_rpc_codes_get_named_error_kinds() {
        assert_eq!(
            ErrorKind::RpcError(-32004).as_str(),
            "rpc_block_unavailable"
        );
        assert_eq!(ErrorKind::RpcError(-32005).as_str(), "rpc_node_unhealthy");
        assert_eq!(ErrorKind::RpcError(-32007).as_str(), "rpc_slot_skipped");
        assert_eq!(ErrorKind::RpcError(-32009).as_str(), "rpc_slot_skipped");
        assert_eq!(
            ErrorKind::RpcError(-32011).as_str(),
            "rpc_tx_history_unavailable"
        );
        assert_eq!(ErrorKind::RpcError(-32601).as_str(), "rpc_method_not_found");
        assert_eq!(ErrorKind::RpcError(-32602).as_str(), "rpc_invalid_params");
        assert_eq!(ErrorKind::RpcError(-99999).as_str(), "rpc_error");
    }

    #[test]
    fn http_4xx_is_neutral_not_a_provider_error() {
        for code in [400, 401, 403, 404, 429] {
            let status = CallStatus::Error(ErrorKind::HttpStatus(code));
            assert_eq!(status.label(), "skipped");
            assert!(!status.is_error());
            assert!(!status.is_success());
        }
        assert_eq!(
            CallStatus::Error(ErrorKind::HttpStatus(429)).error_kind(),
            Some("http_429")
        );
        let server_side = CallStatus::Error(ErrorKind::HttpStatus(503));
        assert_eq!(server_side.label(), "error");
        assert!(server_side.is_error());
    }

    #[test]
    fn malformed_body_is_a_decode_error() {
        let parsed = classify(
            StatusCode::OK,
            &body("not json"),
            RpcMethod::GetSlot,
            &RequestContext::default(),
        );
        assert_eq!(parsed.status, CallStatus::Error(ErrorKind::Decode));
    }

    #[test]
    fn status_labels_and_error_kinds() {
        assert_eq!(CallStatus::Success.label(), "success");
        assert_eq!(CallStatus::Success.error_kind(), None);
        assert_eq!(
            CallStatus::Error(ErrorKind::Timeout).error_kind(),
            Some("timeout")
        );
    }
}
