# Methodology changelog

Dated record of every change that affects published numbers on
solana.com/data or the public Grafana dashboards. Newest first.

## 2026-09-03 — Latency histogram resolution and scope

- Histogram buckets changed from 17 bounds (1ms floor) to 11 bounds spanning
  200µs–5s. Sub-millisecond buckets fix the artifact where a vantage co-located
  with a provider edge reported a constant 0.5ms p50 (interpolation midpoint of
  the old smallest bucket). Percentile charts show a brief mixed-bucket wobble
  for about an hour after the rollout.
- Latency histograms are now recorded for successful requests only. Failed and
  skipped requests were never part of any published latency number (all
  percentile and average queries already filtered `status="success"`); their
  latency remains available per-request in the raw sample data.
- The `target` label was removed from latency histograms (kept on request
  counters). No published number aggregated latency per target; per-target,
  per-request latency remains available through the raw data API.

## 2026-09-03 — Metal and cloud vantage DNS standardized to ECS-forwarding resolvers

Bare-metal vantages (TeraSwitch, Latitude) moved from the provisioning-default
resolver (1.1.1.1) to Google Public DNS (8.8.8.8/8.8.4.4), which forwards
EDNS Client Subnet. Without ECS, geo-DNS providers routed some probes by the
resolver's location instead of the vantage's, and pooled connections held a
misrouted edge for hours — visible as square-wave latency plateaus (for
example, QuickNode at tsw/lax: 0.3ms baseline with recurring 36–90ms
plateaus). Reported and diagnosed by QuickNode (PR #77); verified
independently against our own data before rollout. AWS probes already use the
default VPC resolver (in-region geolocation) and GCP's resolver path forwards
ECS; neither changed. Affected vantages show a step-change down to their true
baseline from 2026-09-03.
