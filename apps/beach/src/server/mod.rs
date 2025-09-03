mod pty;
mod terminal;
mod io;
pub mod terminal_state;
pub mod debug_handler;

use async_trait::async_trait;
use crate::transport::Transport;
use crate::session::{ServerSession, signaling::AppMessage};
use crate::session::handlers::ServerMessageHandler;
use crate::session::signaling::PeerInfo;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock as AsyncRwLock;
use tokio::task::JoinHandle;
use anyhow::Result;

use self::pty::PtyManager;
use self::terminal::{get_pty_size, build_command, enable_raw_mode, disable_raw_mode};
use self::io::{spawn_pty_reader, spawn_stdin_reader, spawn_pty_reader_with_tracker};
use self::terminal_state::TerminalStateTracker;
use self::debug_handler::DebugHandler;

#[async_trait]
pub trait Server {
    type Transport: Transport + Send + 'static;

    async fn start(&self);
    async fn stop(&self);
}

pub struct TerminalServer<T: Transport + Send + 'static> {
    session: Arc<AsyncRwLock<ServerSession<T>>>,
    pty_manager: PtyManager,
    read_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    stdin_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    terminal_tracker: Arc<Mutex<TerminalStateTracker>>,
    debug_handler: Arc<DebugHandler>,
}

impl<T: Transport + Send + 'static> TerminalServer<T> {
    pub fn new(session: ServerSession<T>) -> Arc<Self> {
        // Create terminal state tracker with default size (will be updated when PTY starts)
        let terminal_tracker = Arc::new(Mutex::new(TerminalStateTracker::new(80, 24)));
        let debug_handler = Arc::new(DebugHandler::new());
        
        Arc::new(TerminalServer { 
            session: Arc::new(AsyncRwLock::new(session)),
            pty_manager: PtyManager::new(),
            read_task: Arc::new(Mutex::new(None)),
            stdin_task: Arc::new(Mutex::new(None)),
            terminal_tracker,
            debug_handler,
        })
    }
    
    pub async fn setup_handlers(self: Arc<Self>) {
        // Set debug handler and message handler
        eprintln!("🏖️  Beach Server: Setting up handlers...");
        let mut session = self.session.write().await;
        session.set_debug_handler(Arc::clone(&self.debug_handler));
        session.set_handler(self.clone()).await;
        eprintln!("🏖️  Beach Server: Handlers configured");
    }
}

#[async_trait]
impl<T: Transport + Send + 'static> Server for TerminalServer<T> {
    type Transport = T;

    async fn start(&self) {
        // Get terminal size
        let pty_size = get_pty_size();

        // Build command from session config
        let cmd = {
            let session = self.session.read().await;
            build_command(session.cmd())
        };
       
        // Initialize PTY
        self.pty_manager.init(pty_size, cmd)
            .expect("Failed to initialize PTY");
        
        // Update terminal tracker with actual size
        {
            let mut tracker = self.terminal_tracker.lock().unwrap();
            *tracker = TerminalStateTracker::new(pty_size.cols, pty_size.rows);
        }
        
        // Initialize debug handler with the tracker
        self.debug_handler.set_tracker(Arc::clone(&self.terminal_tracker));
        
        // Print messages before entering raw mode
        eprintln!("🏖️  Beach Server: PTY initialized, setting up I/O...");
        eprintln!("🏖️  Beach Server: Ready! You're now in the Beach shell.");
        eprintln!("🏖️  Type 'exit' or press Ctrl+D to quit.");
        eprintln!(); // Extra newline for clarity
        
        // Enable raw mode for proper terminal interaction
        let _ = enable_raw_mode();
        
        // Start reading from PTY in a background task with terminal state tracking
        let read_task = spawn_pty_reader_with_tracker(
            self.pty_manager.master_reader.clone(),
            self.terminal_tracker.clone()
        );
        *self.read_task.lock().unwrap() = Some(read_task);
        
        // Start reading from stdin and writing to PTY (using spawn_blocking for blocking I/O)
        let stdin_task = spawn_stdin_reader(self.pty_manager.master_writer.clone());
        
        // Store stdin task handle for cleanup
        let stdin_task_stored = {
            let mut guard = self.stdin_task.lock().unwrap();
            *guard = Some(stdin_task);
            guard.as_ref().unwrap().abort_handle()
        };
        
        // Wait for PTY task to complete
        loop {
            let finished = {
                let guard = self.read_task.lock().unwrap();
                guard.as_ref().map(|t| t.is_finished()).unwrap_or(true)
            };
            if finished {
                // PTY ended, abort stdin task to prevent hanging
                stdin_task_stored.abort();
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        
        // Clean up
        self.stop().await;
    }

    async fn stop(&self) {
        // Restore terminal first before printing
        let _ = disable_raw_mode();
        
        eprintln!("\n🏖️  Beach Server: Shutting down...");
        
        // Stop the read task
        if let Some(task) = self.read_task.lock().unwrap().take() {
            task.abort();
        }
        
        // Stop the stdin task - this is important to prevent hanging
        if let Some(task) = self.stdin_task.lock().unwrap().take() {
            task.abort();
        }
        
        // Clean up PTY resources
        self.pty_manager.cleanup();
    }
}

// Method to write stdin to the PTY
impl<T: Transport + Send + 'static> TerminalServer<T> {
    pub fn write_to_pty(&self, data: &[u8]) -> Result<()> {
        self.pty_manager.write(data)
    }
}

// Implement ServerMessageHandler to handle WebSocket messages
#[async_trait]
impl<T: Transport + Send + Sync + 'static> ServerMessageHandler for TerminalServer<T> {
    async fn handle_client_message(&self, _from_peer: &str, message: AppMessage) {
        match message {
            AppMessage::TerminalInput { data } => {
                // Handle terminal input from client
                let _ = self.write_to_pty(&data);
            }
            AppMessage::TerminalResize { cols, rows } => {
                // Handle terminal resize
                // TODO: Implement PTY resize
                // Don't print to stderr while terminal is active - it corrupts the display
                let _ = (cols, rows); // Suppress unused warning
            }
            _ => {
                // Ignore other messages for now
            }
        }
    }
    
    async fn handle_client_joined(&self, peer: &PeerInfo) {
        // Don't print to stderr while terminal is active - it corrupts the display
        // eprintln!("🏖️  Beach Server: Client {} joined the session", peer.id);
        let _ = peer; // Suppress unused warning
    }
    
    async fn handle_client_left(&self, peer_id: &str) {
        // Don't print to stderr while terminal is active - it corrupts the display
        // eprintln!("🏖️  Beach Server: Client {} left the session", peer_id);
        let _ = peer_id; // Suppress unused warning
    }
}