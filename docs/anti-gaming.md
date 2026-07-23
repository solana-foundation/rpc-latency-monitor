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

## The threat model at a glance

| What a provider might try | What stops it | Layer |
|---|---|---|
| Answer fast with empty, null, or truncated payloads | response validation — empty can't win | 0 |
| Serve cached or precomputed answers to known queries | every target moves: rotating accounts, fresh signatures, sliding block slots | 3 |
| Special-case the one heavy gPA query it knows we send | curated target rotation, plus derived targets whose memcmp anchors change every interval | 3 |
| Stamp a far-future slot to poison the tip and make honest rivals look stale | tip anchored on the reference node; providers can raise it, never lower it | 1 |
| Stamp fresh slot numbers on stale or fabricated content | claim verification: blockhashes, pubkeys, signatures, clock timestamps checked against the node after settling | 2 |
| Pre-seed a canned answer for the archival probe | one random, never-repeated deep slot per round; majority blockhash across providers is truth | archival |
| Recognize benchmark traffic and route it to premium hardware | **not prevented** — see [Known limits](#known-limits) | — |

### Why a dedicated reference node, and not the providers themselves?

The obvious cheaper alternative is to cross-check providers against each other —
take the majority answer as truth, flag the outlier — and run no node at all. We
don't, for two reasons that a majority vote cannot fix:

- **Circularity / poisoning.** If truth is derived from the set being graded, a
  colluding or simply majority-wrong set *becomes* the definition of truth and the
  one honest provider is flagged as the liar. With a handful of measured providers,
  "majority" is a thin margin; a single well-placed lie can move it. The reference
  node is an anchor *outside* the measured set precisely so no provider's answer can
  define the baseline it is judged against. (This is the same reasoning that moved
  the chain tip off `max_observed` in Layer 1.)
- **Timing skew on fresh data.** Providers are polled at slightly different instants
  and legitimately sit at different slots — that difference *is* the latency we
  measure. So at any wall-clock moment there is no single "correct" recent blockhash
  to vote on; consensus on fast-moving data manufactures disagreement between honest
  providers. The reference node sidesteps this by verifying a *settled* claim (well
  after the fact) against absolute truth, which a vote has no clean equivalent for.

The **one exception is archival data**, which is immutable and old: every honest
provider must return the byte-identical blockhash for a given ancient slot, with no
timing skew and no poisoning subtlety. There, cross-provider agreement is enough on
its own — and it is what we use, precisely because the two problems above don't
apply. Our reference node can't help here anyway (it runs a limited ledger and has
purged ~40M-slots-ago history), and designating one provider's archival tier as
truth would let a competitor grade its rivals and never be checked itself. So
archival is verified by matching the providers against each other. See the archival
round below.

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
| `getProgramAccounts`, `getTokenAccountsByOwner` | context slot, account count, 3 sampled (pubkey, data), the probed target | samples must exist on the node and satisfy the target's own filters (program owner, dataSize, memcmp); count within `claim_count_tolerance` (default 8) of the node's own count for that target | real pubkeys cannot be invented; the set size is checkable |
| `getTransaction` (recent), `getSignaturesForAddress` | (signature, slot) | the node must know that signature at that slot | transaction signatures cannot be invented |
| `getAccountInfo` (clock sysvar) | slot, unix_timestamp | timestamp must be within 2 minutes of wall time | replayed old clock data carries an old timestamp |
| any slot-bearing method | observed slot | must not exceed the node's time-projected tip by more than `claim_margin` (default 16 slots) | a claim about the future is physically impossible |

### The archival round (cross-provider, no reference node)

`getBlock_archival` and `getTransaction_archival` reach ~40M slots back — history
the reference node has purged — so they are not claim-verified against it. Instead
a coordinated round runs every `archival_interval` (default 30s):

1. Pick **one random slot** from a deep range (`≥ tip − 40M`) that has **never been
   used before** — kept in an in-process set so no slot is queried twice.
2. Query **every provider** for that exact slot's block (timed, so archival latency
   is still measured), and take the **majority blockhash** across them as truth
   (quorum ≥ 2).
3. A provider whose blockhash matches the majority is `match`; a *different*
   blockhash is `mismatch`; no block returned is `skipped` (it may simply not retain
   that depth — never penalized). A signature from the truth block is then looked up
   on each provider and its slot cross-checked the same way.

Because the slot is **random and never repeated**, a provider cannot pre-seed a
canned `slot → blockhash` answer and skip the actual deep read — it can't predict
which ancient slot the next round asks for. And because archival data is immutable,
cross-provider agreement is unambiguous truth (no timing skew, no fork), so no
external archival endpoint is needed and no single provider grades the others.
Results land in the same `rpc_claim_check_total{provider, method, result}` counter.

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
  node stale; or, for the archival round, the provider returned no block for the
  slot — it may not retain that depth, or the slot was skipped and no quorum
  formed). Never counted against a provider.

## Layer 3 — unpredictable targets (always on; derivation opt-in)

