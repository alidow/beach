use clap::{Args, Parser, Subcommand, ValueEnum, builder::BoolishValueParser};
use std::path::PathBuf;

use crate::telemetry::logging::{LogConfig, LogLevel};

#[derive(Parser, Debug)]
#[command(
    name = "beach",
    about = "🏖️  Share a terminal session with WebRTC/WebSocket transports",
    author,
    version = concat!(env!("CARGO_PKG_VERSION"), "-", env!("BUILD_TIMESTAMP"))
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        env = "BEACH_SESSION_SERVER",
        default_value = "https://api.beach.sh",
        help = "Base URL for the beach-road session broker"
    )]
    pub session_server: String,

    #[arg(
        long = "profile",
        global = true,
        env = "BEACH_PROFILE",
        value_name = "PROFILE",
        help = "Select the Beach Auth profile to use for this command"
    )]
    pub profile: Option<String>,

    #[command(flatten)]
    pub logging: LoggingArgs,

    #[command(flatten)]
    pub fallback: FallbackArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Args, Debug, Clone)]
pub struct LoggingArgs {
    #[arg(
        long = "log-level",
        value_enum,
        env = "BEACH_LOG_LEVEL",
        default_value_t = LogLevel::Warn,
        help = "Minimum log level (error, warn, info, debug, trace)"
    )]
    pub level: LogLevel,

    #[arg(
        long = "log-file",
        value_name = "PATH",
        env = "BEACH_LOG_FILE",
        help = "Write structured logs to the specified file"
    )]
    pub file: Option<PathBuf>,
}

impl LoggingArgs {
    pub fn to_config(&self) -> LogConfig {
        LogConfig {
            level: self.level,
            file: self.file.clone(),
        }
    }
}

#[derive(Args, Debug, Clone, Default)]
pub struct FallbackArgs {
    #[arg(
        long = "fallback-cohort",
        env = "BEACH_FALLBACK_COHORT",
        value_name = "COHORT",
        help = "Override the fallback cohort/entitlement identifier when requesting WebSocket rescue tokens"
    )]
    pub cohort: Option<String>,

    #[arg(
        long = "fallback-entitlement-proof",
        env = "BEACH_ENTITLEMENT_PROOF",
        value_name = "TOKEN",
        help = "Signed entitlement proof to accompany fallback token requests (Beach Auth override)",
        hide_env_values = true
    )]
    pub entitlement_proof: Option<String>,

    #[arg(
        long = "fallback-telemetry-opt-in",
        env = "BEACH_FALLBACK_TELEMETRY_OPT_IN",
        num_args = 0..=1,
        default_missing_value = "true",
        value_parser = BoolishValueParser::new(),
        value_name = "BOOL",
        help = "Opt in to fallback-specific telemetry when requesting rescue tokens",
    )]
    pub telemetry_opt_in: Option<bool>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Explicitly host a session (default when no subcommand given)
    Host(HostArgs),
    /// Join an existing session using a session id or share URL
    Join(JoinArgs),
    /// Bootstrap a remote session over SSH and auto-attach the local client
    Ssh(SshArgs),
    /// Query diagnostic state from a running session
    Debug(DebugArgs),
    /// Manage Beach Auth credentials and profiles
    #[command(subcommand)]
    Auth(AuthCommand),
}

#[derive(Subcommand, Debug)]
pub enum AuthCommand {
    /// Start a device login to acquire Beach Auth credentials
    Login(AuthLoginArgs),
    /// Remove Beach Auth credentials
    Logout(AuthLogoutArgs),
    /// Show stored Beach Auth status
    Status(AuthStatusArgs),
    /// Switch the active Beach Auth profile
    SwitchProfile(AuthSwitchArgs),
}

