use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn env_truthy(var: &str) -> Option<bool> {
    std::env::var(var).map(|v| v != "0" && !v.is_empty()).ok()
}

static PERF_ENABLED: Lazy<bool> = Lazy::new(|| {
    env_truthy("BEACH_PERF")
        .or_else(|| env_truthy("BEACH_TELEMETRY"))
        .unwrap_or_else(|| {
            // Backwards compat: BEACH_PROFILE used to double as the perf toggle.
            matches!(std::env::var("BEACH_PROFILE").ok().as_deref(), Some("1"))
        })
});

static STATS: Lazy<Mutex<HashMap<&'static str, PerfStat>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

static GAUGES: Lazy<Mutex<HashMap<&'static str, GaugeStat>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Default)]
struct GaugeStat {
    last: u64,
    samples: u64,
}

#[derive(Default)]
struct PerfStat {
    total_ns: u128,
    max_ns: u128,
    count: u64,
    total_bytes: u128,
}

pub fn enabled() -> bool {
    *PERF_ENABLED
}

pub fn record_duration(label: &'static str, duration: Duration) {
    if !enabled() {
        return;
    }
    let mut stats = STATS.lock().unwrap();
    let entry = stats.entry(label).or_default();
    entry.count += 1;
    let nanos = duration.as_nanos();
    entry.total_ns += nanos;
    if nanos > entry.max_ns {
        entry.max_ns = nanos;
    }
    if entry.count % 200 == 0 {
        print_stat(label, entry);
    }
}

pub fn record_gauge(label: &'static str, value: u64) {
    if !enabled() {
        return;
    }
    let mut gauges = GAUGES.lock().unwrap();
    let entry = gauges.entry(label).or_default();
    entry.last = value;
    entry.samples = entry.samples.saturating_add(1);
    if entry.samples % 200 == 0 {
        eprintln!(
            "[perf] {label}: gauge={} samples={}",
            entry.last, entry.samples
        );
    }
}

pub fn record_bytes(label: &'static str, bytes: usize) {
    if !enabled() {
        return;
    }
    let mut stats = STATS.lock().unwrap();
    let entry = stats.entry(label).or_default();
    entry.total_bytes += bytes as u128;
    entry.count += 1;
    if entry.count % 500 == 0 {
        print_stat(label, entry);
    }
}

fn print_stat(label: &'static str, stat: &PerfStat) {
    let avg_ns = if stat.count > 0 {
        stat.total_ns / stat.count as u128
    } else {
        0
    };
    let avg_us = avg_ns as f64 / 1_000.0;
    let max_us = stat.max_ns as f64 / 1_000.0;
    let mb = stat.total_bytes as f64 / (1024.0 * 1024.0);
    eprintln!(
        "[perf] {label}: count={} avg={avg_us:.2}µs max={max_us:.2}µs bytes={mb:.2}MiB",
        stat.count
    );
}

pub struct PerfGuard {
    label: &'static str,
    start: Instant,
}

impl PerfGuard {
    pub fn new(label: &'static str) -> Option<Self> {
        if !enabled() {
            return None;
        }
        Some(Self {
            label,
            start: Instant::now(),
        })
    }
}

impl Drop for PerfGuard {
    fn drop(&mut self) {
        record_duration(self.label, self.start.elapsed());
    }
}

pub mod logging {
    use clap::ValueEnum;
    use std::fs::OpenOptions;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use tracing::level_filters::LevelFilter;
    use tracing_appender::non_blocking::WorkerGuard;
    use tracing_subscriber::EnvFilter;

    #[derive(Clone, Copy, Debug, Default, ValueEnum, PartialEq, Eq, PartialOrd, Ord)]
    pub enum LogLevel {
        Error,
        #[default]
        Warn,
        Info,
        Debug,
        Trace,
    }

    impl LogLevel {
        pub fn as_str(self) -> &'static str {
            match self {
                LogLevel::Error => "error",
                LogLevel::Warn => "warn",
                LogLevel::Info => "info",
                LogLevel::Debug => "debug",
                LogLevel::Trace => "trace",
            }
        }

