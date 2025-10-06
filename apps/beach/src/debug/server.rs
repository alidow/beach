use super::{DiagnosticRequest, DiagnosticResponse};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

pub type DiagnosticChannel = (Sender<DiagnosticRequest>, Receiver<DiagnosticResponse>);

pub struct DiagnosticServer {
    pub request_rx: Arc<Mutex<Receiver<DiagnosticRequest>>>,
    pub response_tx: Sender<DiagnosticResponse>,
}

impl DiagnosticServer {
    pub fn new() -> (Self, DiagnosticChannel) {
        let (request_tx, request_rx) = std::sync::mpsc::channel();
        let (response_tx, response_rx) = std::sync::mpsc::channel();

        let server = Self {
            request_rx: Arc::new(Mutex::new(request_rx)),
            response_tx,
        };

        (server, (request_tx, response_rx))
    }

    pub fn try_handle_request<F>(&self, mut handler: F) -> Result<(), String>
    where
        F: FnMut(DiagnosticRequest) -> DiagnosticResponse,
    {
        let receiver = self.request_rx.lock().map_err(|e| e.to_string())?;

        while let Ok(request) = receiver.try_recv() {
            let response = handler(request);
            let _ = self.response_tx.send(response);
        }

        Ok(())
    }
}
