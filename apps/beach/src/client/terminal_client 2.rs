/// Full terminal client with TUI, predictive echo, and resilience
use crate::client::{grid_renderer::GridRenderer, predictive_echo::PredictiveEcho};
use crate::protocol::{ClientMessage, ServerMessage, Dimensions, ViewMode, ViewPosition, ControlMessage};
use crate::protocol::signaling::{AppMessage, PeerInfo};
use crate::session::{ClientSession, message_handlers::ClientMessageHandler};
use crate::transport::Transport;
use anyhow::Result;
use async_trait::async_trait;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
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
    
    /// Shutdown signal
    shutdown_tx: mpsc::Sender<()>,
    
    /// Shutdown receiver
    shutdown_rx: Arc<Mutex<mpsc::Receiver<()>>>,
    
    /// Debug log file path
    debug_log: Option<String>,
}

impl<T: Transport + Send + Sync + 'static> TerminalClient<T> {
    /// Create a new terminal client and join an existing session
    pub async fn create(
        config: &crate::config::Config,
        transport: T,
        session_str: &str,
        passphrase: Option<String>,
        debug_log: Option<String>,
    ) -> Result<Arc<Self>> {
        use crate::session::Session;
        
        // Join the session
        let (client_session, server_addr, session_id) = Session::join(session_str, transport, passphrase).await?;
        
        // Create the client
        let client = Self::new(client_session, None, debug_log).await?;
        
        // Connect signaling first
        {
            let session = client.session.clone();
            if let Err(e) = session.write().await.connect_signaling(&server_addr, &session_id).await {
                // eprintln!("⚠️  Failed to establish WebSocket connection: {}", e);
                // eprintln!("⚠️  Continuing without WebSocket connection");
            }
        }
        
        // Then set the handler (which will start the router since signaling is connected)
        client.set_as_handler().await;
        
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
    ) -> Result<Arc<Self>> {
        // Get terminal dimensions
        let (width, height) = crossterm::terminal::size()?;
        
        // Create grid renderer
        let grid_renderer = Arc::new(Mutex::new(GridRenderer::new(width, height)?));
        
        // Create predictive echo
        let client_id = format!("client-{}", uuid::Uuid::new_v4());
        let predictive_echo = Arc::new(Mutex::new(PredictiveEcho::new(client_id.clone())));
        
        // Create channels
        let (server_tx, server_rx) = mpsc::channel(100);
        let (input_tx, input_rx) = mpsc::channel(100);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        
        let subscription_id = format!("sub-{}", uuid::Uuid::new_v4());
        
        let client = Arc::new(Self {
            session: Arc::new(RwLock::new(session)),
            grid_renderer,
            predictive_echo,
            state: Arc::new(RwLock::new(ClientState::Connecting)),
            subscription_id,
            server_tx: server_tx.clone(),
            server_rx: Arc::new(Mutex::new(server_rx)),
            input_tx,
            input_rx: Arc::new(Mutex::new(input_rx)),
            shutdown_tx,
            shutdown_rx: Arc::new(Mutex::new(shutdown_rx)),
            debug_log,
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
                if event::poll(Duration::from_millis(10)).unwrap_or(false) {
                    if let Ok(evt) = event::read() {
                        let _ = event_tx.send(evt).await;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
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
                let area = f.size();
                
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
        // Get the receivers from our stored channels first
        let mut server_rx = self.server_rx.lock().await;
        let mut shutdown_rx = self.shutdown_rx.lock().await;
        
        // Connect and subscribe BEFORE setting up terminal (to avoid terminal issues)
        self.connect_and_subscribe(&mut *server_rx).await?;
        
        // Setup terminal after connection is established
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen)?;
        
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        
        // Start event loops
        let result = self.run_event_loops(&mut terminal, &mut *server_rx, &mut *shutdown_rx).await;
        
        // Cleanup terminal
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        
        result
    }
    
    /// Connect to server and establish subscription
    async fn connect_and_subscribe(&self, server_rx: &mut mpsc::Receiver<ServerMessage>) -> Result<()> {
        *self.state.write().await = ClientState::Connecting;
        
        // eprintln!("⏳ Waiting for server peer ID to be set by JoinSuccess message...");
        
        // Wait for server peer ID to be available (set by JoinSuccess handler)
        // TODO: WebRTC implementation - commented out temporarily
        /*
        let mut retries = 0;
        while retries < 30 {  // Wait up to 3 seconds
            if self.session.read().await.has_server_peer_id().await {
                // eprintln!("✅ Server peer ID found after {} retries", retries);
                break;
            }
            if retries % 10 == 0 {
                // eprintln!("  Still waiting... (retry {})", retries);
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            retries += 1;
        }
        
        if retries >= 30 {
            // eprintln!("❌ Timeout after {} retries - server peer ID never set", retries);
            return Err(anyhow::anyhow!("Timeout waiting for server connection"));
        }
        */
        
        // Wait for WebRTC transport to be ready (if available)
        // TODO: WebRTC implementation - commented out temporarily
        /*
        // eprintln!("⏳ Waiting for WebRTC transport to be ready...");
        let mut webrtc_retries = 0;
        while webrtc_retries < 50 {  // Wait up to 5 seconds for WebRTC
            if self.session.read().await.has_webrtc_transport().await {
                // eprintln!("✅ WebRTC transport ready after {} retries", webrtc_retries);
                break;
            }
            if webrtc_retries % 10 == 0 && webrtc_retries > 0 {
                // eprintln!("  Still waiting for WebRTC... (retry {})", webrtc_retries);
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            webrtc_retries += 1;
        }
        */
        
        // WebRTC transport check is commented out for now
        /*
        if webrtc_retries >= 50 {
            // eprintln!("⚠️  WebRTC transport not available after {} retries, continuing with WebSocket", webrtc_retries);
        }
        */
        
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
        
        let app_msg = AppMessage::Protocol {
            message: serde_json::to_value(&subscribe_msg)?,
        };
        
        // Debug log the subscribe message being sent
        if let Some(ref debug_log) = self.debug_log {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log) {
                use std::io::Write;
                let _ = writeln!(file, "[{}] Client sending Subscribe message: {:?}", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"), subscribe_msg);
                let _ = writeln!(file, "[{}] Client sending AppMessage: {:?}", 
                                 chrono::Utc::now().format("%H:%M:%S%.3f"), app_msg);
            }
        }
        
        self.session.write().await.send_to_server(app_msg).await?;
        
        // Wait for acknowledgment
        if let Some(msg) = server_rx.recv().await {
            match msg {
                ServerMessage::SubscriptionAck { .. } => {
                    *self.state.write().await = ClientState::Connected;
                }
                _ => {
                    return Err(anyhow::anyhow!("Unexpected response: {:?}", msg));
                }
            }
        }
        
        // Wait for initial snapshot
        if let Some(msg) = server_rx.recv().await {
            match msg {
                ServerMessage::Snapshot { grid, .. } => {
                    self.grid_renderer.lock().await.apply_snapshot(grid);
                }
                _ => {
                    return Err(anyhow::anyhow!("Expected snapshot, got: {:?}", msg));
                }
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
        let mut render_interval = time::interval(Duration::from_millis(50));
        
        // Create a channel for events
        let (event_tx, mut event_rx) = mpsc::channel(100);
        
        // Spawn a task to read keyboard events
        tokio::spawn(async move {
            loop {
                if event::poll(Duration::from_millis(10)).unwrap_or(false) {
                    if let Ok(evt) = event::read() {
                        if event_tx.send(evt).await.is_err() {
                            break; // Channel closed
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        
        loop {
            tokio::select! {
                // Handle keyboard events
                Some(evt) = event_rx.recv() => {
                    if let Event::Key(key) = evt {
                        self.handle_key_event(key).await?;
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
    
    /// Handle keyboard input
    async fn handle_key_event(&self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.shutdown_tx.send(()).await?;
            }
            KeyCode::Char(c) => {
                // Predictive echo
                let bytes = vec![c as u8];
                let cursor_pos = {
                    let grid = self.grid_renderer.lock().await;
                    (grid.grid.cursor.col, grid.grid.cursor.row)
                };
                
                let seq = self.predictive_echo.lock().await.predict_input(bytes.clone(), cursor_pos);
                
                // Send to server
                let msg = self.predictive_echo.lock().await.create_input_message(seq, bytes);
                self.send_control_message(msg).await?;
            }
            KeyCode::Up => {
                self.grid_renderer.lock().await.scroll_vertical(-1);
                self.handle_scroll_update().await?;
            }
            KeyCode::Down => {
                self.grid_renderer.lock().await.scroll_vertical(1);
                self.handle_scroll_update().await?;
            }
            KeyCode::Left => {
                self.grid_renderer.lock().await.scroll_horizontal(-1);
            }
            KeyCode::Right => {
                self.grid_renderer.lock().await.scroll_horizontal(1);
            }
            KeyCode::PageUp => {
                self.grid_renderer.lock().await.scroll_vertical(-10);
                self.handle_scroll_update().await?;
            }
            KeyCode::PageDown => {
                self.grid_renderer.lock().await.scroll_vertical(10);
                self.handle_scroll_update().await?;
            }
            _ => {}
        }
        
        Ok(())
    }
    
    /// Handle server messages
    async fn handle_server_message(&self, msg: ServerMessage) -> Result<()> {
        match msg {
            ServerMessage::Delta { changes, .. } => {
                self.grid_renderer.lock().await.apply_delta(&changes);
            }
            ServerMessage::Snapshot { grid, .. } => {
                self.grid_renderer.lock().await.apply_snapshot(grid);
            }
            ServerMessage::Error { code, message, .. } => {
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
    
    /// Send a control message
    async fn send_control_message(&self, msg: ControlMessage) -> Result<()> {
        // For now, wrap in existing protocol
        let client_msg = match msg {
            ControlMessage::Input { bytes, .. } => {
                ClientMessage::TerminalInput {
                    data: bytes,
                    echo_local: None,
                }
            }
            _ => return Ok(()), // Not implemented yet
        };
        
        let app_msg = AppMessage::Protocol {
            message: serde_json::to_value(&client_msg)?,
        };
        
        self.session.write().await.send_to_server(app_msg).await?;
        Ok(())
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
                Err(e) => {
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
        // Get current overscan parameters from grid renderer
        let (from_line, height) = self.grid_renderer.lock().await.get_overscan_params();
        
        // Send ModifySubscription message to update view position
        let modify_msg = ClientMessage::ModifySubscription {
            subscription_id: self.subscription_id.clone(),
            dimensions: Some(Dimensions { 
                width: self.grid_renderer.lock().await.server_width, 
                height 
            }),
            mode: Some(ViewMode::Historical),
            position: Some(ViewPosition {
                time: None,
                line: Some(from_line),
                offset: None,
            }),
        };
        
        let app_msg = AppMessage::Protocol {
            message: serde_json::to_value(&modify_msg)?,
        };
        
        self.session.write().await.send_to_server(app_msg).await?;
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