mod pty;
mod terminal;
mod io;
pub mod terminal_state;
pub mod debug_handler;

use async_trait::async_trait;
use std::io::IsTerminal;
use crate::transport::Transport;
use crate::session::ServerSession;
use crate::protocol::signaling::{AppMessage, PeerInfo};
use crate::session::message_handlers::ServerMessageHandler;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock as AsyncRwLock;
use tokio::task::JoinHandle;
use anyhow::Result;

use self::pty::PtyManager;
use self::terminal::{get_pty_size, build_command, enable_raw_mode, disable_raw_mode};
use self::io::{spawn_stdin_reader, spawn_pty_reader_with_resize};
use self::terminal_state::{TerminalBackend, create_terminal_backend};
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
    terminal_backend: Arc<Mutex<Box<dyn TerminalBackend>>>,
    debug_handler: Arc<DebugHandler>,
    debug_recorder: Arc<Mutex<Option<String>>>,
    debug_log: Arc<Mutex<Option<std::fs::File>>>,
}

impl<T: Transport + Send + 'static> TerminalServer<T> {
    /// Create a new terminal server with a session - this is the high-level API
    pub async fn create(
        config: &crate::config::Config,
        transport: T,
        passphrase: Option<String>,
        cmd: Vec<String>,
        debug_recorder: Option<String>,
        debug_log: Option<String>,
    ) -> Result<Arc<Self>> {
        use crate::session::Session;
        
        // Create the session first (without handler)
        let mut server_session = Session::create(config, transport, passphrase, cmd.clone()).await?;
        
        // Create the terminal server
        let terminal_server = Self::new_with_debug(server_session, debug_recorder, debug_log);
        
        // Now set the handler and debug handler (breaking the circular dependency)
        {
            let mut session = terminal_server.session.write().await;
            session.set_handler(terminal_server.clone()).await;
            session.set_debug_handler(Arc::clone(&terminal_server.debug_handler));
        }
        
        // Start the message router now that all handlers are set
        {
            let session = terminal_server.session.read().await;
            session.start_router().await;
        }
        
        Ok(terminal_server)
    }

    pub fn new(session: ServerSession<T>) -> Arc<Self> {
        Self::new_with_debug_recorder(session, None)
    }

    pub fn new_with_debug_recorder(session: ServerSession<T>, debug_recorder: Option<String>) -> Arc<Self> {
        Self::new_with_debug(session, debug_recorder, None)
    }

    pub fn new_with_debug(session: ServerSession<T>, debug_recorder: Option<String>, debug_log: Option<String>) -> Arc<Self> {
        // Open debug log file if specified
        let debug_log_file = debug_log.and_then(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()
        });
        
        // Detect terminal size FIRST before creating backend
        let (term_cols, term_rows) = match self::terminal::get_terminal_size() {
            Ok((cols, rows)) => {
                if let Some(ref log_file) = debug_log_file {
                    if let Ok(log_file) = log_file.try_clone() {
                        let mut log = log_file;
                        use std::io::Write;
                        let _ = writeln!(log, "[{}] TerminalServer::new detected terminal size: {}x{}", 
                                         chrono::Utc::now().format("%H:%M:%S%.3f"), cols, rows);
                    }
                }
                (cols, rows)
            },
            Err(_) => {
                if let Some(ref log_file) = debug_log_file {
                    if let Ok(log_file) = log_file.try_clone() {
                        let mut log = log_file;
                        use std::io::Write;
                        let _ = writeln!(log, "[{}] TerminalServer::new failed to detect terminal size, using defaults", 
                                         chrono::Utc::now().format("%H:%M:%S%.3f"));
                    }
                }
                (80, 24)
            }
        };
        
        // Create terminal backend with DETECTED size, not hardcoded
        let terminal_backend = create_terminal_backend(term_cols, term_rows, debug_log_file.as_ref())
            .expect("Failed to create terminal backend");
        let terminal_backend = Arc::new(Mutex::new(terminal_backend));
        let debug_handler = Arc::new(DebugHandler::new());
        
        if let Some(ref _path) = debug_recorder {
            // Debug recorder enabled silently
        }
        
        Arc::new(TerminalServer { 
            session: Arc::new(AsyncRwLock::new(session)),
            pty_manager: PtyManager::new(),
            read_task: Arc::new(Mutex::new(None)),
            stdin_task: Arc::new(Mutex::new(None)),
            terminal_backend,
            debug_handler,
            debug_recorder: Arc::new(Mutex::new(debug_recorder)),
            debug_log: Arc::new(Mutex::new(debug_log_file)),
        })
    }

    pub async fn set_session(&self, mut session: ServerSession<T>) {
        // Store the session and set debug handler
        session.set_debug_handler(Arc::clone(&self.debug_handler));
        *self.session.write().await = session;
    }
}

