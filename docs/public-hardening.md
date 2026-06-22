# Hardening the (future) public dashboard

This document describes how we intend to publish the RPC latency dashboard to the public **without
exposing our datasource, our Grafana Cloud stack, or the monitor's `/metrics` endpoint**, and how to
front the public surface with Cloudflare for caching, WAF, rate limiting, and bot mitigation.

> Status: design + committable config only. **Nothing here flips anything public.** Applying any of
> this requires a deliberate, separate change by an operator (toggling the Grafana share, attaching the
> Cloudflare ruleset to a zone). Until then the dashboard stays private and `/metrics` stays
> localhost-only.

## Threat model / goals

We are an independent measurement party in the VIP Trading Program. The published dashboard is the
product; everything behind it is not. Concretely we want:

1. **Public viewers never touch our datasource.** A read-only viewer must not be able to issue ad-hoc
   queries against our Prometheus / Grafana Cloud datasource, enumerate other dashboards, or reach the
   Grafana org. They should only see the rendered panels we chose to publish.
2. **No credentials on the public path.** No Grafana Cloud Prometheus token, no service-account token,
   no API key is ever reachable from the public URL.
3. **`/metrics` is never public.** The raw Prometheus endpoint stays bound to localhost on every VM and
   is scraped only by the co-located Alloy agent (see [Layer 3](#layer-3-metrics-stays-localhost-only)).
4. **Abuse is bounded.** A scraper or a small flood cannot drive load onto our origin or rack up
   Grafana Cloud usage/billing.

We satisfy these with three layers: a **read-only Grafana publish mode** (no datasource access), a
**Cloudflare edge** in front of the public hostname (cache + WAF + rate limit + bot mitigation), and the
existing **localhost-only `/metrics`** binding.

## Layer 1: publish read-only via Grafana, not via our datasource

Pick **one** of the two Grafana mechanisms below. Both render on Grafana's infrastructure/CDN and never
proxy a public request to our datasource. Do **not** publish by making the Grafana org/folder public or
by handing out a viewer login.

### Option A (preferred): Grafana Cloud public dashboard (read-only share)

Grafana's "public dashboard" / external share renders the dashboard for anonymous viewers. Queries run
server-side under a scoped, dashboard-specific context — the anonymous viewer cannot open the datasource,
cannot run arbitrary PromQL, and cannot navigate to other dashboards or Explore.

Setup (operator, one-time, when we decide to go live):

1. Open the dashboard in `https://rpclatency.grafana.net`.
2. Dashboard settings -> **Public dashboard** (a.k.a. external/public share).
3. Enable sharing. Configure:
   - **Time range**: allow the viewer to change it only if we want it; otherwise pin a sensible default
     (e.g. last 6h) to bound query cost.
   - **Annotations**: off (don't leak internal annotations).
   - **Template variables**: disabled for the public share unless a variable is required for the panels.
     Public shares with open variables widen the query surface — keep them off or pinned.
4. Grafana issues a stable public URL with an opaque token, e.g.
   `https://rpclatency.grafana.net/public-dashboards/<opaque-token>`.
5. Point our own friendly hostname at it via Cloudflare (Layer 2) using a redirect or proxied CNAME so
   the public never has to learn the raw Grafana token URL and so we can revoke/rotate centrally.

Hardening notes for Option A:

- **Lock the share to a single dashboard.** Public sharing is per-dashboard; do not enable it on a folder.
- **Scope the datasource.** The dashboard should reference only the Prometheus datasource it needs. Public
  dashboards in Grafana cannot reach Explore or other datasources, but keep the dashboard's queries fixed
  (no editable panels) so there is no query-builder surface.
- **Pin time range + refresh.** A wide-open time range or a fast auto-refresh on a public dashboard
  multiplies backend query volume. Pin both. Let Cloudflare absorb repeat loads (Layer 2).
- **Rotate on leak.** If the token is abused, disable + re-enable the public dashboard to mint a new token;
  the Cloudflare redirect means consumers of our friendly URL are unaffected.

### Option B (fallback): periodic snapshot

If, for any panel, we are not comfortable with live queries running for anonymous viewers, publish a
**snapshot** instead. A Grafana snapshot freezes the *rendered data* into Grafana's snapshot store; viewing
it runs **no** query against our datasource at all — the safest possible posture.

- Create via UI (Share -> Snapshot) or via the HTTP API with a service-account token (kept server-side,
  never exposed): `POST /api/snapshots`.
- Refresh on a schedule (e.g. a cron / scheduled job that re-creates the snapshot every N minutes and
  updates the published pointer). This trades freshness for zero live datasource exposure.
- Snapshots strip series down to the rendered points, so they also avoid leaking high-cardinality label
  sets that a live datasource might expose.

Use Option B for anything sensitive; Option A for the default live dashboard.

### What we explicitly do NOT do

- Do **not** set Grafana org auth to anonymous/public.
- Do **not** create a shared "viewer" login or embed a Grafana API key / service-account token in any page.
- Do **not** expose the Prometheus `remote_write` endpoint, the Grafana Cloud Prometheus query URL, or the
  `GRAFANA_CLOUD_*` tokens (see `grafana/alloy-config.alloy`, `deploy/run-with-doppler.sh`). Those live in
  Doppler and are only present on the VMs.

## Layer 2: front the public hostname with Cloudflare

Put our friendly hostname (e.g. `rpclatency.solana.org`, exact name TBD by ops) on Cloudflare as a
**proxied** record that redirects/rewrites to the Grafana public URL. Cloudflare then gives us cache, a
WAF, rate limiting, and bot mitigation on the public edge — and our origin/Grafana stack only ever sees
Cloudflare's cache-miss traffic.

DNS / routing options (ops choice):

- **Redirect rule** (simplest): a Cloudflare Redirect Rule from `rpclatency.solana.org/*` to the Grafana
  `public-dashboards/<token>` URL (301/302). Cleanest separation; the public never hits our infra at all.
- **Proxied CNAME + transform**: CNAME the hostname to `rpclatency.grafana.net` (orange-cloud / proxied)
  and use a URL-rewrite Transform Rule to map `/` to the public-dashboard path. Lets Cloudflare cache and
  filter in front of Grafana.

Either way, enable on the zone/hostname:

- **Caching**: cache the dashboard HTML/JSON and static assets at the edge with a short TTL (30–60s) so a
  burst of viewers collapses into a single origin fetch. See the cache rule in the committed config.
- **WAF**: Cloudflare Managed Ruleset + our custom ruleset (committed below) — method allow-list, block
  obvious probes, and only allow the dashboard paths.
- **Rate limiting**: per-IP cap so no single client can hammer the public URL (committed below).
- **Bot mitigation**: challenge known-bad / unverified bots while allowing legitimate viewers and good
  bots; turn on Bot Fight Mode (or Super Bot Fight Mode if available on the plan).
- **TLS**: Full (strict). Enable Always Use HTTPS + HSTS.

### Committed config

The custom WAF + rate-limit + cache rules live in
[`deploy/cloudflare/waf-rules.json`](../deploy/cloudflare/waf-rules.json). They are written against
Cloudflare's Rulesets API and are intended to be applied by an operator (or Terraform) to the public
zone — **they are not auto-applied by anything in this repo.** See the apply notes at the top of that
file. A few of the rules reference the public hostname and the dashboard path prefix as placeholders
(`DASHBOARD_HOSTNAME`, `/public-dashboards/`) that ops fills in at apply time.

## Layer 3: `/metrics` stays localhost-only

This is **already the case** and must stay that way — no public path exists to the raw metrics:

- In production (`deploy/gcp/config.yaml`) the server binds `127.0.0.1:9464`, so `/metrics` is reachable
  only from the VM itself.
- The co-located Alloy agent (`grafana/alloy-config.alloy`) scrapes `127.0.0.1:9464` and `remote_write`s
  to Grafana Cloud over an authenticated endpoint. Nothing scrapes the VM from outside.
- No GCP firewall rule opens `9464` (`deploy/gcp/terraform/`), and there is no load balancer or public IP
  mapping for that port.

Guardrails to keep it that way:

- The `0.0.0.0:9464` bind in `config.example.yaml` / `deploy/docker-compose.yaml` is for **local dev** and
  the docker network only. Production must use `127.0.0.1` — do not copy the example bind onto a VM or
  open `9464` in a firewall rule.
- If a future increment needs remote scraping, prefer pushing via Alloy (as today) over exposing
  `/metrics`. If exposure is ever unavoidable, restrict by source range in the GCP firewall and require
  auth — never `0.0.0.0` open to the internet.

## Verification checklist (before/after going live)

- [ ] Public dashboard URL renders panels and **cannot** reach Explore, other dashboards, or the
      datasource settings.
- [ ] No `GRAFANA_CLOUD_*` token, service-account token, or API key appears in any public response or
      page source.
- [ ] `curl https://DASHBOARD_HOSTNAME/api/...` (Grafana API paths) is blocked/redirected by the WAF.
- [ ] `curl http://<vm-external-ip>:9464/metrics` from outside the VM **fails** (connection refused /
      filtered).
- [ ] Rate-limit rule trips under a burst from a single IP (test with a throwaway IP, not production
      traffic).
- [ ] Cache HIT ratio is high for repeat dashboard loads (`cf-cache-status: HIT`).
- [ ] Cloudflare Bot mitigation is enabled on the hostname.

## Related files

- `grafana/dashboard.json` — the dashboard to publish.
- `grafana/alloy-config.alloy` — local scrape + authenticated remote_write (the only egress of metrics).
- `deploy/gcp/config.yaml` — production bind `127.0.0.1:9464`.
- `deploy/cloudflare/waf-rules.json` — Cloudflare WAF / rate-limit / cache rules (apply by ops).
