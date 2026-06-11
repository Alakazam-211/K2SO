# Phase 2.6: Tunnel-provider decision (DEFERRED from Phase 2.5)

**Status**: Deferred from Phase 2.5 Workstream E. Re-opened with **expanded scope** — K2SO is no longer just tunneling the Companion API; it's a tunnel-as-a-service product that lets users expose any localhost port. Phase 2.6 picks up the decision later with a focused re-spike.
**Cycle placement (2026-05-30, Rosson)**: this decision + Phase 3 (K2 Connect) land in the **0.39.X** cycle (NOT 0.40.x — that's the rename/Brain/TTS/Kessel feature set). 0.39.X ships the BYO tier (ngrok + LocalXpose + Cloudflare) and K2 Connect on the pluggable `TunnelProvider` seam; the two pieces explicitly held back are (1) the operated Hosted-backbone service choice and (2) the bifurcated-repo monetization split.
**Date drafted**: 2026-05-23
**Owner**: Rosson + pod-leader
**Blocks**: Phase 3 Workstream B (TLS + auth architecture)
**Does not block**: Phase 2.5 Workstreams A, B, C (build+smoke, test migration, CI sweep) — those proceed without it

---

## tl;dr

Phase 2.5 Workstream D ran a tunnel-provider research subagent and Workstream E was supposed to commit the decision doc. The research returned recommending **Cloudflare Tunnel + K2SO-owned `k2.dev` zone** as the K2SO Hosted middle tier of a three-tier monetization model (Free Tailscale / $2-mo K2SO Hosted / BYO ngrok+LocalXpose).

User then expanded the scope mid-decision (2026-05-23):

> "If we built something custom or used a service that let us use our own domain name we could do something like `<subdomain>.k2.dev` and each user could have their own custom subdomain values that they connect to ($2/mo or something). If they use ngrok then they can have their own custom domain for their own connection points or use one of the default ones that ngrok gives them."

…and then crucially:

> "we need to be able to make it so that when a user 'launches a localhost on port 3000' that the user can then access that demo from their personal computer (somewhere else in the world) by visiting `<subdomain>.k2.dev:3000` (or something like this). We may also want to consider AWS or an even simpler primitive to achieve such a custom end outcome?"

That second requirement turns K2SO from "Companion product with a tunnel" into **"tunnel-as-a-service product where Companion is one feature you can expose."** Different product, different competitive set (now competing with ngrok directly), different pricing power, different infrastructure needs.

Phase 2.5 Workstream D's research was scoped to the narrower "tunnel for Companion" problem. Under the expanded scope, several of its conclusions need revisiting. Phase 2.6 is the dedicated decision phase that picks this up later with a re-spike scoped to the expanded requirement.

---

## What's locked (does not need re-debate)

These were settled during the Phase 2.5 Workstream D research + user feedback and remain valid under the expanded scope:

1. **Three-tier monetization model**:
   - **Free**: Tailscale Funnel — user installs Tailscale, does VPN-style setup, K2SO doesn't operate anything
   - **K2SO Hosted**: $2-?/mo — K2SO operates the backbone + provisions per-user subdomains under K2SO-owned domain
   - **BYO**: ngrok + LocalXpose (with K2SO affiliate link) + Cloudflare for advanced users — K2SO doesn't operate but captures affiliate revenue where possible

2. **K2SO owns `k2.dev`** — domain purchased 2026-05-23. Subdomain provisioning shape (`<user>.k2.dev`, `<user>-<port>.k2.dev`, etc.) is now concrete; backbone implementation will resolve DNS under this zone.

3. **K2SO bearer-token auth end-to-end** — regardless of which tunnel provider is in use, the application-layer auth is K2SO's. Tunnel layer provides transport security (TLS) only. This keeps auth identical across all 5 tunnel providers and across all 3 tiers.

4. **No cert pinning in Phase 3** — would break tunnel provider swaps and Tailscale's automatic cert rotation. Defer to Phase 5+ hardening if ever.

5. **`TunnelProvider` trait + subprocess supervisor in daemon** — pluggable architecture that supports swapping providers without daemon code changes. This is the architectural seam Phase 3 must leave clean even if no Hosted backbone is picked yet.

6. **LocalXpose as BYO affiliate**: confirmed **40% recurring commission**, modest but free revenue (~$48/mo at 100 BYO users × 20% adoption per research). No competing affiliate program exists at ngrok / Cloudflare / Tailscale (all enterprise/MSP-only programs).

7. **Tailscale Funnel free-tier gotcha**: the macOS App Store Tailscale does NOT support Funnel — only the Homebrew/standalone installer does. K2SO docs MUST call this out.

8. **Cloudflare Access dropped from auth model**: free for 50 seats, then $7/user/mo. Use K2SO's own bearer-token model instead. CF Access would force K2SO to manage CF identity in addition to K2SO user accounts.

9. **Tailscale Funnel cannot be the K2SO Hosted backbone**: free tier capped at 3 ports per tailnet, URLs locked to `<device>.<tailnet>.ts.net` (can't use custom k2.dev domain).

---

## What's open under expanded scope

1. **Backbone for the K2SO Hosted middle tier** — Cloudflare Tunnel vs Pangolin (self-hosted CF Tunnel clone) vs FRP self-hosted vs custom Rust tunnel vs AWS-native composition
2. **Per-port URL UX** — `<sub>-3000.k2.dev` (per-port subdomains under the hood) vs `<sub>.k2.dev:3000` (TCP tunneling)
3. **Pricing strategy** — $2/mo for Companion-only made sense, but if K2SO is competing with ngrok at $20/mo Pro for "expose any port," pricing power is much higher. May warrant tier splits (Companion-only / Multi-port / Pro)
4. **AWS-native viability** — not fully evaluated; initial scan suggests AWS isn't simpler or cheaper than Hetzner+self-hosted but warrants a real comparison before locking
5. **Domain decision** — `k2.dev` purchase contingent on rename + trademark research (see open questions)

---

## Snapshot of Phase 2.5 Workstream D research (narrower scope)

For the narrower "tunnel Companion API only" problem, the research subagent (a26cbc26003ae7fcb) returned the following matrix (May 2026 data; verify before final commit):

| Backbone | K2SO eng | K2SO ongoing cost | User friction | Net $/user/mo | Break-even | Notes |
|---|---|---|---|---|---|---|
| **Cloudflare Tunnel + K2SO `k2.dev`** | ~2-3 weeks | ~$6/mo fixed | Low (1-click) | $1.64 (after 18% Stripe) | 4 users | Free tunnels, unmetered egress, 1k-tunnel cap/account |
| Self-hosted FRP on Hetzner CX32 | ~3-4 weeks | ~$14/mo fixed (€7 VPS + $5 control plane + $1 domain) | Low | $1.64 | 9 users | 100% margin, full sovereignty, K2SO becomes SRE org |
| Custom Rust tunnel | 3-6 months | varies | Low | $1.64 | varies | Massive overkill; skip unless open-sourced |

**Verified facts** (carry over to Phase 2.6, valid for both scopes):
- Cloudflare Tunnel is free, unmetered bandwidth, named tunnels free
- Per-CF-account cap: 1,000 named tunnels + 1,000 routes (CNAME + CIDR combined)
- CF Tunnel API: `POST /accounts/{id}/cfd_tunnel` returns `id` + `token`; daemon spawns `cloudflared tunnel run --token <token>`
- Cloudflare Access: free first 50 seats, $7/user/mo beyond (NOT used per decision #8 above)
- DNS provisioning: separate API call (CNAME `<user>.k2.dev` → `<tunnel-id>.cfargotunnel.com`)
- Provisioning latency: 5-15s end-to-end signup → working subdomain
- Hetzner CX22 €3.79/mo, CX32 €6.80/mo, 20 TB traffic included
- FRP scaling: ~300 clients on 1GB RAM (issue #2036), ~1-2k clients on CX32 (4-8GB RAM, tuned ulimit)
- `.dev` domain: ~$11/year via Namecheap
- Bandwidth per K2SO Companion user: ~5 GB/mo blended (idle 500 MB, heavy 10-20 GB)

---

## What changes under the expanded "expose any port" scope

The expanded requirement adds three new dimensions Phase 2.5's research didn't evaluate:

### 1. Per-port URL UX

User stated the desired UX as `<sub>.k2.dev:3000` (port in URL). Technically, most HTTP reverse-proxy tunnels (CF Tunnel HTTP, ngrok HTTP, FRP HTTP) terminate TLS at the edge and forward to a single backend port. Preserving `:3000` through the public URL requires either:

- **TCP tunneling** (not HTTP) — supported by ngrok TCP, FRP TCP, CF Spectrum (paid Enterprise). Public URL ends up `tcp://<sub>.k2.dev:<random-edge-port>` typically.
- **Per-port subdomains** under the hood — `<user>-3000.k2.dev` actually routes to user's `localhost:3000`. UX-equivalent to `<sub>.k2.dev:3000` if K2SO's UI shows the port-in-URL form while operating per-port subdomains underneath. Matches GitHub Codespaces / Replit / Tailscale Funnel patterns.

The per-port-subdomain pattern is the realistic implementation. UX layer can pretty-print it back to `<sub>.k2.dev:3000` style if desired.

### 2. Cloudflare Tunnel weakens under multi-port

Each new port a user exposes requires:
- One new CF route provisioned via API
- One new DNS record provisioned via API
- Token re-issued OR new ingress rule added to the existing tunnel config

The 1,000-route-per-CF-account ceiling gets eaten much faster. If average user runs 3 ports, ~333 users per CF account before sharding. Provisioning latency stays at 5-15s per port. Not disqualifying but adds friction to a "click to expose this port" UX.

### 3. Self-hosted (FRP / Pangolin) gets stronger

Multi-port + dynamic registration is FRP's design center. Each user runs `frpc` with multiple `[http]` blocks (or `[tcp]` blocks) — one per exposed port. K2SO control plane provisions DNS for `<user>-<port>.k2.dev` per port via Cloudflare DNS API (K2SO uses CF as DNS provider, not as tunnel provider). frpc reloads gracefully on config changes.

**Pangolin** (open-source self-hosted alternative to CF Tunnel) is purpose-built for K2SO's exact use case: one operator, many users, many ports per user, branded domain. Bundles WireGuard + Traefik + DNS automation. May be the "simpler primitive" the user is hinting at.

### Revised candidate matrix (expanded scope)

| Provider | Multi-port native | Dynamic port registration | K2SO operates? | Per-port DNS friction | Notes |
|---|---|---|---|---|---|
| **Pangolin (self-hosted)** | ✅ Native | ✅ API + WG | Yes — VPS | None (handled by Pangolin) | OSS, purpose-built; dark horse |
| **FRP self-hosted** | ✅ Native | ✅ frpc config reload | Yes — VPS | K2SO DNS API call per port | Battle-tested; more manual |
| **Cloudflare Tunnel** | ⚠️ Per-route provisioning | ⚠️ 5-15s/port via API | No | High (CF + DNS API per port) | 1k routes per account; $0 cost |
| **CF Spectrum (TCP tunneling)** | ✅ True port-in-URL | ✅ Native | No | None | Enterprise pricing only; check terms |
| **Tailscale Funnel** | ⚠️ Free tier: 3 ports max | ❌ Static config | No | N/A (no custom domain) | Good for free tier only |
| **ngrok TCP** | ✅ Native | ✅ Native | No | None | $20/mo Pro for custom domain; K2SO captures no revenue |
| **Custom Rust + bore + Caddy** | ✅ Design control | ✅ Design control | Yes — VPS + write protocol | None | 3-6 month engineering investment |

---

## Open questions for Phase 2.6 re-spike

These are the questions a focused research pass should answer before committing the decision:

1. **Pangolin deep-dive**: architecture, install ceremony, multi-tenant model, scaling characteristics, operational burden (rolling upgrades, cert rotation, DDoS posture), license, maintainer health
2. **FRP multi-port-per-user pattern**: concrete config example for "one user, many ports, registered dynamically via control plane API"; can frpc be controlled via API or do we manage config files + reload signal?
3. **Cloudflare Spectrum**: actual pricing (Enterprise sales only? real numbers?), TCP/UDP support, port range limits
4. **Competitive pricing**: K2SO's "expose any port" feature competes with ngrok Pro at $20/mo. What's the pricing ladder K2SO should adopt? $2 Companion-only / $5-10 Multi-port / $20+ Pro Companion?
5. **AWS-native primitives** — full evaluation: App Runner + custom domain, Lightsail + FRP, API Gateway + Lambda (unlikely fit), Global Accelerator (probably overkill), CloudFront + ALB. Include a "why AWS isn't simpler" rebuttal section.
6. **Per-port URL UX decision**: `<sub>-3000.k2.dev` vs `<sub>.k2.dev:3000` (presentation layer)
7. **Trademark posture**: `k2.dev` domain is owned. K2 software trademark (USPTO Class 9 + Class 42) clearance still recommended **before public rebrand of K2SO → K2** and before any K2-branded marketing ships. Initial scan: K2 Advisors L.L.C. holds K2 in Class 36 (financial services) — does not conflict with software. Domain ownership is independent of trademark protection.
8. **Self-hosted DDoS exposure**: if Phase 2.6 lands on Pangolin/FRP, what's the realistic DDoS risk for K2SO's tunnel backbone? Mitigations: Cloudflare in front (we're already a CF customer for DNS), rate limiting, ban lists.

---

## Recommended re-spike scope (when Phase 2.6 launches)

Single research subagent, ~30-60 min, scoped to the 8 open questions above. Output: revised decision matrix + concrete `$/mo` pricing recommendation + implementation effort estimate for the chosen backbone + Phase 3 Workstream B implications (which most likely shift from "support pluggable backbone" to "lock the chosen backbone's auth + DNS provisioning patterns").

Subagent brief should explicitly NOT re-evaluate ngrok, Tailscale, or LocalXpose for the K2SO Hosted backbone — those are locked as BYO / Free tier respectively. Focus is on **Pangolin vs FRP vs CF Tunnel vs CF Spectrum vs AWS-native** for the K2SO Hosted backbone under the expanded "expose any port" requirement.

---

## Sequencing

```
Phase 2.5 Workstreams A, B, C (build+smoke, test migration, CI sweep)
                ↓
Phase 2.5 closes WITHOUT Workstream E (decision doc)
                ↓
Phase 2.6 launches when ready (no hard deadline; can interleave with Phase 3 prep work that doesn't depend on tunnel choice)
                ↓
Phase 2.6 re-spike + decision doc commit
                ↓
Phase 3 Workstream B unblocks (TLS + auth architecture locks)
                ↓
Phase 3 ships ngrok BYO + Tailscale free path documented (TunnelProvider trait pluggable, Hosted backbone deferred)
                ↓
Phase 4 builds K2SO Hosted middle tier on chosen backbone
```

**Key change from Phase 2.5's original sequencing**: Phase 2.5 closes WITHOUT Workstream E. Phase 3 can begin Workstreams A (typed router), C (OpenAPI), D-G (Mobile Companion contract, K2SO Connect build, versioning, rate limiting) in parallel with Phase 2.6 since none of those depend on the tunnel decision. Only Phase 3 Workstream B (TLS + auth) blocks on Phase 2.6.

---

## Definition of done (Phase 2.6 → Phase 3 Workstream B gate)

Phase 2.6 is complete when:

1. ✅ Re-spike research subagent has returned answers to all 8 open questions
2. ✅ `tunnel-provider-decision.md` committed at `.k2so/prds/` per the template in Phase 2.5 Workstream E.1
3. ✅ Phase 3 Workstream B section in `.k2so/prds/phase-3-contract-hardening.md` updated with concrete TLS + auth requirements tied to the chosen backbone
4. ✅ Domain locked: `k2.dev` purchased 2026-05-23. K2 software-class trademark search remains a separate prerequisite for a future K2SO → K2 product rebrand (not blocking Phase 2.6 close).
5. ✅ Phase 4 PRD drafted (or stub'd) at `.k2so/prds/phase-4-hosted-tier.md` capturing the K2SO Hosted middle tier implementation scope

---

## Out of scope (explicit non-goals for Phase 2.6)

- **Building the Hosted backbone** — Phase 2.6 only decides. Implementation is Phase 4.
- **Mobile Companion / K2SO Connect contract changes** — Phase 3 Workstreams D + E.
- **K2 trademark filing / domain purchase execution** — handled separately (user-driven; legal consult recommended).
- **Stripe integration** — Phase 4 work.
- **Pricing finalization** — Phase 2.6 can recommend; final pricing locked at Phase 4 launch with usage data.

---

## References

- `.k2so/prds/phase-2.5-validation-and-tunnel-decision.md` — original tunnel decision PRD (Workstream D + E)
- `.k2so/prds/phase-3-contract-hardening.md` — Phase 3 Workstream B depends on Phase 2.6's choice
- Phase 2.5 Workstream D research subagent output (`a26cbc26003ae7fcb`, 2026-05-23) — narrower-scope research, recommendation Cloudflare Tunnel + k2.dev
- User feedback (2026-05-23): "we need to be able to make it so that when a user 'launches a localhost on port 3000' that the user can then access that demo from their personal computer (somewhere else in the world) by visiting `<subdomain>.k2.dev:3000`" → expanded scope
- User feedback (2026-05-23): K2 Advisors L.L.C. trademark (Class 36 financial services) does NOT block K2 in software (Class 9 / 42), but full TESS search in Class 9 + Class 42 still required before domain purchase
- Memory: `project_websocket_companion_plan` — Companion protocol planning
