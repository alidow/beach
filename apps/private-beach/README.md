# Private Beach Surfer (Next.js)

## Purpose
- Premium dashboard for orchestrating Beach sessions inside a Private Beach.
- Hosts layout management, multi-session monitoring, and automation controls on top of Beach Manager APIs.

## Planned Stack
- Next.js 14 (App Router) + TypeScript.
- TailwindCSS + shadcn/ui component primitives.
- Zustand (or similar) for client state, React Query for API data.
- WebRTC/WebSocket bridges to stream terminal & Cabana feeds rendered via custom components.

## Initial Tasks
1. Scaffold Next.js app with TypeScript, ESLint, Tailwind.
2. Implement auth wrapper that consumes Beach Gate tokens & entitlements.
3. Build placeholder dashboard shell (top nav, private beach selector, empty grid).
4. Integrate mock data service for local development before wiring to live manager.

## Dev Scripts
- `npm run dev` – local dev server (Next.js).
- `npm run lint` – lint sources.
- `npm run build` / `npm run start` – production build & serve.

## Folder Topology (Draft)
- `src/app` – Next.js routes/app shell.
- `src/components` – reusable UI widgets (session tiles, control overlays).
- `src/lib` – API clients (manager SDK, Beach Gate auth helpers).
- `src/styles` – Tailwind configuration/extensions.

## Notes
- Keep core UI logic open for plugin-style extension as we explore partner experiences.
- Respect the “UX Minimalism” principle: optimize first for session overview, controller handoff, share-link flow.
