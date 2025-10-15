use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "beach-cabana",
    about = "Experimental Beach GUI sharing host (standalone)",
    author,
    version,
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Enumerate windows that cabana can target.
    ListWindows {
        /// Print the list as JSON (for scripting).
        #[arg(long)]
        json: bool,
    },
    /// Open a local preview for the given window identifier.
    Preview {
        #[arg(long = "window-id")]
        window_id: String,
    },
    /// Capture multiple frames for the given window/display and store them in a directory.
    Stream {
        #[arg(long = "window-id")]
        window_id: String,
        /// Number of frames to capture.
        #[arg(long, default_value_t = 30)]
        frames: u32,
        /// Delay between frames in milliseconds.
        #[arg(long, default_value_t = 100)]
        interval_ms: u64,
        /// Optional directory to store frames (default: temp dir with timestamp).
        #[arg(long)]
        output_dir: Option<PathBuf>,
    },
    /// Capture and encode a short session to an animated GIF.
    Encode {
        #[arg(long = "window-id")]
        window_id: String,
        /// Duration in seconds to capture.
        #[arg(long, default_value_t = 5)]
        duration_secs: u32,
        /// Target frames per second.
        #[arg(long, default_value_t = 10)]
        fps: u32,
        /// Optional maximum width for downscaled frames.
        #[arg(long)]
        max_width: Option<u32>,
        /// Output file path (will be overwritten).
        #[arg(long, default_value = "cabana-output.gif")]
        output: PathBuf,
    },
    /// Derive session keys and prepare to share a target over WebRTC.
    Start {
        /// Full session URL (unique link). Cabana derives the session id from the final path segment.
        #[arg(long)]
        session_url: Option<String>,
        /// Explicit session id (if not using --session-url).
        #[arg(long)]
        session_id: Option<String>,
        /// Session passcode. Required for zero-trust signaling.
        #[arg(long)]
        passcode: String,
        /// Optional pre-selected window id; otherwise Cabana will launch the picker.
        #[arg(long)]
        window_id: Option<String>,
        /// Optional base64 handshake id; if omitted Cabana generates one.
        #[arg(long)]
        handshake_id: Option<String>,
        /// Optional file containing a plaintext signaling payload (e.g. SDP offer) to seal.
        #[arg(long)]
        payload_file: Option<std::path::PathBuf>,
        /// Optional fixture URL (e.g. http://127.0.0.1:8081/signaling) to POST the sealed envelope for testing.
        #[arg(long)]
        fixture_url: Option<String>,
    },
    /// Run a local beach-road fixture that stores sealed signaling envelopes on disk.
    FixtureServe {
        /// Address to listen on (e.g. 127.0.0.1:8081).
        #[arg(long, default_value = "127.0.0.1:8081")]
        listen: SocketAddr,
        /// Directory to store received envelopes.
        #[arg(long, default_value = "./cabana-fixture")]
        storage_dir: PathBuf,
    },
    /// Utility: seal an arbitrary payload with the session keys for debugging.
    SealProbe {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        passcode: String,
        #[arg(long)]
        payload: String,
        #[arg(long)]
        handshake_id: Option<String>,
    },
    /// Utility: open a sealed payload produced by `seal-probe`.
    OpenProbe {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        passcode: String,
        #[arg(long)]
        envelope: String,
    },
    /// Diagnostic: run a local Noise XXpsk2 handshake and inspect derived media keys.
    NoiseDiag {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        passcode: String,
        #[arg(long)]
        handshake_id: Option<String>,
        #[arg(long, default_value = "host")]
        host_id: String,
        #[arg(long, default_value = "viewer")]
        viewer_id: String,
        #[arg(long, default_value = "cabana-cli")]
        prologue: String,
    },
}
