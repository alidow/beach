use std::fs::File;
use std::io::{BufWriter, Write};
use serde::Serialize;
use chrono::{DateTime, Utc};
use anyhow::Result;

use crate::protocol::{ServerMessage, ClientMessage};
use crate::server::terminal_state::Grid;

#[derive(Serialize)]
#[serde(tag = "type")]
pub enum DebugEvent {
    #[serde(rename = "client_message")]
    ClientMessage {
        timestamp: DateTime<Utc>,
        message: ClientMessage,
    },
    
    #[serde(rename = "server_message")]
    ServerMessage {
        timestamp: DateTime<Utc>,
        message: ServerMessage,
    },
    
    #[serde(rename = "client_grid_state")]
    ClientGridState {
        timestamp: DateTime<Utc>,
        grid: Grid,
        scroll_offset: i64,
        view_mode: String,
    },
    
    #[serde(rename = "server_backend_state")]
    ServerBackendState {
        timestamp: DateTime<Utc>,
        grid: Grid,
        cursor_pos: (u16, u16),
    },
    
    #[serde(rename = "server_subscription_view")]
    ServerSubscriptionView {
        timestamp: DateTime<Utc>,
        subscription_id: String,
        grid: Grid,
        view_mode: String,
    },
    
    #[serde(rename = "comment")]
    Comment {
        timestamp: DateTime<Utc>,
        text: String,
    },
}

pub struct DebugRecorder {
    writer: BufWriter<File>,
}

impl DebugRecorder {
    pub fn new(path: &str) -> Result<Self> {
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        Ok(Self { writer })
    }
    
    pub fn record_event(&mut self, event: DebugEvent) -> Result<()> {
        let json = serde_json::to_string(&event)?;
        writeln!(self.writer, "{}", json)?;
        self.writer.flush()?;
        Ok(())
    }
    
    pub fn record_client_message(&mut self, msg: &ClientMessage) -> Result<()> {
        self.record_event(DebugEvent::ClientMessage {
            timestamp: Utc::now(),
            message: msg.clone(),
        })
    }
    
    pub fn record_server_message(&mut self, msg: &ServerMessage) -> Result<()> {
        self.record_event(DebugEvent::ServerMessage {
            timestamp: Utc::now(),
            message: msg.clone(),
        })
    }
    
    pub fn record_client_grid_state(&mut self, grid: &Grid, scroll_offset: i64, view_mode: &str) -> Result<()> {
        self.record_event(DebugEvent::ClientGridState {
            timestamp: Utc::now(),
            grid: grid.clone(),
            scroll_offset,
            view_mode: view_mode.to_string(),
        })
    }
    
    pub fn record_server_backend_state(&mut self, grid: &Grid, cursor_pos: (u16, u16)) -> Result<()> {
        self.record_event(DebugEvent::ServerBackendState {
            timestamp: Utc::now(),
            grid: grid.clone(),
            cursor_pos,
        })
    }
    
    pub fn record_server_subscription_view(&mut self, subscription_id: &str, grid: &Grid, view_mode: &str) -> Result<()> {
        self.record_event(DebugEvent::ServerSubscriptionView {
            timestamp: Utc::now(),
            subscription_id: subscription_id.to_string(),
            grid: grid.clone(),
            view_mode: view_mode.to_string(),
        })
    }
    
    pub fn record_comment(&mut self, text: &str) -> Result<()> {
        self.record_event(DebugEvent::Comment {
            timestamp: Utc::now(),
            text: text.to_string(),
        })
    }
}