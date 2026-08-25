use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;

const MAX_POINTS: u64 = 5000;
const MAX_RANGE_SECONDS: u64 = 400 * 24 * 60 * 60;
const MIN_STEP_SECONDS: u64 = 60;
const DEFAULT_RANGE_SECONDS: u64 = 24 * 60 * 60;
const DEFAULT_TARGET_POINTS: u64 = 500;
const RATE_LIMIT_PER_MINUTE: u32 = 60;
const RATE_WINDOW_SECONDS: u64 = 60;
const ALLOWED_QUANTILES: &[&str] = &["0.5", "0.9", "0.95", "0.99"];
const DEFAULT_QUANTILES: &[&str] = &["0.5", "0.95", "0.99"];
const TEMPLATES: &[&str] = &[
    "latency",
    "latency_buckets",
    "requests",
    "win_rate",
    "claim_checks",
    "samples",
];
const DEFAULT_SAMPLE_LIMIT: u64 = 1000;
const MAX_SAMPLE_LIMIT: u64 = 10000;
const MAX_INGEST_ROWS: usize = 10000;
const MAX_INGEST_BODY_BYTES: usize = 8 * 1024 * 1024;
const SAMPLE_FIELDS: &[&str] = &[
    "ts",
    "provider",
    "method",
    "infra",
    "region",
    "target",
    "status",
    "error_kind",
    "latency_ms",
    "slot",
];

pub struct RawApiConfig {
    pub bind: SocketAddr,
    pub jwt_secret: String,
    pub grafana_url: String,
    pub grafana_token: String,
    pub datasource_uid: String,
    pub database_url: String,
}

impl RawApiConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let need = |key: &str| {
            std::env::var(key).map_err(|_| anyhow::anyhow!("{key} must be set in the environment"))
        };
        Ok(Self {
            bind: std::env::var("RAW_API_BIND")
                .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
                .parse()?,
            jwt_secret: need("RAW_API_JWT_SECRET")?,
            grafana_url: need("GRAFANA_API_URL")?.trim_end_matches('/').to_string(),
            grafana_token: std::env::var("RAW_API_GRAFANA_TOKEN")
                .or_else(|_| need("GRAFANA_API_TOKEN"))?,
            datasource_uid: std::env::var("GRAFANA_DATASOURCE_UID")
                .unwrap_or_else(|_| "grafanacloud-prom".to_string()),
            database_url: need("SAMPLES_DATABASE_URL")?,
        })
    }
}

const MAX_UPSTREAM_CONCURRENCY: usize = 8;
const UPSTREAM_TIMEOUT_SECONDS: u64 = 30;

struct AppState {
    config: RawApiConfig,
    http: reqwest::Client,
    windows: Mutex<HashMap<String, (Instant, u32)>>,
    upstream: tokio::sync::Semaphore,
}

