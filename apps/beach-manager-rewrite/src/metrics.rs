use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounter, IntCounterVec, Opts, Registry, TextEncoder};

pub static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);
pub static BOOT_COUNTER: Lazy<IntCounter> = Lazy::new(|| {
    let c =
        IntCounter::with_opts(Opts::new("manager_rewrite_boot_total", "rewrite boots")).unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static ASSIGNMENT_DECISIONS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "manager_assignment_decision_total",
            "manager assignment decisions by outcome",
        ),
        &["result"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static QUEUE_ENQUEUED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "manager_queue_enqueued_total",
            "messages enqueued into controller queue by kind",
        ),
        &["kind"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static QUEUE_DROPPED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "manager_queue_dropped_total",
            "messages dropped by controller queue backpressure by kind",
        ),
        &["kind"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static PERSIST_SUCCESS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "manager_persist_success_total",
            "successful persistence operations by kind",
        ),
        &["kind"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static PERSIST_ERROR: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "manager_persist_error_total",
            "failed persistence operations by kind",
        ),
        &["kind"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub fn gather() -> Vec<u8> {
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = Vec::new();
    if let Err(err) = encoder.encode(&metric_families, &mut buffer) {
        eprintln!("metrics encode error: {err}");
    }
    buffer
}
