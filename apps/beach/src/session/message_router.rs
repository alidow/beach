use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use anyhow::{Result, anyhow};

use crate::protocol::{ClientMessage, ServerMessage};
use super::multiplexer::SessionBroker;
use super::view_registry::ClientId;
use crate::transport::Transport;
use crate::server::terminal_state::GridDelta;

pub struct MessageRouter<T: Transport + Send + 'static> {
    broker: Arc<SessionBroker<T>>,
    client_channels: Arc<RwLock<Vec<(ClientId, mpsc::Sender<ServerMessage>, mpsc::Receiver<ClientMessage>)>>>,
    router_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
}

impl<T: Transport + Send + Sync + 'static> MessageRouter<T> {
    pub fn new(broker: Arc<SessionBroker<T>>) -> Self {
        Self {
            broker,
            client_channels: Arc::new(RwLock::new(Vec::new())),
            router_handle: Arc::new(RwLock::new(None)),
        }
    }
    
    pub async fn add_client_channel(
        &self,
        client_id: ClientId,
        tx: mpsc::Sender<ServerMessage>,
        rx: mpsc::Receiver<ClientMessage>,
    ) {
        let mut channels = self.client_channels.write().await;
        channels.push((client_id, tx, rx));
    }
    
    pub async fn start(&self) {
        let broker = self.broker.clone();
        let channels = self.client_channels.clone();
        
        let handle = tokio::spawn(async move {
            loop {
                let mut channels = channels.write().await;
                let mut completed = Vec::new();
                
                for (idx, (client_id, _tx, rx)) in channels.iter_mut().enumerate() {
                    if let Ok(msg) = rx.try_recv() {
                        let client_id = client_id.clone();
                        let broker = broker.clone();
                        
                        tokio::spawn(async move {
                            if let Err(e) = broker.handle_client_message(client_id.clone(), msg).await {
                                eprintln!("Error handling message from client {}: {}", client_id, e);
                            }
                        });
                    }
                }
                
                for idx in completed.iter().rev() {
                    channels.remove(*idx);
                }
                
                drop(channels);
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        });
        
        let mut router_handle = self.router_handle.write().await;
        *router_handle = Some(handle);
    }
    
    pub async fn stop(&self) {
        let mut router_handle = self.router_handle.write().await;
        if let Some(handle) = router_handle.take() {
            handle.abort();
        }
    }
    
    pub async fn route_delta_to_view(&self, view_id: String, delta: GridDelta) -> Result<()> {
        self.broker.broadcast_delta(&view_id, delta).await
    }
    
    pub async fn route_to_client(&self, client_id: &str, message: ServerMessage) -> Result<()> {
        let channels = self.client_channels.read().await;
        for (cid, tx, _) in channels.iter() {
            if cid == client_id {
                tx.send(message).await
                    .map_err(|e| anyhow!("Failed to send message to client {}: {}", client_id, e))?;
                return Ok(());
            }
        }
        Err(anyhow!("Client {} not found", client_id))
    }
    
    pub async fn broadcast_to_all(&self, message: ServerMessage) -> Result<()> {
        let channels = self.client_channels.read().await;
        for (client_id, tx, _) in channels.iter() {
            if let Err(e) = tx.send(message.clone()).await {
                eprintln!("Failed to send message to client {}: {}", client_id, e);
            }
        }
        Ok(())
    }
    
    pub async fn remove_client(&self, client_id: &str) -> Result<()> {
        let mut channels = self.client_channels.write().await;
        channels.retain(|(cid, _, _)| cid != client_id);
        self.broker.remove_client(client_id).await?;
        Ok(())
    }
}