Layers 0–2 catch a lie after it is told. This layer removes the ability to prepare
the lie — or the cache — in advance, by making the *question* unpredictable. A
provider can only look good on these probes by being fast at real, unprepared work.

Always on, per method:

- **Account reads** (`getAccountInfo` batch pool, `getMultipleAccounts`) rotate
  over real accounts observed in recent blocks; the batch is re-drawn each cycle
  (anchored by one permanent account so a since-closed ephemeral account can't
  register as a provider failure).
- **`getBlock_recent`** probes a moving slot a fixed depth behind the live tip.
- **`getTransaction_recent`** chases a signature freshly surfaced by
  `getSignaturesForAddress` — it did not exist minutes earlier.
- **Archival probes** use a random, never-repeated slot (see the archival round).
- **Every request** carries `Cache-Control: no-cache` / `Pragma: no-cache`.

For `getProgramAccounts` there are two tiers:

1. **Curated targets** (`gpa_targets`): a static, config-driven list of real heavy
   query shapes. Static on purpose — long-lived, comparable time series, and claim
   verification re-runs the exact filters on the reference node. The trade-off is
   disclosed: a static query is knowable, so a determined provider could
   special-case it.
2. **Derived targets** (`gpa_derive`, opt-in): closes that gap. Every interval
   (default 5m), token accounts observed in recent blocks are unpacked into two
   memcmp anchors — the account's **mint** (holders-by-mint, offset 0) and its
   **owner** (accounts-by-owner, offset 32) — and each candidate query is accepted
   only after a **count-only preflight** (zero-length `dataSlice`) against the
   derive endpoint returns between `min_accounts` and `max_accounts` matches
   (default 5–200). Accepted targets join the rotation under the reserved names
   `derived_token_by_mint` / `derived_token_by_owner` — constant metric labels,
   rotating anchor underneath.

   Why each piece exists:
   - **Anchors come from live chain activity minutes old.** To pre-compute every
     possible derived query, a provider would have to keep a fresh index over all
     mints and owners appearing on chain — which is simply *being a good RPC
     provider*. There is no shortcut that isn't the honest work.
   - **The count bounds keep the probe honest in both directions.** The cap keeps
     result sizes bounded so latency stays comparable across rounds and providers;
     the floor means a single account closing between derivation and probe cannot
     empty the result set (an empty result is scored a failure — the floor
     prevents that false accusation).
   - **The preflight never touches a provider.** It runs against the reference
     node (or another non-benchmarked RPC), so the upcoming target is never
     leaked to anyone being measured, and no provider pays for our derivation.
   - **High-volume mints (USDC, USDT, wSOL) are skipped** as mint anchors — they
     hold millions of accounts and would always fail the cap; skipping them saves
     the derive endpoint guaranteed-futile scans.
   - **Derived targets are validated live, not claim-verified.** Every returned
     account is checked at probe time against the request's own filters (layer 0),
     but no layer-2 claim is generated for derived targets. The reason is the
     doc's own rule: a `mismatch` must have no innocent explanation — and derived
     anchors are *recently active* accounts whose sets legitimately churn (token
     accounts open and close) between the response and verification ≥32 slots
     later, so a delayed count/sample check would misread churn as fabrication.
     (The first fleet deployment produced exactly such false mismatches within
     the hour.) Layer-2 verification for gPA stays on the curated targets, whose
     stable sets make it sound.

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

## Known limits

Being explicit about what these layers do **not** stop is part of being auditable:

- **Traffic recognition and preferential routing.** Our probes are identifiable
  in principle: each region has stable egress IPs, every request carries no-cache
  headers, and the cadence is regular. A provider could recognize that traffic
  and route it to its best hardware or an uncontended path. No content check can
  catch this — the answers on that path are genuinely correct and genuinely fast.
  What the layers guarantee is narrower and still worth stating: a provider
  cannot look good by *lying*, only by actually serving our requests well. The
  residual gap between "serves the benchmark well" and "serves everyone well" can
  be probed by occasional runs from fresh egress IPs compared against the
  resident region's numbers; it cannot be closed by verification. (This limit is
  not unique to us — every public RPC benchmark shares it.)
- **Thin archival quorum at small provider counts.** The archival round's
  majority needs at least two agreeing providers. With few providers configured —
  or when only a couple retain a given depth — a split records `skipped`, not a
  verdict. Fail-open by design; the check strengthens as providers join.
- **Derived-target derivation depends on the derive endpoint.** If it is down or
  slow, no *new* derived targets appear; the last accepted ones keep rotating and
  the curated targets are unaffected. Degrades to the static behavior, never
  distorts a measurement.
- **What we can prove is content, not time.** Verification establishes that data
  was real and fresh; latency itself is measured by our own clock on our own
  connection and is not a claim a provider makes — there is nothing for them to
  forge, and nothing for us to verify.

## Policy

No verdict feeds back into scoring, exclusion, or ranking automatically. Results
are published per provider; the deterrent is that a `mismatch` has no innocent
explanation. Providers disputing a verdict can re-derive it from public chain
data: the claim, the slot, and the canonical content are all reproducible.
