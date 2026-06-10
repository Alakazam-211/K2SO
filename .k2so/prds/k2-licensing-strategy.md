# K2 Licensing Strategy — Fair Source + Tunnel-Routed Commercialization

**Status:** WORKING DECISION (Rosson, 2026-06-10). Executes with the 0.40.X
rebrand/repo migration. Not legal advice — have a lawyer sanity-check the
final texts before the new repo goes public.
**Related:** `0.40.x-to-1.0-weekend-roadmap.md` (rebrand window),
`project_kimi_k2_naming_conflict` (house-mark decision), K2 Connect → "K2
Toge" rename, `secure-tunnel-monetization-roadmap.md`.

---

## The decision in one paragraph

At the 0.40.X rebrand, K2 (app + daemon + CLI) relicenses from MIT to
**FSL-1.1-Apache-2.0** (Sentry's Functional Source License: source-available
"Fair Source," competing use forbidden, each version converts to Apache-2.0
two years after release). The **K2 Connect/Toge tunnel + control plane stays
proprietary** in its separate repo. A **standing Commercial Hosting Grant**
permits anyone to host K2 servers for third parties commercially — *provided
all remote access rides the official K2 Connect/Toge service under its
current pricing (today $3/tunnel)*; anyone wanting their own tunnel
infrastructure for a commercial hosting offering must license that
separately from Alakazam Labs. A **trademark policy** protects the "K2 by
Alakazam Labs" name/logo independently of (and beyond) the code license.

## Why these references shaped it

- **Sentry (FSL):** the license itself. Their history is our scenario —
  permissive license → resellers → BSL → FSL. Competing-use clause + 2-year
  Apache conversion = protection now, goodwill forever.
