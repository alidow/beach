use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry, TextEncoder};

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

pub fn export_prometheus() -> String {
    let metric_families = REGISTRY.gather();
    let mut buf = Vec::new();
    TextEncoder::new().encode(&metric_families, &mut buf).ok();
    String::from_utf8(buf).unwrap_or_default()
}

