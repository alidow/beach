#![recursion_limit = "1024"]

use std::sync::Arc;
use std::time::Duration;

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
        },
    );
    send_host_frame(&*server, HostFrame::Grid { rows: 4, cols: 10 });
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
        },
    );
    send_host_frame(
        &*server,
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::Foreground,
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
        },
    );
    send_host_frame(&*server, HostFrame::Grid { rows: 2, cols: 5 });
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
        },
    );
    send_host_frame(&*server, HostFrame::Grid { rows: 1, cols: 5 });

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
                    max_updates: 1,
                }],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 2,
            },
        },
    );
    send_host_frame(&*server, HostFrame::Grid { rows: 4, cols: 6 });
    send_host_frame(
        &*server,
        HostFrame::Snapshot {
            subscription: 1,
            lane: Lane::Foreground,
            watermark: 1,
            has_more: false,
            updates: vec![Update::Row {
                row: 120,
                seq: 1,
                cells: "tail".chars().map(pack_char).collect(),
            }],
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

    let chunk_rows = count.min(3);
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
                    max_updates: 1,
                }],
                delta_budget: 512,
                heartbeat_ms: 250,
                initial_snapshot_lines: 2,
            },
        },
    );
    send_host_frame(&*server, HostFrame::Grid { rows: 6, cols: 8 });
    send_host_frame(
        &*server,
        HostFrame::Snapshot {
            subscription: 42,
            lane: Lane::Foreground,
            watermark: 1,
            has_more: false,
            updates: vec![Update::Row {
                row: 90,
                seq: 1,
                cells: "recent".chars().map(pack_char).collect(),
            }],
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
                            "empty backfill did not retire range; received start {next_start}"
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
        },
    );
    send_host_frame(&*server, HostFrame::Grid { rows: 4, cols: 10 });
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
        },
    );
    send_host_frame(&*server, HostFrame::Shutdown);

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}
