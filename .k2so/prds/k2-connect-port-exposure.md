---
title: "K2 Connect — Port exposure & wildcard subdomains (Pro tier)"
status: draft
owner: Rosson
created: 2026-06-05
tier: Pro ($7.99/mo)
depends_on: "K2 Connect tunnel (frpc/frps/Caddy), account+billing backend (#618), tunnel monetization roadmap"
related: ".k2so/prds/secure-tunnel-monetization-roadmap.md"
---

# K2 Connect — Port Exposure & Wildcard Subdomains

Turn "you bought `{user}.k2.dev`" into "**you own `*.{user}.k2.dev`**" — a
wildcard slice of k2.dev the user can point at any locally-hosted service.
Effectively **built-in ngrok / Cloudflare-Tunnel**, namespaced to the
subdomain they already own, gated behind a **Pro tier ($7.99/mo)**.

## Motivation

The secure tunnel already carries one service — the K2 daemon — to
`{user}.k2.dev` over 443 (frpc → frps vhost → Caddy TLS). frp is built to
multiplex *many* services over one tunnel; we render only one today
(`crates/k2so-core/src/tunnel/render.rs::render_frpc_toml`). Exposing a
user's local dev servers (`localhost:3000`, `:8080`, …) at public HTTPS
URLs is a small extension of that — and a classic paid capability (ngrok,
Cloudflare Tunnel, Railway, Replit all monetize exactly this).

A developer (or an **agent**) running a dev server gets a real, shareable,
TLS preview URL with one command — without leaving K2.

## Tiers

| | **Free / Base** | **Pro — $7.99/mo** |
|---|---|---|
| Subdomain | `{user}.k2.dev` | `{user}.k2.dev` **+ `*.{user}.k2.dev`** |
| Exposes | the K2 daemon only (remote access) | **any local port** at **any label** |
| Example | `z3thon.k2.dev` → daemon | `testing.z3thon.k2.dev` → `localhost:3000` |
| Infra | existing `*.k2.dev` wildcard cert | per-user `*.{user}.k2.dev` DNS + Caddy on-demand TLS |
| Gate | account exists | **account tier == Pro** (Stripe-backed) |

The base daemon endpoint stays free; the **wildcard** is the Pro unlock.

## User-facing surface

### CLI (primary — dev/agent friendly)
```
k2so expose <port> [--as <label>] [--public | --private]
k2so expose list
k2so unexpose <label>
```
- `k2so expose 3000 --as testing` → prints `https://testing.z3thon.k2.dev`
- `k2so expose 8080 --as api --public` → public shareable preview
- `k2so expose 3000` → auto-labels (e.g. `quiet-fern.z3thon.k2.dev`)
- `k2so unexpose testing` → tears the mapping down

### Settings panel (same daemon route underneath)
An "Exposed ports" list in the K2 Connect section: add a port → get a
copyable URL + a public/private toggle + a status dot. CLI and panel are
two faces of one mechanism.

### Agent self-expose (the differentiator)
Because it's a CLI command, an **agent** can expose its own work: spin up a
dev server on :3000, run `k2so expose 3000 --as preview`, and drop the live
URL into a message to the user or another agent (pairs with the
`msg --command` / messaging plumbing). Self-serve, agent-driven preview
links.

## Mechanics

1. **CLI/panel → daemon route** `POST /cli/tunnel/expose { port, label, visibility }`.
2. **Daemon checks**:
   - account **tier == Pro** (the daemon is signed into the K2 Connect
     account; it reads the tier — never trust a client flag),
   - the **label is valid + free** under the user's namespace (asks the
     control-plane, same canonicalization as the base subdomain claim),
   - **something is actually listening** on `127.0.0.1:{port}` (sanity).
3. **Daemon renders one frpc proxy per mapping** (`{label}.{user}.k2.dev →
   127.0.0.1:{port}`) into the frpc config and **hot-reloads frpc** via its
   admin API (no restart, no disruption to other proxies / the daemon
   tunnel).
4. **Control-plane authorizes** the label under the user's namespace; on
   first Pro mapping it ensures the `*.{user}.k2.dev` DNS + on-demand-TLS
   ask-endpoint will accept it.
5. Returns the public URL.

## Why NOT `{user}.k2.dev:3000` (the naive port-suffix shape)

Rejected — three hard problems:
1. **Cloudflare won't proxy arbitrary ports** (free plan covers a fixed
   port set; `:3000` needs Spectrum or bypassing the proxy).
2. **frps port collisions across tenants** — a raw `:3000` TCP proxy binds
   `:3000` globally; only one user on the whole server could own it →
   forces ugly unique ports + an allocation system.
