# Beach Web V2 Terminal-First UI Implementation Plan

## Context & Goals
- Deliver a single-pane, terminal-first interface that feels effortless on desktop and mobile web.
- Overlay the connection flow as a centered modal atop the live terminal canvas, removing the side-by-side layout.
- After connection, replace the modal with a compact top info strip that can expand into a full drawer for session details/actions.
- Default to a dark, flat aesthetic inspired by ChatGPT; lean on TailwindCSS tokens and shadcn/ui primitives already in the codebase.
- Ship the experience behind a `/v2` route so the existing UI stays stable during rollout.

## Guiding Principles
- **Simplicity first**: minimal chrome, focus on the terminal; subtle motion only where it clarifies state changes.
- **Responsive parity**: interactions (connect, expand info, disconnect) must remain one-step and ergonomic across breakpoints.
- **Accessibility**: trap focus inside overlays, announce state changes, preserve keyboard flows (Esc to close, Enter to submit).
- **Reusability**: reuse existing transport/state machinery (e.g. `BeachTerminal`) while isolating new presentation logic in dedicated components.
- **Theming**: maintain the existing Tailwind design tokens (`--background`, `--foreground`, etc.) while biasing to dark mode defaults.

## Phase 1 – Architecture & Routing Scaffold
- [x] Introduce a lightweight route switch in `main.tsx` so the root `App` chooses between the refreshed shell and the legacy UI (`AppLegacy`) when feature flags or `/v2` paths are present.
- [x] Create a feature flag utility so future A/B toggles (env var, query param) can point to the new shell without altering the old path.
- [x] Ensure build tooling (Vite dev server, preview) automatically serves `/v2` without extra configuration by relying on client-side routing.

## Phase 2 – Layout Shell & Evergreen Terminal Canvas
- [x] Stand up the new `App` shell with a full-viewport container (`min-h-screen`) hosting the terminal frame; keep background gradients subtle/flat.
- [ ] Extract terminal orchestration logic (session state, connect handler) into shared hooks to avoid drift between `App` and `AppLegacy`.
  - Introduce something like `useConnectionController` that manages `sessionId`, `passcode`, `server`, status, and connect/disconnect actions.
- [x] Render `BeachTerminal` stretched to fill the viewport, ensuring it works even while the connection modal is open (read-only until connect).
- [x] Add optional quiet status overlay (e.g., muted watermark) for idle state so the empty terminal doesn’t feel broken.

## Phase 3 – Connection Modal & Flow
- [x] Build a centered modal using shadcn `Dialog` primitives; dim background but keep terminal visible.
- [x] Port the connection fields (session ID, passcode, advanced server input) into the modal with mobile-friendly spacing and larger tap targets.
- [x] Implement validation/disabled states, inline status messaging, and a progress indicator for the connecting state.
- [x] Dismiss the modal when `status === 'connected'`; in error/closed states, reopen or present retry CTA inline.
- [ ] Ensure focus management: autofocus session field on open, trap focus inside modal while visible, restore focus when it closes (follow-up for accessibility review).

## Phase 4 – Connection Info Strip & Drawer
- [x] Introduce a persistent top bar showing host name/IP and connection state chip (latency badge still TODO).
- [x] Add a toggle button (`Info` / chevron) that expands a drawer (custom mobile sheet + desktop inline details) anchored to the top; drawer houses disconnect button, detailed metadata, advanced diagnostics.
- [ ] When connecting, animate the strip into a loading state; on errors, surface alert styling and retry inline.
- [ ] Support both pointer and keyboard interactions (Enter/Space toggles drawer, Esc closes).
- [ ] Include subtle transitions (opacity/slide) with reduced-motion fallbacks via `motion-safe` classes.

## Phase 5 – Responsive & Interaction Polish
- [ ] Audit breakpoints: ensure modal adapts on small screens (full-width sheet, consider safe-area insets) and terminal is never obscured by the on-screen keyboard.
- [ ] Confirm top strip remains tap-friendly (44px target) and drawer becomes full-screen sheet on narrow widths.
- [ ] Verify copy-to-clipboard workflow (long press / context menu); document any limitations for future iterations.
- [ ] Align typography, spacing, icon sizes to ChatGPT-inspired, flat aesthetic—prefer neutral grays, slight blur reduction, limited drop shadows.
- [ ] Sweep for focus outlines, aria attributes, and live-region announcements for status messages.

## Phase 6 – QA & Launch Readiness
- [ ] Manual regression of connection lifecycle (idle → connect → approved → disconnect → reconnect) across `/` and `/v2` routes.
- [ ] Confirm modal and drawer behavior with keyboard navigation and screen reader (VoiceOver or NVDA smoke test).
- [ ] Responsive spot checks on common breakpoints: 360×640, 768×1024, 1440×900.
- [ ] Update documentation (`docs/beach-web-dev.md`) with `/v2` access instructions and rollout notes.
- [ ] Prep follow-up tasks (migrate `/` to new shell, remove legacy panel) once adoption is confirmed.