pub async fn serve(config: RawApiConfig) -> anyhow::Result<()> {
    let bind = config.bind;
    let state = Arc::new(AppState {
        config,
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(UPSTREAM_TIMEOUT_SECONDS))
            .build()?,
        windows: Mutex::new(HashMap::new()),
        upstream: tokio::sync::Semaphore::new(MAX_UPSTREAM_CONCURRENCY),
    });
    tokio::spawn(schema_then_retention(state.clone()));
    let app = Router::new()
        .route("/raw/{template}", get(raw_handler))
        .route(
            "/ingest/samples",
            post(ingest_handler).layer(DefaultBodyLimit::max(MAX_INGEST_BODY_BYTES)),
        )
        .route("/health", get(|| async { "ok" }))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "raw-api listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn raw_handler(
    State(state): State<Arc<AppState>>,
    Path(template): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    let sub = match token.and_then(|t| verify_token(t, &state.config.jwt_secret)) {
        Some(sub) => sub,
        None => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "invalid, expired, or missing token",
            )
        }
    };

    if let Some(retry_after) = take_rate_limit_slot(&state, &sub) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("Retry-After", retry_after.to_string())],
            Json(json!({"error": "rate limit exceeded"})),
        )
            .into_response();
    }

    let query = match parse_query(&template, &params) {
        Ok(query) => query,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };

    let Ok(_permit) = state.upstream.try_acquire() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [("Retry-After", "5")],
            Json(json!({"error": "server busy, retry shortly"})),
        )
            .into_response();
    };

    if query.template == "samples" {
        let rows = match query_samples(&state, &query).await {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(%error, "samples query failed");
                return error_response(StatusCode::BAD_GATEWAY, "upstream query failed");
            }
        };
        if query.format == "csv" {
            return (
                StatusCode::OK,
                [
                    ("Content-Type", "text/csv; charset=utf-8".to_string()),
                    ("Cache-Control", "no-store".to_string()),
                ],
                rows_to_csv(&rows),
            )
                .into_response();
        }
        return (
            StatusCode::OK,
            [("Cache-Control", "no-store")],
            Json(json!({
                "generatedAt": humantime::format_rfc3339_seconds(SystemTime::now()).to_string(),
                "template": query.template,
                "rows": rows,
            })),
        )
            .into_response();
    }

    let series = match run_query(&state, &query).await {
        Ok(series) => series,
        Err(error) => {
            tracing::error!(%error, %template, "upstream query failed");
            return error_response(StatusCode::BAD_GATEWAY, "upstream query failed");
        }
    };

    if query.format == "csv" {
        return (
            StatusCode::OK,
            [
                ("Content-Type", "text/csv; charset=utf-8".to_string()),
                ("Cache-Control", "no-store".to_string()),
            ],
            to_csv(&series),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        [("Cache-Control", "no-store")],
        Json(json!({
            "generatedAt": humantime::format_rfc3339_seconds(SystemTime::now()).to_string(),
            "template": query.template,
            "series": series,
        })),
    )
        .into_response()
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        [("Cache-Control", "no-store")],
        Json(json!({"error": message})),
    )
        .into_response()
}

pub fn verify_token(token: &str, secret: &str) -> Option<String> {
    let mut parts = token.split('.');
    let header_part = parts.next()?;
    let payload_part = parts.next()?;
    let signature_part = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let signature = URL_SAFE_NO_PAD.decode(signature_part).ok()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(header_part.as_bytes());
    mac.update(b".");
    mac.update(payload_part.as_bytes());
    mac.verify_slice(&signature).ok()?;

    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header_part).ok()?).ok()?;
    if header.get("alg")?.as_str()? != "HS256" {
        return None;
    }

    let payload: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload_part).ok()?).ok()?;
    let sub = payload.get("sub")?.as_str()?;
    if sub.is_empty() {
        return None;
    }
    let exp = payload.get("exp")?.as_f64()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs_f64();
    if exp <= now {
        return None;
    }
    Some(sub.to_string())
}

fn take_rate_limit_slot(state: &AppState, sub: &str) -> Option<u64> {
    let mut windows = state.windows.lock().unwrap();
    let now = Instant::now();
    match windows.get_mut(sub) {
        Some((start, count)) if now.duration_since(*start).as_secs() < RATE_WINDOW_SECONDS => {
            if *count >= RATE_LIMIT_PER_MINUTE {
                return Some(RATE_WINDOW_SECONDS - now.duration_since(*start).as_secs());
            }
            *count += 1;
            None
        }
        _ => {
            windows.insert(sub.to_string(), (now, 1));
            None
        }
    }
}

#[derive(Debug)]
pub struct RawQuery {
    pub template: String,
    pub provider: Option<String>,
    pub method: Option<String>,
    pub infra: Option<String>,
    pub region: Option<String>,
    pub status: Option<String>,
    pub start: u64,
    pub end: u64,
    pub step: u64,
    pub quantiles: Vec<String>,
    pub by: String,
    pub format: String,
    pub limit: u64,
}

