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
        } else if method == Method::Get && url.starts_with("/signaling/latest") {
            if let Err(err) = handle_get_latest(request, storage.clone()) {
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

fn parse_query(url: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Some(idx) = url.find('?') {
        let qs = &url[idx + 1..];
        for pair in qs.split('&') {
            let mut it = pair.splitn(2, '=');
            if let Some(k) = it.next() {
                if let Some(v) = it.next() {
                    let key = urlencoding::decode(k).unwrap_or_else(|_| k.into()).to_string();
                    let val = urlencoding::decode(v).unwrap_or_else(|_| v.into()).to_string();
                    map.insert(key, val);
                }
            }
        }
    }
    map
}

fn handle_get_latest(request: tiny_http::Request, storage: Arc<PathBuf>) -> Result<()> {
    let url = request.url().to_string();
    let q = parse_query(&url);
    let session = match q.get("session_id") {
        Some(s) => s,
        None => {
            let resp = Response::from_string("missing session_id").with_status_code(StatusCode(400));
            let _ = request.respond(resp);
            return Ok(());
        }
    };
    let handshake = match q.get("handshake_b64") {
        Some(h) => h,
        None => {
            let resp = Response::from_string("missing handshake_b64").with_status_code(StatusCode(400));
            let _ = request.respond(resp);
            return Ok(());
        }
    };

    let mut latest_path: Option<PathBuf> = None;
    let mut latest_modified: Option<std::time::SystemTime> = None;
    if let Ok(entries) = std::fs::read_dir(&*storage) {
        for entry in entries.flatten() {
            if let Ok(bytes) = std::fs::read(entry.path()) {
                if let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    let sid = doc.get("session_id").and_then(|v| v.as_str());
                    let hid = doc.get("handshake_b64").and_then(|v| v.as_str());
                    if sid == Some(session.as_str()) && hid == Some(handshake.as_str()) {
                        if let Ok(meta) = entry.metadata() {
                            let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                            if latest_modified.map(|t| mtime > t).unwrap_or(true) {
                                latest_modified = Some(mtime);
                                latest_path = Some(entry.path());
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(path) = latest_path {
        let bytes = std::fs::read(path)?;
        let doc: serde_json::Value = serde_json::from_slice(&bytes)?;
        let envelope = doc.get("envelope").and_then(|v| v.as_str()).unwrap_or("");
        let body = serde_json::json!({"envelope": envelope});
        let response = Response::from_string(serde_json::to_string(&body)?).with_status_code(StatusCode(200));
        let _ = request.respond(response);
    } else {
        let response = Response::from_string("not found").with_status_code(StatusCode(404));
        let _ = request.respond(response);
    }
    Ok(())
}
