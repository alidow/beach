use cabana_macos_picker::{PickerError, PickerEvent, PickerHandle, PickerResult};
use crossbeam_channel::{Receiver, unbounded};
use futures_util::StreamExt;
use std::{sync::Arc, thread};
use tokio::{runtime::Runtime, sync::oneshot};

#[derive(Debug, Clone)]
pub enum NativePickerMessage {
    Selection(PickerResult),
    Cancelled,
    Error(String),
}

pub struct NativePickerClient {
    handle: PickerHandle,
    rx: Receiver<NativePickerMessage>,
    shutdown: Option<oneshot::Sender<()>>,
    listener: Option<thread::JoinHandle<()>>,
}

impl NativePickerClient {
    pub fn new() -> Result<Self, PickerError> {
        let handle = PickerHandle::new()?;
        let (tx, rx) = unbounded();
        let listener_handle = handle.clone();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let listener = thread::spawn(move || {
            let runtime = Runtime::new().expect("failed to build tokio runtime for native picker");
            runtime.block_on(async move {
                let mut events = listener_handle.listen();
                futures_util::pin_mut!(events);
                let mut shutdown_rx = shutdown_rx;
                loop {
                    tokio::select! {
                        _ = &mut shutdown_rx => {
                            break;
                        }
                        maybe_event = events.next() => {
                            match maybe_event {
                                Some(PickerEvent::Selection(result)) => {
                                    let _ = tx.send(NativePickerMessage::Selection(result));
                                }
                                Some(PickerEvent::Cancelled) => {
                                    let _ = tx.send(NativePickerMessage::Cancelled);
                                }
                                Some(PickerEvent::Error { message }) => {
                                    let _ = tx.send(NativePickerMessage::Error(message));
                                }
                                None => break,
                            }
                        }
                    }
                }
            });
        });

        Ok(Self {
            handle,
            rx,
            shutdown: Some(shutdown_tx),
            listener: Some(listener),
        })
    }

    pub fn poll(&self) -> Vec<NativePickerMessage> {
        let mut events = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            events.push(event);
        }
        events
    }

    pub fn launch(&self) -> Result<(), PickerError> {
        self.handle.launch()
    }

    pub fn stop(&self) -> Result<(), PickerError> {
        self.handle.stop()
    }
}

impl Drop for NativePickerClient {
    fn drop(&mut self) {
        let _ = self.handle.stop();
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

pub fn available() -> bool {
    PickerHandle::new().is_ok()
}
