use crate::cache::{ServerCache, ClientCache};

pub struct TerminalServerCache {
    rows: Arr<RwLock<Arc<Cell>>>,
}

pub impl ServerCache for TerminalServerCache {
    
}