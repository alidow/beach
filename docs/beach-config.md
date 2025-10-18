Beach CLI key configuration

Location
- Config file path: `~/.beach/config` (TOML)

Keys (client hotkeys)
- Section: `[client.keys]`
  - `scroll_toggle`: list of key combos that toggle between tail and scrollback.
    - Example: `scroll_toggle = ["Ctrl+Esc", "Alt+s"]`
  - `double_esc`: enable ESC ESC to toggle (default: true).
    - Example: `double_esc = true`
  - `copy_shortcuts`: list of key combos that copy the selection and exit copy-mode.
    - Example: `copy_shortcuts = ["Ctrl+c", "Super+c", "Ctrl+Shift+c", "Ctrl+Insert"]`

Notes
- Key names are case-insensitive. Supported modifiers: `Ctrl`, `Alt` (aka `Option`/`Opt`), `Shift`, `Super` (aka `Cmd`/`Command`).
- Supported special keys include: `Esc`, `Enter`, `Tab`, `Backspace`, `PageUp`, `PageDown`, `Home`, `End`, `Space`, `Delete`, `Insert`.

Environment overrides
- `BEACH_SCROLL_TOGGLE_KEY` (comma-separated list) overrides `[client.keys].scroll_toggle`.
  - Example: `BEACH_SCROLL_TOGGLE_KEY="Ctrl+Esc,Alt+s"`
- `BEACH_COPY_SHORTCUTS` (comma-separated list) overrides `[client.keys].copy_shortcuts`.
  - Example: `BEACH_COPY_SHORTCUTS="Ctrl+c,Super+c"`
- `BEACH_COPY_MODE_KEYS` selects vi/emacs bindings within copy-mode (`vi` default, or `emacs`).

Defaults
- Scroll toggle: `Ctrl+Esc` plus ESC ESC (double press within 400ms).
- Copy in copy-mode: `Cmd/Ctrl(OS)+C`, `Ctrl+Shift+C`, `Ctrl+Insert`, and `Ctrl+C`.

Example `~/.beach/config`

```toml
[client.keys]
scroll_toggle = ["Ctrl+Esc", "Alt+s"]
double_esc = true
copy_shortcuts = ["Ctrl+c", "Ctrl+Shift+c", "Super+c"]
```

