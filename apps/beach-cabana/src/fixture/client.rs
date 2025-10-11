use anyhow::{anyhow, Result};
use reqwest::blocking::Client;
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
