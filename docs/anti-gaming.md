# Anti-gaming: how responses are verified

The monitor's numbers are only useful if a provider cannot improve them by serving
stale, cached, or fabricated data. This document describes the defenses, what each
one catches, and — just as important — what happens when data is merely *delayed*
rather than dishonest. The design goal throughout: **a lie must be provable, and an
honest-but-slow answer must never be misclassified as a lie.**

All verification runs against the Foundation's own non-voting agave RPC node (the
*reference node*). The reference node never participates in latency scoring; it
supplies only (a) an unpoisonable view of the chain tip and (b) ground truth for
after-the-fact content checks. Both are location-invariant, which is why one node
serves every vantage point.

## Layer 0 — response validation (always on)

A `200 OK` is not a success. Every response must carry the shape and content the
method implies: a block must contain transactions, a `getProgramAccounts` result
must contain accounts that actually satisfy the request's filters, a signature page
must be non-empty. Failures are classified (`timeout`, `transport`, `http_4xx/5xx`,
`rpc_error`, `decode`, `empty`) and excluded from latency and win % — answering
fast with nothing wins nothing.

## Layer 1 — an unpoisonable chain tip (always on)

Staleness is judged against a *tip*: the current head of the chain as seen from the
probing box. Historically the tip was `max_observed` — the highest slot any
provider claimed — which a malicious provider could poison by stamping a
far-future slot, making every honest competitor look stale.

The tip is now sourced from the reference node (`reference_slot.source: endpoint`),
polled every second and combined with local observations by `max()`. Local
observations can only *raise* the tip — a provider that legitimately runs ahead of
the reference node's view (common for well-peered providers far from the node) is
never penalized — but no provider can lower the bar for others or manufacture
staleness. If the node poll fails, the tip degrades to exactly the old
`max_observed` behavior.

A response whose observed slot trails the tip by more than `max_slot_lag`
(default 30 slots ≈ 12 s) is scored `stale` and excluded from latency and win %.

## Layer 2 — claim verification (opt-in: `claim_checks: true` / `MONITOR_CLAIM_CHECKS=true`)

Layer 1 catches lazy staleness. Layer 2 catches deliberate fabrication: a provider
stamping *fresh* slot numbers onto stale or invented data. Slot numbers are
predictable and therefore forgeable; the content below is not.

Every successful probe response yields a *claim* — no extra provider traffic is
generated. Claims settle for `claim_delay_slots` (default 32) to outlive
processed-commitment forks, then are verified against the reference node and
counted in `rpc_claim_check_total{provider, method, target, result}` (`target` is
the gPA probe target name, empty for other methods).

| Method | Claim | Verified against the node | Unforgeable because |
|---|---|---|---|
| `getLatestBlockhash` | (slot S, blockhash B) | B must be the blockhash of some block in `[S−8, S]` | a blockhash is a hash over block contents, unknowable before the block exists |
| `getBlock` (recent) | (slot S, blockhash B) | B must equal the node's blockhash at exactly S | same |
| `getProgramAccounts`, `getTokenAccountsByOwner` | context slot, account count, 3 sampled (pubkey, data), the probed target | samples must exist on the node and satisfy the target's own filters (program owner, dataSize, memcmp); count must be within `claim_count_tolerance` (default 8) of the node's own count for that target | real pubkeys cannot be invented; the set size is checkable |
| `getTransaction` (recent), `getSignaturesForAddress` | (signature, slot) | the node must know that signature at that slot | transaction signatures cannot be invented |
| `getAccountInfo` (clock sysvar) | slot, unix_timestamp | timestamp must be within 2 minutes of wall time | replayed old clock data carries an old timestamp |
| any slot-bearing method | observed slot | must not exceed the node's time-projected tip by more than `claim_margin` (default 16 slots) | a claim about the future is physically impossible |

### Verdicts

- `match` — content verified byte-for-byte (for accounts: including balances).
- `drift` — accounts only: the sampled pubkeys are real and correct, but data
  bytes differ from the node's current state. This is what legitimate on-chain
  movement between response time and verification time looks like. Informational,
  never an accusation.
- `mismatch` — positive evidence of wrong content: a blockhash that belongs to no
  block in the window, a nonexistent pubkey, an account failing the request's own
  filters, a transaction the chain doesn't know at that slot, or an account count
  far from reality.
- `missing` — the claimed content should exist but no block/transaction was found.
- `implausible` — the claim was ahead of physics (future slot, wrong wall clock).
- `skipped` — *we* couldn't verify (reference node unreachable, block unavailable,
  node stale). Never counted against a provider.

## What if the data is delayed, not dishonest?

Delay and dishonesty are handled by different layers, deliberately:

- **Delayed and honestly stamped** (the provider reports the old context slot it
  actually served from): if the delay exceeds `max_slot_lag` the response is
  `stale` — excluded from latency/win %, and no claim is generated. If the delay
  is within budget, its claims verify normally: for account methods the sampled
  pubkeys still exist and still satisfy the filters, and a balance that moved in
  the meantime records `drift`, not `mismatch`. Mild delay never produces a false
  accusation.
- **Delayed but stamped fresh** (the lie): the stale gate is blind to it — that is
  exactly what layer 2 exists for. Blockhash claims mismatch (the old hash does
  not belong to the freshly-claimed slot's window), account counts drift beyond
  tolerance if the set changed, and replayed clock data trips the timestamp gate.
- **Our reference node is the delayed one**: the checker detects it by comparing
  the node tip to the fleet-observed tip. Past `node_stale_slots` (default 64) the
  checker trusts provider values — implausibility is not flagged, and settling
  claims record `skipped` rather than accumulating silently. The lag is exported
  as `rpc_reference_node_lag` for alerting. Latency is untouched in every case:
  the reference path can degrade, never distort.

Ambiguity always resolves toward the provider. A `mismatch` requires positive,
cryptographically-arguable evidence — if any slot in a verification window was
unavailable on our node, the verdict is `skipped`, not `mismatch`, because the
claimed content could belong to the block we couldn't fetch.

## Policy

No verdict feeds back into scoring, exclusion, or ranking automatically. Results
are published per provider; the deterrent is that a `mismatch` has no innocent
explanation. Providers disputing a verdict can re-derive it from public chain
data: the claim, the slot, and the canonical content are all reproducible.
