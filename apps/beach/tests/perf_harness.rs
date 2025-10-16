use beach::protocol::{self, HostFrame, Lane, LaneBudgetFrame, SyncConfigFrame, Update};

fn pack_char(ch: char) -> u64 {
    (ch as u32 as u64) << 32
}

#[test_timeout::timeout]
#[ignore]
fn report_binary_vs_json_payload_sizes() {
    let sync_config = SyncConfigFrame {
        snapshot_budgets: vec![LaneBudgetFrame {
            lane: Lane::Foreground,
            max_updates: 128,
        }],
        delta_budget: 512,
        heartbeat_ms: 250,
        initial_snapshot_lines: 128,
    };

    let frames = vec![
        HostFrame::Hello {
            subscription: 1,
            max_seq: 0,
            config: sync_config.clone(),
            features: 0,
        },
        HostFrame::Grid {
            cols: 80,
            history_rows: 24,
            base_row: 0,
            viewport_rows: None,
        },
        HostFrame::Snapshot {
            subscription: 1,
            lane: Lane::Foreground,
            watermark: 42,
            has_more: false,
            updates: vec![
                Update::Row {
                    row: 23,
                    seq: 40,
                    cells: "echo Hello, Beach!".chars().map(pack_char).collect(),
                },
                Update::Cell {
                    row: 23,
                    col: 0,
                    seq: 41,
                    cell: pack_char('$'),
                },
                Update::RowSegment {
                    row: 23,
                    start_col: 5,
                    seq: 42,
                    cells: "Binary".chars().map(pack_char).collect(),
                },
            ],
            cursor: None,
        },
        HostFrame::SnapshotComplete {
            subscription: 1,
            lane: Lane::Foreground,
        },
        HostFrame::Delta {
            subscription: 1,
            watermark: 50,
            has_more: false,
            updates: vec![
                Update::Trim {
                    start: 0,
                    count: 1,
                    seq: 51,
                },
                Update::Style {
                    id: 2,
                    seq: 52,
                    fg: 0x00FF00,
                    bg: 0x000000,
                    attrs: 0b0000_0010,
                },
            ],
            cursor: None,
        },
        HostFrame::Shutdown,
    ];

    let json_total: usize = frames
        .iter()
        .map(|frame| serde_json::to_string(frame).expect("json encode").len())
        .sum();
    let binary_total: usize = frames
        .iter()
        .map(|frame| protocol::encode_host_frame_binary(frame).len())
        .sum();

    println!(
        "payload-size-bytes: json={} binary={} savings={} ({}%)",
        json_total,
        binary_total,
        json_total.saturating_sub(binary_total),
        if json_total == 0 {
            0.0
        } else {
            100.0 - (binary_total as f64 / json_total as f64 * 100.0)
        }
    );
}
