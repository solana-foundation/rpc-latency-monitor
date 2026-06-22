# Staff Review — PR #14 [PRO-1347] Harden the Public Dashboard (Docs + Cloudflare Config)

**Verdict: approve-with-nits**

This is a docs + committable-config PR. It changes no Rust, no live infra, and applies nothing — every
factual claim it makes about the existing repo checks out, and the security posture it documents is sound
and appropriately conservative (no anonymous org auth, no embedded tokens, `/metrics` stays localhost).
The one item worth fixing before ops leans on this is a WAF/Grafana path conflict that would silently
break the dashboard under one of the two routing options the doc itself recommends.

## Verification performed (claims are accurate)

- `deploy/gcp/config.yaml` binds `127.0.0.1:9464` — confirmed (`server.bind`).
- `config.example.yaml` binds `0.0.0.0:9464`; `deploy/docker-compose.yaml` maps `9464:9464` — confirmed.
- `grafana/alloy-config.alloy` scrapes `127.0.0.1:9464` and `remote_write`s using `GRAFANA_CLOUD_*`
  env vars — confirmed (lines 3, 6, 9–15).
- No `google_compute_firewall` resource opens `9464` in `deploy/gcp/terraform/*.tf` — confirmed (only
  matches are in `terraform.tfstate*` backups, default network; pre-existing, out of scope).
- All "Related files" referenced by the doc exist.
- `deploy/cloudflare/waf-rules.json` parses as valid JSON — confirmed.

## Blocking issues

None.

## Non-blocking suggestions

1. **WAF `/api/` block vs. Grafana public-dashboard data fetch** —
   `deploy/cloudflare/waf-rules.json` (rule "Block Grafana API / admin / auth / datasource paths").
   Grafana public dashboards render client-side and fetch panel data from
   `/api/public/dashboards/<token>/...`. The rule blocks `starts_with(...uri.path, "/api/")`
   unconditionally on the dashboard host. With the **Redirect Rule** routing option this is fine (the
   public never hits Grafana through our host), but with the **proxied-CNAME + Transform** option —
   which `docs/public-hardening.md` (Layer 2) explicitly offers — this rule will break the live
   dashboard. Recommend either: (a) carve out an allow for `/api/public/dashboards/` ahead of the
   block, or (b) call out in the doc/JSON that the broad `/api/` block is only safe with the redirect
   routing option, not the proxied-CNAME option.

2. **`/d/` and `/dashboards` blocks under proxied-CNAME** — same file, same rule. Grafana serves panel
   assets/links under these prefixes; blocking them is correct for "no navigation to other dashboards"
   but, like #1, is only obviously safe under the redirect option. Tie the WAF ruleset explicitly to
   the redirect routing choice, or note which rules to relax if proxying.

3. **Cache rule path prefixes** — `cache_rules` matches `DASHBOARD_PATH_PREFIX` (`/public-dashboards/`),
   `/public/`, `/avatar/`. If the redirect option is used, these paths live on `rpclatency.grafana.net`,
   not on `DASHBOARD_HOSTNAME`, so the cache rule never matches and the "collapse bursts to one origin
   fetch" benefit is lost (the 301/302 is what gets cached instead). Worth a one-line note that edge
   caching of dashboard payloads only applies under the proxied-CNAME option.

4. **Rate limit `mitigation_timeout: 600` with `action: block`** — `rate_limit_rules`. A 10-minute block
   on a per-`ip.src` + `cf.colo.id` key is aggressive for a public dashboard fronted by NATs/shared
   egress (offices, mobile carriers). Consider `managed_challenge` instead of `block`, or a shorter
   timeout, to avoid locking out legitimate shared-IP viewers. Non-blocking since 120 req/60s is a
   reasonable threshold, but flag the lockout duration for ops.

## Nits

- `docs/public-hardening.md`: the verification checklist references
  `curl https://DASHBOARD_HOSTNAME/api/...` to confirm the WAF blocks API paths — good, but it will
  also (correctly) flag the conflict in suggestion #1 if Grafana's own `/api/public/...` is needed.
  A sentence distinguishing "Grafana admin/query API (block)" from "public-dashboard data API
  (must pass under proxied-CNAME)" would make the checklist unambiguous.
- `deploy/cloudflare/waf-rules.json` uses `_about`/`_status`/`_apply` underscore keys as inline docs.
  Fine for a hand-applied/Terraform-`jsondecode` source file; just confirm the apply path strips these
  meta keys (the `_apply` notes already imply per-phase extraction of `*.rules`, so this is only a
  reminder, not a defect).
- README addition is clear and correctly scoped ("None of this flips anything public on its own.").

## Ticket intent

Satisfies PRO-1347 ("harden the public dashboard") as a design + committable-config deliverable. It does
not flip anything public, which matches the stated intent. The three-layer model (read-only Grafana
publish, Cloudflare edge, localhost-only `/metrics`) is complete and the localhost claim is verified
against the actual prod config.
