use crate::auth::error::AuthError;
use crate::auth::{self, AuthTokenResponse, FRIENDLY_FALLBACK_MESSAGE};
use crate::terminal::cli::{
    AuthCommand, AuthLoginArgs, AuthLogoutArgs, AuthStatusArgs, AuthSwitchArgs,
};
use crate::terminal::error::CliError;
use std::io::{self, Write};
use std::time::Duration;
use time::{Duration as TimeDuration, OffsetDateTime};
use tokio::time::{Instant, sleep};

const MIN_POLL_INTERVAL: u64 = 3;

pub async fn run(command: AuthCommand, profile_override: Option<String>) -> Result<(), CliError> {
    match command {
        AuthCommand::Login(args) => login(args, profile_override).await,
        AuthCommand::Logout(args) => logout(args, profile_override),
        AuthCommand::Status(args) => status(args, profile_override),
        AuthCommand::SwitchProfile(args) => switch(args, profile_override),
    }
}

fn profile_or_default(args_profile: Option<String>, override_profile: Option<String>) -> String {
    args_profile
        .or(override_profile)
        .unwrap_or_else(|| "default".to_string())
}

async fn login(args: AuthLoginArgs, profile_override: Option<String>) -> Result<(), CliError> {
    let profile_name = profile_or_default(args.profile.clone(), profile_override);

    if !args.force && auth::ensure_profile_exists(&profile_name).map_err(auth_err)? {
        return Err(CliError::InvalidArgument(format!(
            "profile '{profile_name}' already exists; re-run with --force to replace it"
        )));
    }

    let config = auth::BeachAuthConfig::from_env().map_err(auth_err)?;
    let (start, client) = auth::perform_device_login(&profile_name, config.clone())
        .await
        .map_err(auth_err)?;

    println!("🔐 Starting Beach Auth login for profile '{profile_name}'");
    println!("Enter the following code on the verification page:");
    println!("  Code: {}", start.user_code);
    println!("  URL:  {}", start.verification_uri_complete);
    println!();
    println!("Waiting for approval...");

    let interval = Duration::from_secs(start.interval.max(MIN_POLL_INTERVAL));
    let deadline = Instant::now() + Duration::from_secs(start.expires_in.max(60));
    let mut last_message = Instant::now();

    loop {
        if Instant::now() >= deadline {
            return Err(CliError::Auth(
                "device code expired before the login completed".into(),
            ));
        }

        sleep(interval).await;

        match auth::complete_device_login(&profile_name, &client, &start.device_code).await {
            Ok(tokens) => {
                if args.set_current {
                    auth::set_current_profile(Some(profile_name.clone())).map_err(auth_err)?;
                    unsafe {
                        std::env::set_var("BEACH_PROFILE", &profile_name);
                    }
                }

                print_login_summary(&profile_name, &tokens);
                return Ok(());
            }
            Err(AuthError::AuthorizationPending) => {
                if last_message.elapsed() >= Duration::from_secs(10) {
                    println!("Still waiting for approval...");
                    last_message = Instant::now();
                } else {
                    print_progress_dot()?;
                }
            }
            Err(AuthError::AuthorizationDenied) => {
                return Err(CliError::Auth(
                    "authorization request was denied by Beach Auth".into(),
                ));
            }
            Err(err) => {
                return Err(auth_err(err));
            }
        }
    }
}

fn print_login_summary(profile_name: &str, tokens: &AuthTokenResponse) {
    println!();
    println!("✅ Beach Auth login complete for profile '{profile_name}'.");
    println!(
        "   Tier: {}",
        tokens.tier.clone().unwrap_or_else(|| "unknown".into())
    );

    if tokens
        .entitlements
        .iter()
        .any(|ent| ent == auth::FALLBACK_ENTITLEMENT_FLAG)
    {
        println!("   Fallback entitlement: enabled ✅");
    } else {
        println!("   Fallback entitlement: not enabled ⚠️");
        println!("   {FRIENDLY_FALLBACK_MESSAGE}");
    }
}

fn print_progress_dot() -> Result<(), CliError> {
    let mut stdout = io::stdout();
    write!(stdout, ".")?;
    stdout.flush()?;
    Ok(())
}

