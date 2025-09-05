/// Client-side subscription management
use tokio::sync::mpsc;
use anyhow::Result;
use std::io::{self, Write};

use crate::protocol::{ClientMessage, ServerMessage, Dimensions, ViewMode, ViewPosition};
use crate::session::{ClientSession, signaling::AppMessage};
use crate::transport::Transport;

/// Manages a client's subscription to a terminal session
pub struct SubscriptionClient<T: Transport + Send + 'static> {
    session: ClientSession<T>,
    subscription_id: String,
    dimensions: Dimensions,
    
    /// Channel to receive server messages
    server_rx: mpsc::Receiver<ServerMessage>,
    
    /// Channel to send stdin input
    stdin_tx: mpsc::Sender<Vec<u8>>,
}

impl<T: Transport + Send + 'static> SubscriptionClient<T> {
    /// Create a new subscription client
    pub async fn new(
        mut session: ClientSession<T>,
        terminal_width: u16,
        terminal_height: u16,
    ) -> Result<Self> {
        let subscription_id = format!("client-sub-{}", uuid::Uuid::new_v4());
        let dimensions = Dimensions {
            width: terminal_width,
            height: terminal_height,
        };
        
        let (server_tx, server_rx) = mpsc::channel(100);
        let (stdin_tx, _stdin_rx) = mpsc::channel(100);
        
        // TODO: Wire up message handler to receive protocol messages
        // session.set_handler(...);
        
        Ok(Self {
            session,
            subscription_id,
            dimensions,
            server_rx,
            stdin_tx,
        })
    }
    
    /// Connect to the server and establish subscription
    pub async fn connect(&mut self) -> Result<()> {
        eprintln!("🔌 Establishing subscription...");
        
        // Send subscription request
        let subscribe_msg = ClientMessage::Subscribe {
            subscription_id: self.subscription_id.clone(),
            dimensions: self.dimensions.clone(),
            mode: ViewMode::Realtime,
            position: None,
            compression: None,
        };
        
        // Wrap in Protocol message
        let app_msg = AppMessage::Protocol {
            message: serde_json::to_value(&subscribe_msg)?,
        };
        
        self.session.send_to_server(app_msg).await?;
        
        eprintln!("📡 Subscription request sent, waiting for acknowledgment...");
        
        // Wait for SubscriptionAck
        if let Some(msg) = self.server_rx.recv().await {
            match msg {
                ServerMessage::SubscriptionAck { subscription_id, status, .. } => {
                    eprintln!("✅ Subscription established: {} (status: {:?})", 
                             subscription_id, status);
                }
                _ => {
                    eprintln!("⚠️  Unexpected response: {:?}", msg);
                }
            }
        }
        
        // Wait for initial Snapshot
        if let Some(msg) = self.server_rx.recv().await {
            match msg {
                ServerMessage::Snapshot { grid, .. } => {
                    eprintln!("📸 Received initial terminal snapshot");
                    eprintln!("   Grid size: {}x{}", grid.width, grid.height);
                    // TODO: Render the grid to terminal
                    self.render_grid(&grid)?;
                }
                _ => {
                    eprintln!("⚠️  Expected snapshot, got: {:?}", msg);
                }
            }
        }
        
        Ok(())
    }
    
    /// Send terminal input from stdin
    pub async fn send_input(&mut self, data: Vec<u8>) -> Result<()> {
        let input_msg = ClientMessage::TerminalInput {
            data,
            subscription_id: Some(self.subscription_id.clone()),
        };
        
        let app_msg = AppMessage::Protocol {
            message: serde_json::to_value(&input_msg)?,
        };
        
        self.session.send_to_server(app_msg).await?;
        Ok(())
    }
    
    /// Handle incoming server messages
    pub async fn handle_server_messages(&mut self) -> Result<()> {
        while let Some(msg) = self.server_rx.recv().await {
            match msg {
                ServerMessage::Delta { changes, .. } => {
                    eprintln!("📝 Received terminal update");
                    // TODO: Apply delta to current view
                    self.apply_delta(&changes)?;
                }
                ServerMessage::Snapshot { grid, .. } => {
                    eprintln!("📸 Received new snapshot");
                    self.render_grid(&grid)?;
                }
                ServerMessage::Error { message, .. } => {
                    eprintln!("❌ Server error: {}", message);
                }
                ServerMessage::Notify { notification_type, .. } => {
                    eprintln!("🔔 Notification: {:?}", notification_type);
                }
                _ => {
                    eprintln!("📨 Other message: {:?}", msg);
                }
            }
        }
        Ok(())
    }
    
    /// Resize the terminal view
    pub async fn resize(&mut self, width: u16, height: u16) -> Result<()> {
        self.dimensions = Dimensions { width, height };
        
        let modify_msg = ClientMessage::ModifySubscription {
            subscription_id: self.subscription_id.clone(),
            dimensions: Some(self.dimensions.clone()),
            mode: None,
            position: None,
        };
        
        let app_msg = AppMessage::Protocol {
            message: serde_json::to_value(&modify_msg)?,
        };
        
        self.session.send_to_server(app_msg).await?;
        Ok(())
    }
    
    /// Render a grid to the terminal (stub)
    fn render_grid(&self, grid: &crate::server::terminal_state::Grid) -> Result<()> {
        // Clear screen
        print!("\x1b[2J\x1b[H");
        
        // Render each row
        for row in &grid.rows {
            for cell in &row.cells {
                print!("{}", cell.c);
            }
            println!();
        }
        
        io::stdout().flush()?;
        Ok(())
    }
    
    /// Apply a delta to the current view (stub)
    fn apply_delta(&self, delta: &crate::server::terminal_state::GridDelta) -> Result<()> {
        eprintln!("   {} cell changes", delta.cell_changes.len());
        // TODO: Actually apply the delta to a maintained grid state
        Ok(())
    }
}

/// Example of how a client would use the subscription system
pub async fn example_client_flow<T: Transport + Send + 'static>(
    session: ClientSession<T>,
) -> Result<()> {
    // Get terminal dimensions
    let (width, height) = termion::terminal_size().unwrap_or((80, 24));
    
    // Create subscription client
    let mut client = SubscriptionClient::new(session, width, height).await?;
    
    // Connect and establish subscription
    client.connect().await?;
    
    // Spawn task to handle server messages
    let mut client_clone = client.clone(); // Would need to implement Clone or use Arc
    tokio::spawn(async move {
        let _ = client_clone.handle_server_messages().await;
    });
    
    // Spawn task to read stdin and send input
    tokio::spawn(async move {
        let stdin = io::stdin();
        let mut buffer = [0; 1024];
        
        loop {
            if let Ok(n) = stdin.read(&mut buffer) {
                if n > 0 {
                    let _ = client.send_input(buffer[..n].to_vec()).await;
                }
            }
        }
    });
    
    // Main loop could handle other events (resize, etc.)
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        // Check for terminal resize, etc.
    }
}