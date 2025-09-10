# Zero‑Trust P2P Terminal Sharing over WebRTC

Status: Draft v1

Owner: beach

Scope: `apps/beach` (server + client), interoperating with the untrusted signaling server in `apps/beach-road` and optional TURN.

Summary: This document specifies a zero‑trust design for establishing end‑to‑end encrypted, mutually authenticated P2P sessions between a Beach Server and Beach Client using WebRTC, while treating the signaling/TURN infrastructure as untrusted. It defines two authentication modes (Passphrase and Clerk Identity), sealed signaling to prevent SDP tampering, an application‑level authenticated key exchange (AKE) with channel binding to the DTLS session, and authorization controls enforced by the Beach Server.

---

## Goals

- Keep all terminal data end‑to‑end encrypted and confidential from signaling/TURN operators.
- Detect and prevent MITM or malicious re‑routing by the signaling server.
- Support two auth modes:
  - Passphrase mode (simple PSK UX).
  - Clerk Identity mode (JWT‑based group authorization).
- Be incremental: work with the existing monorepo and message routes; enable gradual rollout.

## Non‑Goals

- DoS resistance against signaling/TURN (they can drop or delay). 
- Browser client support (this spec targets native Rust components first; browser constraints on exporters may require adjustments).

## Threat Model

Attacker controls signaling (beach‑road) and/or TURN. Capabilities:
- Read/modify/drop signaling messages and ICE candidates.
- Attempt full MITM by terminating two independent WebRTC handshakes and relaying.

We require:
- Confidentiality and integrity for terminal stream and control messages.
- MITM detection before granting access to the PTY.
- Explicit authorization enforced by the Beach Server.

## High‑Level Design

The protocol layers are:
1) Sealed signaling (first): Offer/Answer/ICE are sealed with a key derived from a shared secret (passphrase) or identity keys to detect any change by signaling before the connection forms.
2) WebRTC DTLS/SRTP: Transport encryption at the media layer.
3) Application handshake over a reliable data channel: A short, mutually authenticated exchange derives fresh session keys and mixes in a per‑connection secret (DTLS exporter) to bind to this exact connection and defeat relayed middlemen.
4) Authorization: The Beach Server enforces passphrase or Clerk policy before exposing the PTY.

Two authentication modes:
- Passphrase Mode: Users share a human passphrase. We derive a strong shared key via Argon2id and use it for sealed signaling. After the data channel opens, we confirm with a small handshake (PAKE preferred; Noise‑with‑shared‑key acceptable initially) that mixes in a DTLS exporter.
- Clerk Identity Mode: Client presents a Clerk token; server verifies locally. Run a Noise handshake to authenticate the server (and optionally the client), mixing a DTLS exporter to bind to the exact connection.

TURN remains a dumb relay; it cannot decrypt or successfully bridge the application handshake without secrets.

## Cryptographic Primitives (targets)

- KDF: Argon2id (memory‑hard) for converting passphrases to PSK. Parameters set to resist offline attack while keeping UX acceptable (see Config).
- AEAD for sealed signaling: ChaCha20‑Poly1305 with 96‑bit nonces.
- AKE: 
  - Preferred: PAKE (SPAKE2+ or OPAQUE) in Passphrase Mode to prevent offline guessing.
  - Alternative (simpler to ship first): Noise PSK patterns using `snow` in Passphrase Mode.
  - Identity Mode: Noise IK/XX over X25519, ChaCha20‑Poly1305, BLAKE2s.
- Channel Binding: DTLS exporter (32 bytes) mixed into the AKE transcript/keys.

Notes:
- If the PSK is low‑entropy, Noise‑PSK patterns enable offline guessing from observed handshakes. PAKE avoids this; therefore PAKE SHOULD be used when feasible. Noise‑PSK MAY be used initially with Argon2id‑hardened keys and rate‑limiting.

## Message Transport and Integration Points

