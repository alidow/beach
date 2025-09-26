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
2. Enter:
   - **Session ID**: the UUID printed by `beach-human host`
   - **Base URL**: the beach-road base (e.g., `http://127.0.0.1:8080` by default)
   - **Passcode**: fill if one was shown in the host output
3. Toggle **Auto Connect** and the client will negotiate WebRTC -> data channel. The terminal output should appear in the viewport.

Keyboard input, window resize, and scroll-triggered history backfill are already wired up. Use the checkbox to disconnect/reconnect without reloading the page.

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
