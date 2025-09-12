/// Full terminal client with TUI, predictive echo, and resilience
use crate::client::{grid_renderer::GridRenderer, predictive_echo::PredictiveEcho};
use crate::debug_recorder::DebugRecorder;
use crate::protocol::{ClientMessage, ServerMessage, Dimensions, ViewMode, ViewPosition, ControlMessage};
use crate::protocol::signaling::{AppMessage, PeerInfo};
use crate::session::{ClientSession, message_handlers::ClientMessageHandler};
use crate::transport::Transport;
use anyhow::Result;
use async_trait::async_trait;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers, EnableMouseCapture, DisableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};
use std::io::stdout;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time;

/// Terminal client state
#[derive(Debug, Clone, Copy, PartialEq)]
enum ClientState {
    Connecting,
    Connected,
    Disconnected,
    Reconnecting,
}

/// Queued message for non-blocking send
#[derive(Debug)]
enum QueuedMessage {
    AppMessage(AppMessage),
    RawInput { channel: String, data: Vec<u8> },
}

/// Mouse mode for smart selection
#[derive(Debug, Clone, Copy, PartialEq)]
enum MouseMode {
    /// Normal mode - capture scroll and clicks
    Normal,
    /// Selection mode - let terminal handle selection
    Selecting,
}

/// Main terminal client
pub struct TerminalClient<T: Transport + Send + 'static> {
    /// Client session
    pub(crate) session: Arc<RwLock<ClientSession<T>>>,
    
    /// Grid renderer for display
    grid_renderer: Arc<Mutex<GridRenderer>>,
    
    /// Predictive echo tracker
    predictive_echo: Arc<Mutex<PredictiveEcho>>,
    
    /// Client state
    state: Arc<RwLock<ClientState>>,
    
    /// Subscription ID
    subscription_id: String,
    
    /// Server message sender (for handler)
    server_tx: mpsc::Sender<ServerMessage>,
    
    /// Server message receiver
    server_rx: Arc<Mutex<mpsc::Receiver<ServerMessage>>>,
    
    /// Input sender
    input_tx: mpsc::Sender<Vec<u8>>,
    
    /// Input receiver
    input_rx: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
    
    /// Non-blocking send queue
    send_queue_tx: mpsc::Sender<QueuedMessage>,
    
    /// Shutdown signal
    shutdown_tx: mpsc::Sender<()>,
    
    /// Shutdown receiver
    shutdown_rx: Arc<Mutex<mpsc::Receiver<()>>>,
    
    /// Mouse mode for smart selection
    mouse_mode: Arc<RwLock<MouseMode>>,
    
    /// Debug log file path
    debug_log: Option<String>,
    
    /// Debug recorder for subscription events
    debug_recorder: Arc<Mutex<Option<DebugRecorder>>>,

    /// Throttle: history request in-flight (start_line)
    history_pending_line: Arc<Mutex<Option<u64>>>,
    /// Throttle: last history request send time
    history_last_sent: Arc<Mutex<Option<std::time::Instant>>>,
    /// Track last requested subscription mode
    last_requested_mode: Arc<Mutex<crate::protocol::ViewMode>>,
}

