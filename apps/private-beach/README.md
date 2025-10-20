# Private Beach Surfer (Next.js)

## Purpose
- Premium dashboard for orchestrating Beach sessions inside a Private Beach.
- Hosts layout management, multi-session monitoring, and automation controls on top of Beach Manager APIs.

## Planned Stack
- Next.js 14 (App Router) + TypeScript.
- TailwindCSS + shadcn/ui component primitives.
- Zustand (or similar) for client state, React Query for API data.
- WebRTC/WebSocket bridges to stream terminal & Cabana feeds rendered via custom components.

## Current UI (Phase 4 UX)
- Beaches: list, create, and manage beaches backed by Beach Manager (Postgres).
- Dashboard: live Sessions panel and a tile canvas with layout presets (Grid / 1+3 / Focus), per‑tile controls (Acquire/Release/Stop), and a detail drawer streaming SSE events/state.
- Settings: per‑beach Manager URL and token configuration (local dev) stored via Manager metadata.

## How to Run
1) Start Manager (see docs/private-beach/STATUS.md for Postgres/Redis and `cargo run -p beach-manager`).
2) `cd apps/private-beach && npm install && npm run dev -- -p 3001`
3) Open `http://localhost:3001/beaches`.
4) Click New Beach, fill Name, optionally generate ID, set Manager URL (e.g., `http://localhost:8080`) and a dev token (e.g., `test-token`).
5) Open the beach. Use the left Sessions panel to add sessions to the canvas. Use the layout selector and tile controls. Open the right drawer to view live SSE events.

Notes:
- The tile surface currently shows a live placeholder; WebRTC/WS stream rendering hooks are stubbed for later fast‑path work.
- Auth is dev‑friendly (token field); OIDC/Beach Gate login will replace this in a follow‑up.
- Tile layouts are persisted server-side via Drizzle + Postgres (`PRIVATE_BEACH_DATABASE_URL`); no LocalStorage is used for durable state.
- When using `docker compose up`, the `private-beach-migrate` one-shot service runs the SQL files under `apps/private-beach/drizzle/` against the Surfer database before the dev server starts.

## Dev Scripts
- `npm run dev` – local dev server (Next.js).
- `npm run lint` – lint sources.
- `npm run build` / `npm run start` – production build & serve.
- `npm run db:generate` / `npm run db:migrate` – Drizzle schema snapshot + migrations.

## Styling Setup (Tailwind + shadcn/ui)

1. Install TailwindCSS + PostCSS deps:
```
npm install -D tailwindcss postcss autoprefixer
npx tailwindcss init -p
```

2. Configure `tailwind.config.js` content paths:
```
  content: [
    "./src/pages/**/*.{js,ts,jsx,tsx}",
    "./src/components/**/*.{js,ts,jsx,tsx}",
    "./src/app/**/*.{js,ts,jsx,tsx}",
  ],
```

3. Create `src/styles/globals.css` with Tailwind directives and import it in `src/pages/_app.tsx`:
```
@tailwind base;
@tailwind components;
@tailwind utilities;
```

4. shadcn/ui: we use lightweight Tailwind primitives under `src/components/ui`. If you prefer full shadcn scaffolding, run:
```
npx shadcn-ui@latest init
npx shadcn-ui@latest add button input dialog sheet badge card
```

5. Theme and tokens: keep colors/spacing as CSS variables in `globals.css`; prefer unstyled shadcn primitives + Tailwind utilities for layout.

## Folder Topology (Draft)
- `src/app` – Next.js routes/app shell.
- `src/components` – reusable UI widgets (session tiles, control overlays).
- `src/lib` – API clients (manager SDK, Beach Gate auth helpers).
- `src/styles` – Tailwind configuration/extensions.

## Notes
- Keep core UI logic open for plugin-style extension as we explore partner experiences.
- Respect the “UX Minimalism” principle: optimize first for session overview, controller handoff, share-link flow.
- Set `PRIVATE_BEACH_DATABASE_URL` (defaults in docker-compose to `postgres://postgres:postgres@beach-postgres:5432/private_beach_surfer`) to run the Surfer persistence locally.
