mod pty;
mod terminal;
mod io;
pub mod terminal_state;
pub mod debug_handler;
mod pty_writer_impl;

use async_trait::async_trait;
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
use self::terminal_state::{TerminalBackend, TerminalStateTracker, create_terminal_backend};
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
    debug_recorder: Arc<Mutex<Option<Arc<Mutex<crate::debug_recorder::DebugRecorder>>>>>,
    debug_recorder_path: Arc<Mutex<Option<String>>>,
    debug_log: Arc<Mutex<Option<std::fs::File>>>,
    delta_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Sender<crate::server::terminal_state::GridDelta>>>>,
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
        let server_session = Session::create(config, transport, passphrase, cmd.clone()).await?;
        
        // Clone debug_log path before moving to terminal server
        let debug_log_path = debug_log.clone();
        
        // Create the terminal server
        let terminal_server = Self::new_with_debug(server_session, debug_recorder, debug_log);
        
        // Create a TerminalStateTracker that uses the live terminal backend
        let terminal_tracker = Arc::new(std::sync::Mutex::new(
            TerminalStateTracker::from_backend(terminal_server.terminal_backend.clone())
        ));
        
        // Get the subscription hub from the session
        let subscription_hub = {
            let session = terminal_server.session.read().await;
            session.subscription_hub()
        };
        
        // Create and attach the terminal data source
        use crate::server::terminal_state::TrackerDataSource;
        let (mut data_source, delta_tx) = TrackerDataSource::new(
            terminal_tracker.clone(),
            terminal_server.terminal_backend.clone(),
        );
        
        // Set debug recorder if available
        if let Some(recorder) = terminal_server.debug_recorder.lock().unwrap().as_ref() {
            data_source.set_debug_recorder(Some(recorder.clone()));
        }
        
        // Log data source creation
        if let Some(debug_log) = &mut *terminal_server.debug_log.lock().unwrap() {
            use std::io::Write;
            let _ = writeln!(debug_log, "[{}] Server: Created TrackerDataSource with delta channel", 
                chrono::Local::now().format("%H:%M:%S%.3f"));
        }
        
        subscription_hub.attach_source(Arc::new(data_source)).await;
        
        // Set debug log path if provided
        if let Some(ref path) = debug_log_path {
            subscription_hub.set_debug_log_path(path.clone()).await;
        }
        
        // Set debug recorder if available
        if let Some(recorder) = terminal_server.debug_recorder.lock().unwrap().as_ref() {
            subscription_hub.set_debug_recorder(recorder.clone()).await;
        }
        
        // Store delta_tx for later use in spawn_pty_reader_with_resize
        *terminal_server.delta_tx.lock().await = Some(delta_tx);

        // Wire PTY writer so input from clients is forwarded to the shell
        {
            use crate::server::pty_writer_impl::PtyWriterFromManager;
            subscription_hub.set_pty_writer(Arc::new(PtyWriterFromManager::new_with_debug(
                terminal_server.pty_manager.clone(),
                debug_log_path.clone()
            ))).await;
        }
        
        // Set the terminal server as the message handler directly
        {
            let mut session = terminal_server.session.write().await;
            session.set_handler(terminal_server.clone() as Arc<dyn ServerMessageHandler>).await;
            session.set_debug_handler(Arc::clone(&terminal_server.debug_handler));
        }
        
        // Start the subscription hub's event streaming
        let hub_clone = subscription_hub.clone();
        
        // Log streaming task start
        if let Some(debug_log) = &mut *terminal_server.debug_log.lock().unwrap() {
            use std::io::Write;
            let _ = writeln!(debug_log, "[{}] Server: Starting subscription hub streaming task", 
                chrono::Local::now().format("%H:%M:%S%.3f"));
        }
        
        let _streaming_handle = hub_clone.start_streaming();
        
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
        
        // Create debug recorder if path provided (don't panic on failure)
        let debug_recorder_arc: Option<Arc<Mutex<crate::debug_recorder::DebugRecorder>>> =
            if let Some(path) = debug_recorder.as_ref() {
                match crate::debug_recorder::DebugRecorder::new(path) {
                    Ok(recorder) => Some(Arc::new(Mutex::new(recorder))),
                    Err(e) => {
                        // Log error and continue without debug recorder
                        if let Some(ref log_file) = debug_log_file {
                            if let Ok(log_file) = log_file.try_clone() {
                                let mut log = log_file;
                                use std::io::Write;
                                let _ = writeln!(log, "[{}] Failed to create debug recorder: {}", 
                                                 chrono::Utc::now().format("%H:%M:%S%.3f"), e);
                            }
                        }
                        None
                    }
                }
            } else {
                None
            };
        
        // Create terminal backend with DETECTED size, not hardcoded
        let terminal_backend = create_terminal_backend(
            term_cols, 
            term_rows, 
            debug_log_file.as_ref(),
            debug_recorder_arc.clone()
        ).expect("Failed to create terminal backend");
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
            debug_recorder: Arc::new(Mutex::new(debug_recorder_arc)),
            debug_recorder_path: Arc::new(Mutex::new(debug_recorder.clone())),
            debug_log: Arc::new(Mutex::new(debug_log_file)),
            delta_tx: Arc::new(tokio::sync::Mutex::new(None)),
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
        
        // PtyWriter functionality is now handled directly through pty_manager
        // The subscription hub will call pty_manager.write() and pty_manager.resize()
        // when needed. For now, we'll wire this up later when we refactor
        // the message handling to use the subscription hub properly.
        
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
        let debug_recorder = self.debug_recorder.lock().unwrap().clone();
        let delta_tx_clone = self.delta_tx.lock().await.clone();
        let read_task = spawn_pty_reader_with_resize(
            self.pty_manager.master_reader.clone(),
            self.terminal_backend.clone(),
            Arc::new(self.pty_manager.clone()),
            debug_recorder,
            delta_tx_clone
        );
        *self.read_task.lock().unwrap() = Some(read_task);
        
        // Start reading from stdin and writing to PTY (using spawn_blocking for blocking I/O)
        let stdin_recorder_path = self.debug_recorder_path.lock().unwrap().clone()
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

    /// Wait for connections based on flags
    async fn wait_for_connections(&self, wait_for_webrtc: bool) {
        loop {
            let session = self.session.read().await;
            
            if wait_for_webrtc {
                if session.has_any_webrtc_connected().await {
                    if let Some(ref mut debug_log) = *self.debug_log.lock().unwrap() {
                        use std::io::Write;
                        let _ = writeln!(debug_log, "[{}] Server: WebRTC connection established", 
                                         chrono::Utc::now().format("%H:%M:%S%.3f"));
                    }
                    break;
                }
            } else {
                if session.has_clients().await {
                    if let Some(ref mut debug_log) = *self.debug_log.lock().unwrap() {
                        use std::io::Write;
                        let _ = writeln!(debug_log, "[{}] Server: Client connected", 
                                         chrono::Utc::now().format("%H:%M:%S%.3f"));
                    }
                    break;
                }
            }
            
            drop(session); // Release lock before sleeping
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Start server with optional wait for clients
    pub async fn start_with_wait(&self, wait_for_client: bool, wait_for_webrtc: bool, keep_alive: bool) {
        // Log entry to start_with_wait
        if let Some(ref mut debug_log) = *self.debug_log.lock().unwrap() {
            use std::io::Write;
            let _ = writeln!(debug_log, "[{}] Server: start_with_wait called - wait_for_client={}, wait_for_webrtc={}, keep_alive={}", 
                             chrono::Utc::now().format("%H:%M:%S%.3f"), wait_for_client, wait_for_webrtc, keep_alive);
        }
        
        if wait_for_client || wait_for_webrtc {
            self.wait_for_connections(wait_for_webrtc).await;
        }
        
        // Log before calling start
        if let Some(ref mut debug_log) = *self.debug_log.lock().unwrap() {
            use std::io::Write;
            let _ = writeln!(debug_log, "[{}] Server: About to call start()", 
                             chrono::Utc::now().format("%H:%M:%S%.3f"));
        }
        
        // Now start the server normally
        self.start().await;
        
        // If keep_alive is set, stay running after command exits
        if keep_alive {
            // Disable raw mode before entering keep-alive loop
            let _ = disable_raw_mode();
            
            if let Some(ref mut debug_log) = *self.debug_log.lock().unwrap() {
                use std::io::Write;
                let _ = writeln!(debug_log, "[{}] Server: Entering keep-alive mode", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"));
            }
            
            // Keep the server alive
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

// Implement ServerMessageHandler to handle WebSocket messages
#[async_trait]
impl<T: Transport + Send + Sync + 'static> ServerMessageHandler for TerminalServer<T> {
    async fn handle_client_message(&self, from_peer: &str, message: AppMessage) {
        match message {
            AppMessage::Protocol { message: msg_value } => {
                // Parse and handle protocol messages through subscription hub
                if let Ok(client_msg) = serde_json::from_value::<crate::protocol::ClientMessage>(msg_value) {
                    // Special-case Subscribe to bind to the client's WebRTC transport and use provided ID
                    use crate::protocol::ClientMessage as CMsg;
                    match client_msg {
                        CMsg::Subscribe { subscription_id, dimensions, mode, position, .. } => {
                            // Debug log subscribe reception
                            if let Some(ref mut debug_log) = *self.debug_log.lock().unwrap() {
                                use std::io::Write;
                                let _ = writeln!(debug_log, "[{}] Server: Received Subscribe {{ id: {} }} from {}",
                                    chrono::Utc::now().format("%H:%M:%S%.3f"), subscription_id, from_peer);
                            }
                            let session = self.session.read().await;
                            let hub = session.subscription_hub();
                            // Look up the per-client WebRTC transport
                            // Wait briefly for WebRTC transport to be ready
                            let mut transport_ready = None;
                            for _ in 0..50 { // up to ~5s
                                if let Some(t) = session.get_webrtc_transport(from_peer).await {
                                    if t.is_connected() { transport_ready = Some(t); break; }
                                }
                                let _ = &session;
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            }
                            if let Some(client_transport) = transport_ready {
                                let transport_dyn = client_transport as Arc<dyn crate::transport::Transport>;
                                let config = crate::subscription::SubscriptionConfig {
                                    dimensions,
                                    mode,
                                    position,
                                    is_controlling: true,
                                };
                                drop(session);
                                let result = hub.subscribe_with_id(
                                    from_peer.to_string(),
                                    transport_dyn,
                                    subscription_id,
                                    config,
                                ).await;
                                if let Some(ref mut debug_log) = *self.debug_log.lock().unwrap() {
                                    use std::io::Write;
                                    let _ = writeln!(debug_log, "[{}] Server: subscribe_with_id result: {:?}",
                                        chrono::Utc::now().format("%H:%M:%S%.3f"), result.as_ref().map(|_| "ok").unwrap_or("err"));
                                }
                            }
                        }
                        other => {
                            let session = self.session.read().await;
                            let hub = session.subscription_hub();
                            drop(session);
                            let _ = hub.handle_incoming(&from_peer.to_string(), other).await;
                        }
                    }
                }
            }
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
        // Log client join if debug logging is enabled
        if let Some(ref mut debug_log) = *self.debug_log.lock().unwrap() {
            use std::io::Write;
            let _ = writeln!(debug_log, "[{}] Client {} joined", 
                             chrono::Utc::now().format("%H:%M:%S%.3f"), peer.id);
        }
        
        // Auto-subscribe is now handled in session/mod.rs when WebRTC connection is established
        // This ensures the subscription happens exactly when the transport is ready
    }
    
    async fn handle_client_left(&self, peer_id: &str) {
        // Remove client's subscriptions from the hub
        let session = self.session.read().await;
        let hub = session.subscription_hub();
        drop(session);
        
        let _ = hub.remove_client(&peer_id.to_string()).await;
        
        // Log client leave if debug logging is enabled
        if let Some(ref mut debug_log) = *self.debug_log.lock().unwrap() {
            use std::io::Write;
            let _ = writeln!(debug_log, "[{}] Client {} left", 
                             chrono::Utc::now().format("%H:%M:%S%.3f"), peer_id);
        }
    }
}