- Signaling path (unchanged): `ClientMessage::Signal { to_peer, signal: serde_json::Value }` forwarded by beach‑road; it MUST NOT interpret sealed payloads.
- WebRTC transport module: `apps/beach/src/transport/webrtc/mod.rs` implements SDP wrap/unwrap (sealed signaling) and the application AKE on the first data channel (label: `beach/handshake/1`).
- Session server bridge (beach‑road): continues to forward signals; no access to plaintext SDP or AKE payloads.

## Sealed Signaling (SS)

Purpose: Bind the remote’s DTLS certificate fingerprint and SDP parameters to an out‑of‑band secret (passphrase) or server static key, preventing signaling tampering.

Envelope (JSON):

```
{
  "v": "ssv1",
  "type": "offer" | "answer" | "ice",
  "cipher": "chacha20poly1305",
  "salt": "base64(16 bytes)",
  "nonce": "base64(12 bytes)",
  "ad": {
    "session_id": "…",
    "role": "server" | "client",
    "from_peer": "…",
    "to_peer": "…",
    "salt": "base64(16 bytes)"  // per‑session random
  },
  "sealed": "base64(ciphertext || tag)"
}
```

Plaintext (prior to seal):

```
{
  "sdp": "… full SDP …",
  "fingerprint": "… DTLS cert fingerprint from SDP …",
  "ts": "RFC3339 timestamp"
}
```

Key Derivation (Passphrase Mode):
- Shared key = Argon2id(passphrase, salt = random 16 bytes per session), params from Config.
- AEAD key = HKDF(derived_key, context = "beach:ss:aead:v1").

Key Derivation (Identity Mode):
- Use server’s static key pair. For SS, either:
  - Derive a sealing key from a long‑term server secret (less preferred), or
  - Skip SS and rely on AKE with channel binding (acceptable if exporter is available). This spec RECOMMENDS SS in passphrase mode and OPTIONAL in identity mode.

Verification:
- On receipt, recompute key and AEAD‑open. If open fails, abort.
- Extract SDP and remote fingerprint; proceed to WebRTC.
- Enforce freshness window (e.g., 2 minutes) using the timestamp; keep a nonce cache per session to reject replays.

Replay:
- Include timestamp in plaintext and enforce a short acceptance window (e.g., 2 minutes) and one‑time nonces per session to reduce replay risk.

## WebRTC Establishment

Proceed with normal ICE/DTLS once SS succeeds. Create a reliable, ordered data channel labeled `beach/ctrl/1` (control). After it opens, immediately perform the application handshake and authorization on this channel. Additional channels (including unreliable) can be opened after handshake completes.

## Application Handshake (AKE) over Data Channel

Channel binding: obtain a 32‑byte DTLS exporter secret (label: `beach/bind/1`, context empty) from the underlying DTLS session. Mix this into the handshake prologue or key schedule so the handshake is cryptographically tied to this specific connection.

Two modes:

### Passphrase Mode

Preferred (PAKE):
- Run SPAKE2+ or OPAQUE to derive a strong shared secret without enabling offline guessing. Mix exporter into the key schedule: `K = HKDF(pake_secret, exporter, "beach:ake:psk:v1")`.
- Derive application send/recv keys from `K`.

Alternative (Noise with shared key) for initial implementation:
- Use Noise pattern `XXpsk2` (both sides ephemeral; shared key mixed after e,e,dhe) or `XKpsk2` (server static optional) via `snow`.
- Prologue includes the DTLS exporter and a transcript hash of sealed signaling envelopes.
- Complete handshake, derive `K`, split into `k_tx/k_rx` and `k_control`.

Mutual confirmation:
- Each side MUST send a MAC over the transcript using `k_control` and verify the peer’s MAC before granting PTY access.

### Clerk Identity Mode

