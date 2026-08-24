use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::mpsc;

use crate::rpc::methods::RpcMethod;
use crate::rpc::{CallResult, CallStatus};

const BUFFER: usize = 4096;
const FLUSH_ROWS: usize = 500;
const FLUSH_SECONDS: u64 = 30;

#[derive(Clone)]
pub struct SampleLogger {
    tx: mpsc::Sender<Sample>,
    infra: String,
    region: String,
}

#[derive(Serialize)]
struct Sample {
    ts: f64,
    provider: String,
    method: &'static str,
    infra: String,
    region: String,
    target: String,
    status: &'static str,
    error_kind: &'static str,
    latency_ms: f64,
    slot: Option<u64>,
}

impl SampleLogger {
    pub fn from_env(region: &str) -> Option<Self> {
        let url = std::env::var("SAMPLE_LOG_URL")
            .ok()
            .filter(|v| !v.is_empty())?;
        let token = std::env::var("SAMPLE_LOG_TOKEN")
            .ok()
            .filter(|v| !v.is_empty())?;
        let infra = std::env::var("MONITOR_INFRA").unwrap_or_else(|_| "gcp".to_string());
        let (tx, rx) = mpsc::channel(BUFFER);
        tokio::spawn(flush_loop(url, token, rx));
        tracing::info!("sample logging enabled");
        Some(Self {
            tx,
            infra,
            region: region.to_string(),
        })
    }

    pub fn log(
        &self,
        provider: &str,
        method: RpcMethod,
        status: CallStatus,
        target: &str,
        result: &CallResult,
    ) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or_default();
        let _ = self.tx.try_send(Sample {
            ts,
            provider: provider.to_string(),
            method: method.label(),
            infra: self.infra.clone(),
            region: self.region.clone(),
            target: target.to_string(),
            status: status.label(),
            error_kind: status.error_kind().unwrap_or("none"),
            latency_ms: result.latency.as_secs_f64() * 1000.0,
            slot: result.observed_slot,
        });
    }
}

async fn flush_loop(url: String, token: String, mut rx: mpsc::Receiver<Sample>) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
    else {
        return;
    };
    let mut buffer = Vec::new();
    let mut tick = tokio::time::interval(Duration::from_secs(FLUSH_SECONDS));
    loop {
        tokio::select! {
            received = rx.recv() => match received {
                Some(sample) => {
                    buffer.push(sample);
                    if buffer.len() >= FLUSH_ROWS {
                        flush(&client, &url, &token, &mut buffer).await;
                    }
                }
                None => {
                    flush(&client, &url, &token, &mut buffer).await;
                    return;
                }
            },
            _ = tick.tick() => {
                if !buffer.is_empty() {
                    flush(&client, &url, &token, &mut buffer).await;
                }
            }
        }
    }
}

async fn flush(client: &reqwest::Client, url: &str, token: &str, buffer: &mut Vec<Sample>) {
    let rows = std::mem::take(buffer);
    let count = rows.len();
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({ "rows": rows }))
        .send()
        .await;
    match response {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => tracing::warn!(status = %r.status(), count, "sample flush rejected"),
        Err(error) => tracing::warn!(%error, count, "sample flush failed"),
    }
}
