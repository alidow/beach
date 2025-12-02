use crate::terminal::cli::ActionArgs;
use crate::terminal::error::CliError;

/// Controller action path is disabled; hosts no longer talk to the manager over HTTP or MCP for
/// controller traffic. Use the unified WebRTC channel instead.
pub async fn run(_profile: Option<&str>, _args: ActionArgs) -> Result<(), CliError> {
    Err(CliError::InvalidArgument(
        "controller actions over HTTP/MCP are disabled; use unified WebRTC transport".into(),
    ))
}
