#![recursion_limit = "1024"]

use std::sync::Arc;
use std::time::{Duration, Instant};

use beach_human::cache::terminal::PackedCell;
use beach_human::client::terminal::{ClientError, TerminalClient};
use beach_human::protocol::{
    self, ClientFrame as WireClientFrame, HostFrame, Lane, LaneBudgetFrame, SyncConfigFrame, Update,
};
use beach_human::transport::{Payload, Transport, TransportError, TransportKind, TransportPair};

fn send_host_frame(transport: &dyn Transport, frame: HostFrame) {
    let bytes = protocol::encode_host_frame_binary(&frame);
    transport.send_bytes(&bytes).expect("send frame");
}

fn pack_char(ch: char) -> u64 {
    let packed = PackedCell::from_raw((ch as u32 as u64) << 32);
    packed.into()
}

#[test_timeout::tokio_timeout_test]
async fn client_replays_basic_snapshot() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport).with_render(false);
        match client.run() {
            Ok(()) => {}
            Err(err) => panic!("client error: {err}"),
        }
    });

    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription: 1,
            max_seq: 0,
            config: SyncConfigFrame {
                snapshot_budgets: vec![],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 500,
            },
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 10,
            history_rows: 4,
            base_row: 0,
            viewport_rows: Some(4),
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Snapshot {
            subscription: 1,
            lane: Lane::Foreground,
            watermark: 1,
            has_more: false,
            updates: vec![
                Update::Row {
                    row: 0,
                    seq: 1,
                    cells: "hello".chars().map(pack_char).collect(),
                },
                Update::Row {
                    row: 1,
                    seq: 2,
                    cells: "world".chars().map(pack_char).collect(),
                },
            ],
            cursor: None,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::Foreground,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::Recent,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::History,
        },
    );
    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[test_timeout::tokio_timeout_test]
async fn client_applies_deltas() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport).with_render(false);
        let _ = client.run();
    });

    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription: 1,
            max_seq: 0,
            config: SyncConfigFrame {
                snapshot_budgets: vec![],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 500,
            },
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 5,
            history_rows: 2,
            base_row: 0,
            viewport_rows: Some(2),
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Snapshot {
            subscription: 1,
            lane: Lane::Foreground,
            watermark: 1,
            has_more: false,
            updates: vec![Update::Row {
                row: 0,
                seq: 1,
                cells: "hello".chars().map(pack_char).collect(),
            }],
            cursor: None,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::Foreground,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Delta {
            subscription: 1,
            watermark: 2,
            has_more: false,
            updates: vec![Update::Cell {
                row: 0,
                col: 4,
                seq: 2,
                cell: pack_char('!'),
            }],
            cursor: None,
        },
    );
    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[test_timeout::tokio_timeout_test]
async fn client_emits_input_events() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let (tx, rx) = std::sync::mpsc::channel();

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport)
            .with_render(false)
            .with_input(rx);
        let _ = client.run();
    });

    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription: 1,
            max_seq: 0,
            config: SyncConfigFrame {
                snapshot_budgets: vec![],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 500,
            },
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 5,
            history_rows: 1,
            base_row: 0,
            viewport_rows: Some(1),
        },
    );

    tokio::time::sleep(Duration::from_millis(10)).await;
    tx.send(b"a".to_vec()).expect("send input");

    let message = server
        .recv(Duration::from_secs(1))
        .expect("receive input frame");
    let data = match message.payload {
        Payload::Binary(bytes) => match protocol::decode_client_frame_binary(&bytes) {
            Ok(WireClientFrame::Input { data, .. }) => data,
            other => panic!("unexpected client frame: {other:?}"),
        },
        Payload::Text(text) => panic!("unexpected text payload: {text}"),
    };
    assert_eq!(data, b"a");

    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[test_timeout::tokio_timeout_test]
