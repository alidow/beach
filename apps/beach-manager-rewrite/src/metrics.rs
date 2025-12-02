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

pub fn gather() -> Vec<u8> {
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = Vec::new();
    if let Err(err) = encoder.encode(&metric_families, &mut buffer) {
        eprintln!("metrics encode error: {err}");
    }
    buffer
}
