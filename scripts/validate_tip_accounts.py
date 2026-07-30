#!/usr/bin/env python3
"""Validate tip-accounts.json for the sender leaderboard registry.

Usage: validate_tip_accounts.py [--base BASE_FILE] [--rpc URL] [FILE]

With --base, entries present in FILE but not in BASE_FILE are treated as new:
each must have on-chain history, and unsigned new entries are reported so a
maintainer can verify ownership manually before merging.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.request

import base58
from nacl.exceptions import BadSignatureError
from nacl.signing import VerifyKey

PROVIDER_RE = re.compile(r"^[a-z0-9_-]{2,32}$")
DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
CLAIM_TEMPLATE = "solana-tip-account:{provider}:{address}"

errors: list[str] = []
warnings: list[str] = []


def offchain_envelope(msg: bytes) -> bytes:
    return b"\xffsolana offchain" + b"\x00\x00" + len(msg).to_bytes(2, "little") + msg


def verify_signature(entry: dict) -> bool:
    claim = CLAIM_TEMPLATE.format(**entry).encode()
    key = VerifyKey(base58.b58decode(entry["address"]))
    sig = base58.b58decode(entry["signature"])
    for message in (offchain_envelope(claim), claim):
        try:
            key.verify(message, sig)
            return True
        except BadSignatureError:
            continue
    return False


def has_onchain_history(address: str, rpc: str) -> bool:
    body = json.dumps({
        "jsonrpc": "2.0", "id": 1,
        "method": "getSignaturesForAddress",
        "params": [address, {"limit": 1}],
    }).encode()
    req = urllib.request.Request(rpc, data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=15) as resp:
        result = json.load(resp).get("result")
    return bool(result)


def load_entries(path: str) -> list[dict]:
    with open(path) as f:
        doc = json.load(f)
    accounts = doc.get("accounts")
    if not isinstance(accounts, list):
        errors.append(f"{path}: top-level 'accounts' array missing")
        return []
    return accounts


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("file", nargs="?", default="tip-accounts.json")
    parser.add_argument("--base", help="registry file from the base branch; enables new-entry checks")
    parser.add_argument("--rpc", default="https://api.mainnet-beta.solana.com")
    args = parser.parse_args()

    entries = load_entries(args.file)
    seen: dict[str, str] = {}
    for i, e in enumerate(entries):
        where = f"accounts[{i}]"
        if not isinstance(e, dict) or not {"address", "provider", "added"} <= e.keys():
            errors.append(f"{where}: must have address, provider, added")
            continue
        unknown = set(e) - {"address", "provider", "added", "signature"}
        if unknown:
            errors.append(f"{where}: unknown fields {sorted(unknown)}")
        addr, prov = e["address"], e["provider"]
        try:
            if len(base58.b58decode(addr)) != 32:
                errors.append(f"{where}: {addr} does not decode to 32 bytes")
        except ValueError:
            errors.append(f"{where}: {addr} is not valid base58")
        if not PROVIDER_RE.match(prov):
            errors.append(f"{where}: bad provider slug {prov!r}")
        if not DATE_RE.match(e["added"]):
            errors.append(f"{where}: bad added date {e['added']!r}")
        if addr in seen:
            errors.append(f"{where}: {addr} already claimed by {seen[addr]!r}")
        seen[addr] = prov
        if "signature" in e:
            try:
                if not verify_signature(e):
                    errors.append(f"{where}: signature does not verify for {addr}")
            except Exception as exc:
                errors.append(f"{where}: signature check failed for {addr}: {exc}")

    ordered = [(e.get("provider", ""), e.get("address", "")) for e in entries]
    if ordered != sorted(ordered):
        errors.append("accounts must be sorted by (provider, address) to keep diffs reviewable")

    if args.base:
        base_addrs = {e["address"] for e in load_entries(args.base)}
        new = [e for e in entries if isinstance(e, dict) and e.get("address") not in base_addrs]
        print(f"{len(new)} new entr{'y' if len(new) == 1 else 'ies'} vs base")
        for e in new:
            if "signature" not in e:
                warnings.append(
                    f"UNSIGNED new entry {e['address']} ({e['provider']}) — "
                    "maintainer must verify ownership (treasury sweep linkage) before merge"
                )
            try:
                if not has_onchain_history(e["address"], args.rpc):
                    errors.append(f"new entry {e['address']} has no on-chain history")
            except Exception as exc:
                warnings.append(f"could not check on-chain history for {e['address']}: {exc}")

    for w in warnings:
        print(f"WARNING: {w}")
    for err in errors:
        print(f"ERROR: {err}")
    print(f"{len(entries)} entries, {len(errors)} errors, {len(warnings)} warnings")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