async fn client_requests_backfill_and_hydrates_rows() {
    let _ = tracing_subscriber::fmt::try_init();
    let pair = TransportPair::new(TransportKind::Ipc);
    let client_transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(client_transport).with_render(false);
        let _ = client.run();
    });

    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription: 1,
            max_seq: 0,
            config: SyncConfigFrame {
                snapshot_budgets: vec![LaneBudgetFrame {
                    lane: Lane::Foreground,
                    max_updates: 2,
                }],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 2,
            },
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 6,
            history_rows: 4,
            base_row: 0,
            viewport_rows: Some(4),
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Snapshot {
            subscription: 1,
            lane: Lane::Foreground,
            watermark: 2,
            has_more: false,
            updates: vec![
                Update::Row {
                    row: 0,
                    seq: 1,
                    cells: "head".chars().map(pack_char).collect(),
                },
                Update::Row {
                    row: 3,
                    seq: 2,
                    cells: "tail".chars().map(pack_char).collect(),
                },
            ],
            cursor: None,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::Foreground,
        },
    );

    let (subscription, request_id, start_row, count) = loop {
        match server.recv(Duration::from_secs(2)) {
            Ok(message) => match message.payload {
                Payload::Binary(bytes) => match protocol::decode_client_frame_binary(&bytes) {
                    Ok(WireClientFrame::RequestBackfill {
                        subscription,
                        request_id,
                        start_row,
                        count,
                    }) => break (subscription, request_id, start_row, count),
                    _ => continue,
                },
                Payload::Text(text) => {
                    let trimmed = text.trim();
                    if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                        continue;
                    }
                }
            },
            Err(err) => panic!("failed to receive backfill request: {err}"),
        }
    };

    let chunk_rows = count.min(2);
    let mut updates = Vec::new();
    for offset in 0..chunk_rows {
        let row_id = start_row + offset as u64;
        let label = format!("row-{row_id}");
        updates.push(Update::Row {
            row: row_id as u32,
            seq: 100 + offset as u64,
            cells: label.chars().map(pack_char).collect(),
        });
    }

    send_host_frame(
        &*server,
        HostFrame::HistoryBackfill {
            subscription,
            request_id,
            start_row,
            count: chunk_rows,
            updates,
            more: false,
            cursor: None,
        },
    );

    for _ in 0..5 {
        match server.recv(Duration::from_millis(500)) {
            Err(TransportError::Timeout) => break,
            Ok(message) => {
                if let Payload::Binary(bytes) = &message.payload {
                    if let Ok(WireClientFrame::RequestBackfill {
                        start_row: next_start,
                        ..
                    }) = protocol::decode_client_frame_binary(bytes)
                    {
                        assert!(
                            next_start > start_row,
                            "backfill did not advance; received start {next_start} after {start_row}"
                        );
                        break;
                    }
                }
            }
            Err(err) => panic!("unexpected transport error: {err}"),
        }
    }

    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[test_timeout::tokio_timeout_test]
async fn client_requests_backfill_uses_session_rows() {
    const BASE_ROW: u64 = 33_000;
    const TAIL_ROW: u64 = BASE_ROW + 10;

    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport).with_render(false);
        if let Err(err) = client.run() {
            panic!("client error: {err}");
        }
    });

    // Complete minimal handshake with a snapshot whose rows live at a high absolute offset.
    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription: 1,
            max_seq: 1,
            config: SyncConfigFrame {
                snapshot_budgets: vec![],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 128,
            },
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 80,
            history_rows: 400,
            base_row: 0,
            viewport_rows: Some(400),
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Snapshot {
            subscription: 1,
            lane: Lane::Foreground,
            watermark: 2,
            has_more: false,
            updates: vec![
                Update::Row {
                    row: BASE_ROW as u32,
                    seq: 1,
                    cells: "prompt".chars().map(pack_char).collect(),
                },
                Update::Row {
                    row: TAIL_ROW as u32,
                    seq: 2,
                    cells: "tail".chars().map(pack_char).collect(),
                },
            ],
            cursor: None,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::Foreground,
        },
    );

    let (request_id, start_row, count) = loop {
        let message = server
            .recv(Duration::from_secs(5))
            .expect("receive client frame");
        match message.payload {
            Payload::Binary(bytes) => match protocol::decode_client_frame_binary(&bytes) {
                Ok(WireClientFrame::RequestBackfill {
                    request_id,
                    start_row,
                    count,
                    ..
                }) => break (request_id, start_row, count),
                Ok(_) => continue,
                Err(err) => panic!("decode client frame: {err}"),
            },
            Payload::Text(text) => panic!("unexpected text payload: {text}"),
        }
    };

    assert!(
        start_row >= BASE_ROW && start_row < TAIL_ROW,
        "backfill start_row should target session gap: {start_row} (expected between {BASE_ROW} and {TAIL_ROW})"
    );

    send_host_frame(
        &*server,
        HostFrame::HistoryBackfill {
            subscription: 1,
            request_id,
            start_row,
            count,
            updates: vec![Update::Row {
                row: start_row as u32,
                seq: 3,
                cells: "prompt".chars().map(pack_char).collect(),
            }],
            more: false,
            cursor: None,
        },
    );

    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[test_timeout::tokio_timeout_test]
