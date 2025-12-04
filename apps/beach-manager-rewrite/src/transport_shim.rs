use std::sync::Arc;

use crate::config::BusMode;
use tracing::debug;
use transport_unified_adapter::{IpcUnifiedAdapter, UnifiedBusAdapter};
use transport_webrtc::RtcUnifiedAdapter;

/// Build a bus adapter for the configured transport mode.
pub fn build_bus_adapter(
    mode: &BusMode,
    session_server_base: &str,
) -> Option<Arc<dyn UnifiedBusAdapter>> {
    match mode {
        BusMode::Ipc => {
            debug!("bus adapter configured: IPC");
            Some(Arc::new(IpcUnifiedAdapter::new()))
        }
        BusMode::Rtc => {
            debug!(
                session_server_base,
                "bus adapter configured: RTC (session base)"
            );
            Some(Arc::new(RtcUnifiedAdapter::new(session_server_base)))
        }
        BusMode::Disabled => None,
    }
}
