use async_trait::async_trait;

use crate::server::Server;

pub struct TerminalServer<T: Transport> {
    transport: T,
}

#[async_trait]
impl<T: Transport + Send + 'static> Server for TerminalServer<T> {

    fn new(transport: T) -> Self {
        TerminalServer {
            transport: transport,
        }
    }

    async fn start(&self) {

    }

    async fn stop(&self) {

    }

}