mod resources;
mod tools;

pub use resources::{ResourceDescriptor, TerminalResource};
pub use tools::{
    ACQUIRE_LEASE, LIST_SESSIONS, REQUEST_HISTORY, RESIZE, SEND_KEYS, SEND_TEXT, SET_VIEWPORT,
    TerminalToolDescriptor, handle_list_sessions,
};

use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};

use crate::mcp::auth::LeaseManager;
use crate::mcp::registry::TerminalSession;

use resources::{GridSnapshotRequest, HistoryReadRequest};
use tools::{SendKeysRequest, SendTextRequest};

#[derive(Clone)]
pub struct TerminalSurface {
    session: Arc<TerminalSession>,
}

impl TerminalSurface {
    pub fn new(session: Arc<TerminalSession>) -> Self {
        Self { session }
    }

    pub fn session_id(&self) -> &str {
        &self.session.session_id
    }

    pub fn describe_resources(&self) -> Vec<ResourceDescriptor> {
        TerminalResource::descriptors(self.session_id())
    }

    pub fn read_resource(
        &self,
        resource: &resources::TerminalResource,
        params: Option<&Value>,
    ) -> Result<Value> {
        match resource {
            TerminalResource::Grid => {
                let request = GridSnapshotRequest::from_params(params)?;
                resources::read_grid_snapshot(&self.session, &request)
            }
            TerminalResource::History => {
                let request = HistoryReadRequest::from_params(params)?;
                resources::read_history_segment(&self.session, &request)
            }
            TerminalResource::Cursor => resources::read_cursor_state(&self.session),
        }
    }

    pub fn list_tools(&self, read_only: bool) -> Vec<TerminalToolDescriptor> {
        tools::list_tools(read_only)
    }

    pub fn call_tool(&self, name: &str, params: &Value, leases: &LeaseManager) -> Result<Value> {
        match name {
            tools::ACQUIRE_LEASE => {
                let info = tools::handle_acquire_lease(leases, self.session_id(), params)?;
                Ok(json!({
                    "lease_id": info.lease_id.to_string(),
                    "expires_at": info.expires_at
                }))
            }
            tools::RELEASE_LEASE => {
                tools::handle_release_lease(leases, params)?;
                Ok(json!({"status": "released"}))
            }
            tools::SEND_TEXT => {
                let request = SendTextRequest::from_params(params)?;
                tools::handle_send_text(&self.session, &request, leases)?;
                Ok(json!({"status": "ok"}))
            }
            tools::SEND_KEYS => {
                let request = SendKeysRequest::from_params(params)?;
                tools::handle_send_keys(&self.session, &request, leases)?;
                Ok(json!({"status": "ok"}))
            }
            LIST_SESSIONS => handle_list_sessions(leases, params),
            tools::RESIZE => {
                let response = tools::handle_resize(&self.session, params, leases)?;
                Ok(response)
            }
            tools::SET_VIEWPORT => {
                let response = tools::handle_set_viewport(&self.session, params, leases)?;
                Ok(response)
            }
            tools::REQUEST_HISTORY => {
                let response = tools::handle_request_history(&self.session, params, leases)?;
                Ok(response)
            }
            _ => Err(anyhow::anyhow!("unknown tool: {name}")),
        }
    }

    pub fn start_subscription(
        &self,
        resource: &resources::TerminalResource,
        params: Option<&Value>,
        tx: tokio::sync::mpsc::Sender<Value>,
        subscription_id: String,
        cancel_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<tokio::task::JoinHandle<()>> {
        match resource {
            TerminalResource::Grid => {
                let request = GridSnapshotRequest::from_params(params)?;
                Ok(resources::spawn_grid_subscription(
                    self.session.clone(),
                    request,
                    tx,
                    subscription_id,
                    cancel_rx,
                ))
            }
            TerminalResource::History | TerminalResource::Cursor => {
                Err(anyhow::anyhow!("subscription not supported for resource"))
            }
        }
    }
}
