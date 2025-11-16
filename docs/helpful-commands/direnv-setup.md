# Direnv setup for Beach

We now ship a repository-level `.envrc` that keeps the WebRTC NAT hints up to date
so beach-manager (running inside Docker) advertises a LAN-reachable ICE address.
With direnv installed, every `docker compose up` automatically inherits the
`BEACH_ICE_PUBLIC_IP` / `BEACH_ICE_PUBLIC_HOST` variables—no more manual edits to
`.env.local`.

## One-time installation

1. Install direnv (macOS: `brew install direnv`; Linux: `sudo apt install direnv` or
   grab the binary from https://direnv.net).
2. Add the shell hook to your profile (zsh example):
   ```sh
   echo 'eval "$(direnv hook zsh)"' >> ~/.zshrc
   ```
   Restart the shell or `source ~/.zshrc` so the hook is active.
3. In the repo root, authorize the new `.envrc` once:
   ```sh
   direnv allow
   ```

From now on, every time you `cd /path/to/beach`, direnv sets `BEACH_ICE_PUBLIC_IP`
to your current Wi‑Fi/Ethernet IP (ignoring Docker’s own bridges) and exports it as
`BEACH_HOST_LAN_IP` for other tooling. Running `docker compose up` just works—the
manager shares a host candidate the browser can dial directly, so fast-path doesn’t
depend on hairpin NAT or TURN.

### Overriding

If you need to force a specific IP, simply export `BEACH_ICE_PUBLIC_IP` yourself
before entering the repo; the `.envrc` respects existing values and won’t override
them. You can also `direnv deny` to disable the automation on a per-machine basis.
