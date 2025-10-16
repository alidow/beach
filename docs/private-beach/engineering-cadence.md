# Private Beach Engineering Cadence & Observability Baseline

## Cadence
- **Solo hacker mode:** one-week build loops with end-of-week self-review. Capture checkpoints in `docs/private-beach/roadmap.md` and adjust priorities as needed.
- **Interface-first:** before coding a component, sketch the API/contract in the relevant spec (Beach Manager, Beach Buggy, Private Beach UI).
- **Temp spikes:** quick experiments or integration scaffolding can live under `temp/` but must either graduate into docs/tests or be deleted once the interface is validated.
- **Document deltas:** every meaningful decision/change should land with a short note in the pertinent doc so future contributors have context.

## Observability Baseline
- **Logging:** default to structured tracing (`tracing` crate for Rust services, `console.debug` wrappers for the web). Include correlation IDs (`private_beach_id`, `session_id`, `controller_token`).
- **Metrics:** instrument latency histograms for controller actions, harness state push frequency, and agent onboarding duration. In the interim, simple counters logged to console or temp dashboards are acceptable.
- **Interface tests:** as soon as an API/harness contract is coded, write a thin integration harness (even if temporary) that exercises it end-to-end. Park these under `tests/` or `temp/` while stabilising, then promote to permanent integration tests.
- **Manual checklists:** for critical flows (session registration, controller lease, agent onboarding) maintain brief runbooks in `docs/private-beach/` until automated tests exist.

## Tooling Expectations
- Use `just`/`npm`/`cargo` scripts to keep common workflows one-command simple.
- Prefer lightweight CI (local `cargo fmt && cargo test`, `npm run lint`) before merges even if run manually.
- Keep observability optional for local dev (env toggles) but production-ready defaults must be defined before GA.
