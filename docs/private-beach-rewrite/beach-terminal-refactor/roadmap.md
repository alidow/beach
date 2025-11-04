# Beach Terminal Refactor Roadmap

## 1. Purpose & Context

The legacy `BeachTerminal` component inside `apps/private-beach` owns the entire viewer experience: session connection lifecycle, predictive echo, telemetry, fallback transport handling, sizing, and UI chrome. While the headless transport stack is stable, the component’s sizing and DOM measurement logic is tightly coupled to the old dashboard layout, making it fragile to embed in the rewrite canvas.

**Intent:** evolve `BeachTerminal` into a reusable viewer primitive with pluggable sizing so the new rewrite tiles can reuse the proven terminal behaviour without inheriting layout quirks.

## 2. Guiding Principles

- **Keep what already works.** Transport, predictive echo, reconnect behaviour, telemetry, and existing hooks have been hardened and must remain intact.
- **Callers own layout.** The terminal should accept an explicit sizing contract instead of manipulating DOM measurements internally.
- **Progressive adoption.** Maintain backwards compatibility for the legacy dashboard; the rewrite opts into the new sizing strategy without breaking the current app.
- **Composable, not monolithic.** Expose headless hooks/utilities so future viewers (Playwright, tests, other surfaces) can reuse them without the full component.

## 3. Goals & Non-goals

### Goals
1. Enable `apps/private-beach-rewrite` to mount the legacy viewer logic with predictable sizing.
2. Provide a clear `TerminalSizingStrategy` interface that both the legacy dashboard and rewrite can implement.
3. Decouple optional UI elements (status overlays, keyboard affordances) so they can be selectively enabled.
4. Preserve telemetry events, predictive echo, and reconnect semantics across both apps.
5. Add automated coverage (unit + smoke) to guard the new contract.

### Non-goals
- Rewriting the transport/service layer. We rely on `viewerConnectionService`, `sessionTerminalManager`, etc. as-is.
- Redesigning terminal visuals beyond sizing/focus behaviour.
- Introducing new backend contracts; everything remains client-side.

## 4. Current Architecture Snapshot

- `BeachTerminal` (React component) wraps `SessionTerminalPreviewClient` and consumes `viewerConnectionService`.
- Sizing logic is derived from DOM measurements, padding constants, and “visible preview driver” flags.
- The component directly manipulates container styles, leading to clashes when embedded outside the legacy grid.

## 5. Target Architecture

```
                     ┌──────────────────────────────┐
                     │ viewerConnectionService (shared)
                     └──────────────┬───────────────┘
                                    │
                     ┌──────────────▼───────────────┐
                     │ sessionTerminalManager (shared)
                     └──────────────┬───────────────┘
                                    │
                           ┌────────▼────────┐
                           │ BeachTerminal   │
                           │ (refactored)    │
                           ├────────┬────────┤
                sizing policy ─────▶│ Layout  │◀── overlays toggle
                           └────────┴────────┘
                                    │
                     ┌──────────────▼───────────────┐
                     │ consumers                     │
                     │  - legacy dashboard (status quo)
                     │  - rewrite tiles (new sizing)
                     └──────────────────────────────┘
```

## 6. Implementation Plan

### Phase 0 – Baseline & Safety Nets
- [ ] Snapshot current behaviour: record a short screencast + capture metrics (latency overlay, predictive echo toggles).
- [ ] Ensure `npm run lint` / unit tests pass in both `apps/private-beach` and rewrite.
- [ ] Add Playwright smoke (if not existing) to confirm the viewer renders text for a sample session.

### Phase 1 – Introduce Sizing Strategy Contract
1. Define `TerminalSizingStrategy` in a shared location (`apps/private-beach/src/components/terminalSizing.ts`).
   - Required methods: `nextViewport(tileRect, hostMeta)`, `containerStyle(tileRect)`, `scrollPolicy()`.
2. Implement `LegacySizingStrategy` that reproduces current behaviour (default export for backwards compatibility).
3. Plumb strategy through `BeachTerminal` / `SessionTerminalPreviewClient` via new props with defaults.
4. Update legacy dashboard usage to pass nothing (so it keeps default strategy).
5. Add unit tests to validate strategy outputs.

### Phase 2 – Extract Optional UI Hooks
1. Move predictive echo, keyboard shortcuts, and overlays into discrete hooks/modules that `BeachTerminal` composes internally.
2. Add toggles (`showStatusOverlay`, `enablePredictiveEcho`, etc.) so rewrite can disable pieces it doesn’t need initially.
3. Maintain default props to keep existing experience unchanged.

### Phase 3 – Rewrite Integration
1. Create `RewriteSizingStrategy` in the rewrite app implementing the new interface.
2. Replace `SessionViewer` placeholder with refactored `BeachTerminal`, passing:
   - `sizingStrategy={rewriteSizing}`
   - `className` overrides for tile styling
   - Flags for overlays if desired
3. Verify canvas resize/drag flows; ensure telemetry still fires.

### Phase 4 – Testing & Hardening
- [ ] Cross-app lint/test (`npm run lint`, `npm test`, relevant Playwright specs).
- [ ] Manual regression checklist: resize, reconnect, predictive echo, keyboard shortcuts, telemetry log review.
- [ ] Document integration steps in `/docs/private-beach-rewrite/beach-terminal-refactor/integration.md`.

### Phase 5 – Cleanup & Adoption
- [ ] Remove temporary `SessionViewer` shim.
- [ ] Update workstream docs & sync logs.
- [ ] Plan staging rollout with WS-F telemetry gating.

## 7. Deliverables

- Updated `BeachTerminal` with strategy support and new props.
- Shared sizing strategy interface + legacy implementation.
- Rewrite-specific sizing policy + integration.
- Test coverage additions (unit + e2e where possible).
- Documentation (this roadmap + integration guide + change-log).

## 8. Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Strategy introduces regressions for legacy dashboard | High | Maintain default behaviour, add regression tests, use feature flag if needed |
| Rewrite sizing policy miscalculates viewport | Medium | Start with simple “match tile pixels” policy, rely on telemetry + manual QA |
| Predictive echo hook extraction breaks metrics | Medium | Add unit tests + compare telemetry payloads before/after |
| Timeline creep due to shared component reviews | Medium | Collaborate with WS-D/E reviewers early, split PRs (strategy vs integration) |

## 9. Success Criteria

- Rewrite tiles render with refactored `BeachTerminal` respecting new sizing.
- No regressions for existing dashboard (manual smoke + test suite).
- Telemetry (`canvas.tile.connect.*`, predictive echo events) unchanged.
- Workstream documentation updated, allowing hand-off to another Codex instance.

## 10. Next Steps

1. Align with WS-D/E on exposing the new sizing prop contract (schedule review).
2. Start Phase 1 (`TerminalSizingStrategy` scaffolding) and land behind no-op changes.
3. Prepare “Implementation Guide” doc once Phase 1 PR lands.

