# Shared Controller Lease Manual Test

1. Launch a private beach with the manager + road + surfer stack (`docker compose up beach-manager beach-road beach-surfer`).
2. Start the mock Pong agent via `apps/private-beach/demo/pong/tools/run-agent.sh --auto-pair …` so it acquires controller leases for the child sessions.
3. In Surfer, open the same private beach, pick a child session, and click **Interact** so the tile also calls `POST /sessions/:id/controller/lease`.
4. Confirm the manager logs include `controller.leases` lines showing multiple tokens for the session after each “lease_update” event.
5. Drive both controllers at once: let the agent move one paddle while typing in the Interact tile for the other paddle. No `409 controller_mismatch` responses should appear in Surfer devtools or the agent logs.
6. Watch `queue_actions_validate` log lines for the child session; they should list every active controller token each time one of the controllers sends actions.
7. Optionally, stop the Interact session (click **Stop Interacting**) and verify the remaining controller continues to renew and send actions without interruption.
