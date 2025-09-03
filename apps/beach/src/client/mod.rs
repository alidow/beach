use async_trait::async_trait;
use crate::transport::Transport;
use crate::session::ClientSession;

#[async_trait]
pub trait Client {
    // Use the alias here too
    type Transport: Transport + Send + 'static;

    async fn start(&self);
    async fn stop(&self);
}

// Use the alias on the struct…
pub struct TerminalClient<T: Transport + Send + 'static> {
    session: ClientSession<T>,
}

impl<T: Transport + Send + 'static> TerminalClient<T> {
    pub fn new(session: ClientSession<T>) -> Self {
        TerminalClient { session }
    }
}

// …and on the impl
#[async_trait]
impl<T: Transport + Send + 'static> Client for TerminalClient<T> {
    type Transport = T;

    async fn start(&self) {
        // TODO
    }

    async fn stop(&self) {
        // TODO
    }
}