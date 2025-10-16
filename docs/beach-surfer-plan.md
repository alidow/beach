# Beach Surfer Client (TypeScript + React) – Phased Plan

Goals
- Ergonomic: a drop‑in React component with a small, intuitive API; easy to mock, test, and theme.
- Fast and efficient: smooth at 60fps; resilient under bursty server output; responsive when rapidly scrolling; minimal memory/GC pressure.
- Faithful to server: no terminal emulation in the browser; render server diffs/snapshots precisely.

Non‑Goals
- Emulating VT sequences in the browser (xterm.js). All diffs come from the server.
- Managing the session lifecycle server‑side (handled by beach-road and beach host).

Architecture Overview
- Transport: WebRTC data channel to host (primary), with signaling over beach-road WebSocket. (Optional WS transport can be added later if needed.)
- Protocol: Binary wire codec in TS that exactly mirrors apps/beach protocol (varints + enums). Frames: Hello, Grid, Snapshot, SnapshotComplete, Delta, HistoryBackfill, InputAck, Shutdown. Client: Input, Resize, RequestBackfill.
- State: Headless “grid store” modeled after GridRenderer, adapted for browser. Tracks base_row, cols, row slots (Pending | Loaded | Missing), styles, selection, followTail, predictions.
- UI: React component renders the viewport using virtualized rows and styled spans. Native scroll and native selection for macOS Terminal feel.
- Performance: Decode/batch on a worker, main-thread render via requestAnimationFrame; coalesce updates, virtualize rows, cap in-memory history, and on-demand backfill.

Deliverables & Phases

Phase 0 — Foundations
- Deliverables
  - apps/beach-surfer/ scaffold (Vite, React, TypeScript).
  - Shared ESM package inside apps/beach-surfer/src for protocol types and codec.
  - Tooling: ESLint, Prettier, tsconfig strict, vitest for unit tests.
- DevX
  - Local .env support for session server base URL.
  - Example page wiring session id/passphrase from query params.
- Success
  - `pnpm dev` runs a hot-reloading dev server.
  - Unit test harness runs in CI.

Phase 1 — Protocol & Transport
- Deliverables
  - protocol/wire.ts: varint encoder/decoder; HostFrame/ClientFrame encode/decode; Update decoding; color/attrs mapping.
  - transport/signaling.ts: WS signaling client speaking beach-road’s JSON schema (Join, PeerJoined, Signal, Pong, Error).
  - transport/webrtc.ts: RTCPeerConnection + ordered, reliable RTCDataChannel wrapper, with the same message envelope as Rust (1 byte type, 8 byte seq, 4 byte payload length, payload).
- DevX
  - Type-safe enums and interfaces mirroring Rust wire.rs.
  - Protocol fuzz tests: random sequences round-trip in tests.
- Success
  - Connects to beach-road WS and establishes a data channel to a host in a mocked/integration setup.
  - Decodes Hello and Grid frames into typed objects with unit tests.

Phase 2 — Grid Store (Headless)
- Deliverables
  - terminal/gridStore.ts: core state and mutation methods:
    - applyUpdate(update), applySnapshot(updates), applyDelta(updates)
    - applyTrim(start,count), setStyle(id, fg, bg, attrs)
    - setBaseRow(base), ensureSize(rows, cols), markRow(Pending|Missing)
    - selection/followTail controls; viewport calculations.
  - Minimal memory policy: retain only [viewport ± lookaround] rows; evict beyond limits and mark as Missing; request backfill on demand.
- DevX
  - Expose a headless hook `useBeachTerminalState()` returning state selectors and actions for custom UIs.
- Success
  - Deterministic unit tests mirroring GridRenderer behavior for Cell/Row/RowSegment/Rect/Trim/Style.

