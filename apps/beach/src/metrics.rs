use once_cell::sync::Lazy;
use prometheus::{IntCounterVec, Opts, Registry};

pub static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);

pub static EXTENSION_SENT: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "transport_extension_sent_total",
            "Extension frames sent by the host",
        ),
        &["namespace", "kind", "role", "path"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static EXTENSION_RECEIVED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "transport_extension_received_total",
            "Extension frames received by the host",
        ),
        &["namespace", "kind", "role"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static EXTENSION_FALLBACK: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "transport_extension_fallback_total",
            "Extension send fallbacks to legacy/http paths",
        ),
        &["namespace", "kind", "role", "reason"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});
