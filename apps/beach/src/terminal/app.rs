use crate::client::terminal::{debug, join};
use crate::server::terminal::host;
use crate::terminal::cli::{self, Command, HostArgs};
use crate::terminal::error::CliError;
use crate::transport::ssh;

pub async fn run(cli: cli::Cli) -> Result<(), CliError> {
    let session_base = cli.session_server;

    match cli.command {
        Some(Command::Join(args)) => join::run(&session_base, args).await,
        Some(Command::Ssh(args)) => ssh::run(&session_base, args).await,
        Some(Command::Host(args)) => host::run(&session_base, args).await,
        Some(Command::Debug(args)) => {
            debug::run(args)?;
            Ok(())
        }
        None => host::run(&session_base, HostArgs::default()).await,
    }
}
