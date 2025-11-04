Chrome DevTools MCP with Codex

- Requirements: `node >= 20`, `npm`, Chrome (stable or newer).
- Config: added `mcp_servers.chrome-devtools` to `~/.codex/config.toml` so Codex can use it.

How to use

- In a Codex chat, run: `Check the performance of https://developers.chrome.com`.
- Codex will launch Chrome as needed and record a trace.

Config details

- File: `~/.codex/config.toml`
- Entry:
  - `command`: `npx`
  - `args`: `[-y, chrome-devtools-mcp@latest]`
  - `startup_timeout_sec`: `30`

Optional tweaks

- Headless + isolated profile: add `"--headless=true"` and `"--isolated=true"` to `args`.
- Connect to your own Chrome: start Chrome with `--remote-debugging-port=9222` and add `"--browser-url=http://127.0.0.1:9222"` to `args`.

Troubleshooting

- If sandboxing blocks Chrome from launching, connect via `--browser-url` as above.
- See upstream docs for more: `docs/troubleshooting.md` in the repo.