3. **No automatic TLS** — a raw TCP port bypasses Caddy → plain http + cert
   warnings.
Host-based (subdomain) routing over 443 sidesteps all three.

## Infra (the one real constraint)

**TLS and DNS wildcards match a single label only.** A `*.k2.dev` cert
covers `z3thon.k2.dev` but **NOT** `testing.z3thon.k2.dev`. So nested
wildcards need per-user `*.{user}.k2.dev`:

- **DNS** — on upgrade to Pro, the control-plane provisions a
  `*.{user}.k2.dev` record → Hetzner (one per Pro user; DNS wildcards are
  single-level, so this can't be a zone-wide record).
- **TLS — Caddy On-Demand TLS** (recommended): Caddy mints a cert per
  hostname on first hit, gated by an **ask endpoint** — Caddy asks the
  control-plane "issue a cert for `testing.z3thon.k2.dev`?" → it verifies
  `z3thon` is a real **Pro** account that owns the namespace → yes.
  Unlimited subdomains, zero pre-provisioning. (This is the standard
  multi-tenant pattern — Vercel/Netlify/Railway custom domains.)
  - Alternative: a per-user `*.{user}.k2.dev` cert via DNS-01 — fewer
    certs, more per-user automation. Start with on-demand TLS.
- **frps** — already runs the vhost HTTPS muxer; no change beyond accepting
  the new vhosts.

## Security

- **Opt-in, explicit per mapping** — publishing a local dev server is a
  sharp edge (dev servers often have no auth / chatty debug routes). Clear
  "this is public on the internet" affordance.
- **Visibility per mapping**:
  - **private (default)** — gated behind the K2 Connect login (reuse the
    connect-users session); only the owner's authed users reach it. A
    half-built app isn't wide-open by accident. A real edge over raw ngrok.
  - **public** — a shareable preview link.
- **Lifecycle — auto-expire**: the daemon polls the local port; when it
  stops listening, the proxy is torn down (no dangling public URLs).
  Matches the ephemeral dev-preview mental model. `--persist` opt-out for
  long-lived exposes.
- Tier + label authorization is enforced **server-side** (daemon reads tier
  from the account; control-plane authorizes labels + mints TLS). A client
  can't self-grant Pro.

## Monetization

- **Pro tier: $7.99/mo**, billed via the existing K2 Connect account +
  Stripe backend (#618). The wildcard-expose capability is the headline
  Pro unlock; ties into `secure-tunnel-monetization-roadmap.md`.
- **Gating**: the daemon refuses `expose` (with an upsell message + a link
  to upgrade) when the account tier != Pro. The base daemon subdomain stays
  free. Downgrade → existing wildcard mappings are torn down / suspended
  (control-plane stops authorizing; certs lapse).

## Open-core / licensing note

K2 is MIT (public repo); the K2 Connect control-plane is already a separate
**proprietary** repo (`../k2-connect`) — i.e. K2 Connect is already
open-core. The Pro *enforcement* lives server-side (control-plane authorizes
the wildcard + DNS + TLS only for Pro accounts), so the **client-side**
`expose` code can stay MIT without giving the gate away: a fork can delete
the client tier-check, but the control-plane still says "not Pro" → no DNS,
no cert, no working URL. See the separate licensing discussion for whether
any client module additionally warrants a non-MIT license (likely
unnecessary — the server is the real moat).

## Phasing

- **P1 — daemon multi-proxy + `k2so expose` CLI** (flat MVP): render N frpc
  proxies + hot-reload; CLI surface; daemon `POST /cli/tunnel/expose` with
  the local-listening sanity check. (Public-only first if private-auth
  proxy is heavier.)
- **P2 — control-plane authz + DNS + on-demand TLS**: per-user
  `*.{user}.k2.dev` DNS on upgrade; Caddy on-demand TLS + ask-endpoint;
  label authorization under the namespace.
- **P3 — tier gate + billing**: Pro tier in Stripe; daemon tier-read +
  upsell; downgrade teardown.
- **P4 — UX + lifecycle**: Settings "Exposed ports" panel; private/auth-
  gated visibility; auto-expire on port-close; agent self-expose ergonomics.

## Open questions
- Reserved/blocked labels (e.g. `www`, `api` collisions, abuse handling).
- Abuse / ToS (users hosting arbitrary public content under k2.dev — a
  content-liability surface; needs a ToS + takedown path).
- Rate / bandwidth limits per Pro account (frps + Hetzner egress cost).
- Custom domains (CNAME a user's own domain to their k2.dev expose) — a
  future higher tier.