async fn client_requests_history_after_delta_when_handshake_empty() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport).with_render(false);
        if let Err(err) = client.run() {
            panic!("client error: {err}");
        }
    });

    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription: 1,
            max_seq: 0,
            config: SyncConfigFrame {
                snapshot_budgets: vec![LaneBudgetFrame {
                    lane: Lane::Foreground,
                    max_updates: 0,
                }],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 0,
            },
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 80,
            history_rows: 24,
            base_row: 0,
            viewport_rows: Some(24),
        },
    );
    // Simulate an empty snapshot handshake.
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::Foreground,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::Recent,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::History,
        },
    );

    let first_row: u32 = 100;
    let mut updates = Vec::new();
    for i in 0..50u32 {
        let label = format!("line-{}", first_row + i);
        updates.push(Update::Row {
            row: first_row + i,
            seq: (i + 1) as u64,
            cells: label.chars().map(pack_char).collect(),
        });
    }
    send_host_frame(
        &*server,
        HostFrame::Delta {
            subscription: 1,
            watermark: 50,
            has_more: false,
            updates,
            cursor: None,
        },
    );

    let (request_start, request_count) = loop {
        match server.recv(Duration::from_secs(2)) {
            Ok(message) => match message.payload {
                Payload::Binary(bytes) => match protocol::decode_client_frame_binary(&bytes) {
                    Ok(WireClientFrame::RequestBackfill {
                        start_row, count, ..
                    }) => break (start_row, count),
                    Ok(_) => continue,
                    Err(err) => panic!("decode client frame: {err}"),
                },
                Payload::Text(text) => {
                    let trimmed = text.trim();
                    if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                        continue;
                    }
                }
            },
            Err(TransportError::Timeout) => {
                panic!("client failed to request history after delta");
            }
            Err(err) => panic!("transport error: {err}"),
        }
    };

    assert!(
        request_start < first_row as u64,
        "expected backfill start row below first delta row; got {request_start} (delta row {first_row})"
    );
    assert!(request_count > 0);

    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[test_timeout::tokio_timeout_test]
async fn client_retries_history_when_initial_backfill_empty() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport).with_render(false);
        if let Err(err) = client.run() {
            panic!("client error: {err}");
        }
    });

    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription: 1,
            max_seq: 0,
            config: SyncConfigFrame {
                snapshot_budgets: vec![LaneBudgetFrame {
                    lane: Lane::Foreground,
                    max_updates: 0,
                }],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 0,
            },
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 80,
            history_rows: 24,
            base_row: 0,
            viewport_rows: Some(24),
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::Foreground,
        },
    );

    let (subscription, first_request_id, start_row, count) = loop {
        // Trigger the client's initial request by delivering some tail output.
        let mut seed_updates = Vec::new();
        for row in 0..10u32 {
            let text = format!("seed-{row:03}");
            seed_updates.push(Update::Row {
                row,
                seq: (row + 1) as u64,
                cells: text.chars().map(pack_char).collect(),
            });
        }
        send_host_frame(
            &*server,
            HostFrame::Delta {
                subscription: 1,
                watermark: 10,
                has_more: false,
                updates: seed_updates,
                cursor: None,
            },
        );

        let message = server
            .recv(Duration::from_secs(2))
            .expect("receive first backfill request");
        if let Payload::Binary(bytes) = message.payload {
            if let Ok(WireClientFrame::RequestBackfill {
                subscription,
                request_id,
                start_row,
                count,
            }) = protocol::decode_client_frame_binary(&bytes)
            {
                break (subscription, request_id, start_row, count);
            }
        }
    };

    // Respond with no history so the client still has a gap.
    send_host_frame(
        &*server,
        HostFrame::HistoryBackfill {
            subscription,
            request_id: first_request_id,
            start_row,
            count,
            updates: Vec::new(),
            more: false,
            cursor: None,
        },
    );

    // Deliver a burst of new output beyond the initial seed.
    let mut updates = Vec::new();
    for row in 20..150u32 {
        let text = format!("line-{row:03}");
        updates.push(Update::Row {
            row,
            seq: (row + 1) as u64,
            cells: text.chars().map(pack_char).collect(),
        });
    }
    send_host_frame(
        &*server,
        HostFrame::Delta {
            subscription,
            watermark: 150,
            has_more: false,
            updates,
            cursor: None,
        },
    );

    // Expect the client to retry a history request after noticing the gap.
    let mut observed_retry = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match server.recv(Duration::from_millis(200)) {
            Ok(message) => {
                if let Payload::Binary(bytes) = message.payload {
                    if let Ok(WireClientFrame::RequestBackfill { request_id, .. }) =
                        protocol::decode_client_frame_binary(&bytes)
                    {
                        if request_id != first_request_id {
                            observed_retry = true;
                            break;
                        }
                    }
                }
            }
            Err(TransportError::Timeout) => continue,
            Err(err) => panic!("transport error while awaiting retry: {err}"),
        }
    }

    assert!(
        observed_retry,
        "client never retried backfill after empty response"
    );

    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[test_timeout::tokio_timeout_test]
