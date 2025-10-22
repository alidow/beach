# Surfer UX Foundations — Issue Backlog (Draft)

Owner: Codex — 2025-06-19

> Track implementation issues for Milestone M1 (Surfer UX Foundations).  
> Convert each bullet into a GitHub issue when ready; keep IDs/assignees updated.

## Design System & Theming
- [ ] ISSUE: Extract Tailwind design tokens + color palette into shared `apps/private-beach/ui/tokens.ts`.
- [ ] ISSUE: Integrate shadcn/ui primitives; publish Storybook (or MDX docs) demonstrating buttons, badges, cards, dialogs, toasts.
- [ ] ISSUE: Implement dark-mode token variants; verify contrast meets WCAG 2.1 AA.

## Navigation & Layout
- [ ] ISSUE: Scaffold sidebar + top navigation (sessions, automations, shared state, analytics, settings).
- [ ] ISSUE: Implement responsive layout grid with drag/resize persistence (uses Manager layout API once M0 lands).
- [ ] ISSUE: Add placeholder routes for Automations / Shared State / Analytics with skeleton loaders.

## Accessibility & Performance
- [ ] ISSUE: Add `npm run lint:a11y` (axe-core) and integrate into CI.
- [ ] ISSUE: Configure Lighthouse CI budgets (performance ≥85, accessibility ≥90, best practices ≥90).
- [ ] ISSUE: Document keyboard navigation + reduced-motion QA checklist.

## Auth Migration
- [ ] ISSUE: Design OIDC/session cookie flow for Private Beach (replace `access_token` query param).
- [ ] ISSUE: Implement Clerk/Beach Gate token exchange on the server; persist session cookies; expose local dev overrides.
- [ ] ISSUE: Remove query string token fallback after rollout verification.

## Telemetry & Debuggability
- [ ] ISSUE: Emit UX health metrics (CLS, FID, error rate) to existing telemetry sinks; surface debug overlay toggle.
