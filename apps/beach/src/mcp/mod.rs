pub mod auth;
pub mod bridge;
pub mod client;
pub mod client_proxy;
pub mod protocol;
pub mod registry;
pub mod server;
pub mod terminal;

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct McpConfig {
    pub socket: Option<PathBuf>,
    pub use_stdio: bool,
    pub read_only: bool,
    pub allow_write: bool,
    pub session_filter: Option<Vec<String>>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            socket: None,
            use_stdio: false,
            read_only: true,
            allow_write: false,
            session_filter: None,
        }
    }
}

impl McpConfig {
    pub fn effective_read_only(&self) -> bool {
        self.read_only && !self.allow_write
    }
}

pub use server::{McpServer, McpServerHandle};

pub fn default_socket_path(session_id: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = Path::new(&home).join(".beach").join("mcp");
    dir.join(format!("{session_id}.sock"))
}