        pub fn to_filter(self) -> LevelFilter {
            match self {
                LogLevel::Error => LevelFilter::ERROR,
                LogLevel::Warn => LevelFilter::WARN,
                LogLevel::Info => LevelFilter::INFO,
                LogLevel::Debug => LevelFilter::DEBUG,
                LogLevel::Trace => LevelFilter::TRACE,
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct LogConfig {
        pub level: LogLevel,
        pub file: Option<PathBuf>,
    }

    #[derive(thiserror::Error, Debug)]
    pub enum InitError {
        #[error("logging already initialized")]
        AlreadyInitialized,
        #[error("failed to open log file {path:?}: {source}")]
        Io {
            path: PathBuf,
            source: std::io::Error,
        },
        #[error("failed to configure logger: {0}")]
        Configure(String),
    }

    static INIT: OnceLock<()> = OnceLock::new();
    static GUARD: OnceLock<Option<WorkerGuard>> = OnceLock::new();

    pub fn init(config: &LogConfig) -> Result<(), InitError> {
        if INIT.get().is_some() {
            return Ok(());
        }

        inner_init(config)?;
        INIT.set(()).ok();
        Ok(())
    }

    fn inner_init(config: &LogConfig) -> Result<(), InitError> {
        let level_filter = config.level.to_filter();

        let (env_filter, throttled_deps) = build_env_filter(level_filter);

        let (writer, guard) = match &config.file {
            Some(path) => {
                let file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|source| InitError::Io {
                        path: path.clone(),
                        source,
                    })?;
                tracing_appender::non_blocking(file)
            }
            None => tracing_appender::non_blocking(std::io::stderr()),
        };

        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_level(true)
            .with_target(config.level >= LogLevel::Debug)
            .with_thread_ids(config.level >= LogLevel::Trace)
            .with_thread_names(config.level >= LogLevel::Trace)
            .with_ansi(config.file.is_none())
            .with_writer(writer)
            .finish();

        tracing::subscriber::set_global_default(subscriber)
            .map_err(|err| InitError::Configure(err.to_string()))?;

        let _ = GUARD.set(Some(guard));
        if throttled_deps {
            eprintln!(
                "[beach-log] suppressing dependency trace noise; set BEACH_TRACE_DEPS=1 or BEACH_LOG_FILTER to override"
            );
        }
        Ok(())
    }

    fn build_env_filter(level: LevelFilter) -> (EnvFilter, bool) {
        if let Ok(filter) = std::env::var("BEACH_LOG_FILTER") {
            return (EnvFilter::new(filter), false);
        }
        let (filter, throttled) = default_filter_for(level);
        (EnvFilter::new(filter), throttled)
    }

    const TRACE_DEP_TARGETS: &[&str] = &[
        "hyper",
        "hyper_util",
        "tokio_tungstenite",
        "tungstenite",
        "reqwest",
        "quinn_proto",
        "rustls",
        "mio",
        "h2",
    ];

    fn default_filter_for(level: LevelFilter) -> (String, bool) {
        let base = match level {
            LevelFilter::TRACE => {
                "info,beach_client_core=trace,beach=trace,beach_gate=trace,beach_surfer=trace"
            }
            LevelFilter::DEBUG => {
                "info,beach_client_core=debug,beach=debug,beach_gate=debug,beach_surfer=debug"
            }
            LevelFilter::INFO => "info",
            LevelFilter::WARN => "warn",
            LevelFilter::ERROR => "error",
            LevelFilter::OFF => "off",
        };
        if level == LevelFilter::TRACE && !allow_dependency_traces() {
            (throttle_dependency_traces(base), true)
        } else {
            (base.to_owned(), false)
        }
    }

    fn allow_dependency_traces() -> bool {
        super::env_truthy("BEACH_TRACE_DEPS").unwrap_or(false)
    }

    fn throttle_dependency_traces(base: &str) -> String {
        let mut filter = base.to_owned();
        for target in TRACE_DEP_TARGETS {
            filter.push(',');
            filter.push_str(target);
            filter.push_str("=info");
        }
        filter
    }

    pub fn hexdump(bytes: &[u8]) -> String {
        const WIDTH: usize = 16;
        let mut out = String::new();
        for (i, chunk) in bytes.chunks(WIDTH).enumerate() {
            use std::fmt::Write as _;
            let offset = i * WIDTH;
            let _ = write!(out, "{offset:08x}  ");
            for (j, byte) in chunk.iter().enumerate() {
                if j == WIDTH / 2 {
                    out.push(' ');
                }
                let _ = write!(out, "{byte:02x} ");
            }
            for _ in chunk.len()..WIDTH {
                out.push_str("   ");
            }
            out.push(' ');
            for &byte in chunk {
                let ch = if (0x20..=0x7e).contains(&byte) {
                    byte as char
                } else {
                    '.'
                };
                out.push(ch);
            }
            out.push('\n');
        }
        out
    }
}
