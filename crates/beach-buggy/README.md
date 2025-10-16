# Beach Buggy Harness Runtime

Shared harness library that wraps Beach sessions (terminal, Cabana GUI, future media types). Designed for ultra-low-latency diff streaming and action execution. Implementations inside `apps/beach` and `beach-cabana` link to this crate so they benefit from the same codecs and controller contract.
