# Shred benchmark: provider feeds + leader release stats

**Status:** proposal — needs sign-off (Brian). Decision doc, no code exists.
**Origin:** alessandrod asked (X, 2026-08-23) for a full-block reconstruction latency benchmark for shred providers "like the RPC ones", published on solana.com/data. Austin Federa (DoubleZero) confirmed some leaders sell early shred release — legal per protocol, per-destination selective, and it breaks naive arrival benchmarking. DoubleZero has the data but won't publish ("validators' data"). Community (Cloakd, jasper) wants a neutral publisher. Neutral measurement is this program's mandate; nobody else is positioned to do it.

## What we'd measure

**Phase 1 — provider feeds (the alessandrod ask).** Full-block reconstruction latency per shred provider, from our own vantage points.

- 3 metal boxes to start (tsw/latitude: ewr2, fra2, ams — unmetered NICs; each provider stream is ~30–120 Mbps).
- One UDP port per provider. Jito ShredStream = auth + heartbeat; others allowlist our static IPs.
- Kernel `SO_TIMESTAMPING` arrival stamps per (slot, FEC set, index); independent FEC reconstruction per provider → `t_complete(provider, slot)`.
- Headline metric: `t_complete(provider) − t_complete(turbine baseline)` (baseline via rpc-node-eu or a raw Turbine listener on the same box). Plus completeness % and per-slot winner → win%.
- All deltas are same-box, same-clock (the shredwatch trick) — no clock sync problem.
- Integrity: leader-sig verification per shred; reconstructed block checked against the reference node (same DNA as our RPC claim checks).
- Publishing rides the existing pipeline: Prometheus → Grafana → raw-api samples → solana.com/data.

**Phase 2 — per-leader release stats (the Austin/Cloakd data; politically hot — touches stake flows).**

- Join slots to leaders. An origin-delay fingerprint = a provider feed beating Turbine by an anomalous margin *only* on specific leaders' slots, replicated across vantages and weeks.
- Publish distributions, never imputed intent.

## Gaming model — honest limits

- **Selective delay (Austin's case):** a leader releasing early to a buyer and delaying Turbine shows up in phase 2 as the fingerprint above. Detectable when the discrimination is per-destination and we're on the fast or slow side of it.
- **Tier-scoped measurement:** we measure *our* subscription tier of each provider. We cannot see discrimination between other subscribers. Mitigation: rotating / anonymously-purchased subscriptions where terms allow.
- **The blind spot:** a leader delaying *everyone* equally is indistinguishable from being slow relative to location norms. We state this in the methodology rather than pretend otherwise.

## Relation to IBRL

ibrl.wtf is complementary, not overlapping: it scores block-production quality from ledger/slot data (packing, slot timing). Shred distribution facts never touch the chain, so nobody can derive them from the ledger — they have to be measured. IBRL is also useful political precedent for publishing named per-validator scores, and a potential co-publisher.

## Prior art

- schmiatz/shredwatch — multi-source relative timing, Jito direct-mode implementation.
- Astralane/shred-tools (shreds-monitor) — FEC reassembly + leader-sig verify + ClickHouse; provider-authored, reference only.

## Asks / blockers

1. **Sign-off** — provider list overlaps sender relationships; paid feed subscriptions on customer terms.
2. **Provider subscriptions** — one paid/authorized shred feed per provider on the 3 boxes.
3. **Bandwidth budget** — N providers × ~30–120 Mbps × 3 boxes; metal NICs are unmetered but the budget should be explicit.
4. **Methodology publication** — with the gaming caveats above, before first data goes public.

## Sizing

~2–3 weeks to a credible v1 (phase 1 on 3 boxes, published methodology). Phase 2 is analysis on top of the same capture — no new infra, but it should not ship until the methodology and the political framing are agreed.