async fn client_targets_tail_history_after_large_delta() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport).with_render(false);
        if let Err(err) = client.run() {
            panic!("client error: {err}");
        }
    });

    let subscription = 1;

    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription,
            max_seq: 0,
            config: SyncConfigFrame {
                snapshot_budgets: vec![LaneBudgetFrame {
                    lane: Lane::Foreground,
                    max_updates: 32,
                }],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 32,
            },
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 80,
            history_rows: 400,
            base_row: 0,
            viewport_rows: Some(400),
        },
    );
    // Provide a minimal snapshot so the renderer seeds a baseline but leaves most rows pending.
    send_host_frame(
        &*server,
        HostFrame::Snapshot {
            subscription: 1,
            lane: Lane::Foreground,
            watermark: 1,
            has_more: false,
            updates: vec![Update::Row {
                row: 0,
                seq: 1,
                cells: "seed".chars().map(pack_char).collect(),
            }],
            cursor: None,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription,
            lane: Lane::Foreground,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription,
            lane: Lane::Recent,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription,
            lane: Lane::History,
        },
    );

    // Drain any initial history requests before we trigger the burst.
    let mut initial_requests = Vec::new();
    loop {
        match server.recv(Duration::from_millis(100)) {
            Ok(message) => {
                if let Payload::Binary(bytes) = message.payload {
                    if let Ok(WireClientFrame::RequestBackfill {
                        subscription,
                        request_id,
                        start_row,
                        count,
                    }) = protocol::decode_client_frame_binary(&bytes)
                    {
                        initial_requests.push((subscription, request_id, start_row, count));
                    }
                }
            }
            Err(TransportError::Timeout) => break,
            Err(err) => panic!("unexpected transport error: {err}"),
        }
    }
    let initial_max_request_id = initial_requests
        .iter()
        .map(|(_, request_id, _, _)| *request_id)
        .max()
        .unwrap_or(0);
    for (subscription, request_id, start_row, count) in initial_requests {
        let mut bootstrap = Vec::new();
        for offset in 0..count {
            let row = start_row + offset as u64;
            let text = format!("init-{row:03}");
            bootstrap.push(Update::Row {
                row: row as u32,
                seq: (500 + row) as u64,
                cells: text.chars().map(pack_char).collect(),
            });
        }
        send_host_frame(
            &*server,
            HostFrame::HistoryBackfill {
                subscription,
                request_id,
                start_row,
                count,
                updates: bootstrap,
                more: false,
                cursor: None,
            },
        );
    }

    // Stream a large block of output far beyond the earlier rows.
    let high_base: u32 = 150;
    let mut delta_updates = Vec::new();
    for row in high_base..high_base + 40 {
        let text = format!("tail-{row:03}");
        delta_updates.push(Update::Row {
            row,
            seq: (2000 + row) as u64,
            cells: text.chars().map(pack_char).collect(),
        });
    }
    send_host_frame(
        &*server,
        HostFrame::Delta {
            subscription,
            watermark: (2000 + high_base + 39) as u64,
            has_more: false,
            updates: delta_updates,
            cursor: None,
        },
    );

    // Observe whether the client issues any follow-up requests targeting the new tail.
    let mut followup_requests = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match server.recv(Duration::from_millis(200)) {
            Ok(message) => {
                if let Payload::Binary(bytes) = message.payload {
                    if let Ok(WireClientFrame::RequestBackfill {
                        request_id,
                        start_row,
                        count,
                        ..
                    }) = protocol::decode_client_frame_binary(&bytes)
                    {
                        if request_id <= initial_max_request_id {
                            continue;
                        }
                        followup_requests.push((request_id, start_row, count));
                    }
                }
            }
            Err(TransportError::Timeout) => continue,
            Err(err) => panic!("transport error: {err}"),
        }
    }

    assert!(
        followup_requests.len() <= 1,
        "expected at most one tail request; observed {:?}",
        followup_requests
    );
    if let Some((_, start_row, _)) = followup_requests.first() {
        assert!(
            *start_row >= (high_base as u64).saturating_sub(40),
            "tail request {start_row} outside expected range"
        );
    }

    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[test_timeout::tokio_timeout_test]
