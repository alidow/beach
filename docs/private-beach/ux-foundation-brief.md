# Private Beach Surfer UX Foundations — Kickoff Brief

Owner: Codex (2025-06-19)

## Context
- Roadmap Phase 4 (`docs/private-beach/remaining-phases-plan.md`) calls for a dedicated UX pass covering IA, design system, accessibility, performance, and auth polish.
- Surfer currently reuses ad-hoc Beach Surfer components with minimal theming and relies on query-string tokens for auth.
- This brief captures the starting assumptions and tasks so design + frontend work can proceed in parallel with fast-path transport.

## Goals
1. Establish a cohesive design system (Tailwind tokens + shadcn/ui primitives) shared across Private Beach surfaces.
2. Define the navigation/IA model for the dashboard (sessions, automations, settings, shared state, analytics).
3. Set accessibility + performance acceptance bars and verification steps (axe-core, Lighthouse, keyboard smoke tests).
4. Scope the auth migration path from query-parameter tokens to Beach Gate OIDC flows (cookies/headers).
5. Document layout persistence requirements ahead of the server-source-of-truth milestone (M0).

## Initial Decisions & Assumptions
- **Design System Packaging**
  - Create `apps/private-beach/ui` package exporting Tailwind theme tokens, typography scale, and shared components (button, badge, card, dialog, toast).
  - Favor shadcn/ui base components to accelerate delivery; ensure tokens encode Beach brand palette (sunset orange, deep blue) and dark-mode variant.
- **Navigation / IA**
  - Primary sidebar with sections: Sessions, Automations, Shared State, Analytics, Settings.
  - Secondary top bar houses beach switcher, controller status, latency badge surface, and auth menu.
- **Layout Management**
  - Persist drag-and-resize metadata per beach via Manager layout API (dependent on M0).
  - Provide quick filters and search entry field above layout grid (deferred until data model lands).
- **Accessibility**
  - All interactive components must pass WCAG 2.1 AA contrast and support keyboard focus rings.
  - Motion-sensitive elements expose “reduced motion” preference.
- **Performance**
  - Enforce Lighthouse PWA/performance score ≥ 85 on local harness (90 target).
  - Streaming tiles opt into React `useTransition` + suspense boundaries to avoid blocking layout.

## Deliverables
1. **Design Tokens & Docs**
   - Tailwind config updates with semantic color/spacing/typography tokens.
   - Storybook (or MDX docs) enumerating primary components and variants.
   - FIGMA file link placeholder documented once design artifacts exist.
2. **Navigation & Layout**
   - Sidebar + top-bar scaffolding with responsive behaviour.
   - Placeholder pages for Automations, Shared State, Analytics with loading skeletons.
   - Layout grid component API doc (resizable tiles, persistence hooks).
3. **Accessibility/Performance Verification**
   - `npm run lint:accessibility` script invoking axe-core against key routes.
   - Lighthouse CI config + budget thresholds checked into repo.
   - Manual QA checklist (keyboard navigation, reduced motion, screen-reader smoke steps).
4. **Auth Migration Plan**
   - Document cookie-based session strategy (Clerk/Beach Gate), fallback for local dev.
   - Feature flag plan for switching from query tokens to headers/cookies.

## Next Steps
1. Draft Tailwind token file and base theme (paired with design review).
2. Scaffold sidebar/top-bar components in Surfer with placeholder routes.
3. Add axe-core + Lighthouse scripts to `package.json`.
4. Start auth migration design doc (token exchange, session storage, fallback).

## Open Questions
- How do we expose shared component tokens to Cabana dashboards to avoid drift?
- Should we adopt Storybook or leverage the existing Docs Router within Next.js for component demos?
- Are there legacy browsers we must support that influence Tailwind/ES targets?
- What telemetry should we capture for UX health (CLS, FID, error rates)?