fn logout(args: AuthLogoutArgs, profile_override: Option<String>) -> Result<(), CliError> {
    if args.all && args.profile.is_some() {
        return Err(CliError::InvalidArgument(
            "--all cannot be combined with --profile".into(),
        ));
    }

    let mut store = auth::load_store().map_err(auth_err)?;
    if args.all {
        let names = store.profile_names();
        if names.is_empty() {
            println!("No Beach Auth profiles to remove.");
            return Ok(());
        }

        for name in &names {
            store.remove_profile(name);
        }
        store.current_profile = None;
        store.save().map_err(auth_err)?;
        unsafe {
            std::env::remove_var("BEACH_PROFILE");
        }
        println!(
            "Removed {count} Beach Auth profile(s).",
            count = names.len()
        );
        return Ok(());
    }

    let target = args
        .profile
        .or(profile_override)
        .or_else(|| store.current_profile.clone())
        .ok_or_else(|| CliError::Auth("no Beach Auth profile to remove".into()))?;

    if store.remove_profile(&target).is_some() {
        store.save().map_err(auth_err)?;
        if std::env::var("BEACH_PROFILE")
            .map(|value| value == target)
            .unwrap_or(false)
        {
            unsafe {
                std::env::remove_var("BEACH_PROFILE");
            }
        }
        println!("Removed Beach Auth profile '{target}'.");
        Ok(())
    } else {
        Err(CliError::Auth(format!(
            "profile '{target}' not found in credential store"
        )))
    }
}

fn status(args: AuthStatusArgs, profile_override: Option<String>) -> Result<(), CliError> {
    let store = auth::load_store().map_err(auth_err)?;
    if store.profiles.is_empty() {
        println!("No Beach Auth profiles have been configured.");
        return Ok(());
    }

    let active_name = store.current_profile.clone();
    let selected = args
        .profile
        .or(profile_override)
        .or(active_name.clone())
        .unwrap_or_else(|| "default".to_string());

    println!("Beach Auth profiles:");
    for name in store.profile_names() {
        let marker = if Some(&name) == active_name.as_ref() {
            "*"
        } else {
            " "
        };
        println!("  {marker} {name}");
    }

    if let Some(profile) = store.profile(&selected) {
        println!();
        println!("Profile: {selected}");
        if let Some(email) = &profile.email {
            println!("  Email: {email}");
        }
        if let Some(tier) = &profile.tier {
            println!("  Tier: {tier}");
        }
        println!(
            "  Fallback entitlement: {}",
            if profile.has_fallback_entitlement() {
                "enabled"
            } else {
                "not enabled"
            }
        );
        if let Some(cache) = &profile.access_token {
            let remaining = cache.expires_at - OffsetDateTime::now_utc();
            println!(
                "  Cached access token: expires {}",
                format_duration(remaining)
            );
        } else {
            println!("  Cached access token: none");
        }
        if profile.entitlements.is_empty() {
            println!("  Entitlements: (none)");
        } else {
            println!("  Entitlements: {}", profile.entitlements.join(", "));
        }
    } else {
        println!();
        println!(
            "Profile '{selected}' is not stored locally. Use `beach auth login` to create it."
        );
    }

    Ok(())
}

fn switch(args: AuthSwitchArgs, profile_hint: Option<String>) -> Result<(), CliError> {
    if args.unset && args.profile.is_some() {
        return Err(CliError::InvalidArgument(
            "cannot pass a profile name when --unset is provided".into(),
        ));
    }

    if args.unset {
        auth::set_current_profile(None).map_err(auth_err)?;
        unsafe {
            std::env::remove_var("BEACH_PROFILE");
        }
        println!("Cleared the active Beach Auth profile.");
        return Ok(());
    }

    let profile = args
        .profile
        .or(profile_hint)
        .ok_or_else(|| CliError::InvalidArgument("provide a profile name or use --unset".into()))?;

    auth::set_current_profile(Some(profile.clone()))
        .map_err(|err| CliError::Auth(err.to_string()))?;
    unsafe {
        std::env::set_var("BEACH_PROFILE", &profile);
    }
    println!("Active Beach Auth profile set to '{profile}'.");
    Ok(())
}

fn format_duration(duration: TimeDuration) -> String {
    if duration.is_negative() {
        return "in the past".into();
    }
    let secs = duration.whole_seconds();
    if secs < 60 {
        return format!("in {secs} seconds");
    }
    let mins = duration.whole_minutes();
    if mins < 60 {
        return format!("in {mins} minutes");
    }
    let hours = duration.whole_hours();
    if hours < 48 {
        return format!("in {hours} hours");
    }
    let days = hours / 24;
    format!("in {days} days")
}

fn auth_err(error: AuthError) -> CliError {
    CliError::Auth(error.to_string())
}