Phase 3 — Minimal React Component *(initial implementation landed)*
- Deliverables
  - components/BeachTerminal.tsx with props:
    - sessionId, baseUrl, passcode? (or `transport` override)
    - appearance: fontFamily, fontSize, theme (colors), showStatusLine?
    - behavior: followTail (default true), overscanRows, disableSelection?
    - callbacks: onConnected, onError, onResize, onStatus(message)
    - ref API (imperative): focus(), copySelection(), scrollTo(line), setFollowTail(bool), search(query, dir), exportText(opts)
  - Virtualized viewport (react-window or react-virtuoso). Styled spans per run; soft cursor.
- DevX
  - Showcase app page with connect UI and component playground knobs.
- Success
  - Joining a live session renders visible rows; tail follows; basic scroll works.

Phase 4 — Input, Clipboard, Resize *(key/resize hooks implemented; clipboard pending)*
- Deliverables
  - keymap.ts mapping KeyboardEvent to byte sequences (Ctrl/Alt handling as in Rust encode_key_event).
  - Input handling: send ClientFrame::Input; predictive echo optional; clear on InputAck.
  - Resize observer: compute rows/cols from element/client rect & measured glyph metrics; throttle; send ClientFrame::Resize.
  - Clipboard via native selection; copy button exposes getSelectedText().
- Success
  - Interactive shells respond to typing; window resizes keep server in sync.

Phase 5 — Backfill & Fast Scrolling *(basic backfill planner + scroll trigger complete; gaps: virtualization + smarter gap detection)*
- Deliverables
  - Backfill planner port (simplified from TerminalClient):
    - Track known base_row, highest_loaded_row, pending requests, empty-tail ranges.
    - When scrolled up, find first unloaded gap around viewport and request up to BACKFILL_MAX_ROWS_PER_REQUEST.
    - Throttle and dedupe requests; respect timeouts; treat Trim as authoritative for advancing base.
  - Pending visual placeholders for unloaded rows.
- Success
  - Rapid scroll up shows placeholders then fills quickly without UI jank.

Phase 6 — Performance Optimizations
- Deliverables
  - Worker offload: decode frames and coalesce updates in a Web Worker; postMessage compact batches to main thread.
  - Render scheduler: batch mutations and schedule React updates with requestAnimationFrame; cap to ~60fps; coalesce multiple frames.
  - Memory: ring buffer retention and row compaction outside viewport (e.g., store long rows as text + style runs; only expand to per-cell arrays when in/near viewport).
  - Pools: reuse typed arrays/buffers to reduce GC churn.
- Budgets
  - Under burst (e.g., 10–20 MB/s of updates), main thread stays responsive; no dropped input due to JS pauses > 50ms.
  - Typical render path < 4ms for 24×120; decoding happens off-main thread.
- Success
  - Trace-based benchmarks show steady 60fps interactions during sustained output.

Phase 7 — Ergonomics & Theming
- Deliverables
  - Headless + UI split: `useBeachTerminal()` (hook) and `<BeachTerminal />` (component) exported.
  - Theming via CSS variables (colors for fg/bg, cursor, selection) and light/dark presets.
  - Controlled props for followTail and selection; event callbacks for search results.
- Success
  - Consumers can easily compose their own chrome (toolbar, search input) around the headless hook.

Phase 8 — Reliability & Observability
- Deliverables
  - Reconnect strategy (backoff) for signaling and data channel; display connection state.
  - Metrics: 
    - transport bytes in/out, frames decoded, dropped frames, backfill requests, decode/render timings
    - expose via optional onMetrics callback and debug overlay.
  - Robust error boundaries and fallbacks.
- Success
  - Chaos tests (packet loss/latency) show graceful degradation and recovery.

Phase 9 — Hardening & Docs
- Deliverables
  - Comprehensive docs: API reference, integration recipes, performance guide, troubleshooting.
  - Cross-browser QA (Chromium, WebKit, Gecko).
  - Versioned package publish (internal workspace or npm scoped).
- Success
  - Consumers integrate the component with minimal code and predictable behavior.

Detailed Design Notes