- **Superset (ELv2):** rejected as the base — its only restriction is
  hosted/managed services; a re-skinned *desktop app sold as software*
  (K2's exact shape) slips through.
- **Tolaria (AGPL + trademarks.md, different company):** AGPL rejected
  (doesn't forbid resale/competing hosting — only forces source publication;
  scares business users). Their separate trademark policy is the template
  we ARE adopting.

## The five layers and what each polices

| Layer | Polices | Rule |
|---|---|---|
| 1. FSL-1.1-Apache-2.0 (app/daemon/CLI) | commercialization | nobody provides K2 to third parties as a paid product/service except via layer 3; everything else (internal/team/business use, modification, redistribution) is free |
| 2. Proprietary K2 Connect/Toge | the economics | the only maintained remote-access path runs through the metered service ($3/tunnel); control plane never ships |
| 3. Commercial Hosting Grant (standing, public) | the channel | hosting K2 for clients is PERMITTED iff all remote access uses the official tunnel service under current pricing; own-tunnel commercial hosting requires the Enterprise BYOT tier (layer 3b) |
| 3b. Enterprise "Bring Your Own Tunnel" license | the big-company door | a NAMED, PRICED tier (not ad-hoc negotiation): larger companies commercially host/operate K2 with their own remote-access infrastructure under a paid license — annual, per-server or per-seat |
| 4. Trademark policy ("K2", logo, "K2 by Alakazam Labs") | identity | no re-skin can trade on the brand — including after the Apache conversion matures |
| 5. CLA/DCO on external contributions | future freedom | preserves the unilateral relicensing position we have today (1,352/1,352 commits are Rosson's) |

## Explicitly allowed (the things we WANT people doing)

- Use K2 free as a business tool — any company size, internal/commercial use.
- Self-host; modify; strip the tunnel and use your own VPN/SSH/WireGuard for
  **your own** servers. (The license polices who you commercialize to, not
  what you modify. Internal tunnel-stripping costs us nothing — that user
  was the free tier either way.)
- Host K2 servers **for clients, commercially** — via the Hosting Grant +
  official tunnel. Would-be competitors become channel partners; revenue
  scales with their success at $3/tunnel without bespoke contracts.
- Build non-competing commercial products/integrations on top.
- Read every line of the app/daemon source ("nothing fishy" auditability);
  use 2-year-old versions under plain Apache-2.0.

## Explicitly NOT allowed

- Re-skinning/reselling K2 as a substitute product or service (FSL
  competing use) — with our tunnel, a rebuilt tunnel, or no tunnel.
- Commercial hosting that bypasses the official tunnel service without a
  negotiated license.
- Using the K2 name/logo/brand on any fork or derivative (trademark —
  survives the Apache conversion).

## Accepted residual (eyes open)

- Pre-0.40 snapshots remain MIT forever for anyone who saved one (no forks
  exist as of 2026-06-10; releases were public — risk ≈ nil but nonzero).
  The sooner the cutover, the shorter the MIT tail.
- After each version's 2-year Apache conversion, that old version can be
  forked/commercialized legally — with 2-year-old code, no brand, and us as
  incumbent. Judged the right trade vs. BSL-4yr / PolyForm Shield (no
  conversion) for the goodwill story. Revisit only if it proves abused.
- "Open source" claim discipline: K2 is **Fair Source** (fair.io), not OSI
  open source, until versions convert. README/PROJECT.md/marketing language
  must change with the relicense.

## The commercial ladder (customer-facing shape)

| Tier | Who | Remote access | Price |
|---|---|---|---|
| Free | individuals, teams, any company self-hosting for themselves | anything they like (official tunnel, own VPN, LAN) | $0 |
| Hosted channel (Commercial Hosting Grant) | MSPs/agencies hosting K2 for clients | official K2 Connect/Toge ONLY | then-current tunnel pricing (today $3/tunnel) — self-serve |
| **Enterprise BYOT** | larger companies wanting their own tunnel/remote-access infrastructure under a commercial offering | their own | negotiated annual license |

Two flavors of Enterprise BYOT to offer (both on paper from day one):
1. **Permission-only** — they build/operate their own remote-access layer;
   the license simply lifts the competing-use restriction for their
   deployment scope.
2. **Licensed on-prem K2 Toge** — we license the actual control-plane
   software for them to run inside their network. Strictly better for both
   sides (they don't rebuild, we keep them on our stack), and it is the
   ONLY workable answer for air-gapped/regulated customers who physically
   cannot use a cloud tunnel — the segment the public grant can never serve.

## Budibase study (added 2026-06-10) — lessons applied & rejected

Budibase = per-package mosaic (GPLv3 backend, MPL client so user-built
apps aren't contaminated, BSL-1.1 `pro` package w/ 4-year change date,
Change License **GPLv3**, anti-circumvention production grant, EU
public-sector exception).

**Adopted into our drafts (pushed to Alakazam-211/K2):**
1. **Anti-circumvention condition on the Hosting Grant** — hosting-for-
   others permission is void if K2 Connect integration/metering is
   circumvented in the hosted offering. Scoped to the GRANT only:
   internal self-hosters stripping the tunnel remain explicitly fine.
2. **"What you build with K2 is yours"** clarification (their "apps you
   build don't package GPL code" move) — costs nothing, kills the most
   common licensing-FUD question for a workspace/agent tool.

**Noted as an option, recommend AGAINST:** copyleft Change License
(their BSL→GPLv3 means even expired versions force forks open). FSL's
fixed menu is Apache/MIT; switching to BSL just to get GPL-after-expiry
trades away the "becomes genuinely permissive" goodwill story that
justifies the conversion at all. Protection load stays on trademark +
tunnel, not on the expired tail.

**Cautionary tale:** their license mosaic makes GitHub report
`NOASSERTION` — scanners/legal teams can't classify the repo at a
glance. Validates our single top-level FSL + separate grant docs.
EU public-sector carve-out: parked as a possible Enterprise-tier
sweetener if EU procurement ever knocks.

## Wording cautions for the grant

- Tie the hosting condition to "**a current K2 Connect (K2 Toge) service
  agreement / its then-current pricing**," NOT the literal "$3/tunnel" —
  pricing must be able to move without relicensing.
- Implement as a SEPARATE additional-permissions document
  (`COMMERCIAL_HOSTING_GRANT.md`) on top of unmodified FSL text — FSL is a
  fixed-text license; a sole copyright holder may always grant *more*
  permissions alongside it, never edit the license itself.

## 0.40.X migration checklist (license workstream)

1. [ ] Lawyer pass on FSL choice + grant + trademark policy texts.
2. [ ] New repo: `LICENSE.md` (FSL-1.1-Apache-2.0, exact upstream text),
       `COMMERCIAL_HOSTING_GRANT.md`, `TRADEMARKS.md` (Tolaria-style),
       `NOTICE`, README "Fair Source" section + hosting-grant pointer.
3. [ ] CONTRIBUTING.md gains DCO sign-off (or lightweight CLA) before the
       first external PR is accepted.
4. [ ] Scrub "MIT-licensed" / "open source" claims: README, PROJECT.md,
       k2.dev site, memory `project_vision`.
5. [ ] Old repo: final MIT tag, then archive **private** (per Rosson).
6. [ ] k2.dev: publish the hosting-grant page ("Host K2 for your clients —
       $3/tunnel") as the self-serve channel-partner funnel.
7. [ ] k2.dev: "Enterprise — bring your own tunnel" page (contact-sales
       form; names the two BYOT flavors incl. on-prem K2 Toge for
       air-gapped/regulated buyers).