async fn client_marks_empty_backfill_as_missing() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport).with_render(false);
        if let Err(err) = client.run() {
            panic!("client error: {err}");
        }
    });

    let subscription = 1;

    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription,
            max_seq: 0,
            config: SyncConfigFrame {
                snapshot_budgets: vec![LaneBudgetFrame {
                    lane: Lane::Foreground,
                    max_updates: 32,
                }],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 32,
            },
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 80,
            history_rows: 200,
            base_row: 0,
            viewport_rows: Some(200),
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Snapshot {
            subscription,
            lane: Lane::Foreground,
            watermark: 1,
            has_more: false,
            updates: vec![Update::Row {
                row: 0,
                seq: 1,
                cells: "seed".chars().map(pack_char).collect(),
            }],
            cursor: None,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription,
            lane: Lane::Foreground,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription,
            lane: Lane::Recent,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription,
            lane: Lane::History,
        },
    );

    let (request_subscription, first_request_id, first_start_row, first_count) = loop {
        match server.recv(Duration::from_millis(500)) {
            Ok(message) => {
                if let Payload::Binary(bytes) = message.payload {
                    if let Ok(WireClientFrame::RequestBackfill {
                        subscription,
                        request_id,
                        start_row,
                        count,
                    }) = protocol::decode_client_frame_binary(&bytes)
                    {
                        break (subscription, request_id, start_row, count);
                    }
                }
            }
            Err(TransportError::Timeout) => continue,
            Err(err) => panic!("transport error waiting for first backfill: {err}"),
        }
    };

    send_host_frame(
        &*server,
        HostFrame::HistoryBackfill {
            subscription: request_subscription,
            request_id: first_request_id,
            start_row: first_start_row,
            count: first_count,
            updates: Vec::new(),
            more: false,
            cursor: None,
        },
    );

    let mut duplicate_request = None;
    let deadline = Instant::now() + Duration::from_millis(800);
    while Instant::now() < deadline {
        match server.recv(Duration::from_millis(100)) {
            Ok(message) => {
                if let Payload::Binary(bytes) = message.payload {
                    if let Ok(WireClientFrame::RequestBackfill {
                        start_row, count, ..
                    }) = protocol::decode_client_frame_binary(&bytes)
                    {
                        if start_row == first_start_row && count == first_count {
                            duplicate_request = Some((start_row, count));
                            break;
                        }
                    }
                }
            }
            Err(TransportError::Timeout) => continue,
            Err(err) => panic!("transport error while awaiting duplicate request check: {err}"),
        }
    }

    assert!(
        duplicate_request.is_some(),
        "client failed to retry empty backfill for start={} count={}",
        first_start_row,
        first_count
    );

    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[test_timeout::tokio_timeout_test]