Protocol (TS Codec)
- Implement varint read/write exactly as Rust wire.rs.
- Map Update::Style fg/bg 24‑bit integers to CSS colors; attrs bitmask → CSS font-weight/italic/underline and reverse video.
- Validate PROTOCOL_VERSION before parsing; tolerate unknown frames by dropping (with metrics).

State & Memory Model
- Row slots: Pending | Missing | Loaded(RowState { cells, latestSeq }).
- Eviction: retain [viewport ± N] rows (configurable); evict older to Missing to keep memory bounded; rely on backfill to reload when needed.
- Styles: LRU cache of computed CSS style objects keyed by styleId.
- Predictions: map input seq → predicted positions; clear on InputAck.

Rendering Pipeline
- Compute row display width by trimming trailing spaces (as GridRenderer does) to minimize DOM.
- Convert consecutive cells sharing styleId into spans; prefer textContent over many nested nodes.
- Virtualize rows with overscan; render pending rows as faint placeholders.
- Cursor: soft caret overlay at (row,col) computed from updates; avoid interfering with native selection.

Backfill Behavior
- Disable backfill while followTail is true.
- When user scrolls up, identify first unloaded gap around viewport; issue RequestBackfill within limits; throttle via intervals; avoid reissuing known-empty tail ranges; advance known base on Trim.

Input Encoding
- Keyboard: map Alt/Meta → ESC prefix (optionally), Ctrl → ^A..^Z, arrows/Home/End/Page sequences as in encode_key_event() from Rust.
- Clipboard: use browser selection; provide helpers to copy/export selection text.

Performance Guardrails
- Decode & coalesce in a Worker; main thread only applies consolidated batches per raf.
- Avoid per-cell React components; generate one line element with minimal spans per style run.
- Use typed arrays and buffer pools for parsing to reduce allocations.
- Avoid storing unbounded history; prefer on-demand backfill.

Developer Ergonomics
- Two integration paths:
  - Headless: `const term = useBeachTerminal({ sessionId, baseUrl, passcode });` → returns { state, actions }.
  - Drop‑in: `<BeachTerminal sessionId baseUrl passcode theme={...} onConnected={...} />`.
- Mockable: provide `MockTransport` for storybook and tests to simulate frames.
- Minimal props, sensible defaults, and small imperative ref API for common actions.

Testing Strategy
- Unit: protocol codec (golden vectors vs Rust), grid store updates (mirroring GridRenderer tests), backfill planner edge cases (trim/gaps).
- Integration: headless store + mocked transport (burst streams, large deltas, quick scrolls).
- E2E (Playwright): user typing, resize, scroll/backfill, reconnect, selection/copy.

Rollout Plan
- Start with headless hook and minimal component to unlock early UX iteration.
- Gate worker offload + compaction behind flags; enable after benchmarks stabilize.
- Maintain compatibility with server protocol; keep a feature flag to log/ignore unknown frames.

Open Questions / Risks
- Very long lines: ensure efficient run segmentation and DOM node counts; consider soft-wrapping options in future.
- Backfill under extreme churn: ensure range dedupe and timeout handling match server behavior closely.
- Style explosion: mitigate via style caching and CSS variables.

Appendix — Proposed Public API (v0)
```ts
export type BeachTerminalProps = {
  sessionId: string;
  baseUrl: string; // e.g., http://127.0.0.1:8080
  passcode?: string;
  transport?: Transport; // optional override
  appearance?: { fontFamily?: string; fontSize?: number; theme?: BeachTheme };
  behavior?: { followTail?: boolean; overscanRows?: number; disableSelection?: boolean };
  onConnected?: () => void;
  onError?: (err: Error) => void;
  onResize?: (cols: number, rows: number) => void;
  onStatus?: (text: string) => void;
};

export type BeachTerminalRef = {
  focus(): void;
  copySelection(): void;
  scrollTo(line: number): void;
  setFollowTail(follow: boolean): void;
  search(query: string, dir: 'forward' | 'backward'): void;
  exportText(opts?: { range?: [number, number] }): string;
};
```
