use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use rpc_latency_monitor::config::{Config, ReferenceSource};
use rpc_latency_monitor::metrics::Metrics;
use rpc_latency_monitor::providers;
use rpc_latency_monitor::reference_slot::{poll_reference_endpoint, ReferenceSlot};
use rpc_latency_monitor::rpc::RpcClient;
use rpc_latency_monitor::{scheduler, server};

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
    let config = Config::load(&args.config)?;
    let endpoints = providers::resolve_endpoints(&config.providers)?;

    let metrics = Metrics::new(&config.region)?;
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
    scheduler::spawn_checks(
        &endpoints,
        &config.checks,
        client,
        metrics.clone(),
        reference,
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
