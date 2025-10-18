use anyhow::{anyhow, Result};
use reqwest::blocking::{Client, get};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct FixtureRequest<'a> {
    session_id: &'a str,
    handshake_b64: &'a str,
    envelope: &'a str,
}

#[derive(Debug)]
pub struct FixtureEnvelope<'a> {
    pub session_id: &'a str,
    pub handshake_b64: &'a str,
    pub envelope: &'a str,
}

pub fn post_envelope(url: &str, envelope: FixtureEnvelope<'_>) -> Result<()> {
    let client = Client::new();
    let response = client
        .post(url)
        .json(&FixtureRequest {
            session_id: envelope.session_id,
            handshake_b64: envelope.handshake_b64,
            envelope: envelope.envelope,
        })
        .send()?;

    if !response.status().is_success() {
        return Err(anyhow!("fixture responded with status {}", response.status()));
    }

    Ok(())
}

pub fn get_latest_envelope(base_url: &str, session_id: &str, handshake_b64: &str) -> Result<Option<String>> {
    let url = format!(
        "{}/latest?session_id={}&handshake_b64={}",
        base_url.trim_end_matches('/'),
        urlencoding::encode(session_id),
        urlencoding::encode(handshake_b64)
    );
    let resp = get(&url)?;
    if resp.status().as_u16() == 404 { return Ok(None); }
    if !resp.status().is_success() { return Err(anyhow!("fixture GET failed: {}", resp.status())); }
    let v: serde_json::Value = resp.json()?;
    let env = v.get("envelope").and_then(|x| x.as_str()).map(|s| s.to_string());
    Ok(env)
}