- Server authentication: Noise IK (server static known/pinned) or XX (mutual static). Server static pubkey is distributed out‑of‑band or via beach binary config.
- Client authorization: Client sends a Clerk JWT inside the encrypted payload of the final handshake message. The server verifies offline using Clerk JWKS.
- Bind exporter and (optionally) sealed signaling transcript into the prologue.
- Derive `K`, split as above.

Authorization policy:
- The server MUST check claims (e.g., `sub`, `exp`, `nbf`) and group membership against configured allowlists before enabling PTY.

## Key Schedule and Rekeying

- After AKE, derive keys:
  - `k_tx`, `k_rx` = HKDF(K, info = "beach:stream:v1") split.
  - `k_control` = HKDF(K, info = "beach:control:v1").
- Rekeying: Either leverage Noise built‑in `rekey()` periodically (e.g., every 1 GiB or 10 minutes) or rotate via HKDF(counter).
- All PTY bytes and control frames SHOULD be wrapped in the application cipher; these ride over the data channels established below.

## Data Channels and Reliability Profile

- Control channel (`beach/ctrl/1`): reliable and ordered. Used for handshake, authorization, keyboard input, acknowledgements, and control messages. Must not lose or reorder bytes.
- Output channel (`beach/term/1`): unreliable and unordered (e.g., `max_retransmits = 0`, `ordered = false`). Used for terminal output frames where occasional loss is acceptable because the client periodically resynchronizes with server state.
- Creation rules: reliability/order are fixed at creation; do not attempt to switch modes. Always bring up the reliable control channel first, complete the handshake and authorization, then open the unreliable output channel.
- Resync strategy: send periodic snapshots or state hashes on the reliable channel; server retransmits a fresh snapshot over the unreliable channel when the client reports gaps. See dual‑channel spec for details.

## Failure Handling

- SS open failure → abort signaling and report a clear error to the user.
- AKE failure or MAC mismatch → immediately close data channel and teardown. Rate‑limit retries to mitigate online guessing.
- JWT invalid/unauthorized → close with explicit policy error.

## Backwards Compatibility and Rollout

Phase 1 (Sealed signaling + initial handshake):
- Implement sealed signaling with passphrase and the small post‑connect handshake (Noise with shared key). Keep a feature flag to fall back to plain session descriptions for development.
- Use a reliable control channel for handshake and all client input; add the unreliable output channel after handshake.
- Public mode default: generate short code by default with interstitial; Private mode requires `beach login`.

Phase 2 (Clerk Identity):
- Add Clerk verification and server static key. Allow running in either mode per session.

Phase 3 (PAKE):
- Replace Noise‑PSK with SPAKE2+/OPAQUE for PSK mode to eliminate offline attack surface.

Compatibility:
- beach‑road requires no protocol awareness beyond opaque forwarding of `signal` payloads.

## Configuration (proposed)

Environment variables / flags:
- `BEACH_AUTH_MODE` = `psk` | `clerk` (default `psk` if `--passphrase` provided; `clerk` otherwise).
- `BEACH_PASSPHRASE` or CLI `--passphrase` (server + client).
- `BEACH_SALT_COST` / `BEACH_ARGON2_{MEM_KIB,LANES,ITER}`: Argon2id tuning.
- `BEACH_SS_REQUIRED` = `true|false` (default `true` in release builds).
- `BEACH_NOISE_PATTERN` (for early phases) e.g., `XXpsk2`.
- `BEACH_DC_CTRL_LABEL` (default `beach/ctrl/1`), `BEACH_DC_TERM_LABEL` (default `beach/term/1`).
- `BEACH_DC_TERM_UNRELIABLE` (default `true`).
- Mode & profiles: `BEACH_MODE` (`public|private`), `BEACH_PROFILE`, `BEACH_SESSION_SERVER`.
- Clerk: `CLERK_JWKS_URL`, `CLERK_ALLOWED_GROUPS`, `CLERK_AUDIENCE`.
- `BEACH_SERVER_STATIC_PK/BEACH_SERVER_STATIC_SK` (identity mode).
- `CLERK_JWKS_URL`, `CLERK_ALLOWED_GROUPS` (comma‑sep), `CLERK_AUDIENCE`.