async fn client_recovers_truncated_history_after_tail_burst() {
    let _ = tracing_subscriber::fmt::try_init();
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport).with_render(false);
        if let Err(err) = client.run() {
            panic!("client error: {err}");
        }
    });

    let subscription = 1;

    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription,
            max_seq: 23736,
            config: SyncConfigFrame {
                snapshot_budgets: vec![LaneBudgetFrame {
                    lane: Lane::Foreground,
                    max_updates: 32,
                }],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 32,
            },
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 80,
            history_rows: 400,
            base_row: 0,
            viewport_rows: Some(400),
        },
    );

    // Simulate restored state where only the top 24 rows are immediately available.
    let base_row = 0u32;
    let mut snapshot_rows = Vec::new();
    for offset in 0..24u32 {
        let row = base_row + offset;
        let text = format!("Line {}: Test", offset + 1);
        snapshot_rows.push(Update::Row {
            row,
            seq: row as u64,
            cells: text.chars().map(pack_char).collect(),
        });
    }
    send_host_frame(
        &*server,
        HostFrame::Snapshot {
            subscription,
            lane: Lane::Foreground,
            watermark: base_row as u64 + 23,
            has_more: false,
            updates: snapshot_rows,
            cursor: None,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription,
            lane: Lane::Foreground,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription,
            lane: Lane::Recent,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription,
            lane: Lane::History,
        },
    );

    // Capture the client's initial full-range backfill request.
    let first_request = loop {
        match server.recv(Duration::from_secs(1)) {
            Ok(message) => {
                if let Payload::Binary(bytes) = message.payload {
                    if let Ok(WireClientFrame::RequestBackfill {
                        subscription,
                        request_id,
                        start_row,
                        count,
                    }) = protocol::decode_client_frame_binary(&bytes)
                    {
                        if request_id > 0 {
                            break (subscription, request_id, start_row, count);
                        }
                    }
                }
            }
            Err(TransportError::Timeout) => continue,
            Err(err) => panic!("transport error while awaiting initial backfill: {err}"),
        }
    };

    // Only the first chunk has data; subsequent ones are empty.
    let (_, first_request_id, first_start, _) = first_request;
    let mut first_chunk_updates = Vec::new();
    for offset in 0..24u32 {
        let row = base_row + offset;
        let text = format!("Line {}: Test", offset + 1);
        first_chunk_updates.push(Update::Row {
            row,
            seq: row as u64,
            cells: text.chars().map(pack_char).collect(),
        });
    }
    send_host_frame(
        &*server,
        HostFrame::HistoryBackfill {
            subscription,
            request_id: first_request_id,
            start_row: first_start,
            count: 64,
            updates: first_chunk_updates,
            more: true,
            cursor: None,
        },
    );
    for chunk in [64u64, 128, 192] {
        send_host_frame(
            &*server,
            HostFrame::HistoryBackfill {
                subscription,
                request_id: first_request_id,
                start_row: chunk,
                count: 64,
                updates: Vec::new(),
                more: chunk != 192,
                cursor: None,
            },
        );
    }

    // host sends the large delta burst producing rows 1..150.
    let mut delta_updates = Vec::new();
    for offset in 112..150u32 {
        let row = base_row + offset;
        let text = format!("Line {}: Test", offset + 1);
        delta_updates.push(Update::Row {
            row,
            seq: 20000 + row as u64,
            cells: text.chars().map(pack_char).collect(),
        });
    }
    send_host_frame(
        &*server,
        HostFrame::Delta {
            subscription,
            watermark: 20000 + base_row as u64 + 149,
            has_more: false,
            updates: delta_updates,
            cursor: None,
        },
    );

    // Backfill requests targeting 24.. and 33.. range.
    let mut followup_requests = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match server.recv(Duration::from_millis(200)) {
            Ok(message) => {
                if let Payload::Binary(bytes) = message.payload {
                    if let Ok(WireClientFrame::RequestBackfill {
                        request_id,
                        start_row,
                        count,
                        ..
                    }) = protocol::decode_client_frame_binary(&bytes)
                    {
                        if request_id > first_request_id {
                            followup_requests.push((request_id, start_row, count));
                            if followup_requests.len() >= 2 {
                                break;
                            }
                        }
                    }
                }
            }
            Err(TransportError::Timeout) => continue,
            Err(err) => panic!("transport error while gathering followups: {err}"),
        }
    }

    assert!(
        followup_requests
            .iter()
            .any(|(_, start, _)| *start == base_row as u64 + 24),
        "client never issued follow-up backfill for rows starting at base+24; observed {:?}",
        followup_requests
    );
    assert!(
        followup_requests
            .iter()
            .any(|(_, start, _)| *start <= base_row as u64 + 37),
        "client never issued broader backfill after burst; observed {:?}",
        followup_requests
    );

    for (request_id, start_row, count) in followup_requests {
        let mut updates = Vec::new();
        for offset in 0..count {
            let row = start_row + offset as u64;
            let text = format!("Line {}: Test", (row - first_start) + 1);
            updates.push(Update::Row {
                row: row as u32,
                seq: 21000 + row,
                cells: text.chars().map(pack_char).collect(),
            });
        }
        send_host_frame(
            &*server,
            HostFrame::HistoryBackfill {
                subscription,
                request_id,
                start_row,
                count,
                updates,
                more: false,
                cursor: None,
            },
        );
    }

    // Allow client to process.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let tail_deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < tail_deadline {
        match server.recv(Duration::from_millis(100)) {
            Ok(_) => {}
            Err(TransportError::Timeout) => break,
            Err(err) => panic!("transport error while waiting for tail render: {err}"),
        }
    }

    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[test_timeout::tokio_timeout_test]
