use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use rpc_latency_monitor::config::{Config, ReferenceSource};
use rpc_latency_monitor::metrics::Metrics;
use rpc_latency_monitor::providers;
use rpc_latency_monitor::reference_slot::{poll_reference_endpoint, ArchivalAnchor, ReferenceSlot};
use rpc_latency_monitor::rpc::RpcClient;
use rpc_latency_monitor::{geo, reference_check, scheduler, server};

#[derive(Parser)]
#[command(name = "rpc-latency-monitor")]
struct Args {
    #[arg(long, env = "MONITOR_CONFIG", default_value = "config.yaml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let args = Args::parse();
    let mut config = Config::load(&args.config)?;
    providers::validate_geos(&config.providers).map_err(anyhow::Error::msg)?;
    let endpoints = providers::resolve_endpoints(&config.providers, &config.region)?;
    for p in &config.providers {
        if !providers::serves_region(p, &config.region) {
            info!(provider = %p.name, region = %config.region, "provider excluded in this region by stated coverage");
        }
    }
    let mut gpa_derive = config.gpa_derive.clone();
    if let Some(derive) = gpa_derive.as_mut() {
        derive.endpoint = providers::resolve_url("gpa_derive.endpoint", &derive.endpoint)?;
    }
    if let Some(endpoint) = config.reference_slot.endpoint.as_mut() {
        *endpoint = providers::resolve_url("reference_slot.endpoint", endpoint)?;
    }
    if !config.reference_check.rpc_url.is_empty() {
        config.reference_check.rpc_url =
            providers::resolve_url("reference_check.rpc_url", &config.reference_check.rpc_url)?;
    }

    let metrics = Metrics::new(&config.region, geo::geo_for(&config.region))?;
    let client = Arc::new(RpcClient::new(config.request_timeout)?);
    let reference = ReferenceSlot::new();

    info!(
        region = %config.region,
        provider_count = endpoints.len(),
        check_count = config.checks.len(),
        bind = %config.server.bind,
        "starting rpc-latency-monitor"
    );

    spawn_reference_poller(&config, &reference)?;
    let claims = reference_check::spawn_claim_checker(
        client.clone(),
        metrics.clone(),
        config.reference_check.clone(),
        reference.clone(),
        config.claim_checks,
    );
    let archival_anchor = ArchivalAnchor::default();
    scheduler::spawn_checks(
        &endpoints,
        &config.checks,
        client.clone(),
        metrics.clone(),
        reference.clone(),
        config.max_slot_lag,
        claims,
        config.gpa_targets.clone(),
        gpa_derive,
        archival_anchor.clone(),
    );
    reference_check::spawn_reference_check(
        &endpoints,
        client.clone(),
        metrics.clone(),
        config.reference_check.clone(),
        config.claim_checks,
    );
    let archival_client = Arc::new(RpcClient::new(config.archival_timeout)?);
    reference_check::spawn_archival_check(
        &endpoints,
        archival_client,
        metrics.clone(),
        reference,
        config.archival_interval,
        archival_anchor,
    );

    server::serve(config.server.bind, metrics).await
}

fn spawn_reference_poller(config: &Config, reference: &ReferenceSlot) -> anyhow::Result<()> {
    if config.reference_slot.source != ReferenceSource::Endpoint {
        return Ok(());
    }
    let Some(endpoint) = config.reference_slot.endpoint.clone() else {
        return Ok(());
    };
    let client = RpcClient::new(config.request_timeout)?;
    let interval = config.reference_slot.poll_interval;
    let reference = reference.clone();
    tokio::spawn(poll_reference_endpoint(
        client, endpoint, interval, reference,
    ));
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