impl<T: Transport + Send + Sync + 'static> TerminalClient<T> {
    /// Create a new terminal client and join an existing session
    pub async fn create(
        _config: &crate::config::Config,
        transport: T,
        session_str: &str,
        passphrase: Option<String>,
        debug_log: Option<String>,
        debug_recorder: Option<String>,
        debug_size: bool,
    ) -> Result<Arc<Self>> {
        use crate::session::Session;
        
        // Join the session
        let (client_session, server_addr, session_id) = Session::join(session_str, transport, passphrase).await?;
        
        // Create the client
        let client = Self::new(client_session, None, debug_log, debug_recorder, debug_size).await?;
        
        // Set the handler first so connect_signaling can immediately start the router
        client.set_as_handler().await;

        // Now connect signaling (router starts here because handler is already set)
        {
            let session = client.session.clone();
            if let Err(_e) = session.write().await.connect_signaling(&server_addr, &session_id).await {
                // Continue without WebSocket connection; WebRTC offer may not arrive
                // via signaling if this fails.
            }
        }
        
        Ok(client)
    }
    
    /// Set this client as the handler for its session
    async fn set_as_handler(self: &Arc<Self>) {
        let session = self.session.clone();
        // Box the Arc to get around the type system limitation
        let boxed: Box<dyn ClientMessageHandler> = Box::new(HandlerWrapper {
            client: self.clone(),
        });
        let handler = Arc::from(boxed);
        session.write().await.set_handler(handler);
    }
    
    /// Create a new terminal client
    async fn new(
        session: ClientSession<T>,
        _passphrase: Option<String>,
        debug_log: Option<String>,
        debug_recorder_path: Option<String>,
        debug_size: bool,
    ) -> Result<Arc<Self>> {
        // Get terminal dimensions
        let (width, height) = crossterm::terminal::size()?;
        
        // Create grid renderer
        let grid_renderer = Arc::new(Mutex::new(GridRenderer::new(width, height, debug_size)?));
        
        // Create predictive echo
        let client_id = format!("client-{}", uuid::Uuid::new_v4());
        let predictive_echo = Arc::new(Mutex::new(PredictiveEcho::new(client_id.clone())));
        
        // Create channels
        let (server_tx, server_rx) = mpsc::channel(100);
        let (input_tx, input_rx) = mpsc::channel(100);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let (send_queue_tx, mut send_queue_rx) = mpsc::channel::<QueuedMessage>(100);
        
        let subscription_id = format!("sub-{}", uuid::Uuid::new_v4());
        
        // Create session Arc for the send task
        let session_arc = Arc::new(RwLock::new(session));
        let session_for_send = session_arc.clone();
        let debug_log_for_send = debug_log.clone();
        
        // Spawn non-blocking send task
        tokio::spawn(async move {
            while let Some(msg) = send_queue_rx.recv().await {
                let send_start = std::time::Instant::now();
                
                // Debug log: Processing queued message
                if let Some(ref debug_log) = debug_log_for_send {
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                        use std::io::Write;
                        let _ = writeln!(file, "[{}] SendQueue: Processing message from queue",
                            chrono::Utc::now().format("%H:%M:%S%.3f"));
                    }
                }
                
                let result = match msg {
                    QueuedMessage::AppMessage(app_msg) => {
                        session_for_send.write().await.send_to_server(app_msg).await
                    },
                    QueuedMessage::RawInput { channel: _channel, data } => {
                        // Send raw bytes directly to Input channel
                        let session = session_for_send.read().await;
                        let transport_arc = session.transport();
                        let transport = transport_arc.lock().await;
                        
                        // Try to get Input channel
                        match transport.channel(crate::transport::ChannelPurpose::Input).await {
                            Ok(input_channel) => {
                                input_channel.send(&data).await
                            },
                            Err(_) => {
                                // Fall back to Control channel with JSON wrapping
                                // Extract the input bytes (skip the 0x01 message type byte)
                                if data.len() > 1 && data[0] == 0x01 {
                                    let input_bytes = &data[1..];
                                    let client_msg = crate::protocol::ClientMessage::TerminalInput {
                                        data: input_bytes.to_vec(),
                                        echo_local: None,
                                    };
                                    let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                                        message: match serde_json::to_value(&client_msg) {
                                            Ok(v) => v,
                                            Err(e) => {
                                                eprintln!("Failed to serialize message: {}", e);
                                                continue;
                                            }
                                        }
                                    };
                                    drop(transport);
                                    drop(session);
                                    session_for_send.write().await.send_to_server(app_msg).await
                                } else {
                                    Err(anyhow::anyhow!("Invalid raw input message format"))
                                }
                            }
                        }
                    }
                };
                
                let elapsed = send_start.elapsed();
                
                // Debug log: Send complete with timing
                if let Some(ref debug_log) = debug_log_for_send {
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                        use std::io::Write;
                        match result {
                            Ok(_) => {
                                let _ = writeln!(file, "[{}] SendQueue: Message sent successfully in {}ms",
                                    chrono::Utc::now().format("%H:%M:%S%.3f"), elapsed.as_millis());
                            },
                            Err(e) => {
                                let _ = writeln!(file, "[{}] SendQueue: Send failed after {}ms: {:?}",
                                    chrono::Utc::now().format("%H:%M:%S%.3f"), elapsed.as_millis(), e);
                            }
                        }
                    }
                }
            }
        });
        
        // Initialize debug recorder if path provided
        let debug_recorder = if let Some(path) = debug_recorder_path {
            match DebugRecorder::new(&path) {
                Ok(recorder) => {
                    eprintln!("Debug recorder initialized: {}", path);
                    Some(recorder)
                },
                Err(e) => {
                    eprintln!("Failed to initialize debug recorder: {}", e);
                    None
                }
            }
        } else {
            None
        };
        
        let client = Arc::new(Self {
            session: session_arc,
            grid_renderer,
            predictive_echo,
            state: Arc::new(RwLock::new(ClientState::Connecting)),
            subscription_id,
            server_tx: server_tx.clone(),
            server_rx: Arc::new(Mutex::new(server_rx)),
            input_tx,
            input_rx: Arc::new(Mutex::new(input_rx)),
            send_queue_tx,
            shutdown_tx,
            shutdown_rx: Arc::new(Mutex::new(shutdown_rx)),
            mouse_mode: Arc::new(RwLock::new(MouseMode::Normal)),
            debug_log,
            debug_recorder: Arc::new(Mutex::new(debug_recorder)),
            history_pending_line: Arc::new(Mutex::new(None)),
            history_last_sent: Arc::new(Mutex::new(None)),
            last_requested_mode: Arc::new(Mutex::new(ViewMode::Realtime)),
        });
        
        Ok(client)
    }
    
    /// Show passphrase interstitial if needed
    pub async fn prompt_passphrase() -> Result<String> {
        // Check environment variable first
        if let Ok(passphrase) = std::env::var("BEACH_PASSPHRASE") {
            return Ok(passphrase);
        }
        
        // Show interstitial prompt
        enable_raw_mode()?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        terminal.clear()?;
        
        let mut passphrase = String::new();
        let mut show_cursor = true;
        let mut blink_interval = time::interval(Duration::from_millis(500));
        
        // Create a channel for events
        let (event_tx, mut event_rx) = mpsc::channel(100);
        
        // Spawn a task to read events
        tokio::spawn(async move {
            loop {
                // Read ALL available events before sleeping to avoid dropping keystrokes
                while event::poll(Duration::from_millis(0)).unwrap_or(false) {
                    if let Ok(evt) = event::read() {
                        if event_tx.send(evt).await.is_err() {
                            return; // Channel closed
                        }
                    }
                }
                // Only sleep briefly when no more events are available (1ms for responsiveness)
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        
        let result = loop {
            tokio::select! {
                // Handle keyboard input
                Some(evt) = event_rx.recv() => {
                    if let Event::Key(key) = evt {
                        match key.code {
                            KeyCode::Enter => {
                                if !passphrase.is_empty() {
                                    break Ok(passphrase);
                                }
                            }
                            KeyCode::Char(c) => {
                                passphrase.push(c);
                            }
                            KeyCode::Backspace => {
                                passphrase.pop();
                            }
                            KeyCode::Esc => {
                                break Err(anyhow::anyhow!("Passphrase prompt cancelled"));
                            }
                            _ => {}
                        }
                    }
                }
                
                // Blink cursor
                _ = blink_interval.tick() => {
                    show_cursor = !show_cursor;
                }
            }
            
            // Render after any event
            terminal.draw(|f| {
                let area = f.area();
                
                // Create centered layout
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Percentage(35),
                        Constraint::Length(10),
                        Constraint::Percentage(55),
                    ])
                    .split(area);
                
                let inner = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(25),
                        Constraint::Percentage(50),
                        Constraint::Percentage(25),
                    ])
                    .split(chunks[1]);
                
                // Clear the area
                f.render_widget(Clear, inner[1]);
                
                // Create the prompt box
                let prompt_text = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "Beach Session",
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from("Enter passphrase:"),
                    Line::from(""),
                    Line::from(Span::styled(
                        if show_cursor {
                            format!("{}█", passphrase)
                        } else {
                            passphrase.clone()
                        },
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Press Enter to continue",
                        Style::default().fg(Color::Gray),
                    )),
                ];
                
                let prompt = Paragraph::new(prompt_text)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(Color::Cyan))
                            .title(" Passphrase Required ")
                            .title_alignment(Alignment::Center),
                    )
                    .alignment(Alignment::Center);
                
                f.render_widget(prompt, inner[1]);
            })?;
        };
        
        // Cleanup
        disable_raw_mode()?;
        terminal.clear()?;
        
        result
    }
    
    
    /// Start the client
    pub async fn start(self: Arc<Self>) -> Result<()> {
        // Debug: mark start entry
        if let Some(ref debug_log) = self.debug_log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                use std::io::Write;
                let _ = writeln!(file, "[{}] Client: start() invoked", chrono::Utc::now().format("%H:%M:%S%.3f"));
            }
        }
        // Get the receivers from our stored channels first
        let mut server_rx = self.server_rx.lock().await;
        let mut shutdown_rx = self.shutdown_rx.lock().await;
        
        // Connect and subscribe BEFORE setting up terminal (to avoid terminal issues)
        self.connect_and_subscribe(&mut *server_rx).await?;
        
        // Setup terminal after connection is established
        if let Some(ref debug_log) = self.debug_log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                use std::io::Write;
                let _ = writeln!(file, "[{}] Client: About to enable raw mode and enter terminal UI", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"));
            }
        }
        
        enable_raw_mode()?;
        let mut stdout = stdout();
        // Note: EnableMouseCapture prevents native text selection. 
        // Most terminals allow Shift+drag to bypass mouse capture for selection.
        // TODO: Consider making mouse capture optional via a flag
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        
        if let Some(ref debug_log) = self.debug_log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                use std::io::Write;
                let _ = writeln!(file, "[{}] Client: Terminal UI initialized, starting event loops", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"));
            }
        }
        
        // Start event loops
        let result = self.run_event_loops(&mut terminal, &mut *server_rx, &mut *shutdown_rx).await;
        
        if let Some(ref debug_log) = self.debug_log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                use std::io::Write;
                let _ = writeln!(file, "[{}] Client: Event loops exited with result: {:?}", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"), result);
            }
        }
        
        // Ensure we're not in selection mode when cleaning up
        {
            let mut mode = self.mouse_mode.write().await;
            *mode = MouseMode::Normal;
        }
        
        // Cleanup terminal
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
        terminal.show_cursor()?;
        
        result
    }
    
    /// Connect to server and establish subscription
    async fn connect_and_subscribe(&self, server_rx: &mut mpsc::Receiver<ServerMessage>) -> Result<()> {
        *self.state.write().await = ClientState::Connecting;

        // Debug log entry
        if let Some(ref debug_log) = self.debug_log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                use std::io::Write;
                let _ = writeln!(file, "[{}] Client: connect_and_subscribe() started", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"));
            }
        }

        // In strict WebRTC mode, wait until the data channel is actually connected
        if true /* strict WebRTC is now default */ {
            let start = std::time::Instant::now();
            let timeout = std::time::Duration::from_secs(30);
            
            // Debug log waiting for WebRTC
            if let Some(ref debug_log) = self.debug_log {
                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                    use std::io::Write;
                    let _ = writeln!(file, "[{}] Client: Waiting for WebRTC initialization...", 
                                     chrono::Utc::now().format("%H:%M:%S%.3f"));
                }
            }
            
            // Give a brief moment after peer connected signals
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            
            loop {
                // Check if transport is WebRTC and connected (data channel open)
                let webrtc_ready = {
                    let session_guard = self.session.read().await;
                    let transport = session_guard.session().transport();
                    let guard = transport.lock().await;
                    let is_webrtc = guard.is_webrtc();
                    let is_connected = guard.is_connected();
                    
                    // Debug log connection state
                    if let Some(ref debug_log) = self.debug_log {
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                            use std::io::Write;
                            let _ = writeln!(file, "[{}] Client: WebRTC check - is_webrtc: {}, is_connected: {}", 
                                             chrono::Utc::now().format("%H:%M:%S%.3f"), is_webrtc, is_connected);
                        }
                    }
                    
                    is_webrtc && is_connected
                };
                if webrtc_ready { 
                    // Debug log WebRTC ready
                    if let Some(ref debug_log) = self.debug_log {
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                            use std::io::Write;
                            let _ = writeln!(file, "[{}] Client: WebRTC is ready, proceeding with subscription!", 
                                             chrono::Utc::now().format("%H:%M:%S%.3f"));
                        }
                    }
                    // Small grace period to ensure data channel handlers are fully active
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    break;
                }
                if start.elapsed() > timeout {
                    return Err(anyhow::anyhow!("Timed out waiting for WebRTC initialization"));
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
        
        // Get dimensions and request 2x height for overscan
        let (width, height) = crossterm::terminal::size()?;
        let overscan_height = height * 2;  // Request 2x visible height for smooth scrolling
        let dimensions = Dimensions { width, height: overscan_height };
        
        // Send subscription request with overscan
        let subscribe_msg = ClientMessage::Subscribe {
            subscription_id: self.subscription_id.clone(),
            dimensions,
            mode: ViewMode::Realtime,
            position: None,  // Will be updated when scrolling
            compression: None,
        };
        
        // Record client message being sent
        if let Some(ref mut recorder) = *self.debug_recorder.lock().await {
            let _ = recorder.record_client_message(&subscribe_msg);
        }
        
        let app_msg = AppMessage::Protocol {
            message: serde_json::to_value(&subscribe_msg)?,
        };
        
        // Debug log the subscribe message being sent
        if let Some(ref debug_log) = self.debug_log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                use std::io::Write;
                let _ = writeln!(file, "[{}] Client: About to send Subscribe message", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"));
                let _ = writeln!(file, "[{}] Client sending Subscribe message: {:?}", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"), subscribe_msg);
                let _ = writeln!(file, "[{}] Client sending AppMessage: {:?}", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"), app_msg);
            }
        }
        
        // Actually send the message
        if let Some(ref debug_log) = self.debug_log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                use std::io::Write;
                let _ = writeln!(file, "[{}] Client: Calling send_to_server()...", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"));
            }
        }
        
        self.session.write().await.send_to_server(app_msg).await?;
        
        // Debug log after sending
        if let Some(ref debug_log) = self.debug_log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                use std::io::Write;
                let _ = writeln!(file, "[{}] Client: Subscribe message sent successfully!", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"));
            }
        }
        
        // Get timeout from environment or use default
        let sub_ack_timeout_ms = std::env::var("BEACH_SUB_ACK_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(3000);
        let snapshot_timeout_ms = std::env::var("BEACH_SNAPSHOT_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(3000);
        
        // Wait for acknowledgment with timeout
        let ack_result = tokio::time::timeout(
            Duration::from_millis(sub_ack_timeout_ms),
            server_rx.recv()
        ).await;
        
        match ack_result {
            Ok(Some(ServerMessage::SubscriptionAck { .. })) => {
                *self.state.write().await = ClientState::Connected;
                
                // Debug log successful ack
                if let Some(ref debug_log) = self.debug_log {
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                        use std::io::Write;
                        let _ = writeln!(file, "[{}] Client received SubscriptionAck", 
                                         chrono::Utc::now().format("%H:%M:%S%.3f"));
                    }
                }
            }
            Ok(Some(msg)) => {
                return Err(anyhow::anyhow!("Unexpected response to Subscribe: {:?}", msg));
            }
            Ok(None) => {
                return Err(anyhow::anyhow!("Server connection closed while waiting for SubscriptionAck"));
            }
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Timeout waiting for SubscriptionAck after {}ms. Check server connection and network.",
                    sub_ack_timeout_ms
                ));
            }
        }
        
        // Wait for initial snapshot with timeout
        let snapshot_result = tokio::time::timeout(
            Duration::from_millis(snapshot_timeout_ms),
            server_rx.recv()
        ).await;
        
        match snapshot_result {
            Ok(Some(ServerMessage::Snapshot { grid, .. })) => {
                self.grid_renderer.lock().await.apply_snapshot(grid);
                
                // Debug log successful snapshot
                if let Some(ref debug_log) = self.debug_log {
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                        use std::io::Write;
                        let _ = writeln!(file, "[{}] Client received initial Snapshot", 
                                         chrono::Utc::now().format("%H:%M:%S%.3f"));
                    }
                }
            }
            Ok(Some(msg)) => {
                return Err(anyhow::anyhow!("Expected Snapshot, got: {:?}", msg));
            }
            Ok(None) => {
                return Err(anyhow::anyhow!("Server connection closed while waiting for Snapshot"));
            }
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Timeout waiting for initial Snapshot after {}ms. Server may be unresponsive.",
                    snapshot_timeout_ms
                ));
            }
        }
        
        Ok(())
    }
    
    /// Run the main event loops
    async fn run_event_loops<B: ratatui::backend::Backend>(
        &self,
        terminal: &mut Terminal<B>,
        server_rx: &mut mpsc::Receiver<ServerMessage>,
        shutdown_rx: &mut mpsc::Receiver<()>,
    ) -> Result<()> {
        // Debug log entry
        if let Some(ref debug_log) = self.debug_log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                use std::io::Write;
                let _ = writeln!(file, "[{}] Client: Entered run_event_loops", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"));
            }
        }
        
        let mut render_interval = time::interval(Duration::from_millis(16));
        
        // Create a channel for events
        let (event_tx, mut event_rx) = mpsc::channel(100);
        
        // Spawn a task to read keyboard events
        tokio::spawn(async move {
            loop {
                // Read ALL available events before sleeping to avoid dropping keystrokes
                while event::poll(Duration::from_millis(0)).unwrap_or(false) {
                    if let Ok(evt) = event::read() {
                        if event_tx.send(evt).await.is_err() {
                            return; // Channel closed
                        }
                    }
                }
                // Only sleep briefly when no more events are available (1ms for responsiveness)
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        
        let mut loop_count = 0;
        loop {
            if loop_count == 0 {
                // Debug log first iteration
                if let Some(ref debug_log) = self.debug_log {
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                        use std::io::Write;
                        let _ = writeln!(file, "[{}] Client: First iteration of event loop", 
                                         chrono::Utc::now().format("%H:%M:%S%.3f"));
                    }
                }
            }
            loop_count += 1;
            tokio::select! {
                // Handle keyboard and mouse events
                Some(evt) = event_rx.recv() => {
                    match evt {
                        Event::Key(key) => {
                            self.handle_key_event(key).await?;
                        }
                        Event::Mouse(mouse) => {
                            self.handle_mouse_event(mouse).await?;
                        }
                        _ => {} // Ignore other events like Resize
                    }
                }
                
                // Handle server messages
                Some(msg) = server_rx.recv() => {
                    self.handle_server_message(msg).await?;
                }
                
                // Handle shutdown
                _ = shutdown_rx.recv() => {
                    break;
                }
                
                // Render at regular intervals
                _ = render_interval.tick() => {
                    self.render(terminal).await?;
                }
            }
        }
        
        Ok(())
    }
    
    /// Handle mouse events (scrolling, clicking)
    async fn handle_mouse_event(&self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        use crossterm::event::{MouseEventKind, MouseButton};
        
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                // Scroll viewport up (show earlier content)
                self.grid_renderer.lock().await.scroll_vertical(3);
                
                // Record debug event
                if let Some(ref mut recorder) = *self.debug_recorder.lock().await {
                    let renderer = self.grid_renderer.lock().await;
                    let _ = recorder.record_event(crate::debug_recorder::DebugEvent::ClientScrollEvent {
                        timestamp: chrono::Utc::now(),
                        direction: "up".to_string(),
                        scroll_offset: renderer.scroll_offset as usize,
                        view_line: if renderer.is_historical_mode() { Some(0) } else { None },
                    });
                }
                
                // Trigger history update if needed
                self.handle_scroll_update().await?;
            }
            MouseEventKind::ScrollDown => {
                // Scroll viewport down (show later content)
                self.grid_renderer.lock().await.scroll_vertical(-3);
                
                // Record debug event
                if let Some(ref mut recorder) = *self.debug_recorder.lock().await {
                    let renderer = self.grid_renderer.lock().await;
                    let _ = recorder.record_event(crate::debug_recorder::DebugEvent::ClientScrollEvent {
                        timestamp: chrono::Utc::now(),
                        direction: "down".to_string(),
                        scroll_offset: renderer.scroll_offset as usize,
                        view_line: if renderer.is_historical_mode() { Some(0) } else { None },
                    });
                }
                
                // Trigger history update if needed
                self.handle_scroll_update().await?;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Start selection mode - disable mouse capture to let terminal handle selection
                let mut mode = self.mouse_mode.write().await;
                if *mode == MouseMode::Normal {
                    *mode = MouseMode::Selecting;
                    drop(mode); // Release lock before I/O
                    
                    // Disable mouse capture to allow native selection
                    use std::io::stdout;
                    let mut stdout = stdout();
                    execute!(stdout, DisableMouseCapture)?;
                    
                    // Debug log
                    if let Some(ref debug_log) = self.debug_log {
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                            use std::io::Write;
                            let _ = writeln!(file, "[{}] Entering selection mode", 
                                chrono::Utc::now().format("%H:%M:%S%.3f"));
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // End selection mode - re-enable mouse capture
                let mut mode = self.mouse_mode.write().await;
                if *mode == MouseMode::Selecting {
                    *mode = MouseMode::Normal;
                    drop(mode); // Release lock before I/O
                    
                    // Re-enable mouse capture for scrolling
                    use std::io::stdout;
                    let mut stdout = stdout();
                    execute!(stdout, EnableMouseCapture)?;
                    
                    // Debug log
                    if let Some(ref debug_log) = self.debug_log {
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                            use std::io::Write;
                            let _ = writeln!(file, "[{}] Exiting selection mode", 
                                chrono::Utc::now().format("%H:%M:%S%.3f"));
                        }
                    }
                }
            }
            _ => {
                // Ignore other mouse events for now
            }
        }
        
        Ok(())
    }
    
    /// Handle keyboard input
    async fn handle_key_event(&self, key: KeyEvent) -> Result<()> {
        // First check for control key combinations
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if let KeyCode::Char(c) = key.code {
                // Special handling for Ctrl+C to exit
                if c == 'c' || c == 'C' {
                    self.shutdown_tx.send(()).await?;
                    return Ok(());
                }
                
                // Convert control character combinations
                let bytes = match c {
                    'a' | 'A' => vec![0x01], // Ctrl+A (Start of heading)
                    'b' | 'B' => vec![0x02], // Ctrl+B (Start of text)
                    'd' | 'D' => vec![0x04], // Ctrl+D (End of transmission)
                    'e' | 'E' => vec![0x05], // Ctrl+E (Enquiry)
                    'f' | 'F' => vec![0x06], // Ctrl+F (Acknowledge)
                    'g' | 'G' => vec![0x07], // Ctrl+G (Bell)
                    'h' | 'H' => vec![0x08], // Ctrl+H (Backspace)
                    'i' | 'I' => vec![0x09], // Ctrl+I (Tab)
                    'j' | 'J' => vec![0x0A], // Ctrl+J (Line feed)
                    'k' | 'K' => vec![0x0B], // Ctrl+K (Vertical tab)
                    'l' | 'L' => vec![0x0C], // Ctrl+L (Form feed)
                    'm' | 'M' => vec![0x0D], // Ctrl+M (Carriage return)
                    'n' | 'N' => vec![0x0E], // Ctrl+N (Shift out)
                    'o' | 'O' => vec![0x0F], // Ctrl+O (Shift in)
                    'p' | 'P' => vec![0x10], // Ctrl+P (Data link escape)
                    'q' | 'Q' => vec![0x11], // Ctrl+Q (Device control 1)
                    'r' | 'R' => vec![0x12], // Ctrl+R (Device control 2)
                    's' | 'S' => vec![0x13], // Ctrl+S (Device control 3)
                    't' | 'T' => vec![0x14], // Ctrl+T (Device control 4)
                    'u' | 'U' => vec![0x15], // Ctrl+U (Negative acknowledge)
                    'v' | 'V' => vec![0x16], // Ctrl+V (Synchronous idle)
                    'w' | 'W' => vec![0x17], // Ctrl+W (End of transmission block)
                    'x' | 'X' => vec![0x18], // Ctrl+X (Cancel)
                    'y' | 'Y' => vec![0x19], // Ctrl+Y (End of medium)
                    'z' | 'Z' => vec![0x1A], // Ctrl+Z (Substitute)
                    '[' => vec![0x1B],       // Ctrl+[ (Escape)
                    '\\' => vec![0x1C],      // Ctrl+\ (File separator)
                    ']' => vec![0x1D],       // Ctrl+] (Group separator)
                    '^' => vec![0x1E],       // Ctrl+^ (Record separator)
                    '_' => vec![0x1F],       // Ctrl+_ (Unit separator)
                    _ => return Ok(()),      // Ignore other control combinations
                };
                
                // Control sequences go directly to server without predictive echo
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
                return Ok(());
            }
        }
        
        // Handle regular keys without control modifier
        match key.code {
            KeyCode::Char(c) => {
                // Regular character input - send directly without predictive echo for now
                // Predictive echo is causing issues with position tracking
                let bytes = vec![c as u8];
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            KeyCode::Enter => {
                // Handle Enter key - send directly without predictive echo
                let bytes = vec![b'\r']; // Carriage return
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            KeyCode::Tab => {
                // Handle Tab key - send directly without predictive echo
                let bytes = vec![b'\t'];
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            KeyCode::Backspace => {
                // Handle Backspace key - send without predictive echo for immediate response
                let bytes = vec![0x7F]; // DEL character
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            KeyCode::Esc => {
                // Check if we're in selection mode and cancel it
                let mode = self.mouse_mode.read().await;
                if *mode == MouseMode::Selecting {
                    drop(mode);
                    let mut mode = self.mouse_mode.write().await;
                    *mode = MouseMode::Normal;
                    drop(mode);
                    
                    // Re-enable mouse capture
                    use std::io::stdout;
                    let mut stdout = stdout();
                    execute!(stdout, EnableMouseCapture)?;
                    
                    // Debug log
                    if let Some(ref debug_log) = self.debug_log {
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                            use std::io::Write;
                            let _ = writeln!(file, "[{}] Cancelled selection mode with Escape", 
                                chrono::Utc::now().format("%H:%M:%S%.3f"));
                        }
                    }
                } else {
                    // Handle Escape key - send without predictive echo
                    let bytes = vec![0x1B]; // ESC character
                    let client_msg = crate::protocol::ClientMessage::TerminalInput {
                        data: bytes,
                        echo_local: None,
                    };
                    let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                        message: serde_json::to_value(&client_msg)?,
                    };
                    self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
                }
            }
            KeyCode::Up => {
                // Send arrow key escape sequence to server
                let bytes = vec![0x1B, b'[', b'A']; // ESC[A
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            KeyCode::Down => {
                // Send arrow key escape sequence to server
                let bytes = vec![0x1B, b'[', b'B']; // ESC[B
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            KeyCode::Left => {
                // Send arrow key escape sequence to server
                let bytes = vec![0x1B, b'[', b'D']; // ESC[D
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            KeyCode::Right => {
                // Send arrow key escape sequence to server
                let bytes = vec![0x1B, b'[', b'C']; // ESC[C
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            KeyCode::PageUp => {
                // Check if Shift is held for scrolling
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.grid_renderer.lock().await.scroll_vertical(10);
                    self.handle_scroll_update().await?;
                } else {
                    // Send Page Up escape sequence to server
                    let bytes = vec![0x1B, b'[', b'5', b'~']; // ESC[5~
                    let client_msg = crate::protocol::ClientMessage::TerminalInput {
                        data: bytes,
                        echo_local: None,
                    };
                    let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                        message: serde_json::to_value(&client_msg)?,
                    };
                    self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
                }
            }
            KeyCode::PageDown => {
                // Check if Shift is held for scrolling
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.grid_renderer.lock().await.scroll_vertical(-10);
                    self.handle_scroll_update().await?;
                } else {
                    // Send Page Down escape sequence to server
                    let bytes = vec![0x1B, b'[', b'6', b'~']; // ESC[6~
                    let client_msg = crate::protocol::ClientMessage::TerminalInput {
                        data: bytes,
                        echo_local: None,
                    };
                    let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                        message: serde_json::to_value(&client_msg)?,
                    };
                    self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
                }
            }
            KeyCode::Home => {
                // Send Home key escape sequence
                let bytes = vec![0x1B, b'[', b'H']; // ESC[H
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            KeyCode::End => {
                // Send End key escape sequence
                let bytes = vec![0x1B, b'[', b'F']; // ESC[F
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            KeyCode::Delete => {
                // Send Delete key escape sequence
                let bytes = vec![0x1B, b'[', b'3', b'~']; // ESC[3~
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            KeyCode::Insert => {
                // Send Insert key escape sequence
                let bytes = vec![0x1B, b'[', b'2', b'~']; // ESC[2~
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            // Function keys
            KeyCode::F(n) => {
                let bytes = match n {
                    1 => vec![0x1B, b'O', b'P'],         // ESC O P
                    2 => vec![0x1B, b'O', b'Q'],         // ESC O Q
                    3 => vec![0x1B, b'O', b'R'],         // ESC O R
                    4 => vec![0x1B, b'O', b'S'],         // ESC O S
                    5 => vec![0x1B, b'[', b'1', b'5', b'~'], // ESC[15~
                    6 => vec![0x1B, b'[', b'1', b'7', b'~'], // ESC[17~
                    7 => vec![0x1B, b'[', b'1', b'8', b'~'], // ESC[18~
                    8 => vec![0x1B, b'[', b'1', b'9', b'~'], // ESC[19~
                    9 => vec![0x1B, b'[', b'2', b'0', b'~'], // ESC[20~
                    10 => vec![0x1B, b'[', b'2', b'1', b'~'], // ESC[21~
                    11 => vec![0x1B, b'[', b'2', b'3', b'~'], // ESC[23~
                    12 => vec![0x1B, b'[', b'2', b'4', b'~'], // ESC[24~
                    _ => return Ok(()), // Ignore F13-F24
                };
                let client_msg = crate::protocol::ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                };
                let app_msg = crate::protocol::signaling::AppMessage::Protocol {
                    message: serde_json::to_value(&client_msg)?,
                };
                self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await?;
            }
            _ => {}
        }
        
        Ok(())
    }
    
    /// Handle server messages
    async fn handle_server_message(&self, msg: ServerMessage) -> Result<()> {
        // Record incoming server message with debug recorder
        if let Some(ref mut recorder) = *self.debug_recorder.lock().await {
            if let Err(e) = recorder.record_server_message(&msg) {
                eprintln!("Failed to record server message: {:?}", e);
            }
        }
        
        // Debug log received message
        if let Some(ref debug_log) = self.debug_log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                use std::io::Write;
                let msg_type = match &msg {
                    ServerMessage::Delta { changes, .. } => format!("Delta with {} changes", changes.cell_changes.len()),
                    ServerMessage::Snapshot { grid, .. } => format!("Snapshot {}x{}", grid.width, grid.height),
                    ServerMessage::Error { .. } => "Error".to_string(),
                    _ => "Other".to_string(),
                };
                let _ = writeln!(file, "[{}] Client: Received ServerMessage: {}", 
                    chrono::Utc::now().format("%H:%M:%S%.3f"), msg_type);
            }
        }
        
        match msg {
            ServerMessage::Delta { changes, sequence, .. } => {
                // Log delta application details
                if let Some(ref mut recorder) = *self.debug_recorder.lock().await {
                    // Capture state BEFORE applying delta
                    let renderer = self.grid_renderer.lock().await;
                    
                    // Collect affected line numbers
                    let mut affected_lines = std::collections::HashSet::new();
                    for change in &changes.cell_changes {
                        affected_lines.insert(change.row);
                    }
                    let mut modified_lines: Vec<u16> = affected_lines.into_iter().collect();
                    modified_lines.sort();
                    
                    // Build a seam window around the first modified row (before applying delta)
                    let seam_anchor = modified_lines.first().copied().unwrap_or(0);
                    let seam_start = seam_anchor.saturating_sub(2);
                    let seam_end = seam_anchor.saturating_add(6).min(renderer.grid.height.saturating_sub(1));
                    let mut seam_before_lines: Vec<(u16, String)> = Vec::new();
                    for row in seam_start..=seam_end {
                        let mut line = String::new();
                        for col in 0..renderer.grid.width.min(120) {
                            if let Some(cell) = renderer.grid.get_cell(row, col) { line.push(cell.char); }
                        }
                        seam_before_lines.push((row, line.trim_end().to_string()));
                    }

                    // Count blank lines before
                    let blank_lines_before = renderer.grid.cells.iter()
                        .filter(|row| row.is_empty() || row.iter().all(|cell| cell.char == ' '))
                        .count();
                    
                    // Get sample of lines that will be modified (before state)
                    let lines_before: Vec<String> = modified_lines.iter()
                        .take(5)
                        .filter_map(|&line_num| {
                            renderer.grid.cells.get(line_num as usize).map(|row| {
                                format!("Line {}: {}", line_num, 
                                    row.iter().map(|cell| cell.char).collect::<String>().trim_end())
                            })
                        })
                        .collect();
                    
                    let before_dims = (renderer.server_width, renderer.server_height);
                    drop(renderer);
                    
                    // Apply the delta
                    self.grid_renderer.lock().await.apply_delta(&changes);
                    
                    // Capture state AFTER applying delta
                    let renderer = self.grid_renderer.lock().await;
                    let mut seam_after_lines: Vec<(u16, String)> = Vec::new();
                    for row in seam_start..=seam_end {
                        let mut line = String::new();
                        for col in 0..renderer.grid.width.min(120) {
                            if let Some(cell) = renderer.grid.get_cell(row, col) { line.push(cell.char); }
                        }
                        seam_after_lines.push((row, line.trim_end().to_string()));
                    }
                    
                    // Count blank lines after
                    let blank_lines_after = renderer.grid.cells.iter()
                        .filter(|row| row.is_empty() || row.iter().all(|cell| cell.char == ' '))
                        .count();
                    
                    // Get sample of lines after modification
                    let lines_after: Vec<String> = modified_lines.iter()
                        .take(5)
                        .filter_map(|&line_num| {
                            renderer.grid.cells.get(line_num as usize).map(|row| {
                                format!("Line {}: {}", line_num, 
                                    row.iter().map(|cell| cell.char).collect::<String>().trim_end())
                            })
                        })
                        .collect();
                    
                    let after_dims = if changes.dimension_change.is_some() {
                        Some((renderer.grid.width, renderer.grid.height))
                    } else {
                        None
                    };
                    
                    // Log the delta application
                    let _ = recorder.record_event(crate::debug_recorder::DebugEvent::ClientDeltaApplication {
                        timestamp: chrono::Utc::now(),
                        sequence,
                        cell_changes_count: changes.cell_changes.len(),
                        modified_lines: modified_lines.clone(),
                        has_dimension_change: changes.dimension_change.is_some(),
                        before_dims,
                        after_dims,
                        blank_lines_before,
                        blank_lines_after,
                        lines_before: lines_before.clone(),
                        lines_after: lines_after.clone(),
                    });
                    let _ = recorder.record_client_seam_context(sequence, seam_start, &seam_before_lines, &seam_after_lines);
                    // Record bottom context after delta applied
                    let _ = recorder.record_grid_bottom_context("client_after_delta", &renderer.grid, 6);
                    
                    // Also log to debug file if blank line count changed
                    if blank_lines_before != blank_lines_after {
                        if let Some(ref path) = self.debug_log {
                            if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path) {
                                use std::io::Write;
                                let _ = writeln!(file, "[{}] Client: Delta {} changed blank lines: {} -> {} (lines modified: {:?})",
                                    chrono::Utc::now().format("%H:%M:%S%.3f"),
                                    sequence,
                                    blank_lines_before,
                                    blank_lines_after,
                                    modified_lines
                                );
                                if !lines_before.is_empty() {
                                    let _ = writeln!(file, "  Before: {:?}", lines_before);
                                }
                                if !lines_after.is_empty() {
                                    let _ = writeln!(file, "  After: {:?}", lines_after);
                                }
                            }
                        }
                    }
                    
                    // Also record the full grid state after delta
                    let view_mode = if renderer.is_historical_mode() {
                        "historical"
                    } else {
                        "realtime"
                    };
                    let _ = recorder.record_client_grid_state(
                        &renderer.grid,
                        renderer.scroll_offset as i64,
                        view_mode
                    );
                    // Also log to debug file bottom context after delta (gated)
                    if std::env::var("BEACH_SEAM_DEBUG").ok().is_some() {
                        if let Some(ref path) = self.debug_log {
                        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path) {
                            use std::io::Write;
                            let mut trailing_blanks = 0usize;
                            for row in (0..renderer.grid.height).rev() {
                                let is_blank = (0..renderer.grid.width).all(|col| {
                                    renderer.grid.get_cell(row, col)
                                        .map(|c| c.char == ' ' || c.char == '\0')
                                        .unwrap_or(true)
                                });
                                if is_blank { trailing_blanks += 1; } else { break; }
                            }
                            let _ = writeln!(file, "[{}] Client: BottomContext after delta dims {}x{} trailing_blanks {}",
                                chrono::Utc::now().format("%H:%M:%S%.3f"),
                                renderer.grid.width,
                                renderer.grid.height,
                                trailing_blanks);
                            let start = renderer.grid.height.saturating_sub(5);
                            for row in start..renderer.grid.height {
                                let mut line = String::new();
                                for col in 0..renderer.grid.width.min(100) {
                                    if let Some(cell) = renderer.grid.get_cell(row, col) { line.push(cell.char); }
                                }
                                let _ = writeln!(file, "[{}] Client:   Row {}: '{}'",
                                    chrono::Utc::now().format("%H:%M:%S%.3f"), row, line.trim_end());
                            }
                        }
                        }
                    }
                } else {
                    // No debug recorder, just apply the delta
                    self.grid_renderer.lock().await.apply_delta(&changes);
                }
            }
            ServerMessage::Snapshot { grid, .. } => {
                // Debug logging for snapshot reception
                if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/beach-snapshot-debug.log")
                {
                    use std::io::Write;
                    let mode = self.last_requested_mode.lock().await;
                    let _ = writeln!(debug_file, 
                        "[{}] [SNAPSHOT_RECEIVED] Processing snapshot:",
                        chrono::Utc::now()
                    );
                    let _ = writeln!(debug_file, 
                        "  grid.start_line: {:?}",
                        grid.start_line.to_u64()
                    );
                    let _ = writeln!(debug_file, 
                        "  grid.end_line: {:?}",
                        grid.end_line.to_u64()
                    );
                    let _ = writeln!(debug_file, 
                        "  last_requested_mode: {:?}",
                        *mode
                    );
                    let _ = writeln!(debug_file, 
                        "  Will call enter_historical_mode: {}",
                        *mode == ViewMode::Historical && grid.start_line.to_u64().is_some()
                    );
                }
                
                // If we last requested Historical, set the client anchor to
                // the snapshot's start line before applying so cache is populated.
                {
                    let mode = self.last_requested_mode.lock().await;
                    if *mode == ViewMode::Historical {
                        if let Some(start) = grid.start_line.to_u64() {
                            // Log before and after enter_historical_mode
                            if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open("/tmp/beach-snapshot-debug.log")
                            {
                                use std::io::Write;
                                let _ = writeln!(debug_file, 
                                    "[{}] [ENTER_HISTORICAL] Calling enter_historical_mode({})",
                                    chrono::Utc::now(),
                                    start
                                );
                            }
                            
                            self.grid_renderer.lock().await.enter_historical_mode(start);
                            
                            // Log state after calling enter_historical_mode
                            if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open("/tmp/beach-snapshot-debug.log")
                            {
                                use std::io::Write;
                                let renderer = self.grid_renderer.lock().await;
                                let _ = writeln!(debug_file, 
                                    "[{}] [ENTER_HISTORICAL] After call - is_historical_mode: {}",
                                    chrono::Utc::now(),
                                    renderer.is_historical_mode()
                                );
                            }
                        } else {
                            // Log if we skip enter_historical_mode
                            if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open("/tmp/beach-snapshot-debug.log")
                            {
                                use std::io::Write;
                                let _ = writeln!(debug_file, 
                                    "[{}] [ENTER_HISTORICAL] SKIPPED - grid.start_line is None!",
                                    chrono::Utc::now()
                                );
                            }
                        }
                    }
                }
                // Apply the snapshot
                self.grid_renderer.lock().await.apply_snapshot(grid.clone());
                // Debug: log a small sample of the top few lines we will render
                if let Some(ref path) = self.debug_log {
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                        use std::io::Write;
                        let renderer = self.grid_renderer.lock().await;
                        let start_u64 = grid.start_line.to_u64().unwrap_or(0);
                        let end_u64 = grid.end_line.to_u64().unwrap_or(0);
                        let _ = writeln!(file, "[{}] Client: Snapshot applied (hist_mode={}, start_line={}, end_line={}, dims={}x{})",
                            chrono::Utc::now().format("%H:%M:%S%.3f"),
                            renderer.is_historical_mode(), start_u64, end_u64,
                            renderer.grid.width, renderer.grid.height);
                        for row in 0..renderer.grid.height.min(5) {
                            let mut s = String::new();
                            for col in 0..renderer.grid.width.min(80) {
                                if let Some(c) = renderer.grid.get_cell(row, col) { s.push(c.char); }
                            }
                            let _ = writeln!(file, "[{}] Client:   Top Row {}: '{}'",
                                chrono::Utc::now().format("%H:%M:%S%.3f"), row, s.trim_end());
                        }
                    }
                }
                // Clear pending history throttle (snapshot received)
                {
                    let mut pending = self.history_pending_line.lock().await;
                    *pending = None;
                }
                // Refresh history metadata from the snapshot's line counters so
                // scrollback math uses up-to-date absolute line numbers even when
                // deltas don't carry line info.
                if let (Some(start), Some(end)) = (grid.start_line.to_u64(), grid.end_line.to_u64()) {
                    use crate::subscription::HistoryMetadata;
                    let meta = HistoryMetadata {
                        oldest_line: start,
                        latest_line: end,
                        total_lines: end.saturating_sub(start).saturating_add(1),
                        oldest_timestamp: None,
                        latest_timestamp: Some(grid.timestamp),
                    };
                    self.grid_renderer.lock().await.set_history_metadata(meta);
                }
                
                // Record client grid state after applying snapshot
                if let Some(ref mut recorder) = *self.debug_recorder.lock().await {
                    let renderer = self.grid_renderer.lock().await;
                    let view_mode = if renderer.is_historical_mode() {
                        "historical"
                    } else {
                        "realtime"
                    };
                    let _ = recorder.record_client_grid_state(
                        &renderer.grid,
                        renderer.scroll_offset as i64,
                        view_mode
                    );
                    let _ = recorder.record_grid_bottom_context("client_after_snapshot", &renderer.grid, 6);
                    // And log to debug file bottom context after snapshot (gated)
                    if std::env::var("BEACH_SEAM_DEBUG").ok().is_some() {
                        if let Some(ref path) = self.debug_log {
                        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path) {
                            use std::io::Write;
                            let mut trailing_blanks = 0usize;
                            for row in (0..renderer.grid.height).rev() {
                                let is_blank = (0..renderer.grid.width).all(|col| {
                                    renderer.grid.get_cell(row, col)
                                        .map(|c| c.char == ' ' || c.char == '\0')
                                        .unwrap_or(true)
                                });
                                if is_blank { trailing_blanks += 1; } else { break; }
                            }
                            let _ = writeln!(file, "[{}] Client: BottomContext after snapshot dims {}x{} trailing_blanks {}",
                                chrono::Utc::now().format("%H:%M:%S%.3f"),
                                renderer.grid.width,
                                renderer.grid.height,
                                trailing_blanks);
                            let start = renderer.grid.height.saturating_sub(5);
                            for row in start..renderer.grid.height {
                                let mut line = String::new();
                                for col in 0..renderer.grid.width.min(100) {
                                    if let Some(cell) = renderer.grid.get_cell(row, col) { line.push(cell.char); }
                                }
                                let _ = writeln!(file, "[{}] Client:   Row {}: '{}'",
                                    chrono::Utc::now().format("%H:%M:%S%.3f"), row, line.trim_end());
                            }
                        }
                        }
                    }
                }
            }
            ServerMessage::HistoryInfo { 
                oldest_line, 
                latest_line, 
                total_lines, 
                oldest_timestamp, 
                latest_timestamp,
                ..
            } => {
                // Update history metadata in grid renderer
                use crate::subscription::HistoryMetadata;
                let metadata = HistoryMetadata {
                    oldest_line,
                    latest_line,
                    total_lines,
                    oldest_timestamp: oldest_timestamp.and_then(|ts| 
                        chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)),
                    latest_timestamp: latest_timestamp.and_then(|ts| 
                        chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)),
                };
                self.grid_renderer.lock().await.set_history_metadata(metadata);
            }
            ServerMessage::Error { code, .. } => {
                // Handle error - potentially trigger reconnect
                // eprintln!("Server error {}: {}", code.0, message);
                if code.0 == 1001 || code.0 == 1002 {  // Connection errors
                    *self.state.write().await = ClientState::Disconnected;
                    self.attempt_reconnect().await?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    
    /// Send a control message (non-blocking via queue)
    async fn send_control_message(&self, msg: ControlMessage) -> Result<()> {
        match msg {
            ControlMessage::Input { bytes, .. } => {
                // Debug log: Preparing to queue TerminalInput
                if let Some(ref debug_log) = self.debug_log {
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                        use std::io::Write;
                        let _ = writeln!(file, "[{}] Client: Queueing TerminalInput: {} bytes (will try raw channel)",
                            chrono::Utc::now().format("%H:%M:%S%.3f"), bytes.len());
                    }
                }
                
                // Try to use raw Input channel first
                let has_input_channel = {
                    let session = self.session.read().await;
                    let transport_arc = session.transport();
                    let transport = transport_arc.lock().await;
                    transport.channels().contains(&crate::transport::ChannelPurpose::Input)
                };
                
                let queue_start = std::time::Instant::now();
                
                if has_input_channel {
                    // Use raw bytes on Input channel - super fast!
                    // Format: [0x01 = TerminalInput] + [raw bytes]
                    let mut raw_msg = Vec::with_capacity(1 + bytes.len());
                    raw_msg.push(0x01); // Message type: TerminalInput
                    raw_msg.extend_from_slice(&bytes);
                    
                    self.send_queue_tx.send(QueuedMessage::RawInput {
                        channel: "Input".to_string(),
                        data: raw_msg,
                    }).await
                        .map_err(|_| anyhow::anyhow!("Send queue closed"))?;
                    
                    if let Some(ref debug_log) = self.debug_log {
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                            use std::io::Write;
                            let _ = writeln!(file, "[{}] Client: Queued RAW input in {}μs",
                                chrono::Utc::now().format("%H:%M:%S%.3f"), queue_start.elapsed().as_micros());
                        }
                    }
                } else {
                    // Fall back to JSON protocol
                    let client_msg = ClientMessage::TerminalInput {
                        data: bytes,
                        echo_local: None,
                    };
                    
                    let app_msg = AppMessage::Protocol {
                        message: serde_json::to_value(&client_msg)?,
                    };
                    
                    self.send_queue_tx.send(QueuedMessage::AppMessage(app_msg)).await
                        .map_err(|_| anyhow::anyhow!("Send queue closed"))?;
                    
                    if let Some(ref debug_log) = self.debug_log {
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                            use std::io::Write;
                            let _ = writeln!(file, "[{}] Client: Queued JSON input in {}μs (no Input channel)",
                                chrono::Utc::now().format("%H:%M:%S%.3f"), queue_start.elapsed().as_micros());
                        }
                    }
                }
                
                Ok(())
            }
            _ => Ok(()), // Not implemented yet
        }
    }
    
    /// Attempt to reconnect after disconnection
    async fn attempt_reconnect(&self) -> Result<()> {
        *self.state.write().await = ClientState::Reconnecting;
        
        // Retain last snapshot for resilience
        self.grid_renderer.lock().await.retain_last_snapshot();
        
        // Try to reconnect with exponential backoff
        let mut retry_delay = Duration::from_secs(1);
        let max_delay = Duration::from_secs(30);
        let max_attempts = 5;
        
        for attempt in 1..=max_attempts {
            // eprintln!("Reconnection attempt {} of {}...", attempt, max_attempts);
            
            // Try to re-establish connection
            match self.reconnect_to_server().await {
                Ok(_) => {
                    // eprintln!("Reconnected successfully!");
                    *self.state.write().await = ClientState::Connected;
                    return Ok(());
                }
                Err(_e) => {
                    // eprintln!("Reconnect failed: {}", e);
                    if attempt < max_attempts {
                        tokio::time::sleep(retry_delay).await;
                        retry_delay = std::cmp::min(retry_delay * 2, max_delay);
                    }
                }
            }
        }
        
        *self.state.write().await = ClientState::Disconnected;
        Err(anyhow::anyhow!("Failed to reconnect after {} attempts", max_attempts))
    }
    
    /// Reconnect to the server
    async fn reconnect_to_server(&self) -> Result<()> {
        // Re-send subscription request to restore state
        let (width, height) = crossterm::terminal::size()?;
        let overscan_height = height * 2;
        let dimensions = Dimensions { width, height: overscan_height };
        
        let subscribe_msg = ClientMessage::Subscribe {
            subscription_id: self.subscription_id.clone(),
            dimensions,
            mode: ViewMode::Realtime,
            position: None,
            compression: None,
        };
        
        let app_msg = AppMessage::Protocol {
            message: serde_json::to_value(&subscribe_msg)?,
        };
        
        self.session.write().await.send_to_server(app_msg).await?;
        
        // Note: In a real implementation, we'd wait for SubscriptionAck
        // and handle the new snapshot
        
        Ok(())
    }
    
    /// Handle scroll position updates to update overscan subscription
    async fn handle_scroll_update(&self) -> Result<()> {
        // Debug logging to file
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-scroll-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(debug_file, "[{}] handle_scroll_update: Starting", chrono::Utc::now());
        }
        
        // Snapshot what we need without holding the renderer lock across awaits
        let (calc_request, was_historical, scroll_at, srv_w, srv_h) = {
            let mut gr = self.grid_renderer.lock().await;
            let req = gr.calculate_history_needs();
            let hist = gr.is_historical_mode();
            let offset = gr.scroll_offset as usize;
            let w = gr.server_width;
            let h = gr.server_height;
            (req, hist, offset, w, h)
        };
        
        // Debug log the current state
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-scroll-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(debug_file, 
                "[{}] State: was_historical={}, scroll_offset={}, calc_request={:?}, srv_dims={}x{}",
                chrono::Utc::now(), was_historical, scroll_at, 
                calc_request.as_ref().map(|r| format!("lines {}-{}", r.start_line, r.end_line)),
                srv_w, srv_h
            );
        }

        // Record debug event with the computed request
        if let Some(ref mut recorder) = *self.debug_recorder.lock().await {
            let _ = recorder.record_event(crate::debug_recorder::DebugEvent::ClientHistoryNeedsCheck {
                timestamp: chrono::Utc::now(),
                scroll_offset: scroll_at,
                has_metadata: true,
                view_line: if was_historical { Some(0) } else { None },
                request: calc_request.as_ref().map(|r| (r.start_line, r.end_line)),
            });
        }

        if let Some(history_request) = calc_request {
            // Throttle duplicate/rapid requests
            let mut should_send = true;
            {
                let mut last_sent = self.history_last_sent.lock().await;
                let mut pending = self.history_pending_line.lock().await;
                if let Some(p_line) = *pending {
                    // If we already have a request for a nearby line, skip
                    if (history_request.start_line as i64 - p_line as i64).abs() < (srv_h as i64 / 2) {
                        should_send = false;
                    }
                }
                if let Some(ts) = *last_sent {
                    if ts.elapsed() < std::time::Duration::from_millis(75) {
                        should_send = false;
                    }
                }
                if should_send {
                    *pending = Some(history_request.start_line);
                    *last_sent = Some(std::time::Instant::now());
                }
            }
            if !should_send { return Ok(()); }

            // Log request
            if let Some(ref mut recorder) = *self.debug_recorder.lock().await {
                let _ = recorder.record_event(crate::debug_recorder::DebugEvent::ClientHistoryRequestSent {
                    timestamp: chrono::Utc::now(),
                    mode: "Historical".to_string(),
                    start_line: Some(history_request.start_line),
                    end_line: Some(history_request.end_line),
                });
            }
            
            // Debug log the Historical mode request
            if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/beach-scroll-debug.log")
            {
                use std::io::Write;
                let _ = writeln!(debug_file, 
                    "[{}] SENDING HISTORICAL REQUEST: start_line={}, end_line={}, dimensions={}x{}",
                    chrono::Utc::now(), history_request.start_line, history_request.end_line,
                    srv_w, srv_h.saturating_mul(2)
                );
            }
            
            // Send ModifySubscription to request historical view (non-blocking)
            let modify_msg = ClientMessage::ModifySubscription {
                subscription_id: self.subscription_id.clone(),
                dimensions: Some(Dimensions { width: srv_w, height: srv_h.saturating_mul(2) }),
                mode: Some(ViewMode::Historical),
                position: Some(ViewPosition { time: None, line: Some(history_request.start_line), offset: None }),
            };
            if let Some(ref mut recorder) = *self.debug_recorder.lock().await {
                let _ = recorder.record_client_message(&modify_msg);
            }
            let app_msg = AppMessage::Protocol { message: serde_json::to_value(&modify_msg)? };
            self.send_queue_tx
                .send(QueuedMessage::AppMessage(app_msg))
                .await
                .map_err(|_| anyhow::anyhow!("Send queue closed"))?;
            return Ok(());
        }

        // No history request: if we are at bottom while in historical mode, return to realtime
        if scroll_at == 0 && was_historical {
            // Debug log the return to realtime
            if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/beach-scroll-debug.log")
            {
                use std::io::Write;
                let _ = writeln!(debug_file, 
                    "[{}] RETURNING TO REALTIME MODE: scroll_offset=0, was_historical=true",
                    chrono::Utc::now()
                );
            }
            
            {
                let mut gr = self.grid_renderer.lock().await;
                gr.enter_realtime_mode();
            }
            let modify_msg = ClientMessage::ModifySubscription {
                subscription_id: self.subscription_id.clone(),
                dimensions: None,
                mode: Some(ViewMode::Realtime),
                position: None,
            };
            if let Some(ref mut recorder) = *self.debug_recorder.lock().await {
                let _ = recorder.record_client_message(&modify_msg);
            }
            // Debug logging for state transition to Realtime
            if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/beach-state-flow.log")
            {
                use std::io::Write;
                let mode_before = self.last_requested_mode.lock().await;
                let _ = writeln!(debug_file, 
                    "[{}] [CLIENT_STATE] BEFORE ModifySubscription send to Realtime:",
                    chrono::Utc::now()
                );
                let _ = writeln!(debug_file, 
                    "  last_requested_mode: {:?}",
                    *mode_before
                );
                let _ = writeln!(debug_file, 
                    "  new_mode: ViewMode::Realtime"
                );
            }
            
            let app_msg = AppMessage::Protocol { message: serde_json::to_value(&modify_msg)? };
            self.send_queue_tx
                .send(QueuedMessage::AppMessage(app_msg))
                .await
                .map_err(|_| anyhow::anyhow!("Send queue closed"))?;
            {
                let mut mode = self.last_requested_mode.lock().await;
                *mode = ViewMode::Realtime;
                
                // Log after updating
                if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/beach-state-flow.log")
                {
                    use std::io::Write;
                    let _ = writeln!(debug_file, 
                        "[{}] [CLIENT_STATE] AFTER updating last_requested_mode: {:?}",
                        chrono::Utc::now(),
                        *mode
                    );
                }
            }
            return Ok(());
        }

        // Failsafe: if user is scrolled (offset > 0) but we didn't generate a
        // request (e.g., because client-side anchor is stale) and we haven't
        // requested Historical yet, proactively send a Historical modify using
        // the current end_line as a baseline.
        if scroll_at > 0 {
            let should_force = {
                let mode = self.last_requested_mode.lock().await;
                *mode != ViewMode::Historical
            };
            if should_force {
                let (start_line_guess, width, height2) = {
                    let gr = self.grid_renderer.lock().await;
                    let end = gr.grid.end_line.to_u64().unwrap_or(0);
                    (end.saturating_sub(scroll_at as u64), gr.server_width, gr.server_height.saturating_mul(2))
                };
                let modify_msg = ClientMessage::ModifySubscription {
                    subscription_id: self.subscription_id.clone(),
                    dimensions: Some(Dimensions { width, height: height2 }),
                    mode: Some(ViewMode::Historical),
                    position: Some(ViewPosition { time: None, line: Some(start_line_guess), offset: None }),
                };
                if let Some(ref mut recorder) = *self.debug_recorder.lock().await {
                    let _ = recorder.record_client_message(&modify_msg);
                }
                
                // Debug logging for state transition to Historical
                if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/beach-state-flow.log")
                {
                    use std::io::Write;
                    let mode_before = self.last_requested_mode.lock().await;
                    let _ = writeln!(debug_file, 
                        "[{}] [CLIENT_STATE] BEFORE ModifySubscription send to Historical:",
                        chrono::Utc::now()
                    );
                    let _ = writeln!(debug_file, 
                        "  last_requested_mode: {:?}",
                        *mode_before
                    );
                    let _ = writeln!(debug_file, 
                        "  new_mode: ViewMode::Historical"
                    );
                    let _ = writeln!(debug_file, 
                        "  line_number: {}",
                        start_line_guess
                    );
                }
                
                let app_msg = AppMessage::Protocol { message: serde_json::to_value(&modify_msg)? };
                self.send_queue_tx
                    .send(QueuedMessage::AppMessage(app_msg))
                    .await
                    .map_err(|_| anyhow::anyhow!("Send queue closed"))?;
                {
                    let mut mode = self.last_requested_mode.lock().await;
                    *mode = ViewMode::Historical;
                    
                    // Log after updating
                    if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open("/tmp/beach-state-flow.log")
                    {
                        use std::io::Write;
                        let _ = writeln!(debug_file, 
                            "[{}] [CLIENT_STATE] AFTER updating last_requested_mode: {:?}",
                            chrono::Utc::now(),
                            *mode
                        );
                    }
                }
            }
        }

        Ok(())
    }
    
    /// Render the terminal
    async fn render<B: ratatui::backend::Backend>(
        &self,
        terminal: &mut Terminal<B>,
    ) -> Result<()> {
        let grid_renderer = self.grid_renderer.lock().await;
        let predictive_echo = self.predictive_echo.lock().await;
        
        // Get active prediction positions
        let predictions: Vec<(u16, u16)> = predictive_echo.active_predictions()
            .iter()
            .map(|(_, pred)| pred.position)
            .collect();
        
        terminal.draw(|f| {
            grid_renderer.render(f, &predictions);
        })?;
        
        Ok(())
    }
}


