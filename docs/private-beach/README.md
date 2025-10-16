# Private Beach Documentation Hub

This directory houses the plans, specs, and design notes for the Private Beach premium offering.

## Contents
- `vision.md` – product goals, pillars, architecture overview.
- `data-model.md` – Postgres schema, enums, and relational layout.
- `roadmap.md` – phased execution plan across harness, manager, and UI workstreams.
- `guiding-principles.md` – product boundary decisions, zero-trust stance, performance philosophy.
- `beach-buggy-spec.md` – harness sidecar specification powered by the Beach Buggy runtime.
- `beach-manager.md` – control-plane responsibilities, flows, and security posture.
- `pong-demo.md` – flagship showcase experience outline.
- `intra-beach-orchestration.md` – MCP surfaces and cross-session coordination blueprint.
- Additional design docs under `secure-webrtc/`, `beach-rescue/`, etc.

## How to Use
1. Start with `vision.md` to understand the overall direction.
2. Reference `guiding-principles.md` before proposing new features to keep scope aligned.
3. Consult `data-model.md` + `beach-manager.md` when implementing backend functionality.
4. Use `roadmap.md` to track phase completion and upcoming work.

Questions or edits should be proposed via PR with reviewers from the Private Beach core working group.
