use clap::{Parser, Subcommand, ValueEnum};
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
    #[cfg(feature = "webrtc")]
    /// Host flow: generate sealed SDP offer, optionally POST to a fixture URL, then
    /// poll a fixture directory for the viewer's sealed answer, finalize the session,
    /// complete the Noise handshake, and print the verification code.
    WebRtcHostRun {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        passcode: String,
        /// Streaming codec (png|h264); when --window-id is provided.
        #[arg(long, value_enum, default_value_t = EncodeCodec::H264)]
        codec: EncodeCodec,
        /// Beach Road base URL (e.g., http://127.0.0.1:8080). If omitted, defaults to 127.0.0.1:8080.
        #[arg(long)]
        road_url: Option<String>,
        /// Local peer id (default: host)
        #[arg(long, default_value = "host")]
        from_id: String,
        /// Remote peer id (default: viewer)
        #[arg(long, default_value = "viewer")]
        to_id: String,
        /// Optional fixture URL to POST the sealed host offer JSON.
        #[arg(long)]
        fixture_url: Option<String>,
        /// Fixture directory to poll for the viewer answer JSON (written by fixture-serve).
        #[arg(long)]
        fixture_dir: Option<std::path::PathBuf>,
        /// Optional prologue context for Noise handshake.
        #[arg(long, default_value = "cabana-webrtc")]
        prologue: String,
        /// Optional capture target (macOS) like display:<ID> or a CGWindowID; if provided, host streams PNG frames.
        #[arg(long)]
        window_id: Option<String>,
        /// Number of frames to stream when --window-id is provided.
        #[arg(long, default_value_t = 0)]
        frames: u32,
        /// Interval between frames in milliseconds.
        #[arg(long, default_value_t = 33)]
        interval_ms: u64,
        /// Optional downscale width for PNG frames.
        #[arg(long)]
        max_width: Option<u32>,
    },
    #[cfg(feature = "webrtc")]
    /// Viewer flow: unseal host offer, generate and seal answer, optionally POST to fixture.
    WebRtcViewerAnswer {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        passcode: String,
        /// Host sealed envelope (compact string). Use either this or --host-envelope-file.
        #[arg(long)]
        host_envelope: Option<String>,
        /// Path to a file containing the host sealed envelope string.
        #[arg(long)]
        host_envelope_file: Option<std::path::PathBuf>,
        /// Optional fixture URL to POST the sealed viewer answer JSON.
        #[arg(long)]
        fixture_url: Option<String>,
        /// Optional prologue context for Noise handshake.
        #[arg(long, default_value = "cabana-webrtc")]
        prologue: String,
    },
    #[cfg(feature = "webrtc")]
    /// Viewer run: unseal host offer, generate and (optionally) POST sealed answer, then
    /// receive PNG frames over the secure data channel and save to a directory.
    WebRtcViewerRun {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        passcode: String,
        /// Beach Road base URL (e.g., http://127.0.0.1:8080). If omitted, defaults to 127.0.0.1:8080.
        #[arg(long)]
        road_url: Option<String>,
        /// Local peer id (default: viewer)
        #[arg(long, default_value = "viewer")]
        from_id: String,
        /// Remote peer id (default: host)
        #[arg(long, default_value = "host")]
        to_id: String,
        #[arg(long)]
        host_envelope: Option<String>,
        #[arg(long)]
        host_envelope_file: Option<std::path::PathBuf>,
        #[arg(long)]
        fixture_url: Option<String>,
        #[arg(long, default_value = "cabana-webrtc")]
        prologue: String,
        /// Number of frames to receive.
        #[arg(long, default_value_t = 30)]
        recv_frames: u32,
        /// Directory to save received PNGs (default: temp/cabana-viewer-<ts>).
        #[arg(long)]
        output_dir: Option<std::path::PathBuf>,
    },
    #[cfg(feature = "webrtc")]
    /// Create a session on Beach Road and print the URL + join code.
    RoadCreateSession {
        /// Beach Road base URL (e.g., http://127.0.0.1:8080)
        #[arg(long)]
        road_url: Option<String>,
        /// Explicit session id (if omitted, a random UUID is used)
        #[arg(long)]
        session_id: Option<String>,
        /// Optional passphrase for Road (NOT the zero-trust passcode).
        #[arg(long)]
        road_passphrase: Option<String>,
    },
    #[cfg(feature = "webrtc")]
    /// Join a session on Beach Road (utility for quick setup).
    RoadJoinSession {
        /// Beach Road base URL (e.g., http://127.0.0.1:8080)
        #[arg(long)]
        road_url: Option<String>,
        /// Session id to join
        #[arg(long)]
        session_id: String,
        /// Optional passphrase for Road (NOT the zero-trust passcode).
        #[arg(long)]
        road_passphrase: Option<String>,
        /// Mark as MCP client (optional)
        #[arg(long, default_value_t = false)]
        mcp: bool,
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
    /// Capture and encode a short session to an artifact.
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
        /// Preferred encoder (macOS only).
        #[arg(long, value_enum, default_value_t = EncodeCodec::Gif)]
        codec: EncodeCodec,
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
    #[cfg(feature = "webrtc")]
    /// Local WebRTC + Noise demo: creates two in-process peers, opens a data channel,
    /// completes the zero-trust Noise handshake, prints the verification code, and
    /// exchanges encrypted demo messages.
    WebRtcLocal {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        passcode: String,
        #[arg(long, default_value = "host")]
        host_id: String,
        #[arg(long, default_value = "viewer")]
        viewer_id: String,
        #[arg(long, default_value = "cabana-local-demo")]
        prologue: String,
    },
}

#[derive(Debug, Copy, Clone, ValueEnum)]
pub enum EncodeCodec {
    Gif,
    #[value(name = "h264")]
    H264,
}