pub fn parse_query(template: &str, params: &HashMap<String, String>) -> Result<RawQuery, String> {
    if !TEMPLATES.contains(&template) {
        return Err(format!(
            "unknown template \"{template}\"; expected one of {}",
            TEMPLATES.join(", ")
        ));
    }

    let label = |key: &str| -> Result<Option<String>, String> {
        match params.get(key).filter(|v| !v.is_empty()) {
            None => Ok(None),
            Some(v) if is_valid_label_value(v) => Ok(Some(v.clone())),
            Some(v) => Err(format!("invalid {key} \"{v}\"")),
        }
    };
    let provider = label("provider")?;
    let method = label("method")?;
    let infra = label("infra")?;
    let region = label("region")?;
    let status = label("status")?;

    if region.is_some() && infra.is_none() {
        return Err("region requires infra".to_string());
    }
    if template == "claim_checks" && (infra.is_some() || region.is_some()) {
        return Err(
            "claim_checks has no infra or region dimension; remove those filters".to_string(),
        );
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let end = parse_time(params.get("end"), now)?;
    let start = parse_time(
        params.get("start"),
        end.saturating_sub(DEFAULT_RANGE_SECONDS),
    )?;
    if start >= end {
        return Err("start must be before end".to_string());
    }
    let range = end - start;
    if range > MAX_RANGE_SECONDS {
        return Err(format!(
            "range exceeds retention; maximum is {MAX_RANGE_SECONDS} seconds"
        ));
    }

    let step = match params.get("step") {
        None => (range
            .div_ceil(DEFAULT_TARGET_POINTS)
            .div_ceil(MIN_STEP_SECONDS)
            * MIN_STEP_SECONDS)
            .max(MIN_STEP_SECONDS),
        Some(v) => {
            let step: u64 = v
                .parse()
                .map_err(|_| format!("invalid step \"{v}\"; use whole seconds"))?;
            if step < MIN_STEP_SECONDS {
                return Err(format!("step minimum is {MIN_STEP_SECONDS} seconds"));
            }
            step
        }
    };
    if range / step + 1 > MAX_POINTS {
        return Err(format!(
            "too many points; keep the inclusive point count at or below {MAX_POINTS}"
        ));
    }

    let quantiles = match params.get("q") {
        None => DEFAULT_QUANTILES.iter().map(|q| q.to_string()).collect(),
        Some(v) => {
            let mut quantiles: Vec<String> = Vec::new();
            for q in v.split(',') {
                if !ALLOWED_QUANTILES.contains(&q) {
                    return Err(format!(
                        "invalid quantile \"{q}\"; allowed: {}",
                        ALLOWED_QUANTILES.join(", ")
                    ));
                }
                if !quantiles.iter().any(|existing| existing == q) {
                    quantiles.push(q.to_string());
                }
            }
            quantiles
        }
    };

    let by = params
        .get("by")
        .cloned()
        .unwrap_or_else(|| "status".to_string());
    if by != "status" && by != "error_kind" {
        return Err(format!("invalid by \"{by}\"; use status or error_kind"));
    }

    let format = params
        .get("format")
        .cloned()
        .unwrap_or_else(|| "json".to_string());
    if format != "json" && format != "csv" {
        return Err(format!("invalid format \"{format}\"; use json or csv"));
    }

    let limit = match params.get("limit") {
        None => DEFAULT_SAMPLE_LIMIT,
        Some(v) => {
            let limit: u64 = v.parse().map_err(|_| format!("invalid limit \"{v}\""))?;
            if limit == 0 || limit > MAX_SAMPLE_LIMIT {
                return Err(format!("limit must be between 1 and {MAX_SAMPLE_LIMIT}"));
            }
            limit
        }
    };

    Ok(RawQuery {
        template: template.to_string(),
        provider,
        method,
        infra,
        region,
        status,
        start,
        end,
        step,
        quantiles,
        by,
        format,
        limit,
    })
}

#[derive(Deserialize, serde::Serialize)]
struct IngestRow {
    ts: f64,
    provider: String,
    method: String,
    infra: String,
    region: String,
    #[serde(default)]
    target: String,
    status: String,
    error_kind: String,
    latency_ms: f64,
    #[serde(default)]
    slot: Option<u64>,
}

#[derive(Deserialize)]
struct IngestBody {
    rows: Vec<IngestRow>,
}

async fn ingest_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<IngestBody>,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    match token.and_then(|t| verify_token(t, &state.config.jwt_secret)) {
        Some(sub) if sub == "fleet" => {}
        Some(_) => return error_response(StatusCode::FORBIDDEN, "ingest requires the fleet token"),
        None => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "invalid, expired, or missing token",
            )
        }
    }
    if body.rows.is_empty() || body.rows.len() > MAX_INGEST_ROWS {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("rows must contain 1..={MAX_INGEST_ROWS} entries"),
        );
    }
    for row in &body.rows {
        let labels = [
            &row.provider,
            &row.method,
            &row.infra,
            &row.region,
            &row.status,
            &row.error_kind,
        ];
        if labels.iter().any(|v| !is_valid_label_value(v))
            || !(row.target.is_empty() || is_valid_label_value(&row.target))
            || !row.ts.is_finite()
            || !row.latency_ms.is_finite()
        {
            return error_response(StatusCode::BAD_REQUEST, "invalid row values");
        }
    }
    match insert_samples(&state, &body.rows).await {
        Ok(()) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Err(error) => {
            tracing::error!(%error, "sample ingest failed");
            error_response(StatusCode::BAD_GATEWAY, "storage insert failed")
        }
    }
}

