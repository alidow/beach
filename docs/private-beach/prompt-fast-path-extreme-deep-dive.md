# Extreme Deep-Dive Prompt: Fast-Path ACK/State Failure

## Context

The Pong fast-path smoke test consistently fails because the host never sees the `__ready__` sentinel for the fast-path ACK (`mgr-acks`) and state (`mgr-state`) channels, even though the manager logs that it sends the sentinel repeatedly. Controller (`mgr-actions`) succeeds. We already suspect a secure-transport mismatch (host disables encryption for ACK/State due to missing metadata, manager still encrypts), but we need an exhaustive investigation to rule out other causes and provide the strongest possible diagnosis without modifying code.

## Prompt

```
You are the second AI agent. Your task is to perform an exhaustive code investigation (treat it like a 10x deep dive) into why the fast-path ACK (`mgr-acks`) and state (`mgr-state`) data channels never receive the `__ready__` sentinel during the Pong fast-path smoke test. Do not change any code. Instead, inspect every relevant part of the stack, connect the dots, and produce the most detailed diagnosis and best-guess root causes you can.

Constraints:
  • No automatic code edits. Read, trace, and reason only.
  • Gather evidence from all relevant components: host CLI, manager fast-path stack, metadata plumbing, controller/fast-path bridging, SCTP channel configs, etc.
  • Cross-reference logs (see `temp/pong-fastpath-smoke/<run>/container/*.log`) and instrumentation to corroborate or refute theories.
  • Prioritize accuracy and completeness over brevity. If multiple plausible causes exist, enumerate each with supporting evidence.

Deliverables:
  1. Clear explanation of the handshake flow for fast-path channels (mgr-actions, mgr-acks, mgr-state) on both host (apps/beach/…) and manager (apps/beach-manager/…).
  2. Detailed analysis of how secure transport/encryption is negotiated for each peer, including any metadata dependencies.
  3. Enumeration of every spot where the `__ready__` sentinel could be dropped or misinterpreted (e.g., before framing, during decryption, pending queues, SCTP ordering).
  4. Final diagnosis: best explanation(s) for why the host never sees `__ready__` on ack/state, with file/line citations.
  5. If uncertainty remains, list targeted experiments or logs we should add next (still without changing code).

Be methodical. Assume we need a write-up that could stand on its own in the incident log.
```
