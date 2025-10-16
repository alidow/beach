# Private Beach Product Boundaries & Guiding Principles

## Purpose
- Clarify how the open-source Beach foundation and the paid Private Beach offering relate.
- Set expectations for contributors, customers, and internal teams as we split functionality across products.
- Establish principles to apply when we consider new features or licensing decisions.

## Product Boundary Snapshot

- **Open-Source Beach (source available / Beach Source License)**
  - `apps/beach`: core terminal streaming plus CLI entry points, MCP plumbing for standalone session sharing.
  - `apps/beach-cabana`: GUI capture/streaming stack with optional harness hooks.
  - Shared crates/libraries that power standalone session execution (transport, PTY, diff codecs, harness SDK).
  - Authentication hooks to Beach Gate for optional login but no paywalled capabilities.
- **Paid Private Beach**
  - `apps/private-beach`: premium web workspace, layout management, session dashboards, automation onboarding.
  - `apps/beach-manager`: orchestration/control plane, controller leases, policy enforcement, audit, billing integration, agent bootstrap workflows.
  - `crates/beach-buggy` (+ adapters inside `apps/beach`/`beach-cabana`): high-performance harness runtime delivering structured diffs, OCR, and action execution under Private Beach entitlements.
  - Redis/Postgres-backed coordination features (state cache, action queue) that span multiple sessions/private beaches.
- **Shared / Dual-Use Components**
  - Session harness runtime remains open source; Private Beach supplies orchestration around it.
  - Specialized harness modules (OCR, structured adapters) live alongside the manager service in `crates/beach-buggy` and adapters, ensuring clear versioned contracts.
  - MCP protocol extensions implemented in the manager are public, so core Beach and third parties can integrate.
  - Beach Gate authentication stays common; entitlements determine whether Private Beach APIs respond.

## Guiding Principles
- **Foundation First:** Core session tech (low-latency streaming, MCP basics, harness SDK) stays source-available to encourage ecosystem innovation and agent experimentation.
- **Premium = Orchestration:** Paid features focus on multi-session coordination, management UX, automation tooling, and hosted reliability.
- **Interoperability:** MCP surfaces, harness types, and data schemas are documented and open so that community tools can plug into Private Beach without code forks.
- **Modularity:** Design harnesses, SDKs, and APIs so self-hosters can adopt pieces without buying Private Beach, while premium layers add cohesive experience and scale. Harness implementations live in `crates/beach-buggy` + adapters so contracts stay versioned and auditable, while Beach Manager focuses on orchestration policy.
- **Performance Obsession:** Optimize for latency, throughput, and resource efficiency at every layer; profiling, benchmark suites, and budget enforcement are first-class requirements.
- **P2P First:** Default to WebRTC/UDP peer connections to minimize latency; centralized relays exist but are treated as fallbacks. Architect protocols with unreliable delivery in mind.
- **Zero Trust:** Assume central infrastructure (Beach Gate, Beach Road, state cache) can be compromised—enforce mutual auth, end-to-end encryption, and least-privilege authorization at the application layer.
- **UX Minimalism:** Focus initial UX on the 80% workflows; prioritize intuitive, keyboard-forward actions and progressive disclosure before layering complexity.
- **Developer Experience:** Provide clear APIs, SDKs, and tooling with minimal ceremony so teams can script and automate quickly; documentation and examples accompany every surface.
- **Interface Contracts:** Keep boundaries between components explicit; prototype and test interfaces as soon as code lands (temporary harnesses/tests in `temp/` are encouraged but must converge to maintained suites).
- **Transparency:** Clearly publish which repos, binaries, and endpoints require licenses; avoid subtle feature gating or regressions in source-available flows.
- **Migration-Friendly:** Provide pathways for teams to start in open-source Beach and upgrade to Private Beach without rewriting scripts or losing data.
- **Customer Trust:** Pricing and entitlement checks go through Beach Gate; users can self-verify what data leaves their sessions (state cache retention, shared storage policies).

## Decision Checklist
When evaluating a new capability, ask:
1. Does it serve single-session usability? → keep in open-source Beach.
2. Does it orchestrate or monetize cross-session workflows? → route to Private Beach.
3. Would open access undermine security/compliance promises? → consider Private Beach + clear rationale.
4. Can we expose the protocol/spec openly even if the hosted implementation is paid? → default to yes.
5. Will the feature require ongoing infra (24/7 cache, billing, SLA)? → align with Private Beach service.

## Next Steps
- Share this document with stakeholders for review and publish in internal handbook.
- Reference the boundary statements in marketing, onboarding, and engineering design docs.
- Establish a change-control process so future boundary decisions remain consistent with these principles.
