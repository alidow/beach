# Private Beach Rewrite – WS-F Rollout Plan

_Aligned with docs/private-beach-rewrite/plan.md §3.8 and Milestones 7-8._

## 1. Feature Flag Strategy
- **Flag source**: `NEXT_PUBLIC_PRIVATE_BEACH_REWRITE_ENABLED` drives the default. `resolvePrivateBeachRewriteEnabled()` reads env, `?rewrite=` query overrides, and `localStorage["private-beach-rewrite"]`.
- **Client integration**: `/beaches/[id]` now publishes `data-rewrite-enabled` and emits `canvas.rewrite.flag-state` telemetry. All rewrite surfaces should consume `isPrivateBeachRewriteEnabled()` to gate routing once WS-A/B wire their shells.
- **Operator controls**:
  1. **Local dev** – append `?rewrite=1` (or `0`) or call `rememberPrivateBeachRewritePreference()` in console.
  2. **Preview / Beta** – set env var per Vercel/Render environment for canary cohorts.
  3. **Production rollout** – stage flips via env var; retain query/LS escape hatch for support overrides.

## 2. Telemetry & Analytics Hooks
- **Events added** (see `apps/private-beach/src/lib/telemetry.ts`):
  - `canvas.tile.create` / `remove` with `privateBeachId`, position, rewrite flag state.
  - `canvas.tile.connect.start|success|failure|disposed` emitted from `viewerConnectionService`.
  - `canvas.assignment.success|failure` capturing controller + targets, partial failure metadata.
  - `canvas.rewrite.flag-state` for rollout auditing.
- **Data contract**: IDs are raw session/tile ids; downstream schema consumers should namespace by `privateBeachId`. Pending confirmation from WS-D/E before tile store refactor.

## 3. QA Coverage & Tooling
- **Unit tests**: Vitest specs for assignment helpers, feature flag resolution, and viewer connection telemetry (`apps/private-beach/src/**/__tests__`).
- **Playwright smoke**: `tests/e2e/private-beach-rewrite-smoke.pw.spec.ts` ensures rewrite flag + sandbox telemetry fire. Needs WS-A rewrite route in CI image to enable.
- **Next steps**:
  - Hook smoke spec into CI once the rewrite app is routable.
  - Add telemetry assertion to post-deploy monitor (Grafana or Looker) for event volumes.

## 4. Rollout Phases
1. **Internal Dev** (flag false, opt-in via query/LS): Gather telemetry sanity, ensure no missing events.
2. **Beta Cohort** (env default true on staging & limited prod cohort): Monitor `canvas.tile.connect.failure` and assignment failure rates, compare vs legacy.
3. **Full Launch** (env true everywhere): Keep legacy canvas available behind inverse flag for one sprint as fallback.
4. **Flag Removal**: After two healthy sprints and zero elevated failure telemetry, remove flag scaffolding and retire legacy routes.

## 5. Release Readiness Checklist
- ✅ Telemetry events firing with rewrite flag context (verified via sandbox + tests).
- ✅ Unit + smoke coverage merged; ensure CI executes vitest suite.
- ☐ WS-A/WS-B adopt shared flag helper and expose rewrite route behind flag.
- ☐ Ops dashboard plotting new telemetry events versus legacy baselines.
- ☐ Support playbook updated with query/LS override for incident mitigation.

_Owner: Codex (WS-F). Next review: 2025-11-06 sync after WS-A flag wiring._
