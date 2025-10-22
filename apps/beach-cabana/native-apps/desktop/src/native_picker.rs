use cabana_macos_picker::{PickerError, PickerEvent, PickerHandle, PickerResult};
use futures_util::{FutureExt, Stream, StreamExt};
use std::pin::Pin;

#[derive(Debug, Clone)]
pub enum NativePickerMessage {
    Selection(PickerResult),
    Cancelled,
    Error(String),
}

pub struct NativePickerClient {
    handle: PickerHandle,
    stream: Pin<Box<dyn Stream<Item = PickerEvent>>>,
}

impl NativePickerClient {
    pub fn new() -> Result<Self, PickerError> {
        let handle = PickerHandle::new()?;
        let stream = handle.listen();
        Ok(Self {
            handle,
            stream: Box::pin(stream),
        })
    }

    pub fn poll(&mut self) -> Vec<NativePickerMessage> {
        let mut events = Vec::new();
        loop {
            let next = self.stream.as_mut().next().now_or_never();
            match next {
                Some(Some(PickerEvent::Selection(result))) => {
                    events.push(NativePickerMessage::Selection(result));
                }
                Some(Some(PickerEvent::Cancelled)) => {
                    events.push(NativePickerMessage::Cancelled);
                }
                Some(Some(PickerEvent::Error { message })) => {
                    events.push(NativePickerMessage::Error(message));
                }
                Some(None) => break,
                None => break,
            }
        }
        events
    }

    pub fn launch(&self) -> Result<(), PickerError> {
        self.handle.launch()
    }

}

impl Drop for NativePickerClient {
    fn drop(&mut self) {
        let _ = self.handle.stop();
    }
}
