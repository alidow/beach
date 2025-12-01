# Beach Buggy Harness Runtime

Shared harness library that wraps Beach sessions (terminal, Cabana GUI, future media types). Designed for ultra-low-latency diff streaming and action execution. Implementations inside `apps/beach` and `beach-cabana` link to this crate so they benefit from the same codecs and controller contract.

Separation of concerns:
- CLI hosts (`apps/beach`) should remain controller-agnostic: they negotiate transports, attach the unified bridge, and render/apply terminal state. They should not parse controller frames or be aware of manager-specific labels.
- Beach Buggy owns all manager/controller semantics (actions/acks/state/health) over the unified channel. If you need to change how controllers talk to hosts, update this harness first rather than baking controller logic into the host binary.
