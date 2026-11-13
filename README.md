# rpc-latency-monitor

Neutral, read-only latency monitoring for Solana infrastructure providers, run by the Solana
Foundation.

| Component | What it measures | Docs |
| --- | --- | --- |
| [`rpc-monitor/`](./rpc-monitor) | JSON-RPC read latency, slot lag and errors per provider and region | [README](./rpc-monitor/README.md) |

Each component is its own crate in the root Cargo workspace and has its own `deploy/` tree, so
components build, release and roll out independently.

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Licensed under the Apache License, Version 2.0. See [LICENSE](./LICENSE).
