/// Bus wiring split into explicit subscriber/publisher modules for auditability.
pub mod subscriber {
    pub use crate::bus_ingest::{MANAGER_TOPICS, ingest_message, start_bus_ingest};
}

pub mod publisher {
    pub use crate::bus_publisher::{
        ManagerBusPublisher, TOPIC_ACK, TOPIC_ACTION, TOPIC_HEALTH, TOPIC_STATE,
    };
}
