use std::sync::{Condvar, Mutex};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

const HOST_INPUT_BUFFER_LIMIT: usize = 8192;

pub struct RawModeGuard(bool);

impl RawModeGuard {
    pub fn new(enable: bool) -> Self {
        if enable {
            match enable_raw_mode() {
                Ok(()) => Self(true),
                Err(err) => {
                    eprintln!("⚠️  failed to enable raw mode: {err}");
                    Self(false)
                }
            }
        } else {
            Self(false)
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.0 {
            let _ = disable_raw_mode();
        }
    }
}

#[derive(Default)]
struct GateState {
    paused: bool,
    buffer: Vec<u8>,
}

pub struct HostInputGate {
    state: Mutex<GateState>,
    condvar: Condvar,
}

impl HostInputGate {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(GateState {
                paused: false,
                buffer: Vec::with_capacity(256),
            }),
            condvar: Condvar::new(),
        }
    }

    pub fn pause(&self) {
        let mut state = self.state.lock().unwrap();
        state.paused = true;
    }

    pub fn resume_and_discard(&self) -> usize {
        let mut state = self.state.lock().unwrap();
        let dropped = state.buffer.len();
        state.buffer.clear();
        state.paused = false;
        self.condvar.notify_all();
        dropped
    }

    pub fn intercept(&self, bytes: &[u8]) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.paused {
            return false;
        }
        let available = HOST_INPUT_BUFFER_LIMIT.saturating_sub(state.buffer.len());
        if available == 0 {
            return true;
        }
        let to_copy = available.min(bytes.len());
        state.buffer.extend_from_slice(&bytes[..to_copy]);
        true
    }

    pub fn wait_until_resumed(&self) {
        let mut state = self.state.lock().unwrap();
        while state.paused {
            state = self.condvar.wait(state).unwrap();
        }
    }
}

impl Default for HostInputGate {
    fn default() -> Self {
        Self::new()
    }
}
