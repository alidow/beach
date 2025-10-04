# MCP over WebRTC

## Delivered Behaviour
- `beach host --mcp` advertises the session as MCP-capable in both the interactive banner and bootstrap JSON. The host spins up its local `McpServer` and bridges JSON-RPC traffic onto a dedicated ordered WebRTC data channel labelled `mcp-jsonrpc` for every authorised viewer.
- `beach join --mcp` (or any WebRTC client that sets `mcp=true` on the join request) asks the signaling service for the MCP lane. The client opens the ordered `mcp-jsonrpc` channel alongside the terminal transport and proxies it locally at the standard `~/.beach/mcp/<session>.sock` path (or the configured override).
- Join authorisation prompts show when a viewer has requested MCP so the host can approve or deny with the usual lease controls.

## Transport Details
- MCP frames continue to use newline-delimited JSON-RPC 2.0 messages. The ordered SCTP channel guarantees envelope sequencing; terminal data remains on the existing unordered/unreliable lanes.
- When a viewer requests MCP, the offerer immediately provisions the extra channel and publishes it through the `WebRtcChannels` registry. The answerer waits for that channel and feeds it into the bridge stack.
- Bridges are reference-counted tasks that terminate automatically when the underlying transport closes or the host shuts down.

## Signaling & Metadata
- Join HTTP requests and subsequent WebSocket `ClientMessage::Join` payloads carry an `mcp` boolean. The signaling server stores the flag on each `PeerInfo.metadata` and forwards it to all peers.
- `OffererAcceptedTransport` retains the metadata map. `JoinAuthorizationMetadata` surfaces the `mcp` hint so interactive approval shows "mcp:yes" when applicable.
- Bootstrap JSON schema bumped to `2` with a top-level `mcp_enabled` field for remote automation.

## CLI Ergonomics
- Host banner prints a ready-to-copy command: `beach --session-server <base> join <session> --passcode <code> --mcp`.
- `beach join` grows a `--mcp` switch that launches the WebRTC MCP proxy alongside the terminal client.
- SSH bootstrap output includes the new schema field so remote automation can auto-enable MCP without scraping banners.

## Follow-up Opportunities
- Expose local socket selection for the join-side proxy and document remote tooling recipes.
- Telemetry counters for MCP attachment rates and volume.
- Optional ACLs/tokens layered on top of the existing interactive approval for unattended deployments.