async fn pg_connect(state: &AppState) -> anyhow::Result<tokio_postgres::Client> {
    let (client, connection) =
        tokio_postgres::connect(&state.config.database_url, tokio_postgres::NoTls).await?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

async fn ensure_schema(state: &Arc<AppState>) -> anyhow::Result<()> {
    let client = pg_connect(state).await?;
    client
        .batch_execute(include_str!("../deploy/migrations/0001_probe_samples.sql"))
        .await?;
    Ok(())
}

async fn schema_then_retention(state: Arc<AppState>) {
    loop {
        match ensure_schema(&state).await {
            Ok(()) => break,
            Err(error) => {
                tracing::warn!(%error, "schema apply failed, retrying in 30s");
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        }
    }
    retention_loop(state).await;
}

async fn retention_loop(state: Arc<AppState>) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
    loop {
        tick.tick().await;
        match pg_connect(&state).await {
            Ok(client) => {
                if let Err(error) = client
                    .execute(
                        "DELETE FROM probe_samples WHERE ts < now() - interval '400 days'",
                        &[],
                    )
                    .await
                {
                    tracing::warn!(%error, "sample retention delete failed");
                }
            }
            Err(error) => tracing::warn!(%error, "sample retention connect failed"),
        }
    }
}

async fn insert_samples(state: &AppState, rows: &[IngestRow]) -> anyhow::Result<()> {
    let client = pg_connect(state).await?;
    let ts: Vec<f64> = rows.iter().map(|r| r.ts).collect();
    let provider: Vec<&str> = rows.iter().map(|r| r.provider.as_str()).collect();
    let method: Vec<&str> = rows.iter().map(|r| r.method.as_str()).collect();
    let infra: Vec<&str> = rows.iter().map(|r| r.infra.as_str()).collect();
    let region: Vec<&str> = rows.iter().map(|r| r.region.as_str()).collect();
    let target: Vec<&str> = rows.iter().map(|r| r.target.as_str()).collect();
    let status: Vec<&str> = rows.iter().map(|r| r.status.as_str()).collect();
    let error_kind: Vec<&str> = rows.iter().map(|r| r.error_kind.as_str()).collect();
    let latency_ms: Vec<f64> = rows.iter().map(|r| r.latency_ms).collect();
    let slot: Vec<Option<i64>> = rows.iter().map(|r| r.slot.map(|v| v as i64)).collect();
    client
        .execute(
            "INSERT INTO probe_samples (ts, provider, method, infra, region, target, status, error_kind, latency_ms, slot) \
             SELECT to_timestamp(t), p, m, i, r, tg, st, e, l, sl \
             FROM UNNEST($1::float8[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[], $8::text[], $9::float8[], $10::int8[]) \
             AS x(t, p, m, i, r, tg, st, e, l, sl)",
            &[
                &ts,
                &provider,
                &method,
                &infra,
                &region,
                &target,
                &status,
                &error_kind,
                &latency_ms,
                &slot,
            ],
        )
        .await?;
    Ok(())
}

async fn query_samples(state: &AppState, query: &RawQuery) -> anyhow::Result<Vec<Value>> {
    let client = pg_connect(state).await?;
    let mut sql = String::from(
        "SELECT extract(epoch from ts)::float8 AS ts, provider, method, infra, region, target, status, error_kind, latency_ms, slot \
         FROM probe_samples WHERE ts >= to_timestamp($1) AND ts < to_timestamp($2)",
    );
    let start = query.start as f64;
    let end = query.end as f64;
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![&start, &end];
    let filters = [
        ("provider", &query.provider),
        ("method", &query.method),
        ("infra", &query.infra),
        ("region", &query.region),
        ("status", &query.status),
    ];
    for (name, value) in &filters {
        if let Some(value) = value {
            params.push(value);
            sql.push_str(&format!(" AND {name} = ${}", params.len()));
        }
    }
    sql.push_str(&format!(" ORDER BY ts DESC LIMIT {}", query.limit));
    let rows = client.query(&sql, &params).await?;
    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "ts": row.get::<_, f64>("ts"),
                "provider": row.get::<_, &str>("provider"),
                "method": row.get::<_, &str>("method"),
                "infra": row.get::<_, &str>("infra"),
                "region": row.get::<_, &str>("region"),
                "target": row.get::<_, &str>("target"),
                "status": row.get::<_, &str>("status"),
                "error_kind": row.get::<_, &str>("error_kind"),
                "latency_ms": row.get::<_, f64>("latency_ms"),
                "slot": row.get::<_, Option<i64>>("slot"),
            })
        })
        .collect())
}

