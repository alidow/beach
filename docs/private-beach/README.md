# Private Beach Documentation Hub

This directory houses the plans, specs, and design notes for the Private Beach premium offering.

## Contents
- `vision.md` – product goals, pillars, architecture overview.
- `data-model.md` – Postgres schema, enums, and relational layout.
- `roadmap.md` – phased execution plan across harness, manager, and UI workstreams.
- `engineering-cadence.md` – lightweight solo dev cadence and observability expectations.
- `beach-manager.md` – current control plane status, API contracts, and outstanding work.
- `guiding-principles.md` – product boundary decisions, zero-trust stance, performance philosophy.
- `beach-buggy-spec.md` – harness sidecar specification powered by the Beach Buggy runtime.
- `pong-demo.md` – flagship showcase experience outline.
- `intra-beach-orchestration.md` – MCP surfaces and cross-session coordination blueprint.
- Additional design docs under `secure-webrtc/`, `beach-lifeguard/`, etc.

## How to Use
1. Start with `vision.md` to understand the overall direction.
2. Reference `guiding-principles.md` before proposing new features to keep scope aligned.
3. Consult `data-model.md` + `beach-manager.md` when implementing backend functionality.
4. Use `roadmap.md` to track phase completion and upcoming work.

## Core Stack Assumptions
- Postgres is the durable store for both the Rust control plane (`apps/beach-manager`) and the Private Beach Surfer Next.js app; we do not persist product state in browser storage.
- Drizzle ORM is the canonical TypeScript layer for querying and migrating Surfer-owned tables; generated artifacts stay in `apps/private-beach` while SQL migrations live alongside the manager.
- Every UI workflow should prefer Beach Manager APIs for shared entities (beaches, sessions, memberships) and reach for Drizzle-backed tables only for Surfer-specific UX state that the manager does not yet expose.

Questions or edits should be proposed via PR with reviewers from the Private Beach core working group.