#[derive(Args, Debug)]
pub struct AuthLoginArgs {
    #[arg(
        long = "name",
        value_name = "PROFILE",
        help = "Profile name to create or update (defaults to 'default')"
    )]
    pub profile: Option<String>,

    #[arg(
        long = "set-current",
        action = clap::ArgAction::SetTrue,
        help = "Set this profile as the active profile after login completes"
    )]
    pub set_current: bool,

    #[arg(
        long = "force",
        action = clap::ArgAction::SetTrue,
        help = "Overwrite existing credentials for the profile if present"
    )]
    pub force: bool,
}

#[derive(Args, Debug, Default)]
pub struct AuthLogoutArgs {
    #[arg(
        long = "profile",
        value_name = "PROFILE",
        help = "Remove only the specified profile (defaults to the active profile)"
    )]
    pub profile: Option<String>,

    #[arg(
        long = "all",
        action = clap::ArgAction::SetTrue,
        help = "Remove all stored Beach Auth credentials"
    )]
    pub all: bool,
}

#[derive(Args, Debug, Default)]
pub struct AuthStatusArgs {
    #[arg(
        long = "profile",
        value_name = "PROFILE",
        help = "Show status for a specific profile"
    )]
    pub profile: Option<String>,
}

#[derive(Args, Debug, Default)]
pub struct AuthSwitchArgs {
    #[arg(value_name = "PROFILE", help = "Profile name to mark as active")]
    pub profile: Option<String>,

    #[arg(
        long = "unset",
        action = clap::ArgAction::SetTrue,
        help = "Clear the active profile"
    )]
    pub unset: bool,
}

#[derive(Args, Debug, Default)]
pub struct HostArgs {
    #[arg(
        long,
        value_name = "PROGRAM",
        help = "Override the shell launched for hosting (defaults to $SHELL)"
    )]
    pub shell: Option<String>,

    #[arg(
        trailing_var_arg = true,
        value_name = "COMMAND",
        help = "Command to run instead of the shell"
    )]
    pub command: Vec<String>,

    #[arg(
        long = "local-preview",
        action = clap::ArgAction::SetTrue,
        help = "Open a local preview client in this terminal"
    )]
    pub local_preview: bool,

    #[arg(
        long = "wait",
        action = clap::ArgAction::SetTrue,
        help = "Wait for a peer to connect before launching the host command"
    )]
    pub wait: bool,

    #[arg(
        long = "require-client-approval",
        action = clap::ArgAction::SetTrue,
        help = "Prompt before accepting new clients (defaults to auto-accept)"
    )]
    pub require_client_approval: bool,

    #[arg(
        long = "allow-all-clients",
        action = clap::ArgAction::SetTrue,
        hide = true
    )]
    pub legacy_allow_all_clients: bool,

    #[arg(
        long = "bootstrap-output",
        value_enum,
        default_value_t = BootstrapOutput::Default,
        help = "Control how bootstrap metadata is emitted (default banner or json envelope)"
    )]
    pub bootstrap_output: BootstrapOutput,

    #[arg(
        long = "mcp",
        action = clap::ArgAction::SetTrue,
        help = "Expose an MCP server for this host session"
    )]
    pub mcp: bool,

    #[arg(
        long = "mcp-socket",
        value_name = "PATH",
        help = "Serve the MCP endpoint on the specified unix socket"
    )]
    pub mcp_socket: Option<PathBuf>,

    #[arg(
        long = "mcp-stdio",
        action = clap::ArgAction::SetTrue,
        help = "Serve the MCP endpoint over stdio instead of a socket"
    )]
    pub mcp_stdio: bool,

    #[arg(
        long = "mcp-allow-write",
        action = clap::ArgAction::SetTrue,
        help = "Allow MCP clients to inject input into the session"
    )]
    pub mcp_allow_write: bool,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BootstrapOutput {
    #[default]
    Default,
    Json,
}

#[derive(Args, Debug)]
pub struct JoinArgs {
    #[arg(value_name = "SESSION", help = "Session id or share URL")]
    pub target: String,

