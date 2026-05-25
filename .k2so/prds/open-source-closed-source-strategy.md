# Open Source vs Closed Source Strategy

**Status**: Drafted 2026-05-25 by Rosson, captured by pod-leader. Strategic framing document — informs Phase 2.6 / Phase 3 / Phase 4 / 1.0.0 architectural choices.
**Owner**: Rosson + pod-leader
**Note**: User explicitly flagged that some specifics in the framing "may not be perfectly accurate to our codebase" — the **premise and strategy** are what matter here; concrete repo state should be verified before acting on specifics.

---

## tl;dr

K2SO is already public + MIT-licensed. Already-released versions stay MIT forever (the license is irrevocable for what's already shipped). So:

- **You don't need a license change. You need a repo boundary change.**
- **Keep the core K2 app MIT.** Don't try to relicense — community blowback + forks would continue from the last MIT commit anyway. The MIT framing ("open source, your keys, your terminals") is also a real marketing asset.
- **Split monetization into separate private repos.** K2 Companion (iOS), K2 Connect (paid sync/tunnel service), and Alakazam Engine SKILLs each get their own private repo. The K2 desktop app holds thin MIT-licensed client stubs; the servers + protocols + sync logic stay closed.
- **The 0.40.0 K2SO → K2 rename is the natural split point.** Introduce `k2-connect` and `k2-companion` private repos alongside the public `k2` repo at the same time as the rebrand. Update the public README to make the open-core + paid-services boundary explicit.

The split itself IS the moat. Code that's open stays open; monetizable surfaces live behind a separate repo boundary, not behind a license change.

---

## Current state (verify before acting)

The user's framing (paraphrased):
- K2SO is public-facing
- MIT-licensed
- 144 releases shipped
- Front page advertises "Open source (MIT)" on `k2so.sh` and in README
- That ship has sailed: anyone can fork the last MIT release and continue forever

**Verify before relying on these specifics**:
1. Is the repo at `github.com/Alakazam-211/K2SO` actually public? (We've been pushing to it; check visibility.)
2. Is the LICENSE file MIT? (Spot-check.)
3. How many releases are tagged? (Honest count vs the "144" figure.)
4. Does the README + k2so.sh actually advertise MIT open-source today?

If any of these turn out different from the user's framing, the **strategy doesn't change** — the principle is the same — but the framing in marketing copy should match reality.

---

## Strategic premise

**MIT is irrevocable for shipped versions.** That doesn't constrain the future, but it does constrain what you can do with the past:
- ✅ You CAN license future commits differently
- ✅ You CAN move new features to a private repo with a different license
- ✅ You CAN charge for hosted services even if the client code is MIT
- ❌ You CANNOT pull MIT versions back; forks of them remain legal forever
- ❌ You CANNOT prevent reseller / rebrand attempts on the public code; they're allowed

**The monetization moat is NOT in the code — it's in:**
1. **Reputation + momentum** (Alakazam Labs as the canonical K2 vendor)
2. **Hosted services** (Connect, Companion) — the network effect, the operational reliability, the integrations
3. **Cross-product integration** (Alakazam Engine + auto-builds, Hermes connector, Brain System) — value compounds when products talk to each other
4. **Polish + UX** — competitors who fork have to maintain quality, which is expensive

---

## Per-surface mapping

| Surface | Repo / license | Reasoning |
|---|---|---|
| **K2 desktop app** (the main repo today) | **Public, MIT** — unchanged | Don't relicense. Marketing asset; community foundation; forks would happen anyway. |
| **K2 Connect** (server + sync protocol, monetization rail per 0.40.0 #1) | **Private repo, proprietary** | Monetization product. Desktop app holds thin MIT client stub; server stays closed. Pattern: VS Code (MIT) vs GitHub Codespaces backend (proprietary). |
| **K2 Companion** (iOS app, server, pairing) | **Private repo, proprietary** | Different codebase (Swift / React Native), different distribution (App Store). License however; App Store doesn't require source disclosure. ELv2 acceptable for any SDK pieces. |
| **Hermes integration** (per 0.40.X) | **Public connector interface in K2 + private adapter** | Same playbook as Connect/Companion — open protocol, closed implementation. |
| **Owner/Manager K2 SKILLs for Alakazam Engine** (1.0.0 #3) | **Private monorepo (Alakazam internal)** | Internal agency tooling — should never have been candidates for public. Goes wherever Alakazam Labs keeps its operations code. |
| **Script System** (0.40.X) | **Public, MIT** | Great OSS contribution; builds community; doesn't give away anything monetizable. Stays in the public repo. |
| **Brain / Documentation System** (0.41.0) | **Public, MIT** | Same — OSS contribution; the value is in COMPOSED use (Brain + Connect + Companion), not in the Brain feature alone. |
| **TTS summaries + Kessel v2 renderer** (0.42.0) | **Public, MIT** | Frontend / client features; not monetizable in isolation. |

---

## Concrete action items

### Pre-0.40.0 (before the rename)

1. **Verify current repo state** (the "verify before acting" list above) — confirm MIT + public + advertised positioning are accurate.
2. **Audit the public repo for nascent Connect / Companion / monetization code.** If any monetization plumbing has been committed to the public MIT repo, those specific commits are already out under MIT. Two options:
   - Accept those particular code paths as MIT and develop the *next iteration* fresh in the private repo
   - Stop work on that path in the public repo and restart it cleanly in the private repo
   - **Don't try to delete from history.** Doing so is a tell, doesn't actually remove anything from forks, and creates community drama
3. **Decide which existing in-public code is "compatible with paid hosted Connect/Companion as a closed service."** Pure clients calling out to APIs are fine; embedded server logic that someone could lift to run their own Connect server is the bad case. Identify any such cases and stop adding to them.

### At 0.40.0 (the K2SO → K2 rename moment)

4. **Introduce private repos**: `Alakazam-211/k2-connect` and `Alakazam-211/k2-companion` (or whatever the github org is). Set up with proprietary license headers from day one.
5. **Update the public K2 README** with explicit framing:
   > "K2 is open source (MIT). K2 Connect and K2 Companion are commercial Alakazam Labs products that integrate via the public protocol [link]."
6. **Drop a `THIRD-PARTY-INTEGRATION.md` or similar** in the public repo defining the public protocol surface that Connect/Companion + community alternatives can implement against. This invites community competition on the BACKEND while reserving the brand + hosted offering for Alakazam.
7. **Add license / boundary signaling to source-tree comments**: any thin client stub in the public repo that bridges to Connect should have a top-of-file comment like:
   > `// Client for the K2 Connect protocol. The reference implementation is a commercial Alakazam Labs service; community implementations welcome.`

### Post-0.40.0 / Phase 4 buildout

8. **Build Connect / Companion infrastructure** in the private repos with whatever license + operational model Alakazam wants. No public visibility constraint here.
9. **Bill via Stripe etc.** — these are now closed-source services with their own commercial terms.
10. **Resist the urge to "share code"** between the open repo and the private repos. Any helper that ends up in BOTH places should live in the public repo (because MIT licenses can be consumed by closed code, but not vice versa). The flow is: public can be used by private; private must NEVER be used by public.

---

## What this avoids

- **License change blowback.** Going from MIT to a stricter license (BSL, ELv2, SSPL, etc.) is a community-trust event. We don't need it; the split via repo boundary achieves the same goal without the drama.
- **Trying to enforce limitations on already-released code.** Forks of MIT v0.37.12 can rebrand and resell; that's allowed. Our defense is reputation + hosted services + integration breadth, not legal restriction.
- **Confusing community.** "K2 is open source, K2 Connect is commercial" is clear. "K2 went from MIT to BSL last month" is messy.

---

## What this leaves open (for future strategic conversations)

- **Whether ANY part of Connect/Companion should be open source for protocol clarity / community trust.** E.g., a minimal reference Connect server that runs against the same protocol could be MIT; the production-grade implementation stays private. Some open-core companies do this; some don't.
- **Whether to dual-license certain pieces** (e.g., MIT + commercial) for community adoption + commercial use. Probably not needed; standard MIT works fine for the public surface.
- **Trademark of "K2" / Alakazam branding** — the SOFTWARE may be MIT but the brand isn't. Forks can use the code; they can't call themselves K2.
- **Contributor License Agreements (CLAs).** If future commits to the public repo come from third-party contributors, having a CLA gives Alakazam the rights to relicense going forward. Worth considering once external contributions arrive.

---

## References

- `.k2so/prds/secure-tunnel-monetization-roadmap.md` — master roadmap; Phase 4 K2SO Hosted infrastructure is what this strategy enables
- `.k2so/prds/0.40.x-to-1.0-weekend-roadmap.md` — 0.40.0 includes K2 rename + Connect + Companion launches; that's the strategic execution moment for the split
- `.k2so/prds/phase-2.6-tunnel-decision.md` — the tunnel backbone decision (CF / Pangolin / FRP) is itself an open-vs-closed question; whichever wins, the K2-side client stub is MIT, the backbone operation is closed (or proprietary CF-hosted)
- Memory `project_k2_dev_domain` — K2 domain acquisition + rebrand context
- User strategic note (2026-05-25, before going for a walk): the framing this PRD captures
