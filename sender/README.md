# Sender providers: adding yourself to the tip leaderboard

`sender/tip-accounts.json` is the public registry mapping Solana tip accounts to
the sender/landing provider that operates them. The sender leaderboard
attributes a transaction's tip to your provider only when the tip lands on an
address listed here — **unlisted addresses mean undercounted volume for you**.
Attribution is computed from raw on-chain data; this file is only the
address→provider labeling. Nobody needs our permission to be measured
accurately: open a pull request.

## Quick start

1. Pick your provider slug (lowercase, e.g. `temporal`, `nextblock`). Check
   the existing entries — if you're already listed, use the same slug.
2. For each tip address, prove you control it by signing the claim with that
   address's own key:

   ```
   solana sign-offchain-message "solana-tip-account:<your-slug>:<address>"
   ```

3. Add one entry per address to the `accounts` array in
   `tip-accounts.json`, keeping the array sorted by `(provider, address)`:

   ```json
   {
     "address": "yourTipAccountBase58Address",
     "provider": "your-slug",
     "added": "2026-08-15",
     "signature": "<base58 output from step 2>"
   }
   ```

4. Open the PR. CI validates everything automatically; once merged, the
   pipeline picks your addresses up within about an hour and tips to them
   start counting toward your leaderboard row.

New providers: mention your product docs/site in the PR description so we can
link it. New addresses count **from merge time** — history is not
retroactively re-attributed, so list your addresses before they take volume,
and keep rotations up to date.

## Rules

- One entry per address. An address can belong to exactly one provider; CI
  rejects claims on addresses another provider already holds.
- Do **not** list treasury/consolidation addresses that receive sweeps from
  your tip accounts — tips are counted at the address the user pays, and
  listing both would double count.
- Changing the `provider` of an already-listed address always requires a
  signature — CI rejects unsigned reassignments outright, since that would
  redirect existing attribution. Legitimate unsigned relabels are
  maintainer-only migrations.

## If you cannot sign (cold key, HSM policy)

Omit `signature`. CI flags the entry and a maintainer verifies ownership
out-of-band (on-chain sweep linkage to your known treasury) before merging —
expect this path to be slower than the signed one.

## What CI checks

- valid base58 32-byte addresses, no duplicates, no cross-provider claims
- signatures verify against the claimed address (both the
  `solana sign-offchain-message` envelope and a raw ed25519 signature over the
  claim string are accepted)
- new addresses have on-chain history
- the file stays sorted by `(provider, address)` so diffs are reviewable

Validator: [`scripts/validate_tip_accounts.py`](../scripts/validate_tip_accounts.py)
— run it locally with `python scripts/validate_tip_accounts.py` before pushing.

## Removing addresses

Rotated-out addresses can stay listed (they simply stop receiving tips) or be
removed by PR. Removal does not delete past attribution.

## Consumers

The analytics pipeline syncs merged changes into the `tip_accounts` ClickHouse
table hourly. Third parties are welcome to consume this file — it is the
authoritative registry for the leaderboard, with the signatures providing
per-address proof of ownership.