    #[arg(
        long,
        short = 'p',
        value_name = "CODE",
        help = "Six character alphanumeric passcode (prompted interactively if omitted)"
    )]
    pub passcode: Option<String>,

    #[arg(
        long = "label",
        value_name = "TEXT",
        env = "BEACH_CLIENT_LABEL",
        help = "Optional identifier displayed to the host"
    )]
    pub label: Option<String>,

    #[arg(
        long = "mcp",
        action = clap::ArgAction::SetTrue,
        help = "Expose the host's MCP server locally via WebRTC"
    )]
    pub mcp: bool,

    #[arg(
        long = "inject-latency",
        value_name = "MS",
        help = "Inject artificial latency (ms) to server responses for testing"
    )]
    pub inject_latency: Option<u64>,
}

#[derive(Args, Debug)]
pub struct SshArgs {
    #[arg(value_name = "TARGET", help = "SSH destination (user@host or host)")]
    pub target: String,

    #[arg(
        long = "remote-path",
        default_value = "beach",
        value_name = "PATH",
        help = "Remote beach binary name or absolute path"
    )]
    pub remote_path: String,

    #[arg(
        long = "ssh-binary",
        default_value = "ssh",
        value_name = "BIN",
        help = "SSH executable to invoke"
    )]
    pub ssh_binary: String,

    #[arg(
        long = "ssh-flag",
        value_name = "FLAG",
        action = clap::ArgAction::Append,
        help = "Additional flag to pass through to ssh (repeatable)"
    )]
    pub ssh_flag: Vec<String>,

    #[arg(
        long = "no-batch",
        action = clap::ArgAction::SetTrue,
        help = "Do not force BatchMode=yes when invoking ssh"
    )]
    pub no_batch: bool,

    #[arg(
        long = "copy-binary",
        action = clap::ArgAction::SetTrue,
        help = "Upload the local beach binary to the remote path via scp before launching"
    )]
    pub copy_binary: bool,

    #[arg(
        long = "copy-from",
        value_name = "PATH",
        help = "Override the local binary path to upload (defaults to current executable)"
    )]
    pub copy_from: Option<PathBuf>,

    #[arg(
        long = "scp-binary",
        default_value = "scp",
        value_name = "BIN",
        help = "scp executable to invoke when --copy-binary is set"
    )]
    pub scp_binary: String,

    #[arg(
        long = "verify-binary-hash",
        action = clap::ArgAction::SetTrue,
        help = "Verify the remote binary's SHA-256 hash after upload"
    )]
    pub verify_binary_hash: bool,

    #[arg(
        long = "keep-ssh",
        action = clap::ArgAction::SetTrue,
        help = "Leave the SSH control channel open for log tailing instead of closing after bootstrap"
    )]
    pub keep_ssh: bool,

    #[arg(
        long = "request-tty",
        action = clap::ArgAction::SetTrue,
        help = "Request an interactive TTY from ssh instead of disabling it"
    )]
    pub request_tty: bool,

    #[arg(
        long = "handshake-timeout",
        default_value_t = 30u64,
        value_name = "SECONDS",
        help = "Seconds to wait for the bootstrap handshake before failing"
    )]
    pub handshake_timeout: u64,

    #[arg(
        trailing_var_arg = true,
        value_name = "COMMAND",
        help = "Command to run remotely instead of the default shell"
    )]
    pub command: Vec<String>,
}

#[derive(Args, Debug)]
pub struct DebugArgs {
    #[arg(value_name = "SESSION_ID", help = "Session ID to inspect")]
    pub session_id: String,

    #[arg(
        long,
        short = 'q',
        value_name = "QUERY",
        help = "What to query: cursor, dimensions, cache"
    )]
    pub query: Option<String>,

    #[arg(
        long,
        short = 's',
        value_name = "TEXT",
        help = "Send input text to the session"
    )]
    pub send: Option<String>,
}

pub fn parse() -> Cli {
    Cli::parse()
}
