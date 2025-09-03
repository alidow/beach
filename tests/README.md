# Beach Test Suite

This directory contains integration and system tests for the Beach terminal sharing application.

## Structure

```
tests/
├── README.md           # This file
├── integration/        # Integration tests
│   └── session_server.sh   # Tests session server registration and joining
├── e2e/               # End-to-end tests (future)
└── utils/             # Test utilities and helpers
    └── common.sh      # Common test functions
```

## Prerequisites

- Docker (for Redis)
- Rust toolchain
- Beach workspace built (`cargo build`)

## Running Tests

### Quick Start

```bash
# Start dependencies (Redis)
docker-compose up -d redis

# Run all integration tests
./tests/integration/session_server.sh
```

### Individual Test Suites

#### Session Server Tests
Tests the beach-road session server functionality including:
- Session registration
- Session validation
- Client joining
- Passphrase authentication

```bash
./tests/integration/session_server.sh
```

## Test Environment Variables

- `BEACH_SESSION_SERVER`: Override the session server address (default: localhost:8080)
- `REDIS_URL`: Override Redis connection URL (default: redis://localhost:6379)
- `BEACH_ROAD_PORT`: Override beach-road port (default: 8080)

## Writing New Tests

1. Create a new script in the appropriate directory
2. Source `tests/utils/common.sh` for common functions
3. Follow the naming convention: `test_name.sh`
4. Make the script executable: `chmod +x tests/category/test_name.sh`
5. Update this README with test documentation