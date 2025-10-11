use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tiny_http::{Method, Response, Server, StatusCode};

#[derive(Debug, Deserialize, Serialize)]
struct FixturePayload {
    session_id: String,
    handshake_b64: String,
    envelope: String,
}

pub fn serve(listen: SocketAddr, storage_dir: PathBuf) -> Result<()> {
    fs::create_dir_all(&storage_dir)
        .with_context(|| format!("failed to create fixture storage dir {}", storage_dir.display()))?;

    let server = Server::http(listen)
        .map_err(|err| anyhow!("failed to bind fixture server to {}: {}", listen, err))?;
    println!(
        "Cabana fixture listening on http://{} (writing to {})",
        server.server_addr(),
        storage_dir.display()
    );

    let storage = Arc::new(storage_dir);
    loop {
        let request = match server.recv() {
            Ok(req) => req,
            Err(err) => {
                eprintln!("fixture recv error: {}", err);
                continue;
            }
        };

        let method = request.method().clone();
        let url = request.url().to_string();

        if method == Method::Post && url == "/signaling" {
            if let Err(err) = handle_post(request, storage.clone()) {
                eprintln!("fixture failed to handle request: {err}");
            }
        } else {
            let response = Response::from_string("not found").with_status_code(StatusCode(404));
            let _ = request.respond(response);
        }
    }
}

fn handle_post(mut request: tiny_http::Request, storage: Arc<PathBuf>) -> Result<()> {
    let mut body = Vec::new();
    {
        let mut reader = request.as_reader();
        std::io::Read::read_to_end(&mut reader, &mut body)
            .context("failed to read request body")?;
    }

    let payload: FixturePayload =
        serde_json::from_slice(&body).context("failed to parse json body")?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let filename = format!(
        "{}-{}-{}.json",
        payload.session_id, payload.handshake_b64, timestamp
    );
    let path = storage.join(filename);

    fs::write(&path, serde_json::to_vec_pretty(&payload)?)
        .with_context(|| format!("failed to write fixture payload to {}", path.display()))?;
    println!("Stored sealed envelope at {}", path.display());

    let response = Response::from_string("ok").with_status_code(StatusCode(200));
    request
        .respond(response)
        .context("failed to send fixture response")?;
    Ok(())
}
