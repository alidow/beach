use std::io::IsTerminal;
use std::env;
use anyhow::Result;
use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize};

/// Get terminal size using crossterm (cross-platform)
pub fn get_terminal_size() -> Result<(u16, u16)> {
    // Try to get terminal size using crossterm
    if std::io::stdout().is_terminal() {
        // First try crossterm - this should work on both Windows and Unix
        if let Ok((cols, rows)) = terminal::size() {
            return Ok((cols, rows));
        }
        
        // Fallback: Try environment variables (works on both Windows and Unix)
        if let (Ok(cols_str), Ok(rows_str)) = (env::var("COLUMNS"), env::var("LINES")) {
            if let (Ok(cols), Ok(rows)) = (cols_str.parse::<u16>(), rows_str.parse::<u16>()) {
                if cols > 0 && rows > 0 {
                    return Ok((cols, rows));
                }
            }
        }
        
        // Unix-specific fallbacks
        #[cfg(unix)]
        {
            // Try TIOCGWINSZ ioctl directly
            use libc::{ioctl, winsize, TIOCGWINSZ};
            unsafe {
                let mut ws = winsize {
                    ws_row: 0,
                    ws_col: 0,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                // Try stdout, stderr, and stdin
                for fd in &[0, 1, 2] {
                    if ioctl(*fd, TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
                        return Ok((ws.ws_col, ws.ws_row));
                    }
                }
            }
            
            // Try tput command
            use std::process::Command;
            
            let cols_output = Command::new("tput").arg("cols").output();
            let rows_output = Command::new("tput").arg("lines").output();
            
            if let (Ok(cols_out), Ok(rows_out)) = (cols_output, rows_output) {
                if let (Ok(cols_str), Ok(rows_str)) = (
                    String::from_utf8(cols_out.stdout),
                    String::from_utf8(rows_out.stdout)
                ) {
                    if let (Ok(cols), Ok(rows)) = (
                        cols_str.trim().parse::<u16>(),
                        rows_str.trim().parse::<u16>()
                    ) {
                        if cols > 0 && rows > 0 {
                            return Ok((cols, rows));
                        }
                    }
                }
            }
        }
        
        // Last resort: Use common terminal defaults
        Ok((80, 24))
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
        // Starting shell silently
        CommandBuilder::new(shell)
    } else {
        // Use provided command silently
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