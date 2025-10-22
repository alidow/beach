use once_cell::sync::Lazy;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
    TextEncoder,
};

pub static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);

pub static REDIS_AVAILABLE: Lazy<IntGauge> = Lazy::new(|| {
    let g = IntGauge::new("redis_available", "Redis reachability: 1=up, 0=down").unwrap();
    REGISTRY.register(Box::new(g.clone())).ok();
    g
});

pub static ACTIONS_ENQUEUED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new("actions_enqueued_total", "Actions enqueued to sessions"),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static ACTIONS_DELIVERED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new("actions_delivered_total", "Actions delivered to harnesses"),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static ACTIONS_ACKED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new("actions_acked_total", "Actions acked by harnesses"),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static QUEUE_DEPTH: Lazy<IntGaugeVec> = Lazy::new(|| {
    let g = IntGaugeVec::new(
        Opts::new("actions_queue_depth", "Current depth of action queue"),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(g.clone())).ok();
    g
});

pub static QUEUE_LAG: Lazy<IntGaugeVec> = Lazy::new(|| {
    let g = IntGaugeVec::new(
        Opts::new(
            "actions_queue_pending",
            "Number of pending (unacked) actions",
        ),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(g.clone())).ok();
    g
});

pub static HEALTH_REPORTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new("health_reports_total", "Health reports received"),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static STATE_REPORTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new("state_reports_total", "State diffs received"),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static MANAGER_VIEWER_CONNECTED: Lazy<IntGaugeVec> = Lazy::new(|| {
    let g = IntGaugeVec::new(
        Opts::new(
            "manager_viewer_connected",
            "Manager WebRTC viewer connection state (0=disconnected, 1=connected)",
        ),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(g.clone())).ok();
    g
});

pub static MANAGER_VIEWER_RECONNECTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "manager_viewer_reconnects_total",
            "Total reconnect attempts for manager WebRTC viewer",
        ),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static MANAGER_VIEWER_LATENCY_MS: Lazy<HistogramVec> = Lazy::new(|| {
    let h = HistogramVec::new(
        HistogramOpts::new(
            "manager_viewer_latency_ms",
            "Observed latency (ms) between host heartbeat timestamp and manager receipt",
        )
        .buckets(vec![
            1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0,
        ]),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(h.clone())).ok();
    h
});

pub static MANAGER_VIEWER_KEEPALIVE_FAILURES: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "manager_viewer_keepalive_failures_total",
            "Failed attempts to send keepalive pings from manager viewer",
        ),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static MANAGER_VIEWER_IDLE_WARNINGS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "manager_viewer_idle_warnings_total",
            "Count of idle intervals detected with no host frames",
        ),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static MANAGER_VIEWER_KEEPALIVE_SENT: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "manager_viewer_keepalive_sent_total",
            "Number of keepalive pings attempted by the manager viewer",
        ),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static MANAGER_VIEWER_IDLE_RECOVERIES: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "manager_viewer_idle_recoveries_total",
            "Count of times the manager viewer recovered after an idle interval",
        ),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static CONTROLLER_PAIRINGS_ACTIVE: Lazy<IntGaugeVec> = Lazy::new(|| {
    let g = IntGaugeVec::new(
        Opts::new(
            "controller_pairings_active",
            "Active controller pairings per controller session",
        ),
        &["private_beach_id", "controller_session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(g.clone())).ok();
    g
});

pub static CONTROLLER_PAIRINGS_EVENTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "controller_pairings_events_total",
            "Total controller pairing add/remove events",
        ),
        &["private_beach_id", "controller_session_id", "action"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static ACTION_LATENCY_MS: Lazy<HistogramVec> = Lazy::new(|| {
    let h = HistogramVec::new(
        HistogramOpts::new(
            "action_latency_ms",
            "Ack-reported action latency in milliseconds",
        )
        .buckets(vec![
            1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0,
        ]),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(h.clone())).ok();
    h
});

pub static FASTPATH_ACTIONS_SENT: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "fastpath_actions_sent_total",
            "Actions delivered over fast-path data channels",
        ),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static FASTPATH_ACTIONS_FALLBACK: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "fastpath_actions_fallback_total",
            "Actions that attempted fast-path but fell back to brokered delivery",
        ),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static FASTPATH_ACKS_RECEIVED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "fastpath_acks_received_total",
            "Action acknowledgements received via fast-path",
        ),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static FASTPATH_STATE_RECEIVED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "fastpath_state_received_total",
            "State diffs received via fast-path",
        ),
        &["private_beach_id", "session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static FASTPATH_CHANNEL_CLOSED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "fastpath_channel_closed_total",
            "Fast-path data channel closures observed by the manager",
        ),
        &["private_beach_id", "session_id", "channel"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static FASTPATH_CHANNEL_ERRORS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "fastpath_channel_errors_total",
            "Fast-path data channel errors / parse failures observed by the manager",
        ),
        &["private_beach_id", "session_id", "channel"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub fn export_prometheus() -> String {
    let metric_families = REGISTRY.gather();
    let mut buf = Vec::new();
    TextEncoder::new().encode(&metric_families, &mut buf).ok();
    String::from_utf8(buf).unwrap_or_default()
}