#[async_trait]
impl<T: Transport + Send + 'static> Server for TerminalServer<T> {
    type Transport = T;

    async fn start(&self) {
        // Get terminal size using improved detection
        let pty_size = get_pty_size();
        
        // Log the detected PTY size and method
        if let Some(debug_log) = self.debug_log.lock().unwrap().as_mut() {
            use std::io::Write;
            let _ = writeln!(debug_log, "[{}] Server::start detected terminal size: {}x{}", 
                             chrono::Utc::now().format("%H:%M:%S%.3f"), 
                             pty_size.cols, pty_size.rows);
            
            // Also log which detection method worked
            if let Ok((cols, rows)) = crossterm::terminal::size() {
                let _ = writeln!(debug_log, "[{}] Detection method: crossterm ({}x{})", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"), cols, rows);
            } else {
                let _ = writeln!(debug_log, "[{}] Detection method: fallback (crossterm failed)", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"));
            }
        }

        // Build command from session config
        let cmd = {
            let session = self.session.read().await;
            let cmd_vec = session.cmd();
            
            // Log the command being executed
            if let Some(debug_log) = self.debug_log.lock().unwrap().as_mut() {
                use std::io::Write;
                let _ = writeln!(debug_log, "[{}] Server::start executing command: {:?}", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"), cmd_vec);
            }
            
            build_command(cmd_vec)
        };
       
        // Initialize PTY
        self.pty_manager.init(pty_size, cmd)
            .expect("Failed to initialize PTY");
        
        // Resize terminal backend to match current PTY size (in case it changed)
        {
            let mut backend = self.terminal_backend.lock().unwrap();
            let (current_width, current_height) = backend.get_dimensions();
            
            // Only resize if dimensions differ
            if current_width != pty_size.cols || current_height != pty_size.rows {
                // Log the resize attempt
                if let Some(debug_log) = self.debug_log.lock().unwrap().as_mut() {
                    use std::io::Write;
                    let _ = writeln!(debug_log, "[{}] Server::start resizing backend from {}x{} to {}x{}", 
                                     chrono::Utc::now().format("%H:%M:%S%.3f"), 
                                     current_width, current_height,
                                     pty_size.cols, pty_size.rows);
                }
                
                let _ = backend.resize(pty_size.cols, pty_size.rows);
            } else {
                if let Some(debug_log) = self.debug_log.lock().unwrap().as_mut() {
                    use std::io::Write;
                    let _ = writeln!(debug_log, "[{}] Server::start backend already at correct size {}x{}", 
                                     chrono::Utc::now().format("%H:%M:%S%.3f"), 
                                     pty_size.cols, pty_size.rows);
                }
            }
        }
        
        // Initialize debug handler with the backend
        self.debug_handler.set_backend(Arc::clone(&self.terminal_backend));
        
        // Server is ready, no need for verbose messages
        
        // Enable raw mode for proper terminal interaction
        let _ = enable_raw_mode();
        
        // Start reading from PTY in a background task with terminal state tracking and resize support
        let debug_recorder_path = self.debug_recorder.lock().unwrap().clone();
        let read_task = spawn_pty_reader_with_resize(
            self.pty_manager.master_reader.clone(),
            self.terminal_backend.clone(),
            Arc::new(self.pty_manager.clone()),
            debug_recorder_path
        );
        *self.read_task.lock().unwrap() = Some(read_task);
        
        // Start reading from stdin and writing to PTY (using spawn_blocking for blocking I/O)
        let stdin_recorder_path = self.debug_recorder.lock().unwrap().clone()
            .map(|p| p.replace(".log", ".stdin.log"));
        let stdin_task = spawn_stdin_reader(
            self.pty_manager.master_writer.clone(), 
            stdin_recorder_path
        );
        
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
        
        // Shutting down silently
        
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