fn rows_to_csv(rows: &[Value]) -> String {
    let mut out = vec![SAMPLE_FIELDS.join(",")];
    for row in rows {
        out.push(
            SAMPLE_FIELDS
                .iter()
                .map(|field| {
                    let value = &row[*field];
                    if value.is_null() {
                        String::new()
                    } else if let Some(s) = value.as_str() {
                        escape_csv(s)
                    } else {
                        value.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    out.join("\n")
}

fn is_valid_label_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn parse_time(value: Option<&String>, fallback: u64) -> Result<u64, String> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Ok(fallback);
    };
    if value.chars().all(|c| c.is_ascii_digit()) {
        return value
            .parse()
            .map_err(|_| format!("invalid time \"{value}\""));
    }
    humantime::parse_rfc3339(value)
        .map_err(|_| format!("invalid time \"{value}\"; use unix seconds or RFC 3339"))
        .and_then(|t| {
            t.duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .map_err(|_| format!("invalid time \"{value}\""))
        })
}

pub fn build_selector(query: &RawQuery, extra_matchers: &[&str]) -> String {
    let mut matchers: Vec<String> = extra_matchers.iter().map(|m| m.to_string()).collect();
    if let Some(v) = &query.provider {
        matchers.push(format!("provider=\"{v}\""));
    }
    if let Some(v) = &query.method {
        matchers.push(format!("method=\"{v}\""));
    }
    if let Some(v) = &query.infra {
        matchers.push(format!("infra=\"{v}\""));
    }
    if let Some(v) = &query.region {
        matchers.push(format!("region=\"{v}\""));
    }
    if matchers.is_empty() {
        String::new()
    } else {
        format!("{{{}}}", matchers.join(","))
    }
}

#[derive(Deserialize)]
struct PromResponse {
    status: String,
    #[serde(default)]
    data: Option<PromData>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct PromData {
    #[serde(default)]
    result: Vec<PromSeries>,
}

#[derive(Deserialize)]
struct PromSeries {
    #[serde(default)]
    metric: HashMap<String, String>,
    #[serde(default)]
    values: Vec<(f64, String)>,
}

async fn query_range(
    state: &AppState,
    promql: &str,
    query: &RawQuery,
) -> anyhow::Result<Vec<PromSeries>> {
    let url = format!(
        "{}/api/datasources/proxy/uid/{}/api/v1/query_range",
        state.config.grafana_url, state.config.datasource_uid
    );
    let response = state
        .http
        .get(&url)
        .header(
            "Authorization",
            format!("Bearer {}", state.config.grafana_token),
        )
        .query(&[
            ("query", promql),
            ("start", &query.start.to_string()),
            ("end", &query.end.to_string()),
            ("step", &query.step.to_string()),
        ])
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("prometheus returned HTTP {status}");
    }
    let payload: PromResponse = response.json().await?;
    if payload.status != "success" {
        anyhow::bail!(
            "prometheus response: {}",
            payload
                .error
                .unwrap_or_else(|| "invalid payload".to_string())
        );
    }
    Ok(payload.data.map(|d| d.result).unwrap_or_default())
}

async fn run_query(state: &AppState, query: &RawQuery) -> anyhow::Result<Vec<Value>> {
    let step = query.step;
    match query.template.as_str() {
        "latency" => {
            let selector = build_selector(query, &["status=\"success\""]);
            let mut series = Vec::new();
            for quantile in &query.quantiles {
                let promql = format!(
                    "1000 * histogram_quantile({quantile}, sum by (le, provider)(rate(rpc_latency_seconds_bucket{selector}[{step}s])))"
                );
                let results = query_range(state, &promql, query).await?;
                series.extend(to_series(
                    results,
                    &["provider"],
                    &[("quantile", quantile.as_str())],
                ));
            }
            Ok(series)
        }
        "latency_buckets" => {
            let selector = build_selector(query, &["status=\"success\""]);
            let promql = format!(
                "sum by (provider, le)(increase(rpc_latency_seconds_bucket{selector}[{step}s]))"
            );
            Ok(to_series(
                query_range(state, &promql, query).await?,
                &["provider", "le"],
                &[],
            ))
        }
        "requests" => {
            let extra: &[&str] = if query.by == "error_kind" {
                &["status=\"error\""]
            } else {
                &[]
            };
            let selector = build_selector(query, extra);
            let by = &query.by;
            let promql =
                format!("sum by (provider, {by})(increase(rpc_requests_total{selector}[{step}s]))");
            Ok(to_series(
                query_range(state, &promql, query).await?,
                &["provider", by],
                &[],
            ))
        }
        "win_rate" => {
            let selector = build_selector(query, &["status=\"success\""]);
            let promql = format!(
                "1000 * sum by (provider)(rate(rpc_latency_seconds_sum{selector}[{step}s])) / sum by (provider)(rate(rpc_latency_seconds_count{selector}[{step}s]))"
            );
            let series = to_series(
                query_range(state, &promql, query).await?,
                &["provider"],
                &[],
            );
            Ok(compute_win_rates(series))
        }
        "claim_checks" => {
            let selector = build_selector(query, &[]);
            let promql = format!(
                "sum by (provider, method, result)(increase(rpc_claim_check_total{selector}[{step}s]))"
            );
            Ok(to_series(
                query_range(state, &promql, query).await?,
                &["provider", "method", "result"],
                &[],
            ))
        }
        other => anyhow::bail!("unhandled template {other}"),
    }
}

fn to_series(
    results: Vec<PromSeries>,
    label_keys: &[&str],
    extra_labels: &[(&str, &str)],
) -> Vec<Value> {
    results
        .into_iter()
        .filter_map(|series| {
            let mut labels = serde_json::Map::new();
            for (key, value) in extra_labels {
                labels.insert(key.to_string(), json!(value));
            }
            for key in label_keys {
                labels.insert(key.to_string(), json!(series.metric.get(*key)?));
            }
            let points: Vec<Value> = series
                .values
                .iter()
                .filter_map(|(ts, value)| {
                    let value: f64 = value.parse().ok()?;
                    if !value.is_finite() {
                        return None;
                    }
                    Some(json!([*ts as u64, value]))
                })
                .collect();
            Some(json!({"labels": labels, "points": points}))
        })
        .collect()
}

fn compute_win_rates(series: Vec<Value>) -> Vec<Value> {
    let mut winners: HashMap<u64, (String, f64)> = HashMap::new();
    for entry in &series {
        let provider = entry["labels"]["provider"].as_str().unwrap_or_default();
        for point in entry["points"].as_array().into_iter().flatten() {
            let ts = point[0].as_u64().unwrap_or_default();
            let value = point[1].as_f64().unwrap_or(f64::MAX);
            match winners.get(&ts) {
                Some((_, best)) if *best <= value => {}
                _ => {
                    winners.insert(ts, (provider.to_string(), value));
                }
            }
        }
    }
    let total_timestamps = winners.len() as u64;
    series
        .into_iter()
        .map(|entry| {
            let provider = entry["labels"]["provider"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let points = entry["points"].as_array().cloned().unwrap_or_default();
            let samples = points.len() as u64;
            let wins = points
                .iter()
                .filter(|point| {
                    let ts = point[0].as_u64().unwrap_or_default();
                    winners
                        .get(&ts)
                        .map(|(w, _)| w == &provider)
                        .unwrap_or(false)
                })
                .count() as u64;
            let win_pct = if total_timestamps > 0 {
                (10000.0 * wins as f64 / total_timestamps as f64).round() / 100.0
            } else {
                0.0
            };
            json!({
                "labels": entry["labels"],
                "wins": wins,
                "samples": samples,
                "totalTimestamps": total_timestamps,
                "winPct": win_pct,
            })
        })
        .collect()
}

pub fn to_csv(series: &[Value]) -> String {
    let mut label_keys: Vec<String> = series
        .iter()
        .flat_map(|s| {
            s["labels"]
                .as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect();
    label_keys.sort();
    label_keys.dedup();

    let summary = series.iter().any(|s| s.get("winPct").is_some());
    let mut rows: Vec<Vec<String>> = Vec::new();
    if summary {
        let mut header: Vec<String> = label_keys.clone();
        header.extend(["wins", "samples", "win_pct"].map(String::from));
        rows.push(header);
        for entry in series {
            let mut row: Vec<String> = label_keys
                .iter()
                .map(|k| entry["labels"][k].as_str().unwrap_or_default().to_string())
                .collect();
            row.push(entry["wins"].to_string());
            row.push(entry["samples"].to_string());
            row.push(entry["winPct"].to_string());
            rows.push(row);
        }
    } else {
        let mut header = vec!["timestamp".to_string()];
        header.extend(label_keys.clone());
        header.push("value".to_string());
        rows.push(header);
        for entry in series {
            for point in entry["points"].as_array().into_iter().flatten() {
                let mut row = vec![point[0].to_string()];
                row.extend(
                    label_keys
                        .iter()
                        .map(|k| entry["labels"][k].as_str().unwrap_or_default().to_string()),
                );
                row.push(point[1].to_string());
                rows.push(row);
            }
        }
    }
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|field| escape_csv(field))
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape_csv(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mint(payload: &Value, secret: &str) -> String {
        let encode = |v: &Value| URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap());
        let body = format!(
            "{}.{}",
            encode(&json!({"alg": "HS256", "typ": "JWT"})),
            encode(payload)
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body.as_bytes());
        format!(
            "{body}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        )
    }

    fn future_exp() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            + 3600.0
    }

    #[test]
    fn token_roundtrip_and_rejections() {
        let token = mint(&json!({"sub": "chainstack", "exp": future_exp()}), "s");
        assert_eq!(verify_token(&token, "s").as_deref(), Some("chainstack"));
        assert!(verify_token(&token, "wrong").is_none());
        assert!(verify_token(&mint(&json!({"sub": "x", "exp": 1.0}), "s"), "s").is_none());
        assert!(verify_token(&mint(&json!({"exp": future_exp()}), "s"), "s").is_none());
        assert!(verify_token(&mint(&json!({"sub": "x"}), "s"), "s").is_none());
        assert!(verify_token("not.a.token", "s").is_none());
    }

    #[test]
    fn parse_applies_defaults_and_guards() {
        let q = parse_query("latency", &HashMap::new()).unwrap();
        assert_eq!(q.end - q.start, DEFAULT_RANGE_SECONDS);
        assert_eq!(q.step, 180);
        assert_eq!(q.quantiles, vec!["0.5", "0.95", "0.99"]);

        assert!(parse_query("promql", &HashMap::new()).is_err());
        let mut p = HashMap::new();
        p.insert("provider".to_string(), "evil{}".to_string());
        assert!(parse_query("latency", &p).is_err());
        let mut p = HashMap::new();
        p.insert("region".to_string(), "fra2".to_string());
        assert!(parse_query("latency", &p).is_err());
        let mut p = HashMap::new();
        p.insert("infra".to_string(), "tsw".to_string());
        assert!(parse_query("claim_checks", &p).is_err());
        let mut p = HashMap::new();
        p.insert("start".to_string(), "0".to_string());
        p.insert("end".to_string(), "999999999".to_string());
        assert!(parse_query("latency", &p).is_err());
    }

    #[test]
    fn selector_quotes_only_validated_values() {
        let mut p = HashMap::new();
        p.insert("provider".to_string(), "fluxrpc".to_string());
        p.insert("infra".to_string(), "tsw".to_string());
        p.insert("region".to_string(), "fra2".to_string());
        let q = parse_query("latency", &p).unwrap();
        assert_eq!(
            build_selector(&q, &["status=\"success\""]),
            "{status=\"success\",provider=\"fluxrpc\",infra=\"tsw\",region=\"fra2\"}"
        );
    }

    #[test]
    fn win_rate_missing_timestamps_count_against() {
        let series = vec![
            json!({"labels": {"provider": "a"}, "points": [[1u64, 10.0], [2u64, 10.0]]}),
            json!({"labels": {"provider": "b"}, "points": [[2u64, 5.0]]}),
        ];
        let out = compute_win_rates(series);
        assert_eq!(out[0]["wins"], 1);
        assert_eq!(out[0]["samples"], 2);
        assert_eq!(out[0]["winPct"], 50.0);
        assert_eq!(out[1]["wins"], 1);
        assert_eq!(out[1]["samples"], 1);
        assert_eq!(out[1]["winPct"], 50.0);
        assert_eq!(out[1]["totalTimestamps"], 2);
    }

    #[test]
    fn csv_renders_points_and_summaries() {
        let series = vec![json!({
            "labels": {"provider": "helius", "quantile": "0.99"},
            "points": [[1755561600u64, 42.5]],
        })];
        assert_eq!(
            to_csv(&series),
            "timestamp,provider,quantile,value\n1755561600,helius,0.99,42.5"
        );

        let summary = vec![json!({
            "labels": {"provider": "helius"},
            "wins": 3, "samples": 4, "winPct": 75.0,
        })];
        assert_eq!(
            to_csv(&summary),
            "provider,wins,samples,win_pct\nhelius,3,4,75.0"
        );
    }
}