/// Wrapper to allow Arc<TerminalClient<T>> to be used as ClientMessageHandler
struct HandlerWrapper<T: Transport + Send + Sync + 'static> {
    client: Arc<TerminalClient<T>>,
}

#[async_trait]
impl<T: Transport + Send + Sync + 'static> ClientMessageHandler for HandlerWrapper<T> {
    async fn handle_server_message(&self, message: AppMessage) {
        if let AppMessage::Protocol { message } = message {
            if let Ok(server_msg) = serde_json::from_value::<ServerMessage>(message) {
                let _ = self.client.server_tx.send(server_msg).await;
            }
        }
    }
    
    async fn handle_peer_joined(&self, _peer: &PeerInfo) {
        // Not implemented for minimal client
    }
    
    async fn handle_peer_left(&self, _peer_id: &str) {
        // Not implemented for minimal client
    }
}

/// ClientMessageHandler implementation for Arc<TerminalClient>
#[async_trait]
impl<T: Transport + Send + Sync + 'static> ClientMessageHandler for Arc<TerminalClient<T>> {
    async fn handle_server_message(&self, message: AppMessage) {
        if let AppMessage::Protocol { message } = message {
            if let Ok(server_msg) = serde_json::from_value::<ServerMessage>(message) {
                let _ = self.server_tx.send(server_msg).await;
            }
        }
    }
    
    async fn handle_peer_joined(&self, _peer: &PeerInfo) {
        // Not implemented for minimal client
    }
    
    async fn handle_peer_left(&self, _peer_id: &str) {
        // Not implemented for minimal client
    }
}
