use super::{DiagnosticRequest, DiagnosticResponse};
use std::io::{Read, Write};
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

pub fn diagnostic_socket_path(session_id: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("beach-debug-{}.sock", session_id));
    path
}

#[cfg(unix)]
pub fn start_diagnostic_listener(
    session_id: String,
    request_tx: std::sync::mpsc::Sender<DiagnosticRequest>,
    response_rx: std::sync::mpsc::Receiver<DiagnosticResponse>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    use std::sync::{Arc, Mutex};
    use tracing::debug;

    let socket_path = diagnostic_socket_path(&session_id);

    // Remove existing socket if present
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;

    debug!(
        target = "debug::ipc",
        socket_path = %socket_path.display(),
        "diagnostic listener started"
    );

    let response_rx = Arc::new(Mutex::new(response_rx));

    let handle = std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    let request_tx = request_tx.clone();
                    let response_rx = response_rx.clone();

                    std::thread::spawn(move || {
                        if let Err(e) = handle_diagnostic_connection(&mut stream, request_tx, response_rx) {
                            tracing::warn!(target = "debug::ipc", error = %e, "diagnostic connection error");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(target = "debug::ipc", error = %e, "failed to accept connection");
                    break;
                }
            }
        }

        // Cleanup
        let _ = std::fs::remove_file(&socket_path);
    });

    Ok(handle)
}

#[cfg(unix)]
fn handle_diagnostic_connection(
    stream: &mut UnixStream,
    request_tx: std::sync::mpsc::Sender<DiagnosticRequest>,
    response_rx: std::sync::Arc<std::sync::Mutex<std::sync::mpsc::Receiver<DiagnosticResponse>>>,
) -> std::io::Result<()> {
    // Read request
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;

    let request: DiagnosticRequest = serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    // Send request to handler
    request_tx
        .send(request)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;

    // Wait for response
    let response = {
        let rx = response_rx.lock().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        rx.recv()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?
    };

    // Send response
    let response_bytes = serde_json::to_vec(&response)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let len = response_bytes.len() as u32;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(&response_bytes)?;
    stream.flush()?;

    Ok(())
}

#[cfg(unix)]
pub fn send_diagnostic_request(
    session_id: &str,
    request: DiagnosticRequest,
) -> std::io::Result<DiagnosticResponse> {
    let socket_path = diagnostic_socket_path(session_id);
    let mut stream = UnixStream::connect(&socket_path)?;

    // Send request
    let request_bytes = serde_json::to_vec(&request)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let len = request_bytes.len() as u32;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(&request_bytes)?;
    stream.flush()?;

    // Read response
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;

    let response: DiagnosticResponse = serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    Ok(response)
}

#[cfg(not(unix))]
pub fn start_diagnostic_listener(
    _session_id: String,
    _request_tx: std::sync::mpsc::Sender<DiagnosticRequest>,
    _response_rx: std::sync::mpsc::Receiver<DiagnosticResponse>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "diagnostic IPC not supported on non-Unix platforms",
    ))
}

#[cfg(not(unix))]
pub fn send_diagnostic_request(
    _session_id: &str,
    _request: DiagnosticRequest,
) -> std::io::Result<DiagnosticResponse> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "diagnostic IPC not supported on non-Unix platforms",
    ))
}
