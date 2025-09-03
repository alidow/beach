use std::io::IsTerminal;
use std::env;
use anyhow::Result;
use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize};

/// Get terminal size using crossterm
pub fn get_terminal_size() -> Result<(u16, u16)> {
    // Try to get terminal size using crossterm
    if std::io::stdout().is_terminal() {
        // Use crossterm to get terminal size
        match terminal::size() {
            Ok((cols, rows)) => Ok((cols, rows)),
            Err(_) => {
                // Fallback to default if we can't get size
                Ok((80, 24))
            }
        }
    } else {
        Err(anyhow::anyhow!("Not running in a terminal"))
    }
}

/// Get PTY size configuration
pub fn get_pty_size() -> PtySize {
    if let Ok((cols, rows)) = get_terminal_size() {
        PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    } else {
        // Default size if we can't detect terminal size
        PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// Get the default shell for the current user
pub fn get_default_shell() -> String {
    // Try environment variables first
    if let Ok(shell) = env::var("SHELL") {
        return shell;
    }
    
    // Fallback to common shells
    for shell in &["/bin/bash", "/bin/zsh", "/bin/sh"] {
        if std::path::Path::new(shell).exists() {
            return shell.to_string();
        }
    }
    
    // Last resort
    "/bin/sh".to_string()
}

/// Build command from provided arguments or default shell
pub fn build_command(cmd: &[String]) -> CommandBuilder {
    if cmd.is_empty() {
        // Get default shell
        let shell = get_default_shell();
        eprintln!("🏖️  Beach Server: Starting shell: {}", shell);
        CommandBuilder::new(shell)
    } else {
        // Use provided command
        eprintln!("🏖️  Beach Server: Starting command: {:?}", cmd);
        let mut cmd_builder = CommandBuilder::new(&cmd[0]);
        for arg in cmd.iter().skip(1) {
            cmd_builder.arg(arg);
        }
        cmd_builder
    }
}

/// Enable raw mode for terminal
pub fn enable_raw_mode() -> Result<()> {
    terminal::enable_raw_mode()?;
    Ok(())
}

/// Disable raw mode for terminal
pub fn disable_raw_mode() -> Result<()> {
    terminal::disable_raw_mode()?;
    Ok(())
}