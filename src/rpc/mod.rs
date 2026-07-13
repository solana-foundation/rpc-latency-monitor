pub mod methods;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderValue, CACHE_CONTROL, PRAGMA};
use reqwest::StatusCode;
use serde_json::Value;

use crate::rpc::methods::RpcMethod;

#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub tip_slot: Option<u64>,
    pub recent_signature: Option<String>,
    pub recent_accounts: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CallResult {
    pub latency: Duration,
    pub status: CallStatus,
    pub observed_slot: Option<u64>,
    pub signature: Option<String>,
    pub accounts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    Success,
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

    #[inline]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error(_) => "error",
        }
    }

    #[inline]
    pub const fn error_kind(self) -> Option<&'static str> {
        match self {
            Self::Success => None,
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
            Self::HttpStatus(_) => "http_status",
            Self::RpcError(_) => "rpc_error",
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
                    accounts: Vec::new(),
                });
            }
        };

        let http_status = response.status();
        let body = response.bytes().await;
        let latency = start.elapsed();

        let parsed = match body {
            Ok(bytes) => classify(http_status, &bytes, method),
            Err(_) => Parsed::error(ErrorKind::Transport),
        };
        Some(CallResult {
            latency,
            status: parsed.status,
            observed_slot: parsed.observed_slot,
            signature: parsed.signature,
            accounts: parsed.accounts,
        })
    }
}

struct Parsed {
    status: CallStatus,
    observed_slot: Option<u64>,
    signature: Option<String>,
    accounts: Vec<String>,
}

impl Parsed {
    fn error(kind: ErrorKind) -> Self {
        Self {
            status: CallStatus::Error(kind),
            observed_slot: None,
            signature: None,
            accounts: Vec::new(),
        }
    }
}

fn send_error_kind(error: &reqwest::Error) -> ErrorKind {
    if error.is_timeout() {
        ErrorKind::Timeout
    } else {
        ErrorKind::Transport
    }
}

fn classify(status: StatusCode, body: &[u8], method: RpcMethod) -> Parsed {
    if !status.is_success() {
        return Parsed::error(ErrorKind::HttpStatus(status.as_u16()));
    }
    let Ok(json) = serde_json::from_slice::<Value>(body) else {
        return Parsed::error(ErrorKind::Decode);
    };
    if let Some(error) = json.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        return Parsed::error(ErrorKind::RpcError(code));
    }
    let Some(result) = json.get("result") else {
        return Parsed::error(ErrorKind::Decode);
    };
    if !method.is_valid_result(result) {
        return Parsed::error(ErrorKind::Empty);
    }
    Parsed {
        status: CallStatus::Success,
        observed_slot: method.observed_slot(result),
        signature: method.recent_signature(result),
        accounts: method.recent_accounts(result),
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
        );
        assert_eq!(parsed.status, CallStatus::Error(ErrorKind::Empty));
        assert_eq!(parsed.observed_slot, None);
    }

    #[test]
    fn success_extracts_signature_for_signature_query() {
        let parsed = classify(
            StatusCode::OK,
            &body(r#"{"jsonrpc":"2.0","result":[{"signature":"abc"}],"id":1}"#),
            RpcMethod::GetSignaturesForAddress,
        );
        assert_eq!(parsed.signature, Some("abc".to_string()));
    }

    #[test]
    fn json_rpc_error_is_classified_with_code() {
        let parsed = classify(
            StatusCode::OK,
            &body(r#"{"jsonrpc":"2.0","error":{"code":-32005,"message":"unhealthy"},"id":1}"#),
            RpcMethod::GetHealth,
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
        );
        assert_eq!(parsed.status, CallStatus::Error(ErrorKind::HttpStatus(500)));
    }

    #[test]
    fn malformed_body_is_a_decode_error() {
        let parsed = classify(StatusCode::OK, &body("not json"), RpcMethod::GetSlot);
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
