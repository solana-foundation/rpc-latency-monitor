# Error kinds

Every non-success probe outcome carries an `error_kind` label on
`rpc_requests_total`. The table below is the canonical mapping from label value
to human-readable copy — UIs (dashboards, solana.com/data tooltips) should
render these descriptions on hover rather than the raw slug.

Outcomes with `status="skipped"` are neutral: they count neither for nor
against a provider and are excluded from success/failure rates.

| `error_kind` | Status | Label | Description |
|---|---|---|---|
| `timeout` | error | Timeout | No response within the per-check deadline (default 10s). |
| `transport` | error | Connection failed | DNS, TLS, or connection-level failure before an HTTP response. |
| `http_5xx` | error | Server error | Provider returned HTTP 500–599. |
| `http_429` | skipped | Rate limited | Provider returned HTTP 429. Treated as neutral: rate limits reflect the monitor's key/plan configuration, not serving quality. |
| `http_4xx` | skipped | Client error | Provider returned another HTTP 4xx. Treated as neutral: indicates a request/credential problem on the monitor's side. |
| `rpc_block_unavailable` | error | Block unavailable | JSON-RPC -32004: the requested block is not available on this node. |
| `rpc_node_unhealthy` | error | Node behind | JSON-RPC -32005: the node is unhealthy / behind the cluster. |
| `rpc_slot_skipped` | error | Slot skipped | JSON-RPC -32007/-32009 on a method that should not hit skipped slots. (On fixed-slot block probes this is `status="skipped"` and neutral — a skipped slot is a chain event, not a provider fault.) |
| `rpc_tx_history_unavailable` | error | History unavailable | JSON-RPC -32011: transaction history is not available from this node. |
| `rpc_method_not_found` | error | Method not supported | JSON-RPC -32601: the provider does not support this method. |
| `rpc_invalid_params` | error | Invalid params | JSON-RPC -32602: the provider rejected the request parameters. |
| `rpc_error` | error | RPC error | Any other JSON-RPC error object. |
| `decode` | error | Malformed response | HTTP 200 but the body is not valid JSON-RPC. |
| `empty` | error | Empty result | HTTP 200 but the payload fails content validation — a null account, a zero-transaction block, an empty account or signature list. A non-answer cannot win on latency. |
| `stale` | error | Stale data | The response's slot trails the reference tip by more than `max_slot_lag` (default 30 slots ≈ 12s). |

Suggested tooltip map for the web UI:

```ts
export const ERROR_KIND_LABELS: Record<string, { label: string; description: string }> = {
  timeout: { label: "Timeout", description: "No response within the 10s deadline" },
  transport: { label: "Connection failed", description: "DNS, TLS, or connection-level failure" },
  http_5xx: { label: "Server error", description: "HTTP 500–599 from the provider" },
  http_429: { label: "Rate limited", description: "HTTP 429 — neutral, reflects key/plan limits" },
  http_4xx: { label: "Client error", description: "HTTP 4xx — neutral, monitor-side request issue" },
  rpc_block_unavailable: { label: "Block unavailable", description: "Node does not have the requested block (-32004)" },
  rpc_node_unhealthy: { label: "Node behind", description: "Node is unhealthy or behind the cluster (-32005)" },
  rpc_slot_skipped: { label: "Slot skipped", description: "Skipped slot on a method that should not hit one (-32007/-32009)" },
  rpc_tx_history_unavailable: { label: "History unavailable", description: "Transaction history not available (-32011)" },
  rpc_method_not_found: { label: "Method not supported", description: "Provider does not support this method (-32601)" },
  rpc_invalid_params: { label: "Invalid params", description: "Provider rejected the request parameters (-32602)" },
  rpc_error: { label: "RPC error", description: "Other JSON-RPC error" },
  decode: { label: "Malformed response", description: "HTTP 200 but not valid JSON-RPC" },
  empty: { label: "Empty result", description: "HTTP 200 but no usable data — counted as failure" },
  stale: { label: "Stale data", description: "Response slot trails the chain tip beyond budget" },
};
```
