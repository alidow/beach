


pub trait Server<T: Transport, C: ServerCache> {
    fn new(transport: T) -> Self;

    async fn start(&self);

    async fn stop(&self);
}