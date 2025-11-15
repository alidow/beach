use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

/// Minimal JSON-RPC client for interacting with the host MCP server.
pub struct McpClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: AtomicU64,
}

impl McpClient {
    pub async fn connect(path: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path).await?;
        let (read_half, write_half) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(read_half),
            writer: write_half,
            next_id: AtomicU64::new(1),
        })
    }

    pub async fn initialize(&mut self) -> Result<(), McpClientError> {
        let _ = self.call_method("initialize", json!({})).await?;
        Ok(())
    }

    pub async fn list_tools(&mut self) -> Result<Vec<Value>, McpClientError> {
        let response = self.call_method("tools/list", json!({})).await?;
        let tools = response
            .get("tools")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(tools)
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<Value, McpClientError> {
        self.call_method(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments
            }),
        )
        .await
    }

    async fn call_method(&mut self, method: &str, params: Value) -> Result<Value, McpClientError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut encoded = serde_json::to_string(&request)?;
        encoded.push('\n');
        self.writer.write_all(encoded.as_bytes()).await?;
        self.writer.flush().await?;
        let mut line = String::new();
        loop {
            line.clear();
            let read = self.reader.read_line(&mut line).await?;
            if read == 0 {
                return Err(McpClientError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "mcp server closed connection",
                )));
            }
            let value: Value = serde_json::from_str(&line)?;
            let response_id = value.get("id");
            let Some(response_id) = response_id else {
                continue;
            };
            if !matches_id(response_id, id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                let message = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                return Err(McpClientError::Rpc(message));
            }
            let result = value.get("result").cloned().unwrap_or_else(|| Value::Null);
            return Ok(result);
        }
    }
}

fn matches_id(value: &Value, expected: u64) -> bool {
    match value {
        Value::Number(num) => num.as_u64() == Some(expected),
        Value::String(text) => text.parse::<u64>().ok() == Some(expected),
        _ => false,
    }
}

#[derive(Debug, Error)]
pub enum McpClientError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("rpc error: {0}")]
    Rpc(String),
}
