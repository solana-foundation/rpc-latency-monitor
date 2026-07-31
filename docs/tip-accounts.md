# Sender tip-account registry

`tip-accounts.json` is the public registry mapping Solana tip accounts to the
sender/landing provider that operates them. The sender leaderboard attributes
a transaction's tip to your provider only when the tip lands on an address
listed here — unlisted addresses mean undercounted volume for you. Attribution
is computed from raw on-chain data; this file is only the address→provider
labeling.

New addresses count from the day they are merged and synced (within about an
hour of merge). History is not retroactively re-attributed.

## Adding or rotating your addresses

Open a pull request that edits `tip-accounts.json`:

```json
{
  "address": "yourTipAccountBase58Address",
  "provider": "your-provider-slug",
  "added": "2026-08-15",
  "signature": "base58 signature, see below"
}
```

- `provider` is your slug as shown on the leaderboard (lowercase; check the
  existing entries). New providers: pick a slug and mention your docs/site in
  the PR so we can link the product.
- Keep the `accounts` array sorted by `(provider, address)` — CI enforces it.
- One entry per address. Do not list treasury/consolidation addresses that
  receive sweeps from your tip accounts; tips are counted at the address the
  user pays, and listing both would double count.

### Proving ownership

Tip accounts are provider-controlled keys, so prove each claim by signing it
with the address's own key:

```
solana sign-offchain-message "solana-tip-account:<provider>:<address>"
```

Put the printed base58 signature in the entry's `signature` field. CI verifies
it (both the Solana offchain-message envelope and a raw ed25519 signature over
the same string are accepted).

If you cannot sign (cold key, HSM policy), omit `signature`. CI flags the
entry and a maintainer verifies ownership out-of-band (on-chain sweep linkage
to your known treasury) before merging — expect this path to be slower.

Changing the `provider` of an address already in the registry always requires
a signature — CI rejects unsigned reassignments outright, since that would
redirect existing attribution. Legitimate unsigned relabels are done by
maintainers directly.

### What CI checks

- valid base58 32-byte addresses, no duplicates, no address already claimed by
  another provider
- signatures verify against the claimed address
- new addresses have on-chain history
- file stays sorted

## Removing addresses

Rotated-out addresses can stay listed (they simply stop receiving tips) or be
removed by PR. Removal does not delete past attribution.

## Consumers

The analytics pipeline syncs merged changes into the `tip_accounts` ClickHouse
table on a schedule. Third parties are welcome to consume this file — it is
the authoritative registry for the leaderboard at face value, with the
signatures providing per-address proof of ownership.
