# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Beach is a terminal sharing application that uses WebRTC and WebSocket transports for real-time state synchronization. The project consists of three main applications:

- **beach** (`apps/beach/`) — Rust terminal client for hosting/joining sessions
- **beach-road** (`apps/beach-road/`) — Rust session server (broker/signaling)
- **beach-surfer** (`apps/beach-surfer/`) — React/TypeScript web client

## Development Commands

### Rust Applications (beach, beach-road)

```bash
# Build all Rust packages
cargo build

# Build specific package
cargo build -p beach
cargo build -p beach-road

# Run tests
cargo test

# Run specific package tests
cargo test -p beach
cargo test -p beach-road

# Run with logging
RUST_LOG=debug cargo run -p beach

# Format code
cargo fmt

# Lint
cargo clippy
```

### Web Application (beach-surfer)

```bash
# Development server
cd apps/beach-surfer && npm run dev

# Build
cd apps/beach-surfer && npm run build

# Run tests
cd apps/beach-surfer && npm run test

# Watch tests
cd apps/beach-surfer && npm run test:watch

# Preview production build
cd apps/beach-surfer && npm run preview
```

### Integration Tests

```bash
# Start Redis dependency
docker-compose up -d redis

# Run integration tests
./tests/integration/session_server.sh
```

### Deployment

```bash
# Deploy beach-road to AWS EC2
./scripts/deploy-beach-road.sh --region us-east-2 --key-name YOUR_KEY --ssh-key ~/.ssh/id_rsa
```

## Architecture

### beach (`apps/beach/`)

The terminal client is organized around the lifecycle of a Beach state-sync application:

- **cache/** — Runtime caches holding local copies of shared state. Terminal grids live in `cache/terminal/`
- **client/** — Client executables and presentation logic (TUI, input handling, predictive echo). Terminal joins in `client/terminal/`
- **mcp/** — Model Context Protocol bridges exposing Beach sessions to external tools
- **model/** — Value objects representing synchronized state and diffs
- **protocol/** — Wire-format definitions for frames, bootstrap envelopes, and feature flags
- **server/** — Host-side orchestration, PTY runtimes, and viewer management. Terminal hosting in `server/terminal/`
- **session/** — Broker interactions, session registration, authorization, and shared UX helpers
- **sync/** — Synchronization pipelines, delta streams, backfill schedulers, and prioritized lanes
- **telemetry/** — Logging, metrics, and performance guards
- **transport/** — Transport abstractions (WebRTC, WebSocket, IPC, SSH bootstrap) and supervision utilities

**Conventions:**
- Reusable/generic code at parent directory level
- Implementation-specific code under `*/terminal/` subdirectories
- Keep `main.rs` thin: argument parsing, logging setup, delegation only
- Cross-module APIs preferred over deep imports
- Every structural change requires a deterministic test

### beach-road (`apps/beach-road/`)

Session server (broker/signaling) handling:
- Session registration and validation
- Client joining and passphrase authentication
- WebRTC signaling via WebSocket
- Redis-backed session storage

Key modules:
- **handlers.rs** — HTTP/REST endpoint handlers
- **websocket.rs** — WebSocket signaling logic
- **signaling.rs** — WebRTC signaling protocol
- **storage.rs** — Redis session storage
- **cli.rs** — CLI with debug client commands

### beach-surfer (`apps/beach-surfer/`)

React/TypeScript web client for joining terminal sessions:

- **components/** — React components including BeachTerminal
- **transport/** — WebRTC and WebSocket transport implementations
- **terminal/** — Terminal grid state management and backfill controller
- **protocol/** — Wire format protocol matching Rust implementation

Built with Vite, Vitest, and TailwindCSS.

## Running Beach Locally

### Host a terminal session:
```bash
# Start Redis for session server
docker-compose up -d redis

# Start beach-road session server
cargo run -p beach-road

# In another terminal, host a session
cargo run -p beach
# Or with custom session server:
cargo run -p beach -- --session-server http://localhost:8080
```

### Join a session:
```bash
# Join via CLI
cargo run -p beach -- join <SESSION_ID>

# Or join via web client
cd apps/beach-surfer && npm run dev
# Navigate to http://localhost:5173 and enter session ID
```

### SSH bootstrap:
```bash
cargo run -p beach -- ssh user@host
```

## Environment Variables

### beach (terminal client)
- `BEACH_SESSION_SERVER` — Session server URL (default: `https://api.beach.sh`)
- `BEACH_LOG_LEVEL` — Log level: error, warn, info, debug, trace (default: warn)
- `BEACH_LOG_FILE` — Path to write structured logs

### beach-road (session server)
- `REDIS_URL` — Redis connection URL (default: `redis://localhost:6379`)
- `RUST_LOG` — Logging level (default: warn)
- `BEACH_ROAD_PORT` — Server port (default: 8080)

### Playwright Tests (beach-surfer)
```bash
# Install browsers
npx playwright install

# Run end-to-end tests
npx playwright test
```

## Key Technical Details

- **State Synchronization:** Uses delta streams with backfill for terminal grid state
- **Transports:** Supports WebRTC (primary), WebSocket (fallback), IPC, and SSH bootstrap
- **Terminal Emulation:** Uses Alacritty terminal emulator (`alacritty_terminal` crate)
- **MCP Integration:** Exposes Beach sessions as Model Context Protocol servers for external tools
- **Session Management:** Redis-backed with SHA-256 passphrase hashing
- **Authorization:** Join requests can be approved/denied by host with prompt system
