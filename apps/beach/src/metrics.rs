use once_cell::sync::Lazy;
use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, IntGaugeVec, Opts, Registry};

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

pub static WEBRTC_CHUNKED_MESSAGES: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "webrtc_chunked_messages_total",
            "Logical messages processed by the WebRTC chunker",
        ),
        &["direction"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static WEBRTC_CHUNKS_EMITTED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "webrtc_chunks_emitted_total",
            "Chunk frames sent or received over WebRTC",
        ),
        &["direction"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static WEBRTC_CHUNK_PARTIAL_GCED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "webrtc_chunk_partial_gced_total",
            "Partial chunked messages evicted or expired",
        ),
        &["reason"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static WEBRTC_CHUNK_MALFORMED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "webrtc_chunk_malformed_total",
            "Malformed chunk frames observed on the WebRTC transport",
        ),
        &["direction"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static WEBRTC_CHUNK_OVERSIZED_DROPPED: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "webrtc_chunk_oversized_dropped_total",
            "Chunked messages rejected due to size limits",
        ),
        &["direction"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static WEBRTC_SIGNALING_ATTEMPTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "webrtc_signaling_attempts_total",
            "Attempts to call signaling endpoints (attach/offer/answer)",
        ),
        &["kind", "peer_session_id"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static WEBRTC_SIGNALING_RESULTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "webrtc_signaling_results_total",
            "Outcomes of signaling requests by kind",
        ),
        &["kind", "peer_session_id", "result"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static WEBRTC_SIGNALING_RETRIES: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "webrtc_signaling_retries_total",
            "Retries triggered by retryable signaling statuses",
        ),
        &["kind", "peer_session_id", "status"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static CONTROLLER_PEER_SESSION_EVENTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "controller_peer_session_events_total",
            "Controller channel lifecycle events keyed by peer_session_id",
        ),
        &["peer_session_id", "event"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static WEBRTC_CHUNK_REASSEMBLY_LATENCY: Lazy<HistogramVec> = Lazy::new(|| {
    let mut opts = HistogramOpts::new(
        "webrtc_chunk_reassembly_latency_ms",
        "Latency to reassemble chunked WebRTC payloads",
    );
    opts.buckets = vec![5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 5000.0];
    let h = HistogramVec::new(opts, &["direction"]).unwrap();
    REGISTRY.register(Box::new(h.clone())).ok();
    h
});

pub static FRAMED_MESSAGES: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "framed_messages_total",
            "Framed transport messages observed",
        ),
        &["direction", "namespace", "kind"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static FRAMED_ERRORS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "framed_transport_errors_total",
            "Integrity and reassembly errors for framed transport",
        ),
        &["reason"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static FRAMED_QUEUE_DEPTH: Lazy<IntGaugeVec> = Lazy::new(|| {
    let g = IntGaugeVec::new(
        Opts::new(
            "framed_transport_queue_depth",
            "Framed transport queue depth indicators",
        ),
        &["metric"],
    )
    .unwrap();
    REGISTRY.register(Box::new(g.clone())).ok();
    g
});

pub static FRAMED_OUTBOUND_QUEUE_DEPTH: Lazy<IntGaugeVec> = Lazy::new(|| {
    let g = IntGaugeVec::new(
        Opts::new(
            "framed_outbound_queue_depth",
            "Outbound framed transport queue depth by namespace/priority",
        ),
        &["namespace", "priority"],
    )
    .unwrap();
    REGISTRY.register(Box::new(g.clone())).ok();
    g
});

pub static FRAMED_OUTBOUND_QUEUE_LATENCY: Lazy<HistogramVec> = Lazy::new(|| {
    let mut opts = HistogramOpts::new(
        "framed_outbound_queue_latency_ms",
        "Latency from enqueue to data-channel send for framed transport",
    );
    opts.buckets = vec![
        1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0,
    ];
    let h = HistogramVec::new(opts, &["namespace", "priority"]).unwrap();
    REGISTRY.register(Box::new(h.clone())).ok();
    h
});

pub static WEBRTC_DTLS_FAILURES: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "webrtc_dtls_failures_total",
            "DTLS/transport failures observed on peer connections",
        ),
        &["role"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static CONTROLLER_FRAMES: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        Opts::new(
            "controller_frames_total",
            "Controller frames observed on the primary transport",
        ),
        &["direction", "kind"],
    )
    .unwrap();
    REGISTRY.register(Box::new(c.clone())).ok();
    c
});

pub static CONTROLLER_QUEUE: Lazy<IntGaugeVec> = Lazy::new(|| {
    let g = IntGaugeVec::new(
        Opts::new(
            "controller_queue_depth",
            "Controller action queue depth/lag gauges",
        ),
        &["metric"],
    )
    .unwrap();
    REGISTRY.register(Box::new(g.clone())).ok();
    g
});

pub static CONTROLLER_LATENCY: Lazy<HistogramVec> = Lazy::new(|| {
    let mut opts = HistogramOpts::new(
        "controller_queue_latency_ms",
        "Latency between enqueue and application of controller actions",
    );
    opts.buckets = vec![
        1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0,
    ];
    let h = HistogramVec::new(opts, &["stage"]).unwrap();
    REGISTRY.register(Box::new(h.clone())).ok();
    h
});