Key storage:
- Server static key SHOULD be stored on disk with restricted permissions and not logged. Never write passphrases or derived keys to logs.

## Logging and Privacy

- Do not emit handshake or keying material to stdout/stderr.
- Use `--debug-log` file plumbing already present in `apps/beach` for internal diagnostics.
- Logs MUST redact tokens, passphrases, and derived keys. Only log high‑level events and error codes.

## Open Questions / Implementation Notes

- DTLS exporter access: Ensure the chosen WebRTC stack exposes a DTLS exporter (32 bytes). If not, consider binding via the DTLS certificate fingerprint + SRTP keys as a weaker substitute.
- PAKE vs Noise‑PSK: Ship Noise‑PSK quickly for developer testing, then migrate to SPAKE2+/OPAQUE.
- SAS (short auth string): Optionally display a 4–6 word verifier derived from `K` to allow human verification on first use.
- Multi‑client sessions: Server handshake and authorization run per client connection. Key isolation per client.
- Public code length: with PAKE and strict rate‑limiting/TTL, short numeric codes can be acceptable for public sessions. Document expected TTL, rate limits, and UI prompts.

## References

- Noise Protocol Framework, patterns IK/XX and PSK modifiers.
- OPAQUE/PAKE literature (e.g., CFRG drafts), SPAKE2+.
- RFC 5705 (TLS exporter), DTLS‑SRTP exporter usage.
- Argon2id parameters and guidance.

## Implementation Checklist (Initial)

- [ ] Add SS wrapper/unwrap in `apps/beach/src/transport/webrtc/mod.rs` (passphrase mode first).
## Developer Experience (DX)

Two simple modes map to clear user flows and URLs.

- Public Beach Mode
  - No sign‑in. Default when no credentials/profile/env vars are present (AWS‑CLI style resolution from `~/.beach/config`, `~/.beach/credentials`, `BEACH_PROFILE`).
  - Start: `beach [-- cmd ...] [--passphrase|--pp <code>]`.
    - If no code provided, Beach generates a short, human‑friendly code and shows an interstitial (press Enter or wait ~60s). The code is ephemeral and rate‑limited.
    - Session URLs are issued under `public.<host>/session-id` so trust is obvious.
  - Join: `beach --join <url|id> [--passphrase <code>]` (or prompt). Supports `BEACH_PASSPHRASE`.
  - Security: sealed session descriptions using the code; then a small handshake over a reliable control channel binds to the exact connection.

- Private Beach Mode
  - Requires authentication via Clerk. `beach login` opens a browser or device code flow and writes profiles/credentials like AWS CLI. `--profile` and env vars can select profiles.
  - Start requires auth; joins require auth. Sessions live under `private.<host>/...`.
  - The server enforces authorization (group membership, policies). A passphrase can be added as an extra gate but is not a substitute for auth in this mode.

Passphrase UX and security:
- Default public sessions use a short, user‑friendly code (e.g., 6 digits or two words + number) combined with rate‑limiting and short TTL.
- Because the code protects the sealed session description and is never sent in plaintext, and because the post‑connect handshake can use a password‑authenticated method, offline guessing is prevented while keeping the code short and memorable.

- [ ] Add data‑channel handshake with Noise and DTLS exporter binding.
- [ ] Gate PTY start until handshake + authorization succeed on the reliable control channel.
- [ ] Clerk mode: verify JWT (JWKS fetch/cache) and enforce group policy.
- [ ] Config flags + sensible defaults.
- [ ] Redacted debug logging via `--debug-log` only.
- [ ] Open second, unreliable channel for terminal output after handshake; implement periodic resyncs.