async fn client_resolves_missing_rows_after_empty_backfill() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let client_transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(client_transport).with_render(false);
        let _ = client.run();
    });

    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription: 42,
            max_seq: 0,
            config: SyncConfigFrame {
                snapshot_budgets: vec![LaneBudgetFrame {
                    lane: Lane::Foreground,
                    max_updates: 2,
                }],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 2,
            },
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 8,
            history_rows: 6,
            base_row: 0,
            viewport_rows: Some(6),
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Snapshot {
            subscription: 42,
            lane: Lane::Foreground,
            watermark: 2,
            has_more: false,
            updates: vec![
                Update::Row {
                    row: 0,
                    seq: 1,
                    cells: "head".chars().map(pack_char).collect(),
                },
                Update::Row {
                    row: 5,
                    seq: 2,
                    cells: "recent".chars().map(pack_char).collect(),
                },
            ],
            cursor: None,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 42,
            lane: Lane::Foreground,
        },
    );

    let (subscription, request_id, start_row, count) = loop {
        match server.recv(Duration::from_secs(2)) {
            Ok(message) => match message.payload {
                Payload::Binary(bytes) => match protocol::decode_client_frame_binary(&bytes) {
                    Ok(WireClientFrame::RequestBackfill {
                        subscription,
                        request_id,
                        start_row,
                        count,
                    }) => break (subscription, request_id, start_row, count),
                    _ => continue,
                },
                Payload::Text(text) => {
                    let trimmed = text.trim();
                    if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                        continue;
                    }
                }
            },
            Err(err) => panic!("failed to receive backfill request: {err}"),
        }
    };

    // Respond with an empty backfill chunk (rows already trimmed).
    send_host_frame(
        &*server,
        HostFrame::HistoryBackfill {
            subscription,
            request_id,
            start_row,
            count,
            updates: Vec::new(),
            more: false,
            cursor: None,
        },
    );

    let mut saw_retry = false;
    for _ in 0..5 {
        match server.recv(Duration::from_millis(500)) {
            Err(TransportError::Timeout) => break,
            Ok(message) => {
                if let Payload::Binary(bytes) = &message.payload {
                    if let Ok(WireClientFrame::RequestBackfill {
                        start_row: next_start,
                        ..
                    }) = protocol::decode_client_frame_binary(bytes)
                    {
                        assert_eq!(
                            next_start, start_row,
                            "retry should target same range after empty response"
                        );
                        saw_retry = true;
                        break;
                    }
                }
            }
            Err(err) => panic!("unexpected transport error: {err}"),
        }
    }
    assert!(saw_retry, "client did not retry after empty backfill");

    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[test_timeout::tokio_timeout_test]
async fn client_handles_binary_snapshot_and_delta() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport).with_render(false);
        match client.run() {
            Ok(()) | Err(ClientError::Shutdown) => {}
            Err(err) => panic!("client error: {err}"),
        }
    });

    let sync_config = SyncConfigFrame {
        snapshot_budgets: vec![LaneBudgetFrame {
            lane: Lane::Foreground,
            max_updates: 64,
        }],
        delta_budget: 512,
        heartbeat_ms: 250,
        initial_snapshot_lines: 64,
    };

    send_host_frame(
        &*server,
        HostFrame::Hello {
            subscription: 1,
            max_seq: 0,
            config: sync_config.clone(),
            features: 0,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Grid {
            cols: 10,
            history_rows: 4,
            base_row: 0,
            viewport_rows: Some(4),
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Snapshot {
            subscription: 1,
            lane: Lane::Foreground,
            watermark: 1,
            has_more: false,
            updates: vec![Update::Row {
                row: 0,
                seq: 1,
                cells: "hello".chars().map(pack_char).collect(),
            }],
            cursor: None,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::Foreground,
        },
    );
    send_host_frame(
        &*server,
        HostFrame::Delta {
            subscription: 1,
            watermark: 2,
            has_more: false,
            updates: vec![Update::Cell {
                row: 0,
                col: 5,
                seq: 2,
                cell: pack_char('!'),
            }],
            cursor: None,
        },
    );
    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}
