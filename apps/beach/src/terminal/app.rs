use crate::client::terminal::{debug, join};
use crate::server::terminal::host;
use crate::terminal::cli::{self, Command, HostArgs};
use crate::terminal::error::CliError;
use crate::transport::ssh;
use std::env;
use tracing::info;

pub async fn run(cli: cli::Cli) -> Result<(), CliError> {
    let session_base = cli.session_server;

    apply_fallback_overrides(&cli.fallback);

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

fn apply_fallback_overrides(overrides: &cli::FallbackArgs) {
    if let Some(cohort) = overrides.cohort.as_ref() {
        if cohort.trim().is_empty() {
            unsafe {
                env::remove_var("BEACH_FALLBACK_COHORT");
            }
            info!(target: "beach::fallback", "cleared fallback cohort override");
        } else {
            unsafe {
                env::set_var("BEACH_FALLBACK_COHORT", cohort);
            }
            info!(target: "beach::fallback", fallback_cohort = %cohort, "fallback cohort override applied");
        }
    }

    if let Some(proof) = overrides.entitlement_proof.as_ref() {
        if proof.trim().is_empty() {
            unsafe {
                env::remove_var("BEACH_ENTITLEMENT_PROOF");
            }
            info!(target: "beach::fallback", "cleared fallback entitlement proof override");
        } else {
            unsafe {
                env::set_var("BEACH_ENTITLEMENT_PROOF", proof);
            }
            info!(
                target: "beach::fallback",
                entitlement_proof_provided = true,
                "fallback entitlement proof override applied"
            );
        }
    }

    if let Some(opt_in) = overrides.telemetry_opt_in {
        unsafe {
            env::set_var(
                "BEACH_FALLBACK_TELEMETRY_OPT_IN",
                if opt_in { "1" } else { "0" },
            );
        }
        info!(
            target: "beach::fallback",
            fallback_telemetry_opt_in = opt_in,
            "fallback telemetry override applied"
        );
    }
}
