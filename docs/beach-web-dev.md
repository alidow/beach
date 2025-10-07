# Beach Web Client — Local Development

This guide walks through running the React/WebRTC client locally and pointing it at a live beach-human host session.

## 1. Install dependencies

From the repository root:

```bash
cd apps/beach-web
pnpm install   # or npm install / yarn install if you prefer
```

> **Tip:** pnpm is fastest, but the project works with npm/yarn as well.

## 2. Start a beach-human host session

In a separate terminal, launch a host session so the browser client has something to connect to. Example:

```bash
cargo run --bin beach-human -- host
```

A session ID and (optionally) passcode will be printed in the terminal output when the host registers with beach-road. Leave this process running; it will maintain the live terminal.

## 3. Run the web client in dev mode

Back in `apps/beach-web`:

```bash
pnpm dev
```

Vite will boot a dev server at <http://localhost:5173>. The console will show the exact URL.

## 4. Open the browser UI

1. Navigate to the dev server URL in Chrome/Edge (Chromium) or the latest Safari/Firefox.
2. Pick your entry point:
   - `/` renders the legacy split layout (connection pane + terminal).
   - `/v2` (or append `?ui=v2`) loads the new terminal-first preview described below.
3. Enter the session details supplied by your host:
   - **Session ID**: the UUID printed by `beach-human host`.
   - **Session Server**: the beach-road base (e.g., `http://127.0.0.1:8080`).
   - **Passcode**: only if one was shown in the host output.
4. Press **Connect**. Within a few seconds the terminal should attach and begin streaming live output.

Keyboard input, window resize, and scroll-triggered history backfill are already wired up. The top info bar exposes disconnect/reconnect controls in the new `/v2` shell.

### Terminal-first preview (`/v2`)

- Visit <http://localhost:5173/v2> (or append `?ui=v2` to any dev URL) to try the new single-pane experience.
- A full-screen terminal renders immediately; the connection dialog floats centered on desktop and becomes a sheet on mobile.
- Once connected, the modal dismisses and a slim info bar appears at the top. Toggle it to view connection metadata, disconnect, or retry.
- The preview defaults to dark mode, flattens the chrome, and is tuned for both pointer and touch input.

## 5. Running tests

The protocol/transport/grid store have unit coverage. Run the suite with:

```bash
pnpm test
```

## 6. Troubleshooting

- If the page stays on “Connecting…”, check the browser console for network or ICE errors.
- Ensure `beach-human` and `beach-road` are on compatible commits/protocol versions.
- For remote testing, expose the dev server via `vite --host 0.0.0.0` and update the Base URL to point at the reachable beach-road instance.

Happy hacking!
