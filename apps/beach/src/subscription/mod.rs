pub type SubscriptionId = String;
pub type ClientId = String;

pub mod data_source;
pub mod hub;

// Re-export key types
pub use data_source::{HistoryMetadata, PtyWriter, TerminalDataSource};
pub use hub::{SubscriptionConfig, SubscriptionHandler, SubscriptionHub, SubscriptionUpdate};
