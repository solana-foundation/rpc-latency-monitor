# Sender / Landing Latency Measurement — Proposal

## Goal

Extend the monitor to measure, per landing provider per region, how fast and reliably a submitted
transaction **lands** on-chain — without the measurement itself becoming expensive. Read-RPC latency is
free to probe; landing probes cost real SOL (base fee + priority fee + tip), so cost control is the
central design constraint.

Assumed scope: **5 landing providers** (e.g. Jito, Helius Sender, bloXroute, Astralane, Temporal —
config-driven; Nozomi is an easy add). Each has a different submit endpoint and tip/fee model.

## What we measure

Per probe: build a tiny transaction from a funded probe keypair (a 0-lamport self-transfer + memo so the
signature is traceable), attach a compute-budget priority fee and the provider's tip, submit it through
the provider, then detect when it lands.

- `sender_land_latency_seconds` — histogram `{provider, region, outcome}` (submit → first `confirmed`).
- `sender_landed_total` / `sender_dropped_total` — counters `{provider, region}`.
- `sender_land_slots` — histogram `{provider, region}` (landed slot − submit slot).
- `sender_spend_lamports_total` — counter `{provider, region}` (cost tracking).
- `sender_budget_remaining_lamports` — gauge `{provider, region}`.

Landing detection reuses our own RPC (Helius/public): poll `getSignatureStatuses` every ~2s up to a
timeout; first `confirmed` → landed (record latency + slot); timeout → dropped. No paid calls.

## Cost drivers

Per probe cost = **base fee (5,000)** + **priority fee** + **tip**. Base + a modest priority fee is
~10,000 lamports. The **tip** dominates and is provider/contention-dependent (Jito-style tips range
~1k–100k+ lamports). bloXroute/others use subscription or per-tx fees rather than SOL tips, so the
SOL-tip model below is the worst case.

## Cost model (5 providers; $150/SOL; base+priority = 10,000 lamports)

`cost/day = providers × regions × probes_per_day × (10,000 + tip)` lamports.

| Scenario | Cadence | Regions | Tip (lamports) | SOL/month | ≈ USD/month |
|---|---|---|---|---|---|
| **A (recommended)** | 5 min | 3 | 10,000 | 2.6 | **~$390** |
| B | 5 min | 3 | 100,000 | 14.3 | ~$2,140 |
| C | 1 min | 7 | 50,000 | 90.7 | ~$13,600 |
| D | 15 min | 3 | 10,000 | 0.9 | ~$130 |

The three levers — **cadence, region count, tip** — swing cost ~30×. Continuous high-tip probing in all
7 regions (C) blows past a $10k cap; modest sampling (A/D) is well under $500/mo.

## Cost-control levers (the design)

1. **Low cadence** — default one probe per provider per region every **5 minutes** (landing performance
   moves slowly; 5-min sampling is plenty for a dashboard). Per-provider override.
2. **Modest fixed tip** for the standard probe (default 10,000 lamports) — measures the realistic
   "modest-tip" trader scenario rather than worst-case bidding.
3. **Occasional tip-ladder sweep** instead of constant high tips: hourly, run one probe at a few tip
   levels (e.g. 1k / 10k / 100k) to chart the landing-vs-tip curve traders care about — captures
   tip-sensitivity at a tiny fraction of the cost of high-tipping every probe.
4. **Hard budget guard** — a configurable `max_lamports_per_day` per provider×region. A running spend
   counter stops sending when the cap is hit and emits `sender_budget_remaining_lamports = 0`. This is a
   real ceiling, mapping directly to the program's cap discussion.
5. **Tiny transactions** — 0-lamport self-transfer + memo, minimal compute units, so only the tip is a
   meaningful cost.
6. **One in-flight probe per provider** — never resend while a prior probe is pending; avoids runaway
   spend during contention/outages.
7. **Sender regions ⊆ RPC regions** — landing depends mostly on the leader schedule, not client region,
   so senders default to a 3-region subset (us-east, eu, sgp) while read-RPC runs all 7. Configurable.
8. **Reuse read RPC for landing detection** — `getSignatureStatuses` polling against our existing
   endpoints, so detection adds no provider cost.

## Recommended default

Scenario A + budget guard: 5 landing providers, 3 regions, 5-min cadence, 10,000-lamport tip, hourly
tip-ladder, `max_lamports_per_day` per provider×region sized to keep the fleet under ~$500/month. One
funded probe keypair per region with a low-balance alert.

## Code structure (scaffolded separately for review)

- `sender::config` — providers, per-provider tip model + submit endpoint, cadence, budget caps, keypair.
- `sender::budget` — atomic per-provider×region lamport spend vs daily cap (the guard).
- `sender::landing` — `getSignatureStatuses` polling → landed (latency, slot) / dropped.
- `sender::probe` — the loop: build+sign+submit, detect landing, record metrics, decrement budget.
- `sender::metrics` — the metrics above.

Transaction building/signing and the per-provider submit adapters require `solana-sdk` + a funded
keypair and real spend to validate, so they land as a reviewed increment after this proposal is agreed.
